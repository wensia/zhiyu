# 知余：服务端权威 + Tauri 瘦客户端迁移计划

> 状态：**已评审，可执行**
> 分支：`feat/thin-client-server`
> 生成：2026-08-09
> 执行者：codex

## 事实基线（直读代码核实，不引用 docs/module-architecture.md）

> 依据 learning `zhiyu-docs-lag-behind-code`（confidence 9/10）：该文档滞后于实现，
> 架构判断一律以代码为准。以下每条都标注了核实来源。

| 事实 | 来源 | 结论 |
|---|---|---|
| 服务端已部署且在线 | `curl https://zhiyu.askfish.net/readyz` → HTTP 200 | 「部署到服务器」**已完成** |
| 真实账本在服务器 | 作者确认 | `/opt/zhiyu/data/preview.db` 是权威数据 |
| 桌面库无真实数据 | `app.zhiyu.desktop/zhiyu.db`：`ledger_transactions=0`、`ledger_accounts=0`、`debts=0` | 桌面端**无需数据迁移** |
| production 会拒绝启动 | `config.rs:19-20` `bail!("production requires a real EmailSender")` | 线上靠 `APP_ENV=development` 绕过（`Dockerfile:50`） |
| 完整密码认证已存在 | `auth.rs` register/login/logout/session cookie | **不需要新建认证** |
| 前端用相对路径 | `client.ts:39` `fetch(\`/api/v1${path}\`)` | 换窗口 URL 即可，前端**零改动** |
| 写操作已带幂等键 | `client.ts` 每个写方法带 `Idempotency-Key`；`transactions.rs:119,165,213` 走 `replay_idempotency` | 离线重放的服务端保护**已就位** |
| 前端无任何离线机制 | 仅 sidebar 折叠状态用 localStorage | 离线队列是**纯新建** |
| React Query 已在用 | `package.json` `@tanstack/react-query ^5.87.1` | 离线队列走官方持久化，**不自造** |

## 目标架构

```
                        ┌─────────────────────────────┐
                        │  常在线服务器                │
   Tauri 壳             │  zhiyu.askfish.net          │
   ┌──────────┐         │  ┌───────────────────────┐  │
   │ WebView  │────────▶│  │ Axum /api/v1          │  │
   │ 远程 URL │  session│  │ (权威，持明文)         │  │
   └──────────┘  cookie │  └───────────┬───────────┘  │
        │                │              │              │
   ┌────▼─────┐          │      ┌───────▼────────┐    │
   │ 离线队列 │          │      │ /data/preview  │    │
   │ IndexedDB│          │      │     .db        │    │
   └──────────┘          │      └───────┬────────┘    │
                         │              │              │
   hermes / openclaw ───▶│  API 密钥    │              │
   （外部系统）  Authorization header    │              │
                         │      ┌───────▼────────┐    │
                         │      │ VACUUM INTO    │    │
                         │      │ + restic       │────┼──▶ 腾讯云 COS
                         │      └────────────────┘    │
                         └─────────────────────────────┘
```

**信任模型的显式变更**：服务器持明文数据库，是唯一权威副本。这与旧 local-first
设计（远端只存密文、设备持密钥）根本冲突，为已知且接受的取舍，见 `docs/adr/0001`。

## 评审已定决策

| # | 议题 | 决定 |
|---|---|---|
| 1 | production 门禁死锁 | **拆开邮件与生产标志**：新增 self-host 模式，走生产安全行为但不要求真邮件 |
| 2 | 幂等键在发送时现算 | **提到调用方生成并持久化**，随 mutation variables 一起进队列 |
| 3 | `ensure_local_session` 孤儿化 | **改造成 API 密钥发放**，回归测试跟着迁移 |
| 4 | 机器认证形式 | **单一 API 密钥**，走 `Authorization` header，全权限，不拆 scope |

---

## 执行状态（2026-08-10）

