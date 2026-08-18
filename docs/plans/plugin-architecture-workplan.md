# 插件式架构开工计划（Codex 分批执行）

- 依据：`docs/plans/plugin-architecture-2026-08-17.md`（最终方案）。本文只讲怎么动手。
- 执行方式：Claude 出批次与验收线 → Codex 执行一批 → Claude 独立验收（`pnpm check` + 对账 + 人工过一遍）→ 下一批。**每批一个可回滚的单元。**
- 全局验收线（每批都要过）：`pnpm check`（fmt / clippy -D warnings / cargo test / web check）全绿；本批之外的行为不变；**任何一批结束后账户余额与月度收支统计一个数不变**。
- 全局约束：不改 `apps/desktop`；不碰部署配置；不提交 git（由作者决定）；SQL 迁移只增不改历史文件；不把真实账本数据写进代码、测试或输出。

## 现状事实（Codex 开工前必须知道）

- 后端 `apps/api`：Axum + libsql/SQLite。迁移是 `apps/api/src/db.rs` 里 `include_str!` 常量按序执行（最新 `0023`）。测试 `apps/api/tests/api_flow.rs` 起完整 TestApp。
- 核心目前直接认识插件（要拆的四处）：
  1. `apps/api/src/transactions.rs:29` `TRANSACTION_SELECT` LEFT JOIN `repayment_events` / `debt_addition_events` / `debts` / `counterparties` 算 `debt_link`（`DebtLinkView`，`domain.rs`）。
  2. `apps/api/src/transactions.rs:379,403` 统计/日历汇总 SQL 用 `NOT EXISTS (...repayment_events / debt_addition_events / debts...)` 排除债务流水。
  3. `apps/api/migrations/0016_debt_transaction_links.sql` 重建的 `ledger_account_movements` 视图：除流水外，还把 `debts.origin_kind='cash_movement' AND transaction_id IS NULL` 的本金、以及未挂流水的追加/还款事件算进余额。
  4. `apps/api/src/transactions.rs:14-17,26`：核心 `use crate::debts::{ensure_active_ledger_account_if_present, idempotency_key, replay_idempotency, request_hash, store_idempotency, validate_note}` 与 `crate::imports::normalize_counterparty`；`imports/mod.rs:32,36` 也引用 debts 里的这些工具。
- 债务本身**不创建流水**：`debts.rs:210` INSERT 时接受客户端传入的可选 `transaction_id`（前端有「关联已有流水」选择器）；不传则只在视图里算余额。
- 流水表 `ledger_transactions`（`0013`）：`kind IN ('income','expense','transfer')`，有 `account_id` / `transfer_from_account_id` / `transfer_to_account_id`、`category_source`、`event_id`（`0018`）、`payee_key`。
- 前端：导航项在 `apps/web/src/navigation.ts`（静态数组），路由在 `apps/web/src/App.tsx:210-227`；功能目录 `apps/web/src/features/`（debt-workspace / transaction-workspace / import-workspace / account-workspace）。
- 本机有一份真实账本快照可做迁移演练：`~/Library/Application Support/app.zhiyu.desktop/backups/2026-08-17T00:24:30Z.db`（**只许复制到临时目录操作，不许改原件，不许把内容写进任何产出**）。

## 进度（2026-08-17 ~ 08-18）

**全部批次已完成并验收。** 41 个文件、约 +17.4k/−3.4k 行，8 个新迁移（0024–0031），全部真实快照演练余额/统计一致；`pnpm check` 全绿。**未提交，待作者审阅后自行 commit。**


