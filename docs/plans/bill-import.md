# 知余：账单导入（微信 / 支付宝）实施规格 v3

> 状态：**主帅裁决通过，待 Codex 施工**  
> 日期：2026-08-11  
> 执行者：Codex（`gpt-5.6-sol`，low effort，串行任务）  
> 范围：只做手动文件上传导入；邮箱、招商银行、账户映射、transfer、跨渠道模糊去重均不在本期。

## 一、事实基线

样本文件位于用户本机 `~/Downloads/`，含真实姓名、账号和完整流水：

- 支付宝 CSV：235 笔
- 微信 xlsx：297 笔

**真实文件严禁复制进仓库、fixture、日志、测试快照、提交信息或外部审计附件。** Codex 只使用手工虚构的脱敏 fixture；真实文件只供主帅在本机最终验收。

| 维度 | 支付宝 | 微信 |
|---|---|---|
| 容器 | 裸 CSV，无 zip | xlsx |
| 编码 | GBK；实现用 GB18030 解码 | xlsx 内部格式 |
| 换行 | CRLF 与 LF 混用 | 不适用 |
| 实测表头 | 第 24 行（1-based） | 第 18 行（1-based） |
| 列数 | 12 个命名列，可带尾随空列 | 11 个命名列 |
| 时间类型 | 字符串 | calamine `Data::DateTime`，297/297 |
| 金额类型 | 十进制字符串 | calamine `Data::Float`，297/297 |
| 单号 | 235/235 尾部带 `\t`，trim 后全部非空唯一 | 297/297 全部非空唯一 |
| 空值 | 空字符串 | 可选文本常用精确值 `/` |

支付宝列：

```text
交易时间, 交易分类, 交易对方, 对方账号, 商品说明, 收/支, 金额,
收/付款方式, 交易状态, 交易订单号, 商家订单号, 备注, (可选尾随空列)
```

微信列：

```text
交易时间, 交易类型, 交易对方, 商品, 收/支, 金额(元), 支付方式,
当前状态, 交易单号, 商户单号, 备注
```

实测状态全集：

- 支付宝成功：`交易成功`、`支付成功`、`退款成功`、`还款成功`、`放款成功`
- 支付宝等待：`等待发货`、`等待对方确认收货`、`等待确认收货`
- 支付宝关闭：`交易关闭`
- 支付宝方向：`支出`、`收入`、`不计收支`
- 微信终态成功：`支付成功`、`已存入零钱`、`已转账`、`对方已收钱`、`提现已到账`、`还款成功`、`充值完成`、`已全额退款`、`已退款¥{金额}`、`已退款(¥{金额})`
- 微信方向：`支出`、`收入`、`/`

微信 `交易类型` 含动态商户前缀，不是封闭枚举，必须作为普通文本保存。

## 二、不可推翻的范围决策

1. 解析在服务端 Rust 完成，符合服务端权威架构。
2. 不做跨渠道模糊去重。只按 `(user_id, source_channel, external_id)` 做同源精确幂等。
3. 小额记录不聚合、不丢弃。
4. 不扩展 `ledger_transactions.kind`；中性记录只留 staging。
5. 不做账户映射，正式交易 `account_id=NULL`；源 `pay_method` 必须保存在 staging。
6. 支付宝的 `对方账号` 本期明确不保存，减少不必要的敏感信息持久化。
7. 原始上传文件不持久化；只存规范化 staging 字段、basename 文件名和 SHA-256。
8. 0 元成功交易保留为 `zero_amount` staging，不写 ledger，不阻断同批其他记录。
9. 未知状态可诊断地隔离为 blocked batch；未知方向或结构错误整文件失败。

## 三、migration 0012

新增 `apps/api/migrations/0012_bill_imports.sql`，并在 `apps/api/src/db.rs` 增加 `include_str!` 常量和 `(12, ...)` 注册。漏注册会破坏 `known_migration_versions()` 与备份恢复的版本判断，必须单独测试。

目标 DDL：