| 任务 | 状态 | 备注 |
|---|---|---|
| T1 备份（手动） | **已完成** | 本机 + 服务器各一份，integrity ok / fk 0，双重校验 |
| T1 备份脚本 | **已返工，未验证** | 服务器**未装 restic**，脚本尚未实跑过 |
| T4 桌面壳瘦身 | **已完成** | 净删 2945 行，`lib.rs` 425→76 |
| T2 self-host 模式 | **已完成并上线** | 邮件路由返回 `email_unavailable`，已线上验证 |
| T3 API 密钥 | **已完成并上线** | Bearer 认证、CSRF 豁免均已线上验证 |
| T5 git 备份下线 | **已完成** | `backup.rs` 873→573 |
| T6 HTTPS/cookie 实测 | 待做 | 需真实 Tauri 窗口验证 WebView 持有 `__Host-` cookie |
| T7 文档 | **已完成** | ADR-0001 + supersede 标记 |
| **生产部署** | **已完成** | 见下 |

### 生产部署结果（2026-08-10 00:59）

镜像重建并重启，`zhiyu` 容器 healthy。构建前抓到并修掉一个阻塞：
`Cargo.toml` 的 workspace 含 `apps/desktop/src-tauri`，而 Dockerfile 只 `COPY apps/api`，
cargo 加载 workspace 时因缺 member manifest 整体失败。补 stub manifest 解决（`10d0046`）。
8-04 那次构建时桌面端尚未进 workspace，故此前未暴露。

线上验证全部通过：

| 项 | 结果 |
|---|---|
| 迁移版本 | 7 → **10**（8/9/10 全部应用） |
| 新表 | `ledger_transactions`、`api_keys` 均存在 |
| 原有数据 | `debts=5` `repayments=28` `accounts=3`，integrity ok |
| `/readyz` | HTTP 200 |
| `GET /api/v1/transactions` + Bearer | HTTP 200 |
| self-host 邮件路由 | `email_unavailable` |
| API 密钥读取真实账本 | 通过 |
| 无 Origin 的 Bearer 写请求 | HTTP 422（非 403，证明 header 认证不进 CSRF 分支） |

回滚材料：`/opt/zhiyu/backups/preview.db.before-thin-client.20260810-004708`
（本机同名副本一份），旧源码目录 `/opt/zhiyu/app.bak-*`。

**遗留**：服务器未装 restic，`scripts/server-backup.sh` 至今没有实跑验证过；
签发 API 密钥的 CLI 是 `docker exec zhiyu zhiyu-api-key <邮箱>`，
密钥必须签给 `demo-20260802@zhiyu.app` 才能读到真实账本。

### 生产环境实测结论（2026-08-10）

服务器 `ubuntu@139.155.151.124`，密钥 `~/.ssh/ai-ditui-139-155-151-124.pem`。

- 数据库：`/opt/zhiyu/data/preview.db`（宿主机）← 挂载到容器 `/data`
- **生产库停在迁移 7，本地在 9**。缺迁移 8、9，因此**没有 `ledger_transactions` 表**
- 库内只有债务/往来数据：`debts: 5`、`repayment_events: 28`、`debt_addition_events: 3`、
  `counterparties: 5`、`ledger_accounts: 3`；唯一用户是 `demo-20260802@zhiyu.app`
- `zhiyu` 容器 `Up 6 days`，镜像约 8-04 构建，此后的提交（记账 `49e6cc9`、
  桌面版 `73d677f`、备份 `6eb5d73`）均未上线
- **宿主机和容器内都没有 `sqlite3`**
- `/opt/zhiyu/data` 属主是 `nobody:nogroup`，ubuntu 用户无写权限
- 没有 `-wal` / `-shm` 文件

**后果**：桌面壳现在指向 `zhiyu.askfish.net`，打开记账页会失败。阶段一必须补一步
「重建镜像 + 跑迁移 8、9」，且该步骤需要作者在服务器上执行，不能交给本地 agent。

### `scripts/server-backup.sh` 的三个阻碍

1. **路径混用容器与宿主机**（`:5-9`）。写快照到 `/data/backups`（容器内），却检查
   `/opt/zhiyu/data/backups`（宿主机）。容器内跑第 65 行必挂；宿主机跑第 40 行必挂。
2. **两边都没有 `sqlite3`**，第 37 行的前置检查直接 fail。
3. **权限**：ubuntu 用户创建 `/opt/zhiyu/data/backups` 会被拒。