| 批次 | 状态 | 备注 |
|---|---|---|
| A | ✅ 已验收 | Codex 17 分钟。`idempotency.rs`、`plugins.rs`、`GET /api/v1/plugins`、前端 `plugins/registry.ts`；`transactions.rs` 不再引用 debts/imports。附带修了工作树里 3 处早已存在的门禁阻塞（`transaction-workspace.tsx` lint/类型缺口——日历面板「查看」只补了类型未接线；桌面端过期断言）。 |
| B | ✅ 已验收 | Codex 约 2 小时（最后一轮卡死，由 Claude 接手收尾）。`0024` 迁移 + `transaction_auto_created` 列 + 应用层自动建/同步/归档流水 + `tests/debt_cash_migration.rs` 对账测试 + `bin/debt_migration_drill.rs`。真实快照演练：本金 4 / 追加 3 / 还款 3 条，8 账户 13 个月，余额与统计逐项一致。Claude 补守卫：本金 − 追加 ≤ 0 的记录跳过（否则 CHECK 让迁移中止）；这类异常记录仍留在视图债务分支，**批次 C3 删视图分支前必须先统计并处理它们**。 |
| C1 | ✅ 已验收 | `0025` `pnl_scope` + 索引；统计/日历 SQL 改用 `pnl_scope='counted'`，`transactions.rs` 已无 `NOT EXISTS`；债务插件在 13 条路径维护语义；`tests/transaction_pnl_scope_migration.rs`；演练 0024→0025：11 笔回填为 excluded，余额与统计一致；前端流水行显示「不计入收支」。 |
| C2 | ✅ 已验收 | Codex 19 分钟。`0026 transaction_links` + 回填；`TRANSACTION_SELECT` 改子查询聚合 JSON、不再 join 任何债务表；`DebtLinkView`→通用 `TransactionLinkView`，`links: Vec<_>`；债务插件在 13 条路径维护关联（含联系人改名刷新 label）；前端 registry 加 `linkHref/linkLabel`，`debtLink` 全清；`tests/transaction_links_migration.rs`；演练 25→26：11 条关联逐项一致，余额/统计一致。 |
| C3 | ✅ 已验收（1 项转 D1） | Codex ~50 分钟（末尾再次卡死，Claude 接手）。`0027 created_by` + 回填、`ledger_account_movements` 只含流水四段、`db.rs` 迁移守卫（追加 > 本金则拒绝启动）、`delete_transaction` 归档时解除 links 并改为 200 返回归档后流水、债务插件 `reconcile_transaction_links`（启动 + 读取前）、前端删除提示。演练 26→27：debts 10 / bill-imports 1769 / 异常 0，余额统计一致。**遗留**：`discarding_imported_linked_repayment_nulls_link_and_restores_debt_movement` 红——导入丢弃硬删被关联流水后，还款「经过账户」却无流水（旧行为靠视图债务分支补回）；按铁律 2 应由债务插件在「流水被删除」通知后重建自动流水——归入 D1。 |
| D1 | ✅ 已验收 | Codex 26 分钟。`lifecycle.rs`（`after_transactions_written` 建议→核心落库；`after_transactions_deleted` 删除前插件快照 + 删除后分发）；`plugins.rs` 注册 `suggestion_providers` / `deletion_handlers` 与 `owns_transactions`；`0028 category_rule_id`；`POST /categories/rules/{id}/revert`；导入提交不再直接调 categorize；核心 `delete_transaction` 对独占插件的流水返回 409；债务处理者对「有账户的记录」在流水被硬删后重建自动流水（C3 遗留测试改为 `..._recreates_debt_movement`）。演练 23→28 一致，服务可起。 |
| D2 | ✅ 已验收 | Codex 18 分钟。核心写入函数 `insert/update/archive/hard_delete_transaction_row`（`OnExternalConflict` 显式选项）；imports/duplicates/debts/categories 全部改走核心，插件目录无任何对 `ledger_transactions` 的写 SQL；`0029` 回填导入关联（1769 条一致）；前端 registry `bill-imports` 徽标跳转导入详情。演练 23→29 一致；门禁全绿。 |
| F | ✅ 已验收 | Codex 23 分钟。`0030 plugin_settings`；`GET/PATCH /api/v1/plugins`（重开先自检、返回 reconciled）；路由前缀→插件映射在 `plugins.rs`，中间件对已关闭插件 409 `plugin_disabled`；`lifecycle.rs` 按用户过滤提供者/处理者；前端 `plugins/context.ts`+`state.tsx`、导航过滤、`PluginDisabledPage`、徽标置灰、`/app/settings/plugins` 设置页。Claude 修了 2 条 react-refresh lint 警告；门禁全绿；真实快照 23→30 迁移后服务可起。 |
| E1 | ✅ 已验收 | Codex 16 分钟。`0031 dashboards/dashboard_widgets`；`/api/v1/dashboards*`（CRUD、default、widgets 整体替换、widget-types）；`/api/v1/statistics/aggregate`（day/month/category/account，固定 counted+未归档+非转账，≤366 天）；核心 4 组件契约在 `dashboards.rs`，`plugin:debts:overview` 在 `plugins.rs`；前端 client/types 就位；门禁全绿。 |
| E2 | ✅ 已验收 | Codex 31 分钟。`features/statistics/`（workspace / grid / controls / widgets / shared / period）+ `react-grid-layout`；页签 button-tabs、月份控件、编辑/完成、添加组件 Sheet、组件配置、占位卡、空状态、防抖全量保存与失败回滚；vitest 149 全绿；`e2e/design-contract.spec.ts` 加默认布局断言。Claude 用真实浏览器（Playwright 注册临时用户）走完 空态→默认布局→编辑→添加插件组件→只读→插件页→窄屏，截图核对；修了 3 处：添加组件抽屉正文无内边距、插件设置行无内边距（用了不存在的 `--space-5`）+ 多余边框、抽屉里空插件组不该出现；组件空态改居中。 |