```sql
CREATE TABLE import_batches (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    source_channel TEXT NOT NULL
        CHECK (source_channel IN ('alipay', 'wechat')),
    parser_version INTEGER NOT NULL DEFAULT 1
        CHECK (parser_version > 0),
    file_name TEXT NOT NULL DEFAULT ''
        CHECK (length(file_name) <= 255),
    file_sha256 TEXT NOT NULL
        CHECK (length(file_sha256) = 64),
    period_start TEXT NOT NULL,
    period_end TEXT NOT NULL,
    total_count INTEGER NOT NULL CHECK (total_count > 0),
    status TEXT NOT NULL DEFAULT 'preview'
        CHECK (status IN ('preview', 'blocked', 'committed', 'discarded')),
    committed_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (period_start <= period_end)
);

CREATE INDEX idx_import_batches_user
    ON import_batches(user_id, created_at DESC, id DESC);

CREATE TABLE import_records (
    id TEXT PRIMARY KEY,
    batch_id TEXT NOT NULL
        REFERENCES import_batches(id) ON DELETE CASCADE,
    row_index INTEGER NOT NULL CHECK (row_index > 0),
    external_id TEXT NOT NULL
        CHECK (
            length(trim(external_id)) > 0
            AND length(external_id) <= 256
        ),
    merchant_order_id TEXT NOT NULL DEFAULT ''
        CHECK (length(merchant_order_id) <= 256),
    occurred_at TEXT NOT NULL,
    occurred_on TEXT NOT NULL,
    direction TEXT NOT NULL
        CHECK (direction IN ('income', 'expense', 'neutral')),
    amount_cents INTEGER NOT NULL
        CHECK (
            amount_cents >= 0
            AND amount_cents <= 9007199254740991
        ),
    channel_category TEXT NOT NULL DEFAULT ''
        CHECK (length(channel_category) <= 4096),
    counterparty TEXT NOT NULL DEFAULT ''
        CHECK (length(counterparty) <= 4096),
    product TEXT NOT NULL DEFAULT ''
        CHECK (length(product) <= 4096),
    pay_method TEXT NOT NULL DEFAULT ''
        CHECK (length(pay_method) <= 4096),
    channel_status TEXT NOT NULL DEFAULT ''
        CHECK (length(channel_status) <= 128),
    source_note TEXT NOT NULL DEFAULT ''
        CHECK (length(source_note) <= 4096),
    disposition TEXT NOT NULL
        CHECK (
            disposition IN (
                'import',
                'pending',
                'neutral',
                'closed',
                'zero_amount',
                'unknown',
                'duplicate'
            )
        ),
    transaction_id TEXT
        REFERENCES ledger_transactions(id) ON DELETE SET NULL,
    created_at TEXT NOT NULL,

    CHECK (disposition <> 'neutral' OR direction = 'neutral'),
    CHECK (
        disposition NOT IN ('import', 'duplicate')
        OR (
            direction IN ('income', 'expense')
            AND amount_cents > 0
        )
    ),
    CHECK (
        disposition <> 'zero_amount'
        OR (
            direction IN ('income', 'expense')
            AND amount_cents = 0
        )
    ),
    CHECK (transaction_id IS NULL OR disposition = 'import'),
    UNIQUE(batch_id, row_index)
);

CREATE INDEX idx_import_records_batch
    ON import_records(batch_id, row_index);

CREATE UNIQUE INDEX idx_import_records_transaction
    ON import_records(transaction_id)
    WHERE transaction_id IS NOT NULL;

ALTER TABLE ledger_transactions
ADD COLUMN source_channel TEXT NOT NULL DEFAULT ''
CHECK (source_channel IN ('', 'alipay', 'wechat'));

ALTER TABLE ledger_transactions
ADD COLUMN external_id TEXT NOT NULL DEFAULT ''
CHECK (
    (source_channel = '' AND external_id = '')
    OR
    (
        source_channel IN ('alipay', 'wechat')
        AND length(trim(external_id)) > 0
    )
);

ALTER TABLE ledger_transactions
ADD COLUMN import_batch_id TEXT
REFERENCES import_batches(id) ON DELETE SET NULL
CHECK (
    import_batch_id IS NULL
    OR source_channel IN ('alipay', 'wechat')
);

CREATE UNIQUE INDEX idx_ledger_transactions_external
    ON ledger_transactions(user_id, source_channel, external_id)
    WHERE external_id != '';

CREATE INDEX idx_ledger_transactions_import_batch
    ON ledger_transactions(user_id, import_batch_id)
    WHERE import_batch_id IS NOT NULL;
```

`import_records` 不冗余保存 `user_id`，避免 batch 与 record 所属用户不一致；所有 record 访问必须先验证 batch 归属。

`import_batch_id` 使用 nullable FK。既有交易迁移后为 NULL；本功能不物理删除 batch，因此正常流程不会触发 `ON DELETE SET NULL`。

### 迁移测试硬要求