另：第 4 行 `readonly PATH=...` 应去掉，restic 装在非标准路径时会找不到。

**修法**：统一按宿主机路径；sqlite3 走一次性容器
（`docker run --rm -v /opt/zhiyu/data:/src:ro -v <out>:/out alpine sh -c "apk add sqlite && ..."`，
已实测可用）；备份输出目录改到 ubuntu 有写权限的位置。

---

## 阶段一：连上远程并可用

目标：桌面壳指向服务器，删掉内嵌后端与 git 备份，服务端有可信备份。
**做完即可日常使用。**

### T1 服务端备份先行（**必须第一个做**）

服务器上有真实账本，动任何东西之前先有可回滚的备份。

新增 `scripts/server-backup.sh`（在服务器跑，不是应用代码）：

```
1. VACUUM INTO 生成快照 → /data/backups/ledger.sqlite3 + manifest.json
2. 运行 integrity_check + foreign_key_check
3. restic backup /opt/zhiyu/data/backups
4. restic forget --keep-daily 7 --keep-weekly 4 --keep-monthly 6 --prune
```

restic repo 指向腾讯云 COS（S3 兼容端点）。加密、增量、去重、保留策略全部由 restic
承担。cron 或 systemd timer 触发。

**验收**：跑一次 `restic restore` 到临时目录，用 `scripts/backup-drill.sh` 的思路
验证恢复出来的库能通过完整性校验。没有验证过的备份不算备份。

### T2 self-host 模式（解决 Issue 1）

`apps/api/src/config.rs`：

- `from_env()` 的 production 分支不再无条件 `bail!`。引入第三种 `app_env`（如
  `self-host`）或一个独立的 `REQUIRE_EMAIL` 开关，使「生产安全行为」与「必须有真邮件」解耦。
- `is_production()` 的语义改为「是否启用生产安全行为」，`self-host` 也返回 true。
- 结果：`cookie_name()` 返回 `__Host-zhiyu_session`（`config.rs:51`），
  `session_cookie_header()` 加上 `; Secure`（`auth.rs:531`）。
- 邮件相关路由（register / verify-email / forgot-password / reset-password）在
  self-host 模式下明确返回不可用，而不是静默失败。

`Dockerfile:50` 与 `deploy/docker-compose.yml` 改用新模式。

**注意**：`__Host-` 前缀要求 `Path=/` 且无 `Domain` 属性，`auth.rs:528` 当前已满足
`Path=/`，确认没有别处注入 `Domain`。

### T3 API 密钥（解决 Issue 3 + 4）

把 `auth.rs:426` 的 `ensure_local_session` 改造为 API 密钥的签发与校验：

- 保留其核心行为：幂等地确保用户存在、签发长期凭证、**只清理过期凭证而不撤销仍有效的**
  （这是 8-09 修复的 `desktop-session-entry-revocation`，必须保住）。
- 密钥走 `Authorization: Bearer <key>` header，不走 cookie。
- `lib.rs:222` 的 `csrf_guard` 无需改动：它只在 `unsafe_method && has_session_cookie`
  时校验 Origin（`lib.rs:236`），header 认证天然不进这个分支。
- 认证中间件在 cookie 解析失败后回落到 header 校验。
- 密钥通过环境变量或一次性 CLI 生成，**不入库明文**，比照 `sessions.token_hash` 存哈希。

`api_flow.rs:124,129` 两个回归测试跟着迁移到新函数名。

### T4 桌面壳瘦身

删除 `apps/desktop/src-tauri/src/`：

| 文件 | 行数 | 处置 |
|---|---|---|
| `backup_github.rs` | 1048 | 删 |
| `backup_runner.rs` | 600 | 删 |
| `backup_http.rs` | 297 | 删 |
| `backup_page.rs` | 236 | 删 |
| `backup_config.rs` | 228 | 删 |
| `backup_state.rs` | — | 删 |
| `lib.rs` | 425 → 约 60 | 删 `start_server`(:82)、`observe_successful_write`(:143)、恢复门禁(:184)、`ensure_local_session` 调用(:201)、`ENTER_PATH`(:26) |