## 批次

### 批次 A：通用工具上移 + 插件注册表骨架（行为不变）

目标：核心不再 `use` 任何插件目录；两端各有一张插件名单；界面与接口行为零变化。

1. 新建 `apps/api/src/idempotency.rs`：把 `debts.rs` 里 `idempotency_key` / `request_hash` / `replay_idempotency` / `store_idempotency`（`debts.rs:1450-1600` 附近）原样搬过去（含它们依赖的表/类型），`debts.rs`、`imports/mod.rs`、`transactions.rs` 改为引用新模块。
2. `validate_note` 搬到 `domain.rs`；`ensure_active_ledger_account_if_present` 搬到 `accounts.rs`；`imports::normalize_counterparty` 搬到 `domain.rs`（或新建 `apps/api/src/counterparty.rs`），`imports` 改为引用核心。
3. 新建 `apps/api/src/plugins.rs`：内置插件常量表 `[{ id: "debts", name: "债务" }, { id: "bill-imports", name: "账单导入" }, { id: "auto-categorize", name: "自动分类" }]`，字段：`id`（永久稳定）、`name`、`description`；加 `GET /api/v1/plugins` 返回名单（当前全部 `enabled: true`，无开关）；进 OpenAPI（`export_openapi` 与 `apps/web/src/api/generated.ts` 若有生成流程则同步）。
4. 前端新建 `apps/web/src/plugins/registry.ts`：同样三条，且每条声明它贡献的导航项（`{ path, label, mobileLabel, icon, group }`）；`navigation.ts` 改为「核心导航项 + 从 registry 收集插件导航项」拼出 `navigationItems`，**顺序与现在完全一致**（债务、日历、流水、统计、账户）；`App.test.tsx` 现有断言不变。
5. 验收：`pnpm check` 全绿；`grep -n "crate::debts\|crate::imports" apps/api/src/transactions.rs` 为空；`grep -rn "use crate::debts" apps/api/src/imports/` 为空；`GET /api/v1/plugins` 有集成测试；界面截图级无差异（导航顺序、文案不变）。

### 批次 B：债务迁数（余额一个数不变）

目标：所有「动过现金但没挂流水」的债务本金 / 追加 / 还款都补成等价流水并链接；迁移前后每个账户余额、每月收支统计**完全一致**；此后新建的现金变动债务/追加/还款若未指定 `transaction_id`，服务端自动创建流水并链接。

1. 迁移 `0024_debt_cash_movements_to_transactions.sql`（或在 `db.rs` 里用 Rust 做，若 SQL 表达不了）：
   - `debts`：`origin_kind='cash_movement' AND account_id IS NOT NULL AND transaction_id IS NULL` → 插入 `ledger_transactions`（`kind`：`borrow_in`→`income`，`lend_out`→`expense`；`amount_cents=principal_cents`；`occurred_on=debts.occurred_on`；`account_id`；`currency`；`note`/`description` 写明「债务本金」+ 方向；`user_id`；`created_at/updated_at`），并回填 `debts.transaction_id`。
   - `debt_addition_events`（追加借款，`account_id IS NOT NULL AND transaction_id IS NULL`）：`borrow_in`→`income`，`lend_out`→`expense`。
   - `repayment_events`（`account_id IS NOT NULL AND transaction_id IS NULL`）：按视图现有符号——`(lend_out AND kind='payment') OR (borrow_in AND kind='reversal')`→`income`，否则→`expense`。
   - **不动** `ledger_account_movements` 视图（它的债务分支在 `transaction_id` 回填后自然为空，余额因此按构造不变）；视图清理放批次 C。
   - 现有统计的 `NOT EXISTS` 排除靠 `transaction_id` 关联生效，因此统计也不变。