1. 从 v11 数据库升级到 v12。
2. 升级前插入至少一条既有交易；升级后断言新列为 `'' / '' / NULL`。
3. `known_migration_versions()` 含 12，备份相关测试继续通过。
4. `PRAGMA foreign_key_check` 无结果。
5. 多条手工交易可继续使用空 external_id。
6. 同用户、同渠道、同 external_id 第二次插入失败；跨渠道同 ID 允许。
7. DDL 的 CHECK、partial index、nullable FK、`ON DELETE SET NULL` 使用项目固定的 libsql 0.9.30 执行验证。

## 四、模块与依赖

模块布局：

```text
apps/api/src/imports/mod.rs       handlers / router-facing DTO
apps/api/src/imports/model.rs     parser model / enums / common validation
apps/api/src/imports/alipay.rs
apps/api/src/imports/wechat.rs
```

新增依赖：

```toml
calamine = { version = "0.26", features = ["dates"] }
encoding_rs = "0.8"
csv = "1.3"
```

`chrono`、`sha2` 复用现有 workspace 依赖。Axum 开启 `multipart` feature。

不得为生成测试 xlsx 引入生产 writer 依赖；提交一个静态、纯虚构的脱敏小 xlsx，生成脚本放 scratch 且不提交。

## 五、解析器契约

### 5.1 类型

```rust
pub enum SourceChannel {
    Alipay,
    Wechat,
}

pub enum Direction {
    Income,
    Expense,
    Neutral,
}

// parser 只产生基础 disposition
pub enum BaseDisposition {
    Import,
    Pending,
    Neutral,
    Closed,
    ZeroAmount,
    Unknown,
}

// staging/API 可在基础结果上覆盖 Duplicate
pub enum StoredDisposition {
    Import,
    Pending,
    Neutral,
    Closed,
    ZeroAmount,
    Unknown,
    Duplicate,
}

pub struct ParsedRecord {
    pub row_index: i64,             // 源文件 1-based 物理行号
    pub external_id: String,
    pub merchant_order_id: String,
    pub occurred_at: String,        // YYYY-MM-DD HH:MM:SS，本地时间
    pub occurred_on: String,        // YYYY-MM-DD
    pub direction: Direction,
    pub amount_cents: i64,
    pub channel_category: String,
    pub counterparty: String,
    pub product: String,
    pub pay_method: String,
    pub channel_status: String,
    pub source_note: String,
    pub disposition: BaseDisposition,
}
```

### 5.2 全文件规则

1. 先读取完整文件到内存并检查 10 MiB 文件上限。
2. 解析器先生成完整 `Vec<ParsedRecord>`，解析阶段不得创建 batch 或写数据库。
3. 表头后只允许跳过所有字段/单元格均为空的整行。
4. 任一其他非空行出现缺列、坏日期、坏金额、未知方向、非法类型、空 external_id、文件内重复 external_id 或长度超限，整个上传 422；禁止跳行、默认值或部分成功。
5. **唯一例外是未知交易状态**：方向、金额、时间、单号等结构合法时生成 `Unknown`，之后 batch 状态为 `blocked`。
6. 零数据行文件 422。
7. 最多扫描 16 个 worksheet、每表最多 128 列、最多 100000 条非空数据记录；超限 422 `import_resource_limit`。
8. `period_start/end` 取所有成功解析记录（包括 pending/neutral/closed/zero/unknown）的 `occurred_on` 最小/最大值。
9. 金额汇总、计数累加均使用 checked arithmetic。

### 5.3 表头和列映射

- 表头嗅探只用于找候选行，找到后仍必须验证完整必需列集。
- 按 trim 后列名建立索引，禁止按固定列号读取。
- 必需列缺失或重复时整文件失败；额外未知列允许忽略。
- 支付宝尾随空列允许有或没有。
- 微信按 workbook 顺序扫描；必须恰好一个 worksheet、恰好一行完整表头。零个或多个命中均失败。
- `row_index` 是源文件 1-based 物理行号，不是数据数组下标。

### 5.4 方向映射

方向必须在 disposition 前封闭解析：

- 支付宝：`支出`→Expense，`收入`→Income，`不计收支`→Neutral。
- 微信：`支出`→Expense，`收入`→Income，精确值 `/`→Neutral。
- 匹配前 `trim()`；空或其他值整文件 422 `unknown_import_direction`，错误包含行号、字段名和截断后的值。
- 微信 `/` 转空的规则只适用于可选文本字段，绝不能先应用到 `收/支`。

### 5.5 状态与 disposition

命中即停，顺序固定：

1. 未知状态 → `Unknown`。
2. 支付宝 `交易关闭` → `Closed`。
3. 支付宝等待状态三种 → `Pending`。
4. 已知成功状态 + Neutral direction → `Neutral`。
5. 已知成功状态 + Income/Expense + `amount_cents == 0` → `ZeroAmount`。
6. 已知成功状态 + Income/Expense + `amount_cents > 0` → `Import`。