`apps/desktop/src-tauri/Cargo.toml` 摘掉 `zhiyu-api`、`axum`、`libsql` 依赖。
桌面端不再 link 后端、不再开 SQLite 连接。

窗口指向 `https://zhiyu.askfish.net`，URL 从配置读取（开发时指向本地 dev server）。

### T5 服务端 git 备份下线

`apps/api/src/backup.rs`：删 `Committed`(:295)、`commit_snapshot`(:306)、`push`(:350)、
`git_status`(:385)、`git`(:402)，约 125 行。

**保留**：`Manifest`(:27)、`create_snapshot`(:47)、`verify_snapshot`(:83)、
`check_against_manifest`(:139)、`restore`(:170)、`quarantine_database_group`(:263)。
这些是 T1 的服务端备份要用的。

### T6 HTTPS 与 cookie 实测

T2 完成后 cookie 带 `Secure` + `__Host-` 前缀。**必须在真实 Tauri 窗口里验证**
登录、刷新、重启应用后会话仍在。WebView 的 cookie 行为不能假定与浏览器一致。

同时确认 TLS 在哪里终结：`docker-compose.yml` 只 `expose: 8790` 并接入外部网络
`ai-ditui`，说明前面有反向代理。`__Host-` 前缀要求整条链路是 https。

### T7 文档

- 新增 `docs/adr/0001-server-authoritative-thin-client.md`，显式 supersede
  `module-architecture.md` 的 D1/D4/D5 与 §1.1。
- `module-architecture.md` 顶部加 superseded 标记指向 ADR-0001。
- 为 8-07 的 design doc（跨模块时间线与日历模块）补 supersede 记录——它整份建立在
  local-first 架构上。

---

## 阶段二：离线可用

### T8 离线队列

用 `@tanstack/react-query` 官方持久化，**不自造**：
`@tanstack/react-query-persist-client` + IndexedDB persister，配合
`queryClient.resumePausedMutations()`。

### T9 幂等键改造（**最高风险项**）

`client.ts:135` 的 `const idempotencyKey = () => crypto.randomUUID()` 在发送瞬间求值。
`resumePausedMutations()` 会重新执行 mutationFn → 新 key → 服务端认不出重复 → **重复记账**。

**修法**：把 `Idempotency-Key` 从 `client.ts` 内部提到调用方参数，在创建 mutation 时
生成，随 variables 一起持久化。涉及约 18 个 api 方法签名。

**顺手做的 DRY 清理**：这 18 个方法现在每个都重复
`headers: { "Idempotency-Key": idempotencyKey() }`，抽成一个统一的写请求包装。

### T10 离线 UX 与乐观锁冲突

- 断网状态可见，不能静默失败；展示队列深度与待同步条目。
- **乐观锁在离线下会失效**：`archiveDebt(id, version)`、`updateTransaction` 等都带
  `version` 字段。离线期间服务端 version 可能已变（虽然单用户概率低，但多端时会遇到）。
  重放时 version 冲突需要明确策略：默认拒绝并把冲突条目留在队列里让用户处理，
  不要自动覆盖。

---

## 测试要求

**Rust**（`apps/api/tests/api_flow.rs`）：
- API 密钥：有效 / 无效 / 空 / 过期 四种情况
- API 密钥不触发 CSRF Origin 校验（回归：确认 header 认证走的不是 cookie 分支）
- 迁移后的 `ensure_local_session` 回归测试：重复签发不撤销仍有效的凭证
- self-host 模式下 cookie 带 `Secure` 与 `__Host-` 前缀
- self-host 模式下邮件路由返回明确不可用

**前端**（vitest）：
- **关键**：幂等键在 mutation 创建时固定，模拟离线 → 重放 → 断言请求携带**同一个** key
- 离线队列持久化后跨页面刷新仍在
- 乐观锁 version 冲突时条目留在队列且用户可见

**E2E**（Playwright）：
- Tauri 壳连远程的完整登录流程
- 断网 → 记账 → 恢复网络 → 断言只产生一笔

