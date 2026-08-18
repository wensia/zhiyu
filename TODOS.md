# TODOS

来源：2026-08-16 的两轮代码审查（Claude `/review` + Codex `/codex review` 两批）。
已修的不在此列，只记**尚未处理**的。每条都标了「我验过没有」——没验过的不要当成事实。

## 产品 backlog

- [ ] **外部插件商店** — 2026-08-17 决定暂不做。插件式架构（根 / 内置插件 / 外部插件）见
      `docs/plans/plugin-architecture-2026-08-17.md`；届时需另设计沙箱、签名、分发、权限撤销，
      不复用内置插件接口宣称安全。
- [ ] **纯本机版（不部署也能用）** — 2026-08-17 决定暂缓，先只做服务器版。方案与施工图见
      `docs/plans/product-architecture-2026-08-17.md`、`desktop-topology-review-2026-08-17.md`。

## 待定：需要业务决策

- [ ] **`bind_import_account` 绕过乐观锁** — `apps/api/src/imports/mod.rs:750`
      绑定账户改了 `account_id` 却不递增 `version`。后果不是 Codex 说的「discard 误删」
      （`retained_modified_count` 表明 discard 保留 version≠1 是刻意的，而 bind 属批次级
      导入收尾，整批丢弃时一并删掉说得通），而是 **`update_transaction` 的
      `WHERE version=?` 并发控制被绕过**：用户基于旧 `account_id` 提交编辑不会 409。
      **决策点**：`version` 目前同时承担「乐观锁」和「用户是否改过」两个职责，
      要么 bind 递增 version（discard 将不再删这些行），要么拆出独立的 `user_modified` 标记。
      *已验证：是。变量名与两处 SQL 都读过。*

- [ ] **退款两侧方向的真实形态** — `apps/api/src/imports/duplicates.rs:306`
      已加金额守卫与 transfer 排除。Codex 建议再要求 direction/kind 兼容，但真实退款
      可能是平台 expense 对银行 income（退款到账），要求全等会杀掉真匹配。
      需要看真实账单里退款两侧到底长什么样再收紧。
      *已验证：fixture 里两侧都是 expense；真实数据未看。*

- [ ] **联系人只能新增和改名，删不掉也归不了档** — `apps/api/src/lib.rs:203-206`
      路由只有 `GET`/`POST`/`PATCH`，而 `UpdateCounterpartyRequest` 只含
      `displayName`/`note`/`version`。表里有 `archived_at` 字段，接口却不暴露。
      后果：删掉某人名下最后一笔债务后，「按联系人」页永久留一张 0 笔的空卡片，
      界面上没有任何清理入口，只能进数据库删。
      **决策点**：给联系人加归档（软删，`archived_at` 已就位）还是加真删除
      （零引用时才允许）。前者与债务归档一致，后者更符合「这人根本不该在这儿」。
      *已验证：是。2026-08-17 实测——建一笔测试债务再删掉，空联系人卡片留在页面上，
      `DELETE /counterparties/{id}` 返回 405，最后用 SQL 清的。*

## 代码层面成立、当前数据下不触发

- [ ] **currency 校验缺失（两处）** — `mod.rs:1092`/`:1225` 的 external-ID 碰撞检查未比较
      currency；`mod.rs:517` 的 commit 不校验账户币种与 `candidate.currency`。
      四个渠道当前全是 CNY，接第五个渠道（外币卡）前必须补。
      *已验证：`model.rs:114` 的 currency 确为动态字段，代码层面成立。*

- [ ] **微信金额浮点解析** — `apps/api/src/imports/wechat.rs:323`
      `(value * 100.0).round()`，`1.005 → 100` 分（应 101）、`0.145 → 14`（应 15）。
      根因是十进制小数在 f64 的下溢。微信账单是两位小数，误差远小于 0.5，round 能纠正，
      故当前不触发；三位小数才咬人。修法：加整数分容差检查，或从原始字符串定点解析。
      *已验证：是，实算过。*

- [ ] **`checked_mul` 双 None 相等** — `duplicates.rs:288`
      `a.checked_mul(1000) == b.checked_mul(1001)` 两侧同时溢出会得到 `None == None` → true。
      `amount_cents <= 9007199254740991` 的 CHECK 下不可达，属埋雷。
      *已验证：是，两轮审查独立发现同一条。*

## 未验证 —— Codex 提出，我没能力或没条件核实

以下来自 Codex，**我没有逐条验证**，不背书也不否定。

- [ ] `duplicates.rs:405` — 簇提升丢失时间戳资格条件，秒级精确匹配可能被误提升
- [ ] `duplicates.rs:253` — ambiguous 边绕过 `used` 集合
- [ ] `duplicates.rs:238` — 贪心优先级 + `used` 可能产出少于最大二分匹配的配对数
- [ ] `duplicates.rs:198` — `NOT EXISTS` 把已 dismissed 的交易也永久排除在未来匹配之外
- [ ] `duplicates.rs:286` — `withdraw_fee` 接受充值文本，未要求平台 transfer / 银行 income / platform > bank

以下四条需要真实银行 PDF 才能验，而脱敏纪律禁止样本入库：

- [ ] `cmb.rs:185`/`cmbc.rs:140` — 表头未识别或日期被拆词时整页静默 `continue` 丢失
- [ ] `cmb.rs:199`/`cmbc.rs:154` — 相邻日期中点分桶，多行单元格跨中点会串到相邻交易
- [ ] `cmb.rs:200`/`cmbc.rs:155` — 末行用固定 48pt 回退边界，页脚/汇总词可能混入最后一笔
- [ ] `cmb.rs:347` — 先删所有逗号会把 `1,23.45` 静默解析成 `123.45`；不支持括号负数与全角数字

## 工程卫生

- [ ] **测试间歇失败** — `apps/api/tests/api_flow.rs:122`（verify-email 断言返回 500）
      四次全量跑中失败一次，单独跑通过。邮件目录是每 TestApp 独立 tempdir，非跨测试串扰。
      **根因未定位**，最可能是并行下的资源竞争。会让 CI 不可信。

- [ ] **clippy** — `duplicates.rs:475` `ListUnit` 两个变体差 576 vs 48 字节，建议 Box 大的那个

- [ ] **Codex 覆盖缺口** — 已过 Codex 的只有 `imports/mod.rs`、`duplicates.rs` 与四个解析器。
      `categorize.rs`、`self_transfer.rs`、`categories.rs`、10 个 migration、整个前端**未过**。
      首次全量尝试（9500 行 + 43 新文件）超时零产出；有效粒度是单批 ~3000 行以内。

- [ ] **`wechat.xlsx` 命名** — `tests/fixtures/wechat.xlsx` 不带 `synthetic` 后缀，与另三个不一致
      （内容已核验干净：首行「静态脱敏测试 fixture」，全是 `虚构XX`/`FAKE-WX-XXXX`）

- [ ] **`transactionDebtDirection` 无自身防护** — `apps/web/src/features/transaction-debt.ts:13`
      transfer 会落进 `borrow_in`，当前靠调用方 `:779`/`:805` 的守卫拦住

- [ ] **无法清空分类** — `transactions.rs:253` 的 `category_id=COALESCE(?10, category_id)`
      传 null 保留原值，用户没有取消分类的路径。设计权衡，非 bug。