只有 `Import` 可能写入 `ledger_transactions`。`Duplicate` 不是 parser 状态，而是数据库精确去重层对基础 Import 的覆盖结果。

未知状态处理：

- record 保留原 `channel_status`，disposition=`unknown`；
- 整个 batch status=`blocked`；
- 上传仍返回 201 和可预览 batch；
- blocked batch 禁止 commit，只能 discard；
- UI 展示未知状态、行号和原因；绝不跳过未知行后放行其余记录。

### 5.6 金额

全链路范围：`0..=9_007_199_254_740_991` 分。

支付宝：

- 金额字段 trim 后只接受 `^\d+(?:\.\d{1,2})?$`。
- 按整数元和小数位拆分，使用 `checked_mul(100)`、`checked_add`；禁止先转 `f64`。
- 负数、符号、指数、千位分隔、三位以上小数或超上限均失败。

微信：

- `Data::Float`：先 `is_finite()` 且非负，再计算 `(v * 100.0).round()`；转换为 i64 前检查结果有限且在范围内。
- `Data::Int`：视为整数元，使用 `checked_mul(100)`。
- 其他类型失败；禁止直接截断。

真实证据：`283.71 * 100 == 28370.999999999996`，直接 cast 会少一分，round 后才是 28371。

### 5.7 时间

- 支付宝严格按 `%Y-%m-%d %H:%M:%S` 解析，不用字符串切片假装校验。
- 微信 `交易时间` 接受 `Data::DateTime` 并通过 calamine 日期 API得到 `NaiveDateTime`；可接受能严格解析的 `Data::DateTimeIso`。
- 禁止把 Excel 序列号当普通 f64 手工换算。
- 账单时间已经是 UTC+08:00 本地时间，直接保存原本地年月日时分秒，不转 UTC，不读取用户 timezone 二次换算。

### 5.8 微信单元格类型

- `交易时间`：DateTime，兼容合法 DateTimeIso。
- `金额(元)`：Float 或 Int。
- `交易单号`：必须是非空 String，不得经过 f64。
- 其余必需文本列：String。
- 可选文本列：String 或 Empty。
- Bool、Error 或未列明类型整文件失败。
- 禁止对任意单元格统一 `.to_string()` 后继续解析。

### 5.9 文本归一化与隐私

- 支付宝 `GB18030.decode` 后必须检查 `had_errors`；出现替换错误整文件失败。文件开头可去 BOM。
- 时间、金额、状态、方向、external_id、merchant_order_id 使用 Unicode `trim()`。
- 描述字段 `channel_category/counterparty/product/pay_method/source_note` 去除首尾空白，内部内容不改写；微信 trim 后精确等于 `/` 时为空字符串。
- 禁止全字段 `/ → ""`。
- external_id / merchant_order_id 最多 256 字符；status 最多 128；描述字段最多 4096。按 Unicode 字符计数，超限失败，不截断 staging。
- 单文件内对 trim 后 external_id 建集合；重复时整文件失败，错误含第一次和第二次物理行号。
- 不保存支付宝 `对方账号`。

### 5.10 微信退款状态

- `已退款(¥0.95)`：带括号，代表原支出交易状态，原交易金额照常入账。
- `已退款¥0.95`：不带括号，代表独立退款收入，照常入账。
- 两条都保留，不配对、不冲抵、不合并。
- 动态金额部分必须匹配合法非负十进制格式；不要用无边界 `starts_with("已退款")` 接受任意垃圾状态。

## 六、纯解析验收基准

本节数字是**数据库 duplicate 覆盖前的 parser 基础 disposition**。测试应直接调用 parser，或使用没有同渠道 imported transaction 的全新测试库；本节不包含 duplicate。

支付宝，总 235：

| disposition | direction | 笔数 | 金额 |
|---|---|---:|---:|
| import | expense | 157 | 1878.76 元 |
| import | income | 0 | 0.00 元 |
| zero_amount | expense | 4 | 0.00 元 |
| neutral | — | 24 | 1714.11 元 |
| pending | — | 41 | 1951.64 元 |
| closed | — | 9 | 938.38 元 |

微信，总 297：

| disposition | direction | 笔数 | 金额 |
|---|---|---:|---:|
| import | expense | 244 | 42787.34 元 |
| import | income | 36 | 20288.40 元 |
| neutral | — | 17 | 9561.01 元 |

微信全量金额合计必须为 72636.75 元。