2. 应用层：`debts.rs` 创建债务 / 追加 / 还款时，若 `origin_kind='cash_movement'`（或有 `account_id`）且未提供 `transaction_id` → 同一事务内自动创建流水并链接；提供了 `transaction_id` 则沿用现状。撤销还款（reversal）沿用现有事件模型，同样自动创建对应流水。
3. 对账测试（`apps/api/tests/`）：构造「迁移前形态」的数据（在应用 `0024` 之前插入未挂流水的债务/追加/还款）→ 记录每个账户余额与每月收支 → 应用 `0024` → 断言逐项相等，且每条被迁移的事件都有 `transaction_id`。若迁移运行器不支持「跑到某一版」，为测试加一个只在 `cfg(test)` 下可用的入口。
4. 真实数据演练：把 `~/Library/Application Support/app.zhiyu.desktop/backups/2026-08-17T00:24:30Z.db` **复制**到临时目录，用 `backup_drill` 风格脚本或一次性 Rust 测试跑迁移，输出「迁移前/后余额与月度统计逐项一致：是/否」以及迁移了多少条；只输出计数和一致性，不输出任何金额明细。
5. 验收：`pnpm check` 全绿；对账测试通过；演练报告「一致」；前端流水页能看到新补的债务流水（带现有 debt_link 标签）——这是预期变化，写进本批说明。

### 批次 C：拆三处耦合，债务成为第一个内置插件

目标：`transactions.rs`、`ledger_account_movements` 不再出现任何债务表名；核心新增「算账语义」「关联」「来源」三种通用标记；债务插件改用它们；余额与统计一个数不变。分三步各自对账。

- C1 算账语义：迁移 `0025`：`ledger_transactions` 加 `pnl_scope TEXT NOT NULL DEFAULT 'counted' CHECK (pnl_scope IN ('counted','excluded'))`；回填：所有已链接债务的流水 → `excluded`。统计与日历汇总 SQL 改为 `AND t.pnl_scope='counted'`，**删除**三处 `NOT EXISTS`。债务插件在创建/链接流水时设 `excluded`，解绑时设回 `counted`。前端流水页在筛选/展示上把 `excluded` 流水标为「不计入收支」。
- C2 通用关联：迁移 `0026`：`transaction_links(id, user_id, transaction_id, plugin_id, kind, ref_id, label, created_at, UNIQUE(transaction_id, plugin_id, kind, ref_id))`；从 `debts.transaction_id` / `debt_addition_events.transaction_id` / `repayment_events.transaction_id` 回填（`plugin_id='debts'`，`kind ∈ principal|addition|repayment`，`ref_id=debt_id`，`label=联系人名`）。`TRANSACTION_SELECT` 改为对 `transaction_links` 的聚合，`DebtLinkView` → 通用 `TransactionLinkView { plugin_id, kind, ref_id, label }`（`domain.rs`、OpenAPI、`apps/web/src/api/types.ts` 同步）。前端流水页标签由 registry 里插件声明的 `linkHref(kind, refId)` 生成跳转（债务：`/app/debts/:id`）。债务插件在建链/解链时维护 `transaction_links`；`debts.*.transaction_id` 列保留为插件私有。
- C3 来源与余额：迁移 `0027`：`ledger_transactions` 加 `created_by TEXT NOT NULL DEFAULT 'user'`（值：`user` | `import` | `plugin:debts`），回填批次 B 补出的流水与已链接的债务流水为 `plugin:debts`，导入生成的为 `import`。重建 `ledger_account_movements` **只看流水**（删除债务分支）。删除规则：删/归档一条 `created_by='plugin:debts'` 的流水时，核心解除其 `transaction_links` 并返回来源信息供前端提示（前端删除确认框显示「这笔由债务插件创建，删除会解绑还款」）；反向：删除债务事件时归档它自动创建的流水（用户显式关联的流水不动）。
- 验收：`grep -n "repayment_events\|debt_addition_events\|debts d\|counterparties" apps/api/src/transactions.rs` 为空；`grep -n "debts\|repayment_events\|debt_addition_events" apps/api/migrations/0027*.sql` 只出现在回填语句里、视图定义里没有；对账测试跨 B/C 仍然一致；`pnpm check` 全绿；e2e（`apps/web/e2e/transaction-flow.spec.ts`）通过。

### 批次 D：导入与自动分类接入（后续）
导入（含去重、转账识别）通过核心写入路径创建流水（`created_by='import'`，转账落为 `kind='transfer'` 核心事实，`transaction_links(plugin_id='bill-imports', kind='batch', ref_id=import_id)`）；自动分类改为「流水生命周期」提交分类建议（`category_source='rule'` 已有，加可追溯/可撤销）。

### 批次 E：统计吃通用语义 + 网格面板（后续）
### 批次 F：设置里的插件开关页（后续）

## 交接给 Codex 的方式
每批一个独立 Codex 任务，提示词 = 本文对应批次全文 + 「现状事实」段 + 全局约束；要求 Codex 结束时给出：改动文件清单、迁移条数、验收命令的实际输出、未完成项。Claude 收到后独立复跑 `pnpm check` 与对账，再放行下一批。