**备份演练**：`restic restore` → 完整性校验，纳入 CI 或至少手动记录一次结果。

## 失败模式

| 新代码路径 | 现实失败场景 | 有测试? | 有错误处理? | 用户可见? |
|---|---|---|---|---|
| 离线队列重放 | 幂等键漂移导致重复记账 | T9 必须补 | 服务端 `replay_idempotency` | **否——静默错账，critical** |
| API 密钥校验 | 密钥泄露被外部滥用 | 部分 | 无速率限制 | 否 |
| 服务器不可达 | 断网/服务器重启时记账 | T10 | 阶段一无，阶段二有队列 | 阶段一：请求失败 |
| restic 投递 | COS 凭证过期，备份静默停止 | 无 | 需要告警 | **否——直到需要恢复时才发现** |
| version 乐观锁 | 离线期间服务端已变更 | T10 | 需明确策略 | 需要设计 |

**两个 critical gap**（无测试 + 无错误处理 + 静默）：
1. 幂等键漂移 → T9 的测试是阻塞项，不能跳
2. 备份静默失败 → `server-backup.sh` 必须在失败时发出可见信号（退出码 + 日志 + 可选通知）

## NOT in scope

| 项 | 理由 |
|---|---|
| local-first 线的代码删除 | 那些 crate 在独立 worktree `products/zhiyu-local-first-m1`，不在本仓库。本次只做文档 supersede |
| 数据迁移 | 真实账本已在服务器上，桌面库为空 |
| 多用户 / 权限系统 | 个人单用户，`user_id` 隔离已足够 |
| API 密钥的细粒度 scope | 个人项目，单一全权限密钥；需要时再拆 |
| 端到端加密 | 服务器持明文是本方案的显式取舍，见 ADR-0001 |
| hermes / openclaw 的具体集成 | 本计划只提供 API 密钥；对端如何调用是各自的事 |
| Shard / Collections / Calendar 模块 | 属于已 supersede 的 local-first 路线 |
| 真实邮件发送 | self-host 模式下邮件路由直接标记不可用 |

## What already exists（复用，不重建）

| 能力 | 位置 | 本计划如何用 |
|---|---|---|
| 密码认证全套 | `auth.rs` register/login/logout/me | 直接用，不新建 |
| 服务端幂等 | `transactions.rs` + `idempotency_records` 表 | 离线重放的服务端保护 |
| 幂等地签发凭证 | `auth.rs:426` `ensure_local_session` | 改造成 API 密钥发放 |
| 快照与校验 | `backup.rs:47-160` | 搬到服务端 cron |
| 恢复与隔离 | `backup.rs:170-293` | 保留，恢复流程不变 |
| 备份演练脚本 | `scripts/backup-drill.sh` | 改造为验证 restic 恢复 |
| React Query | `main.tsx` + `App.tsx` | 离线队列的宿主 |
| 相对路径 API | `client.ts:39` | 换 URL 即可 |
| Docker 部署 | `Dockerfile` + `deploy/docker-compose.yml` | 已在跑 |

## 执行顺序与并行

```
Lane A（阶段一，顺序执行，共享 apps/api/）
  T1 备份先行 → T2 self-host 模式 → T3 API 密钥 → T5 git 下线

Lane B（阶段一，独立，只碰 apps/desktop/）
  T4 桌面壳瘦身

Lane C（阶段一，独立，只碰 docs/）
  T7 文档与 ADR

  → T6 HTTPS/cookie 实测（需要 A 和 B 都完成）

Lane D（阶段二，需要阶段一完成，只碰 apps/web/）
  T9 幂等键改造 → T8 离线队列 → T10 离线 UX
```

A、B、C 可并行。T9 必须在 T8 之前——先把幂等键修对，再上队列，否则队列一上线就在
制造错账。

## 给 codex 的执行提示

- **T1 必须最先做完并验证**，服务器上是真实账本。
- 每个 T 结束跑 `cargo test` 与 `pnpm check`。
- 不要动 `products/zhiyu-local-first-m1` worktree。
- 文档与代码冲突时以代码为准（见 learning `zhiyu-docs-lag-behind-code`）。