支付宝 income=0 是正确口径：该样本中所有标记“收入”的行都尚未完成或已关闭。不要按文件头官方分类自行改写状态机。

## 七、API 契约

共五个路由：三个写操作、两个查询操作。全部使用现有 `AuthUser`、`ApiError`、utoipa，并注册到 OpenAPI。

### 7.1 资源归属

- 客户端不得传 user_id；所有 user_id 只来自 AuthUser。
- 所有 `{id}` 路由先按 `import_batches.id = ? AND user_id = AuthUser.id` 查询；未命中统一 404，不泄露其他用户资源是否存在。
- records 只在 batch 所有权验证后按 batch_id 读取。

### 7.2 服务端幂等

三个写路由都强制 `Idempotency-Key` 并复用现有：

```text
idempotency_key → request_hash → replay_idempotency → 业务 → store_idempotency → commit
```

operation：

- 上传：`create_import`
- 确认：`commit_import:{batch_id}`
- 放弃/撤销：`discard_import:{batch_id}`

request hash：

- 上传：规范化结构 `{fileSha256, normalizedFileName, requestedChannel}`；未显式 channel 时使用字符串 `auto`。
- commit / discard：空请求体；batch_id 已在 operation 中。

上传读取和 parser 验证可在事务前完成；进入 `TransactionBehavior::Immediate` 后必须先 replay，再做 duplicate 查询和任何数据库写入。业务数据与幂等响应必须同事务提交。

相同 key + 相同请求重放原响应；相同 key + 不同请求沿用现有幂等冲突语义。前端 hook 只生成/复用 key，不能替代服务端保证。

### 7.3 multipart 上传

`POST /api/v1/imports`

multipart 规则：

- 必须恰好一个二进制字段 `file`。
- 可有零或一个文本字段 `channel`，仅 `alipay` / `wechat`；属于 multipart，不是 query。
- 重复 file、重复 channel、未知字段均 422。
- 路由请求体上限 11 MiB；读取文件字段后单独检查实际字节不超过 `10 * 1024 * 1024`，超出 413。
- 前端使用 FormData，不得手工设置 `Content-Type`。

文件名：

- 只保存 basename，同时处理 `/` 与 `\` 路径分隔；删除控制字符；按 Unicode 字符截断到 255；空时使用稳定占位名。

渠道：

- 显式 channel 时按指定 parser 严格解析。
- auto：zip magic `PK\x03\x04` 尝试微信 xlsx；否则按 GB18030 解码并找支付宝完整表头。
- 无法识别返回 422 `unsupported_import_file`。

行为：

1. 校验 multipart/文件上限，算 SHA-256，完整解析到内存。
2. 开 Immediate 事务并执行幂等 replay。
3. 对基础 Import 只读查询现有 ledger，生成准确 duplicate 预览；不得写/改/delete ledger。
4. 创建 batch 和全部 records；任一失败全部回滚。
5. 若有 Unknown，batch=`blocked`，否则 `preview`。
6. 存幂等响应并提交；返回 201。

同一用户存在相同 file_sha256 且 `committed_at IS NOT NULL` 的历史批次时，返回最新一条：

```text
ORDER BY committed_at DESC, id DESC LIMIT 1
```

结构化字段为 `previousCommittedBatchId/previousCommittedAt`，但仍允许创建新 preview/blocked batch。相同 hash 表示文件字节相同；允许继续仅用于主动复查、撤销后重导和验证幂等。带新增记录的新导出文件 hash 会变化。

### 7.4 duplicate 与 payload mismatch

上传和确认阶段遇到相同 `(user_id, source_channel, external_id)` 时：

1. 查找原 ledger 对应的历史 import_record。
2. 比较 `source_channel/external_id/direction/amount_cents/occurred_on`。
3. 全部一致才是 Duplicate。
4. 不一致回滚并返回 409 `external_id_payload_mismatch`。
5. 找不到历史 staging provenance 视为内部不变式破坏，回滚并返回 500；禁止静默 duplicate。

已归档交易仍参与唯一索引和 duplicate 检测。

上传时判为 Duplicate 的 record 保持 duplicate；若旧交易之后被合法移除，用户需重新上传生成新预览。

### 7.5 预览 DTO

summary 针对整个 batch，不受分页或 filter 影响：

```json
{
  "importIncome":  { "count": 0, "amountCents": 0 },
  "importExpense": { "count": 0, "amountCents": 0 },
  "pending":       { "count": 0, "amountCents": 0 },
  "neutral":       { "count": 0, "amountCents": 0 },
  "closed":        { "count": 0, "amountCents": 0 },
  "zeroAmount":    { "count": 0, "amountCents": 0 },
  "unknown":       { "count": 0, "amountCents": 0 },
  "duplicate":     { "count": 0, "amountCents": 0 }
}
```

响应还包含：batch id、status、channel、parserVersion、fileName、period、totalCount、整批 payMethods 去重计数、unknown issues（行号和状态原文）、历史同 hash 提示。

### 7.6 确认入账

`POST /api/v1/imports/{id}/commit`

在单个 Immediate 事务中：

1. replay idempotency。
2. 验证 batch 属于 AuthUser，状态必须精确为 preview；blocked/committed/discarded 或并发状态变化返回 409 `import_batch_state_conflict`。
3. 候选固定为 `disposition='import' AND transaction_id IS NULL`。
4. 前置断言：候选 direction 只能 income/expense、amount 在 `1..=MAX_SAFE_CENTS`、external_id 非空；失败 422 并带行号。
5. 生成 ledger 字段并调用与现有创建交易相同的 amount/date/category/note 领域校验。
6. 使用 targeted UPSERT：

```sql
INSERT INTO ledger_transactions (...)
VALUES (...)
ON CONFLICT(user_id, source_channel, external_id)
WHERE external_id != ''
DO NOTHING;
```

7. 影响行数 1：插入成功，回填新 transaction_id。
8. 影响行数 0：只按目标 external-id 冲突处理；重新执行核心 payload 比较，一致后把 record 改为 duplicate，不一致 409 并回滚。
9. 其他 NOT NULL/CHECK/FK/主键/唯一约束错误全部回滚；禁止 `INSERT OR IGNORE`、`INSERT OR REPLACE` 或解析错误字符串猜约束。
10. 条件更新 batch `preview→committed`，检查影响行数为 1，设置 committed_at/updated_at。
11. 存幂等响应并 commit。

即使全部 Import 已在预览或并发阶段变为 Duplicate、实际插入 0 条，也可以成功进入 committed。

ledger 映射：

- kind = direction
- amount_cents / occurred_on 直取
- account_id = NULL
- source_channel / external_id / import_batch_id 填入
- category：channel_category trim；控制字符替换为空格并折叠连续空白；`.chars().take(60)`；再调用现有 `validate_category`
- note：取 trim 后非空的 counterparty/product/source_note，以 ` · ` 连接；控制字符替换为空格并折叠；按 2000 字符截断；再调用现有 `validate_note`
- 仍调用现有 `validate_amount` 和日期校验

### 7.7 放弃/撤销

`DELETE /api/v1/imports/{id}`

允许状态转移：

```text
preview   --DELETE--> discarded
blocked   --DELETE--> discarded
committed --DELETE--> discarded
discarded             终态
```

在单个 Immediate 事务中：

1. replay idempotency。
2. 验证 batch 归属和状态；discarded 用新 key 调用返回 409，同 key 重试 replay 原响应。
3. preview/blocked 不处理 ledger，只关闭 batch。
4. committed 先验证所有非空 transaction_id 的归属链：record.batch_id、record.disposition=`import`、ledger.id、ledger.user_id、ledger.import_batch_id 必须全部匹配当前用户和 batch；任一不一致返回 500 并整批回滚。
5. 只物理删除同时满足 `version=1 AND archived_at IS NULL` 的上述导入交易。
6. version>1 或 archived_at 非空的交易保留，transaction_id 和 source_channel/external_id/import_batch_id provenance 均保留。
7. 删除交易后由 FK 将对应 record.transaction_id 置 NULL。
8. 条件更新 batch 为 discarded，检查影响行数为 1；保留原 committed_at，只更新 updated_at。
9. 不物理删除 batch 或 records。
10. 返回 `deletedCount` 和 `retainedModifiedCount`。

`discarded` 仅表示批次关闭，不保证账本中没有相关交易。

### 7.8 查询

`GET /api/v1/imports`

- page 默认 1。
- pageSize 默认 50，范围 1..=200。
- 排序 `created_at DESC, id DESC`。
- 只返回当前用户批次。

`GET /api/v1/imports/{id}`

- 先校验归属。
- page/pageSize 同上。
- 可选 disposition、direction filter。
- records 固定 `row_index ASC`。
- summary/payMethods 始终针对整批，不受分页/过滤影响。

每条 record 返回只读计算字段 outcome，不新增数据库列：

- preview + import → `will_import`
- committed + import + transaction_id 非空 → `imported`
- duplicate → `duplicate`
- pending/neutral/closed/zero_amount → `excluded`
- blocked + unknown → `blocked`
- discarded + committed_at 为空 + import → `abandoned`
- discarded + committed_at 非空 + import + transaction_id 为空 → `removed`
- discarded + import + transaction_id 非空 → `retained_modified`

### 7.9 HTTP 状态和 error code

- upload 成功 201；其余成功 200。
- 文件实际字节超限 413 `payload_too_large`。
- multipart/格式/解析行错误 422：
  - `invalid_multipart`
  - `unsupported_import_file`
  - `invalid_import_header`
  - `invalid_import_row`
  - `unknown_import_direction`
  - `duplicate_external_id_in_file`
  - `import_resource_limit`
- 非法批次状态 409 `import_batch_state_conflict`。
- 同 external ID 核心字段不一致 409 `external_id_payload_mismatch`。
- 不存在或不属于当前用户 404。
- 其他数据库/不变式错误 500 且事务回滚。

## 八、批次状态机

允许的转移只有：

```text
preview   --commit--> committed
preview   --discard-> discarded
blocked   --discard-> discarded
committed --discard-> discarded
discarded             终态
```

规则：

- commit 只接受 preview。
- discard 接受 preview/blocked/committed。
- 相同 Idempotency-Key 在状态检查前 replay；不同 key 对已完成或非法状态返回 409。
- 状态读取、业务处理、条件状态更新、幂等响应写入均在同一 Immediate 事务。
- 状态更新 SQL 带原状态条件并检查影响行数恰好为 1。
- committed 设置 committed_at；后续 discarded 保留 committed_at。

## 九、前端

新增最小可用流程：

```text
选择文件 → 上传 → 分组预览 → 确认入账 / 放弃 → 已提交批次可撤销
```

路由与入口固定为：

- 不新增一级侧栏/移动底栏导航，避免改变现有快捷键和压缩移动端空间。
- 在 `/app/transactions` 顶栏 actions 中把“导入账单”作为“记一笔”的次级操作。
- `/app/transactions/imports`：批次列表和选择文件入口。
- `/app/transactions/imports/:id`：预览、确认、放弃、撤销页面。
- 导入子路由继续保持“流水”一级导航激活；详情页自行设置并清理 topbar slots，不扩充 AppShell 的债务详情硬编码。

要求：

1. 使用现有 TanStack Query、API client、`Modal`、`ConfirmDialog`、`InlineNotice`、`TablePagination`、Toast 和 workspace 页面视觉；不引入新设计语言，也不复制一个新的通用组件库。
2. 修改 API client：JSON body 才加 `Content-Type: application/json`；FormData 不设置 Content-Type，由浏览器生成 boundary。三个写操作使用现有 `useIdempotentMutation`，上传 mutation 每次执行时从 File 重新构造 FormData。
3. 后端 OpenAPI 完成后运行 `pnpm --dir apps/web api:generate`；不得手改 `generated.ts` 绕过漂移检查。
4. summary 显示整批计数和金额，明细按服务端 disposition filter 分组/分页，不能只分组当前混合分页。
5. commit/discard 后至少失效 `transactions`、`transaction-summary`、`transaction-categories`、imports list/detail query；跟随现有统一刷新额外失效 ledger-accounts 也允许。
6. 文案：
   - pending：源账单尚未完成，本批次暂不计入收支；完成后重新导出上传，新批次重新判断。
   - neutral：渠道标记为不计收支或中性资金移动，例如充值、提现、还款、账户间资金移动；本期不写入收支账本。
   - closed：渠道已关闭或取消，不写入账本。
   - zero_amount：成功但实付 0 元，保留在导入记录中，不写入正式收支账本。
   - unknown：发现未支持的渠道状态，为防止错账，本批次已阻止确认。
   - duplicate：同用户、同渠道、同交易单号已存在；已归档交易也视为存在。
7. 固定提示：本期不做账户映射，导入交易 `account_id=NULL`，会进入交易列表和收支统计，但不会改变任何资金账户余额。
8. 状态按钮：preview 显示“确认入账/放弃”；blocked 仅“放弃”；committed 显示“撤销导入”；discarded 无写操作。
9. commit 后用服务端响应刷新 summary/records，展示并发阶段新增的 duplicate。
10. 撤销返回 retainedModifiedCount>0 时，明确提示仍保留多少条用户已编辑或归档的交易。
11. 历史同 hash 只做提示，不阻止用户继续。

## 十、测试与验收

### 10.1 脱敏 fixture

放在 `apps/api/tests/fixtures/`，所有人名、商户、账号、交易单号均为虚构。

支付宝 fixture 必须覆盖：

- GB18030、CRLF、表头前说明行、完整表头、可选尾随空列。
- 9 种实测状态。
- 三种方向。
- 0 元成功交易。
- external_id 尾部 `\t`。
- 字段内逗号。
- pay_method 含 `&优惠`、空 pay_method。
- source_note。

微信静态 xlsx fixture 必须覆盖：

- 原生 DateTime/Float。
- `/` 方向与 `/` 可选文本。
- `已退款(¥x)` 与 `已退款¥x`。
- 动态 `xxx-退款` 交易类型。
- source_note。

fixture 中不得出现真实姓名、账号、手机号或真实交易单号。

### 10.2 parser 失败测试

至少覆盖：

- 未知状态生成 blocked batch 所需 Unknown，而非静默放行。
- 未知/空方向。
- 负数、NaN、Infinity、超上限、非法小数。
- 成功状态零金额生成 ZeroAmount。
- 空 external_id、文件内重复 external_id。
- 缺失/重复表头、多个 worksheet/header 命中。
- GB18030 解码错误。
- 部分空行和完全空行。
- 单元格类型错误和资源上限。
- 除未知状态外每个失败都断言数据库无 batch、records、ledger 写入。

### 10.3 API/事务测试

至少覆盖：

- upload 不写 ledger，preview duplicate 准确。
- batch+records 原子写入和失败回滚。
- commit 全批原子。
- targeted external-id 冲突变 duplicate；其他约束错误整批回滚。
- payload mismatch 返回 409。
- 相同 Idempotency-Key 重放同一响应；不同 key 重复 commit/discard 返回 409。
- blocked 不可 commit。
- 两个 batch 对同 external_id 先后或并发确认，最终只新增一条。
- 其他用户访问 batch 统一 404。
- version=1 且未归档的导入交易被撤销物理删除。
- version>1 或 archived 的交易被保留，provenance 和 staging transaction_id 仍在。
- 删除交易后 staging transaction_id 变 NULL。
- import `account_id=NULL` 会改变交易/收支统计，但不改变 `ledger_account_movements` 和 `ledger_account_balances`。

### 10.4 完成标准

1. Rust：`cargo fmt --check`、`cargo test`、`cargo clippy -- -D warnings` 通过。
2. Web：`pnpm --dir apps/web test`、`pnpm --dir apps/web typecheck`、`pnpm --dir apps/web build` 通过；OpenAPI 生成文件无漂移。
3. E2E：现有 `transaction-flow.spec.ts` 仍针对拆页前的旧 UI，施工时先按当前“流水”独立路由修正旧断言，再增加导入流程用例；最终 `pnpm test:e2e` 通过。不得把旧基线失败误算成本功能回归，也不得直接删除测试规避。
4. migration 12 注册、升级、备份相关测试通过。
5. 主帅使用真实两份文件运行独立脚本，纯 parser 聚合精确命中第六节全部数字。
6. 同一文件连续两次上传→确认：第二批中所有基础 Import 为 duplicate；pending/neutral/closed/zero 的分类不变；第二次新增 ledger=0，总笔数不变。
7. 支付宝真实文件确认成功写入 157 笔；4 笔 zero_amount 留 staging，不打挂事务。
8. 浏览器实际走通：上传 → 预览 → 确认 → 交易列表可见 → 编辑一条 → 撤销 → 未编辑记录消失、编辑记录保留。
9. blocked、历史同 hash、duplicate、zero、账户余额不变等文案在真实 UI 中可见且与后端结果一致。

## 十一、隐私与日志

- 服务端不得持久化上传字节。
- production log 不得写文件内容、完整行、external_id、交易对方、商品、pay_method、source_note。
- parser error 可向当前认证用户返回行号、字段名和截断后的错误值；服务端日志只记录错误类别、渠道、行号、request_id。
- 真实验收脚本只打印聚合计数和金额，不打印单条明细。
- 文件名只存规范化 basename。

## 十二、明确不做

- 不接 IMAP / POP3 / SMTP，不碰 `email.rs`。
- 不做招商银行 parser。
- 不做跨渠道模糊去重或“疑似重复”。
- 不改交易 kind CHECK，不加 transfer。
- 不做账户映射，不创建 ledger_accounts。
- 不做小额聚合、自动分类、规则引擎。
- 不保存支付宝对方账号，不扩联系人功能。
- 不重构无关的 transactions/accounts/debts 模块。
- 不做 XLSX zip entry 未压缩总量预检；本期仅有文件、sheet、列、行上限，残余 zip bomb 风险留待后续安全硬化。
