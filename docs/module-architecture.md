# 知余 local-first Platform Kernel 与模块架构

> **SUPERSEDED：** 本文的 **D1、D4、D5 与 §1.1** 已由
> [`docs/adr/0001-server-authoritative-thin-client.md`](adr/0001-server-authoritative-thin-client.md)
> 作废。当前架构判断以 ADR-0001 与代码实现为准。
>
> 状态：**D1–D10 已决策；Phase 0 证据基线收口中。本文与实现候选尚未形成可复核的提交基线，不代表已合并、打包或发布**。
>
> 所有者于 2026-08-06 确认：Platform Kernel 是产品根；Finance、Shard、Collections 是同级模块；根签名 control records、加密签名的 Vault platform/domain events 与认证加密对象是唯一同步真相；JSONL 只用于显式导出；归档不改变财务余额；桌面端采用可折叠左侧边栏 + 页面，模块可固定到左侧边栏。
>
> 证据基线：2026-08-06 对 `main` 与隔离的 `codex/modular-local-first-m1` worktree 做了源码核对。两边当时均有未提交修改，因此本文中的 `CURRENT` 只描述该次审计快照，不证明代码已提交、合并、打包、部署或通过生产验收。

本文是知余产品级模块架构的规范入口。它定义产品边界、真相源、模块合同、同步与恢复不变量，以及旧 Web/API 账本迁入新架构的路线。具体协议细节与威胁模型仍由对应实现文档维护，但不得与本文的决策相冲突。

**TARGET 文档权威序**：版本化的 D1–D10 ADR（目标路径 `docs/adr/platform-kernel-v1.md`）> 本文 > `local-first-v1`、threat model 等协议/实现文档 > checklist 与测试记录。高层只决定产品边界和不变量，低层可以补充 wire format、算法与验收步骤，但不能改写上层决策；发现冲突时必须 fail closed、登记冲突并修订低层文档。Phase 0 必须提供一个可从 fresh checkout 读取的 `docs/architecture/index.md`，列出权威文档、适用 commit 与已被 supersede 的旧方案。在这些文件进入具名 commit 前，当前 working tree 只能视为候选规范。

## 0. 阅读标记

- **DECIDED**：所有者已经确认，实施不得自行改写。
- **CURRENT**：审计时在代码中可见，不等于已合并或已发布。
- **TARGET**：目标合同，进入对应阶段前必须转化为代码和测试门禁。
- **NON-GOAL**：当前范围明确不做。

## 1. 已决策的产品边界

| 编号 | 决策 |
| --- | --- |
| D1 | 知余是 local-first 个人工作空间，不是 Finance-rooted 应用，也不是 Notion clone。 |
| D2 | Platform Kernel 负责 Vault/身份、事件与对象、同步/冲突/恢复、模块宿主、全局引用/搜索/关系、调度/通知、迁移/导入/导出。财务语义不得进入平台内核。 |
| D3 | 顶层模块为 `dev.zhiyu.finance`、`dev.zhiyu.shard`、`dev.zhiyu.collections` 及未来模块。账户、凭证、债务、订阅、预算、基金属于 Finance 内部领域，不是平台级模块。 |
| D4 | V1 产品拓扑是 single-owner、multi-device、multi-vault。M1 当前仍是单 Vault/单窗口限制，不能把目标范围写成当前能力。 |
| D5 | Vault 中根签名的 control records、签名加密的 platform/domain events 与认证加密的模块原生对象共同构成规范真相；本地投影可重建；Git 是不可信复制传输；JSONL/CSV/Markdown 导出不是同步或恢复真相。 |
| D6 | V1 模块采用编译期内建 + 每 Vault 运行时启停。动态链接、WASM、第三方安装市场和不可信插件沙箱不在 V1。 |
| D7 | 归档只改变可见性和后续可写性，不改变历史余额。改变财务结果必须使用 correction、reversal 或显式 void 事件。 |
| D8 | 公共 HTTP API 路径与模块 ID 解耦。内部模块化不得把现有 `/api/v1/*` 改成 `/api/v1/mod/{key}`；破坏性 API 变化必须进入新 API 版本。 |
| D9 | 时间驱动副作用走持久调度和确定性 occurrence key。GET 请求不得隐式写账。提醒意图参与 Vault 同步，设备 OS 的待通知状态不参与同步。 |
| D10 | 桌面主壳固定为可折叠左侧边栏 + 当前页面。启用的 UI 模块可按 Vault 固定并排序；固定不等于启用或授权。固定列表参与 Vault 同步，窗口的展开/折叠状态只保存在本机。 |

### 1.1 当前非目标

- 家庭/团队共享 Vault、多 owner 权限与高频协同编辑。
- 依赖 CRDT 的通用多人协作。
- 未经签名和加密的远端业务数据。
- 将 SQLite/WAL 文件直接复制到多设备。
- 在设备撤权、签名 checkpoint 和恢复演练落地前做破坏性 compaction/GC。
- 把 Web 多用户中央服务或水平扩容伪装成已经支持的 local-first 宿主。

## 2. 当前实现边界

### 2.1 `main`：旧 Web/API 账本与兼容适配层

**CURRENT**：`main` 仍是 React + Axum + libSQL/SQLite 的邮箱账户型 Web 应用：

- `AppState` 持有数据库、配置、`EmailSender` 与限流器；路由和 utoipa OpenAPI 仍集中手写。
- 业务数据按 `user_id` 隔离，HTTP 请求使用 cookie/session 与请求幂等键。
- HEAD `f402899` 包含迁移 0001–0008；审计时 0009 transactions 只存在于 dirty worktree，不能称为已发布 baseline。
- dirty Web `AppShell` 已出现左侧边栏、`⌘/Ctrl+B` 折叠和 localStorage 记忆，但导航仍硬编码 Finance 页面；它不是模块宿主，也不证明 Tauri 桌面壳已经满足 D10。审计时两个 worktree 的 Web AppShell 都会全局劫持 `⌘/Ctrl+B`，`main` 还会全局阻止 `Tab`；这是违反 §3.3 的 CURRENT 缺陷，不是可复用的桌面交互实现。
- legacy Web 在窄视口可以保留 adapter-specific 的移动底栏，但该行为不得回灌为 Tauri Desktop Shell 模式，也不得作为 D10 验收证据。
- OpenAPI 及生成的 TypeScript 类型是 **HTTP adapter 合同**，不是整个 Platform Kernel 或 Tauri IPC 的唯一合同。
- Dockerfile/Compose 审计时固定为 development 运行方式；production 配置会因缺少真实邮件实现直接拒绝启动，镜像也尚未具备完整 Git/凭据/恢复能力。

这条实现继续承担兼容、数据盘点和导入来源，但不得发展出第二套 canonical sync。

### 2.2 `codex/modular-local-first-m1`：local-first 基础施工线

**CURRENT**：隔离 worktree 的 HEAD 与 `main` 同为 `f402899`，local-first 文件仍是 working-tree 候选而非独立分支提交；其中已能看到以下分层，但整体仍是 dirty、未合并施工状态：

- `finance-core`：复式记账领域不变量。
- `crypto-vault`、`event-protocol`：恢复根、设备密钥、签名加密 envelope 与控制链。
- `vault-sqlite`：append-only event store、staging 与可重建 projection。
- `finance-application`：command → event → promote/replay → projection。
- `sync-git`、`vault-sync`：per-device refs、控制分支、抓取验证与同步编排。
- `module-protocol`：最小 `ModuleManifest` 校验预留；当前不等于可运行的模块宿主。
- Tauri v2 桌面壳、Stronghold、`RuntimeServices` ports 与 `vaultSessionId` 作用域。
- Tauri `DesktopApp` 当前在 `VaultGate` 后直接渲染单个 `VaultWorkspace`；`module_list` 仍返回空列表，尚无模块侧栏、固定或排序能力。

**CURRENT LIMIT**：M1 当前只实现 Finance 基础与单 Vault 生命周期；对象存储、完整模块宿主、Collections、Shard、全局关系、时间调度和 multi-vault UX 仍是 TARGET。另有三项必须显式偿还的当前技术债：

- `vault-sqlite` 直接依赖 `finance-core`，尚未满足 §3.2 的 Platform 领域中立依赖方向；Phase 2 必须把 Finance projection builder 与领域错误映射移回 Finance-owned 边界，再通过领域中立 registry/port 接入。
- `module-manifest.v1` 仍允许 `contributions.views[].mount = "sidebar"`；它不得被解释为 D10 导航合同，下一版必须废弃该值。
- renderer `outbox.sqlite` 位于 app data 全局作用域，条目没有 `VaultId` 且 pending list 不按 Vault 过滤；在 multi-vault 前必须按 §6.1–§6.2 收口。

## 3. 目标架构

```text
┌──────────────────────────────────────────────────────────────┐
│ Desktop Shell (Tauri)                                        │
│ collapsible left sidebar · pinned modules · active page      │
├──────────────────────────────────────────────────────────────┤
│ Runtime adapters                                              │
│ Tauri IPC (canonical) · HTTP compatibility · trusted headless │
├──────────────────────────────────────────────────────────────┤
│ Platform Kernel                                               │
│ Vault & identity      Event/Object Store    Sync/Recovery      │
│ Module Host           EntityRef/Search      Relation Index     │
│ Scheduler             Notification         Migration/Export   │
├──────────────────────────────────────────────────────────────┤
│ Domain modules                                                │
│ Finance                Shard                 Collections       │
│ accounts/journals      Markdown/mindmap      records/fields    │
│ debts/subscriptions    attachments           views/relations   │
└──────────────────────────────────────────────────────────────┘
                     │ encrypted signed packs/objects
                     ▼
                 untrusted Git remote
```

### 3.1 Platform Kernel 的职责

1. **Vault 与身份**：创建、锁定、解锁、恢复、设备加入、会话失效和多 Vault 生命周期。
2. **规范存储**：append-only 事件、控制记录、模块原生对象、协议版本和持久 command dedup。
3. **同步与恢复**：传输适配、远端回退/改写检测、staging、验证、promote、确定性 replay、冲突与冻结状态。
4. **模块宿主**：manifest、宿主兼容、依赖/capability、每 Vault 启停、贡献点和 schema/upcaster 注册。
5. **跨模块基座**：稳定 `EntityRef`、全局搜索索引、关系索引与 quick link；索引必须可从 Vault 重建。
6. **时间能力**：持久 Scheduler 执行 due intent；Notification 负责各设备本地系统投递。
7. **迁移与可携带性**：协议升级、模块 payload upcast、旧系统导入、显式明文导出与一致性检查。
8. **Runtime ports**：向 Tauri、未来受信 headless host 与兼容 HTTP 层暴露宿主中立的 typed ports。

### 3.2 Platform Kernel 明确不拥有的内容

- `Money`、账户类型、Journal、复式平衡、债务、订阅和余额口径：属于 Finance。
- Markdown、mindmap、attachment 的内容语义：属于 Shard。
- dynamic fields、record、table/board/calendar view：属于 Collections。
- Axum `Router<AppState>`、utoipa、cookie、邮件、Turso、Docker：属于 HTTP/self-host adapter。
- GitHub Device Flow、HTTPS/file transport 的具体实现：属于 transport/credential adapter。

该边界必须有依赖方向测试：平台 crate/package 不得 import Finance、Shard 或 Collections 的领域类型。

### 3.3 Desktop Shell 与模块固定

**DECIDED**：Vault 解锁后的桌面主界面采用稳定的两区结构；V1 中文 LTR 环境中的 leading edge 即左侧：

```text
Desktop Window
├─ Collapsible Sidebar
│  ├─ Shell-owned platform entries
│  └─ Ordered pinned modules
└─ Page Host
   └─ Active module/platform page
```

#### 内容与所有权

- Sidebar、Page Host、Vault 切换、全局搜索、模块管理和设置属于 Desktop Shell；模块不得替换 Shell 或直接修改其他模块的导航。
- 有 UI 的 module descriptor 可声明一个稳定 `moduleHome`，引用本模块 page ID；page ID 使用 `{ModuleId}/{localId}` 命名空间并在发布后保持稳定。废弃页面必须提供 versioned replacement 或稳定的 unavailable mapping，不能让既有 pin/deep link 静默落到未知页面。该声明不取决于模块当前是否启用。Sidebar 的 label 与 icon token 始终从当前 descriptor 解析；icon 是宿主 token，不是任意 URL。同一模块仍可贡献零到多个页面，headless 模块不强造导航项。
- 固定操作只把 `ModuleId` 加入有序 `pinnedModules`；点击固定项打开该模块的 `moduleHome`。路径、显示名、图标和版本不得复制进固定记录。
- 未固定但已启用的模块仍可从模块管理页、command palette、搜索或 deep link 打开；固定不是功能发现、启用状态、授权或 capability 的真相源。
- M1 manifest 的 `mount: "sidebar"` 自本文起视为 deprecated，不能驱动导航、默认 pin 或固定状态；下一版 schema 删除该导航含义，CI 禁止 built-in fixture 再声明 sidebar mount。Sidebar 的唯一模块入口真相是当前 descriptor 的 `moduleHome` 与 Vault 的 `pinnedModules`。
- 初次创建 Vault 时由 host onboarding policy 一次性写入默认固定列表 `[dev.zhiyu.finance]`；这只是 V1 创建策略，不是 module protocol 永久默认。后续安装、manifest/host 版本升级不得静默重新固定、取消固定或改变用户顺序。

#### 状态与同步

| 状态 | 作用域 | 是否进入 canonical Vault / 同步 |
| --- | --- | --- |
| `pinnedModules` 与顺序 | 每 Vault | 是，属于 platform preference |
| 模块 enable/disable | 每 Vault | 是，属于 module configuration |
| Sidebar expanded/collapsed | 本机窗口 | 否 |
| Sidebar 宽度、临时 overlay、当前焦点 | 本机窗口 | 否 |
| 当前页面 route | 本机窗口；URL 可编码可分享 deep link | 否，不得成为领域事件 |

- 固定列表使用 full-set typed command `SetPinnedModules(orderedModuleIds, expectedHeadEventId, expectedRevision)`。revision 只用于可读的 OCC 诊断，`expectedHeadEventId` 才唯一绑定父 head；设备内任一不匹配都返回当前 head/revision/list 且零副作用，调用方只能显式 rebase/retry，不能 storage overwrite。
- 两台离线设备基于同一 parent event 写出不同完整列表时，两个合法 sibling head events 都保留，并以 `hash(streamId, sorted(competingHeadEventIds))` 形成稳定 `conflictId`，确定性投影为 `pinned_modules_conflict`。冲突期间 Shell 继续使用最后一个无冲突祖先的 effective list，显示候选列表与冲突入口，禁止墙钟/到达顺序 LWW 或自动交错排序。
- `ResolvePinnedModulesConflict(selectedOrderedModuleIds, conflictId, competingHeadEventIds)` 由 owner 显式选择或合并最终顺序。resolver 必须引用当前完整、排序后的 head set；期间出现新 head 或 conflictId 改变时整条命令以 `revision_conflict` 零副作用失败。冲突只阻止新的 pin/unpin/reorder 写入，不阻断模块领域 command、只读查询或 Scheduler；resolver UI 属于 Shell-owned 模块管理/设置页并可从 projection 重建。
- 模块禁用、未安装或 runtime 不兼容时保留其固定记录，并在原槽位显示不可执行占位、原始 `ModuleId` 与原因；模块管理页也必须显示原因。重新可用后按原顺序恢复。
- 固定/取消固定、排序、折叠和切页均不得产生模块领域事件，也不得改变模块数据、余额、同步权限或 recovery 行为。
- 打开固定项、模块管理项或 deep link 前，Shell 必须分别检查 enable state、runtime compatibility 与 authorization/capability，并落到稳定的 `module_disabled`、`module_unavailable` 或 `forbidden` Shell-owned 页面；固定记录本身永不授予访问权。

#### Deep link 合同

- Tauri 注册版本化 `zhiyu://` scheme，并把外部打开请求转发给单实例 Shell；deep link 只编码稳定 platform route 或已包含 ModuleId namespace 的 `PageId` 与非敏感参数，不携带 `vaultSessionId`、权限或业务 snapshot。
- Vault 锁定时仅在本机保存经过 schema 校验的 pending route intent；解锁后重新执行 Vault/module/runtime/authorization 检查再导航。切换 Vault、取消解锁或参数校验失败时不得把 intent 作为领域事件或跨 Vault 字符串链接执行。
- deep link、当前 route 和返回栈都是 local shell state；它们可以恢复界面意图，但永远不能授予 capability、启用模块或改变 canonical data。

#### 交互与自适应门禁

- 展开态显示 icon + label；折叠态保留可辨识 icon rail、active state、accessible name，以及 hover 和 keyboard focus 都可见的说明。
- 折叠按钮始终可达并支持 pointer；若提供键盘快捷键，必须可发现、可配置或上下文感知，且不得在 `input`、`textarea`、`contenteditable` 或模块编辑器内劫持按键（尤其不能抢占 Markdown 的加粗快捷键）。按钮必须暴露正确的 `aria-expanded`/`aria-controls`。折叠不得重挂载或刷新当前页面，也不得把焦点丢到 document body。
- Sidebar 项使用真实导航语义并标记当前页；Tab/Shift+Tab 必须能到达折叠按钮与固定模块，禁止全局拦截 Tab 来模拟桌面行为。
- 固定、取消固定与排序必须同时提供非拖拽的键盘路径；拖拽只能是增强交互，不能成为唯一入口。
- breakpoint 由内容实际是否容纳决定：常规窗口保持展开/折叠两态，较窄桌面窗口优先退为 icon rail；再窄时允许从 leading edge 临时 overlay，但 Tauri 桌面不得静默替换成移动端底部导航。
- overlay 打开后必须约束焦点、使背景 Page Host inert，支持 Escape 关闭，并把焦点还给原触发按钮。
- Page Host 是页面内容与滚动的主区域；Sidebar 保持稳定 chrome。调整窗口、切换 Vault、deep link、刷新 projection 或同步状态变化不得遮挡页面关键操作。
- 验收必须覆盖真实 macOS Tauri 窗口的展开、折叠、窄窗口/overlay、pointer、Tab/Shift+Tab、Shell 快捷键与编辑器快捷键隔离、deep link 及模块禁用/恢复；仅 Chromium 截图或单元测试不能替代。

## 4. 身份、隔离与作用域

| 标识 | 含义 | 生命周期 |
| --- | --- | --- |
| `PrincipalId` | Vault 内稳定的逻辑行为主体；V1 每个 Vault 只有一个 owner principal。 | durable platform identity |
| `VaultId` | 数据、模块状态、索引、远端仓库和恢复边界。 | durable canonical |
| `DeviceId` | 某 Vault 内的同步 actor；拥有独立设备密钥、序列和 Git ref。 | durable canonical |
| `vaultSessionId` | 解锁后发给 renderer 的短期 capability；新会话使旧值失效。 | ephemeral/local only |
| `sourceSessionGeneration` | 由 host 按 Vault 单调递增的非秘密世代号；renderer 不得自行提供。只用于判断旧 outbox 草稿需要重新授权，不是 capability。 | durable local metadata；不参与同步 |
| `ModuleId` | 反向域名稳定 ID，例如 `dev.zhiyu.finance`。 | durable protocol |

**DECIDED**：模块启停、模块数据与 remote 配置按 `VaultId` 归属，不按 email user 归属。email、OAuth subject 或 self-host 管理员只能在 adapter 边界映射到 `PrincipalId`；恢复根是 Vault 控制权，不是用户账号；`DeviceId` 只表达同步 actor。三者不得复用同一字段。

每个 Vault 拥有独立 canonical store、projection、module configuration 与 remote。V1 的 `EntityRef` 只能指向同一 Vault：

```ts
interface EntityRef {
  version: 1
  vaultId: string
  moduleId: string
  entityType: string
  entityId: string
}
```

`EntityRef` 只包含稳定身份，不嵌入可变名称、金额、权限或 projection snapshot；持有 ref 也不授予访问权。解析器必须区分 `active`、`archived`、`missing`、`module_unavailable` 与 `forbidden`：归档实体仍可解析，未知 module/type 保留原始 ref，反向引用索引可重建且不升级为新的全局实体真相表。

跨 Vault 关系、共享 owner 和跨 Vault 全文索引均为后续能力；V1 遇到跨 Vault ref 必须拒绝，不得静默降级为字符串链接。

## 5. 模块合同

### 5.1 稳定身份与协议版本

**CURRENT**：`module-protocol` 的 M1 schema 已验证 `id`、`version`、`hostApiRange`、只读 `contributions.views` 与封闭 capability 集合；它明确不实现安装、Registry、TUF、脚本运行时或财务写授权。当前 `views.mount` 仍接受 `sidebar/main/settings`，其中 `sidebar` 只是待废弃的 M1 预留，不满足 §3.3 的 Shell-owned navigation 合同。

**TARGET**：下一版 manifest 在保持严格未知字段策略的前提下，增加以下合同；不得悄悄改变 M1 schema 的含义：

| 合同 | 要求 |
| --- | --- |
| identity | `id` 为反向域名永久 ID；`version` 使用 semver；route slug、表名和显示名与 ID 解耦。 |
| compatibility | `hostApiRange` 明确宿主兼容范围；不兼容时 fail closed。 |
| dependencies | 依赖稳定 `ModuleId + version range/capability`；启动时检查缺失、重复和环。 |
| contributions | 一个模块可贡献零到多个 page/route/view/command/widget/settings panel；page ID 按 `{ModuleId}/{localId}` 永久命名，并可选声明一个稳定 `moduleHome` 供模块级固定。headless 模块无需导航项；不再提供模块直接挂载 Sidebar 的贡献点。 |
| capabilities | 只请求完成工作所需的 typed ports；未知 capability 拒绝。 |
| data schemas | 登记 event/resource type、payload schema version、纯函数 upcaster 与 projection builder。 |
| export | 登记显式导出器；导出格式不是 canonical import/sync 协议。 |

### 5.2 信任模型

- V1 built-in modules 与宿主同进程、同签名、同信任域；capability 是可执行合同和最小权限边界，但不是第三方代码安全沙箱。
- 模块只接收带 `PrincipalId + VaultId + scoped ports` 的 `ModuleContext`，不接收原始 `AppState`、数据库连接、Stronghold 或 Git transport。
- 模块间写入必须调用目标模块 command port；读取走对方 query port、`EntityRef` 或平台派生索引。禁止跨模块 raw SQL 写表。
- manifest 中“请求 capability”不等于获权；host 按 `VaultId + PrincipalId + ModuleId + runtime` deny-by-default 授权，未知项、通配符和隐式继承一律拒绝。
- 不提供 `raw_sql`、通用 `ledger.write` 或跨 namespace 的 `event.append`。模块只能调用领域 command；写入本模块 event/resource namespace 时，host 仍校验登记的 schema 与 capability。
- 允许 projection 共享物理 SQLite 文件，但所有权必须可静态检查；禁止跨模块可变外键、共享可写业务对象或绕过 port 修改他域 projection。
- 将来若支持不可信第三方模块，必须单独设计进程/沙箱、签名分发、权限撤销与升级信任；不得复用 built-in ABI 并宣称安全。

### 5.3 模块粒度

模块按领域不变量与数据所有权划分，不按页面、表前缀或路由划分：

- `dev.zhiyu.finance`：accounts、journals、balances、debts、subscriptions、budgets、funds 等内部 bounded contexts。
- `dev.zhiyu.shard`：`.md`、`.mindmap`、attachments 及其内容/链接语义。
- `dev.zhiyu.collections`：records、dynamic fields、relations、tables、boards、calendar views。

订阅、基金或债务可以在 Finance 内部独立演进，但不能为了验证 registry 人为升级为平台模块。模块宿主必须至少经 Finance 与一个非财务模块验证后，才抽象通用扩展点。

### 5.4 启停语义

模块配置是 Vault 中的 canonical platform state，并参与同步。built-in Finance 默认启用，但允许按 Vault 禁用；Platform Kernel 本身不可禁用。

启用模块前必须原子验证完整 dependency closure；缺失、不兼容或已禁用依赖返回稳定错误。禁用仍有 enabled dependents 的模块默认拒绝，只有用户明确确认并给出有序 cascade 时才联动禁用，禁止静默产生半启用状态。

模块配置使用单一 Vault 级 full-set stream `platform/module-configuration`，而不是每个 ModuleId 独立写流。`SetModuleConfiguration(fullEnabledMap, expectedHeadEventId, expectedRevision)` 必须先原子验证完整 dependency closure，再把最终全量 map 写成一个 event；cascade enable/disable 也只能形成这一条事件，不能拆成可能部分到达的逐模块更新。每个 `ModuleConfigurationSet` 还必须携带版本化 `moduleCatalogFingerprint`，绑定本次验证所依据的 descriptor/dependency catalog。

旧 Host 遇到 full-set 中未知的 `ModuleId` 时必须 byte-identical 保留对应配置项，不能按本机 registry 丢弃、默认禁用或重排。若 Host 无法取得并验证事件绑定的 catalog、未知模块依赖或完整 closure，它可以保留记录并维持最后一个已验证 effective map，但所有 module-configuration command 与 resolver 必须 fail closed。`affectedModules` 与 dependent closure 也按事件绑定的 catalog 计算，不能由各设备漂移的本地 manifest 各自推断。

多设备并发产生的不同 full-set 配置进入确定性 platform conflict，不使用墙钟 LWW；effective map 停留在最后一个无冲突 head。冲突 projection 从候选 map 的 diff 计算 `affectedModules` 及其 dependent closure；冲突未解决时，仅对该 closure 的新领域 command 与 schedule side effect fail closed，不能让各设备各自猜测 effective state。其他模块以及所有只读查询、导出、恢复、同步、upcast 和 projection 继续可用。

| 行为 | 模块启用 | 模块禁用 |
| --- | --- | --- |
| UI/route/command | 可达 | 隐藏或返回稳定 `module_disabled` |
| 新领域副作用 | 允许 | 禁止；持久 job 在 claim 和 commit 前都复核状态 |
| canonical data 接收 | 正常 | 继续接收、验签和保存，避免设备间协议分叉 |
| schema upcast/projection | 正常 | 继续维护，确保恢复与再次启用 |
| search/relation index | 维护 | 继续维护；默认 UI 可隐藏结果 |
| export/recovery | 可用 | 仍可用 |
| data deletion | 不自动发生 | 绝不因禁用发生 |

V1 不提供真正 uninstall。未来卸载也必须保留可识别的 module ID、schema 与 opaque data，使其可导出、恢复和重新安装。

合法但当前未知、未安装或不兼容模块的数据必须保留为 `unsupported/unprojected`；它不是损坏数据，不得因宿主无法投影就删除、改写或送入安全 quarantine。

#### Platform state 事件与 resolver

- V1 至少登记版本化的 `PinnedModulesSetV1`、`PinnedModulesConflictResolvedV1`、`ModuleConfigurationSetV1` 与 `ModuleConfigurationConflictResolvedV1`。事件必须包含所属 `VaultId`、稳定 stream ID、schema version、唯一 `parentEventId`、显示用 revision、command ID 与 payload fingerprint；module-configuration 事件还包含 `moduleCatalogFingerprint`。具体 wire encoding 由协议 ADR 定义。
- `pinnedModules` 使用单一 `platform/preferences/pinned-modules` stream；全部模块启停状态使用单一 `platform/module-configuration` full-set stream。纯函数 reduce 必须在任意合法到达顺序下得到相同 effective state、competing head EventIds、`conflictId`、affected closure 与显示 revision。
- 模块配置冲突由 `ResolveModuleConfigurationConflict(fullEnabledMap, conflictId, competingHeadEventIds)` 显式解决，并重新验证 dependency closure；不能按时间或“最后看到”自动选胜。resolver 未覆盖当前完整 head set或解决期间出现新 head 时零副作用失败。解决前模块管理页必须展示候选、影响范围和仍可执行的只读/恢复操作。
- platform conflict 是可重建 projection，不是 ingest quarantine 或 `Frozen`。只有签名、密文、envelope/控制链非法才进入安全处置；合法的 owner 并发意图必须保留并可解决。

### 5.5 前端贡献与一致性门禁

- Shell 只读 manifest/descriptor 与 contributions，不得出现 `if moduleId === "dev.zhiyu.finance"` 一类宿主分支。
- route/page ID、path ownership、`moduleHome`、title/breadcrumb/back action、adaptive priority、loading/error boundary 和 runtime capability 必须由贡献合同表达；已发布 ID 的删除或重命名必须有 replacement/unavailable fixture 与 drift test。
- Sidebar 从 platform preference 中的有序 `ModuleId` 列表解析当前 descriptor；固定状态不得反向写入 module manifest，也不得从当前 route 或业务数据猜测。
- Rust descriptor/DTO 与 TypeScript registry 必须生成或做 drift check；CI 检查 ID 唯一、route collision、依赖 DAG、宿主版本与 capability。
- descriptor/contribution 可在模块未启用时被 Shell 读取，但业务 chunk 只在 enable、runtime compatibility 与 authorization 检查通过后加载；deep link 分别返回稳定的 `module_disabled`、`module_unavailable` 或 `forbidden` 页面。

## 6. 数据真相与写路径

### 6.1 分层真相

| 层 | 定位 | 是否可删除重建 |
| --- | --- | --- |
| control records | Vault genesis、设备授权和控制链 | 否 |
| platform events | 模块配置、固定列表等签名加密、append-only 的平台状态 | 否 |
| domain events | 签名加密、append-only 的业务变化 | 否 |
| module resource objects | Shard 等模块拥有的原生内容/附件对象；远端表示必须加密并按 hash 引用 | 否 |
| `event-store.sqlite` | 本设备已接受 canonical records 的持久存储 | 否，不是 cache |
| encrypted Git packs/objects | 不可信远端上的复制副本 | 可重新抓取，但不能当明文工作树真相 |
| `projection.sqlite` / indexes | 账户余额、列表、搜索、关系、冲突箱等派生视图 | 是 |
| renderer `outbox.sqlite` | 按 Vault 隔离的本地明文 command 草稿；每项至少绑定 `VaultId + sourceSessionGeneration + ModuleId + commandId + payloadFingerprint` | 否；删除会丢失尚未提交的草稿。它不是 sync outbox 或 canonical dedup |
| JSONL/CSV/Markdown export | 用户主动生成的明文可携带副本 | 是；不参与 canonical replay |

**DECIDED**：删除“SQLite 业务表 → `sync_outbox` → JSONL → pull/rebase”的复制模型。它与现有 event protocol、设备身份、冲突和恢复模型不兼容，也会制造第二套真相。

**CURRENT DEBT**：dirty M1 的 app-global `outbox.sqlite.pending_commands` 没有 `VaultId`，并在验证当前 session 后返回全部 pending。multi-vault 下这会让 Vault A 的草稿在 Vault B 中出现或被误提交。

**TARGET**：每条 outbox entry 以 `(VaultId, commandId)` 隔离；`sourceSessionGeneration` 仅用于判断是否要重新授权，不能当作 durable capability，也不得持久化可重用的 `vaultSessionId`。list/retry/ack/discard API 先由当前有效 session 解析 `VaultId`，再只作用于该 Vault。renderer/app crash 保留草稿；新 session generation 解锁同一 Vault 后，旧草稿进入 `needs_reauthorization` 并由用户显式恢复。跨 Vault 草稿不得自动展示、恢复或提交，只有切回并解锁目标 Vault 后才能处理。

主动 lock/abort 必须先把 Vault session 置为 `locking` 并拒绝新 submit，再用 barrier 收敛全部 in-flight command：canonical outcome 已可查询的才 ack；已在 pre-append barrier 前确定取消的才可 discard；无法确认是否 append 的必须持久化为 `outcome_unknown`，失效 capability 后保留到下次解锁查询，不能清除或自动重试。只处于未提交 draft 状态的当前 Vault entries 才能在 lock/abort 时清除。`discard` 永远只删除已证明未提交的草稿，不能取消可能已 append 的 command。

### 6.2 Canonical 写路径

```text
UI
  → Runtime Port(vaultSessionId + commandId)
  → Platform authorization / Platform 或 Module command
  → 领域完整校验（无效命令零副作用）
  → 必要时先持久化 resource object，再构造引用它的 canonical event
  → 原子 append 到 event store
  → deterministic promote/replay
  → projection/index 更新
  → encrypted pack/object 经 transport 复制
```

- 同一次用户操作的重试复用 `commandId`；`commandId` 与 canonical input fingerprint 必须进入签名加密的 canonical record，不能作为远端明文 metadata 泄漏。同 ID 不同输入必须显式冲突。
- command dedup 分为三层，三者不得互相冒充：
  1. **store-local pre-append lookup**：本设备 event store 的物理 guard，阻止同一输入重复 append；它是本地优化，不是跨设备真相。
  2. **distributed command outcome**：deterministic replay 从 canonical records 派生可删除重建的 `command_outcomes`；重启、同步收敛或 projection rebuild 后的重试以这一层为准。同 ID + 同 fingerprint 只形成一个确定性逻辑结果，同 ID + 不同 fingerprint 形成显式 conflict。
  3. **renderer command outbox**：只保存尚未确认提交的 UI 草稿并支持 renderer crash 恢复；它不是 sync queue、event store 或 dedup 真相。
- replay 在应用领域效果前先按 `commandId` 归并 outcome：同 fingerprint 的多设备重复记录必须具有相同 result fingerprint，并只应用一次；原始 records 仍保留作审计。result 不同或 input fingerprint 不同都进入显式 conflict，不能按到达顺序重复记账或选胜。
- post-commit 内存事件只能作为可丢提示；提醒、自动记账、导出等持久副作用必须有 durable intent/job 和幂等键。
- transport 只复制 opaque canonical records；更换 Git 为其他传输不得改变 Finance/Shard/Collections 语义。

### 6.3 各模块的数据形态

- **Finance**：复式 Journal/event 是财务真相；余额是 projection。金额不使用浮点；修正用原子 correction（reversal + replacement）。
- **Shard**：Markdown、mindmap 与 attachment 保持模块原生资源，不被强制压成 Collections row 或 Finance event；共享 change protocol 只负责身份、版本、加密和同步。
- **Collections**：动态 records/fields/relations 是本模块拥有的数据；table/board/calendar 是可重建 view。

不同数据形态共享 Vault、身份、change envelope、同步和 `EntityRef`，不共享一个万能业务 schema。

### 6.4 Module object 约束

- object ID 必须由认证加密后的 canonical object bytes 内容寻址；远端只保存不可变密文，明文 hash、文件名、路径和内容不得泄漏。
- durable save 必须先保证 object 可持久读取，再提交引用它的 event；发布单元必须保持 event/object closure，不能永久产生 dangling ref。
- 合法引用但本地尚未取回的 object 表示 `content_pending`，不得伪装成已删除、空内容或冲突胜者。
- 在 §8.5 checkpoint、设备撤权、离线恢复和 restore drill 门禁满足前，不做 canonical object GC。

## 7. 归档、修正与删除

### 7.1 财务不变量

- 归档账户：隐藏并拒绝新 posting；历史 posting 与余额不变。
- 归档 Journal/流水：只影响默认列表可见性；历史 posting 与余额不变。
- 归档债务、订阅等 Finance 内部实体：不能隐式撤销已经形成的 Journal。
- 改变金额、账户或日期：生成可审计 correction；取消经济影响：生成显式 reversal/void。
- canonical event/resource 不物理删除。用户发起的“删除”必须落为领域允许的 tombstone、archive 或 reversal。
- UI 与 API 必须把 Archive、Void/Reversal 和 Correction 作为不同操作呈现，不能继续用含混的“删除”掩盖余额变化。
- **NON-GOAL**：V1 不提供会计期间锁定。回摆到原业务日期的 correction/reversal 必须完整出现在审计历史中；若未来增加 period close，需另立 Finance ADR，不得静默改变既有事件的经济生效日。

### 7.2 旧 HTTP 语义的迁移

**CURRENT（dirty 0009 candidate）**：未提交的旧 HTTP `ledger_transactions` 视图会排除 `archived_at IS NOT NULL` 的流水，因此归档会回滚余额；债务与账户归档的余额行为又不同。

**TARGET**：导入旧账本时保持导入前可见余额，同时迁到统一不变量：

1. 为每条旧流水生成原始 Journal。archived transaction 必须由单个原子 `ImportLegacyTransaction` command 生成 `original`、`archive_reversal`、`archive_visibility` 三个有稳定 output role 的 events，不能用三个会碰撞或留下半完成状态的独立 command。
2. 对旧系统中 `archived_at IS NOT NULL` 且当前不计余额的流水，生成逐 posting 精确取反并引用原 Journal 的正式 Finance `Reversal`；其经济生效日沿用原流水业务日期，记录时间使用 `archived_at`，provenance 为 `legacy_http_archive`。
3. 再生成只改变可见性的正式 `ArchiveJournal` event，使列表保持隐藏，但余额由原 Journal + Reversal 决定。`Reversal`、`ArchiveJournal` 与 `UnarchiveJournal` 的 event kind、schema、upcaster、projection、target uniqueness 和 reversal-of-reversal 规则必须先于 importer 落地。
4. 除非另有 ADR 正式引入一等 Void event，V1 importer 不得临时发送 `VoidJournal`，也不得用要求 replacement 的 `CorrectJournal` 代替 Reversal。
5. 导入后逐账户、逐币种比较余额；任何差异阻断切换。staging Vault 在原 Journal、Reversal、ArchiveJournal 全部写入并完成对账前不得激活。

这样不会因新语义“重新计入”历史归档流水，也不会继续让归档承担财务删除职责。

若兼容旧的 restore/delete 操作，Finance projection 必须维护该 legacy Journal 的 `economicHead`：初始为原 Journal，每次 toggle 成功后更新为本次新生成的 Reversal。delete 只能在当前可见时原子生成 `Reversal(economicHead) + ArchiveJournal`；restore 只能在当前 archived 时原子生成 `Reversal(economicHead) + UnarchiveJournal`。例如导入已归档记录形成 `J → R1(J)`，restore 形成 `R2(R1)`，再次 delete 必须形成 `R3(R2)`，不能再次 reversal 原始 `J`。每个 target 只允许被直接 reversal 一次；命令携带当前 visibility/economic-head EventId 与显示 revision，重复或并发 toggle 必须幂等成功或零副作用 conflict。任一子步骤失败必须零副作用，且不得改写或删除既有 Journal/Reversal。

旧 debt `DELETE` 若继续兼容，也必须映射为领域允许的显式 Void/Reversal（如需改变经济结果）与 Archive，不得在 canonical store 物理删除债务历史。

## 8. 同步、冲突、恢复与安全

### 8.1 Remote 与设备拓扑

- **DECIDED**：V1 remote cardinality 是 `1 Vault : 1 Git repository`。一个 repository 只能承载一个 `VaultId` 的控制链和设备 append refs；同一 repository 不得绑定第二个 Vault。规范判断来自根签名 Vault genesis/control state 的 `VaultId`，规范化 remote URL 只能作为本机预检，不能替代抓取后的身份校验。
- 空 remote 只允许一个 Vault 以 compare-and-create 完成首次 bootstrap；竞争失败者必须重新 fetch 并因 `VaultId` 不匹配拒绝绑定。multi-vault registry 也必须拒绝把两个本地 Vault 指向已识别为同一 repository 的 remote。
- V1 专用 repository 使用 `refs/heads/vault-control` 与 `refs/heads/device/<opaque-device-id>`；这些裸 ref 只因 repository 被单个 Vault 独占才安全。未来若支持一 repository 多 Vault，必须升级协议并使用版本化 Vault namespace，不能重新解释 V1 refs。
- remote 被视为不可信复制层。公开或私有托管都必须从第一条业务记录起加密，不存在“Stage 3 再加密”。
- 远端 delete、rewind、rewrite、非后代 tip 或非法树结构必须进入 `Frozen`；本地不得 force push 或反向“修复”远端来掩盖异常。
- generic Git transport 是架构目标；CURRENT 实现支持范围必须从代码核验，不能把尚未接线的 SSH/Gitea 写成已支持。

#### 最小 DeviceId 撤权

- multi-device production release 必须支持根签名的 `DeviceRevoked` control record，至少绑定 `VaultId`、`DeviceId` 与 `acceptedThrough { deviceSeq, eventId }`。`acceptedThrough` 必须锚定签发者已验证的该设备连续 chain prefix/tip，`deviceSeq`、`eventId` 与设备链内容逐项匹配；未来、未知、断链或属于其他设备的 anchor 一律使撤权记录无效并 fail closed。对包含该有效撤权记录的同一完整输入集，cutoff 以内已接受历史保持有效；该 DeviceId 更高序列的记录最终不得进入 effective canonical set，并稳定归类为 `revoked_device`。
- 其他设备不得替被撤权设备发布、复用其私钥或延续其 sequence。恢复一台被撤权设备必须生成新密钥和新 `DeviceId`，不能“解除”旧 ID 的撤权。
- V1 最小撤权只终止后续 canonical 写权限；丢失设备已经取得的历史明文读取能力要到 key epoch rotation 与密钥再分发后才能撤销。产品 UI、threat model 与发布说明必须明确这一限制。
- 设备在看到撤权记录前若曾暂时 promote cutoff 之后的记录，取得新 control frontier 后必须确定性重算 disposition：原始 bytes 保留作 wire/audit 证据，但不再进入 effective canonical set，projection rebuild 移除其领域影响并显示 security recovery report。协议 ADR 必须用相反到达顺序验证该结果收敛。

### 8.2 导入状态机

```text
fetch isolated refs
  → verify control/device chain + signature + envelope
  → append durable wire/staging record
  → promote accepted canonical records
  → deterministic catch-up/replay
  → rebuild derived conflict/quarantine/index state
```

- fetch 顺序不得改变最终 event set、余额、冲突或 quarantine 结果。

| 输入/异常 | 稳定 disposition | 是否进入 canonical set |
| --- | --- | --- |
| 协议、签名、密文或 envelope 结构非法 | ingest quarantine | 否 |
| 根控制链/远端历史 rewind、rewrite 或非法 tip | `Frozen` | 冻结 promote，保留已接受历史 |
| 已撤权 DeviceId 超过 cutoff 的 append | `revoked_device` | 否 |
| envelope 合法但 module/schema 未来未知 | `unsupported/unprojected` | 是，opaque 保留 |
| 合法的领域或 platform divergent revisions | 可重建 conflict projection | 是，等待对应 resolver |

- ingest quarantine、`Frozen`、`revoked_device`、unsupported 与业务/platform conflict 不得混成一个“同步失败”，也不得相互升级处置。preference 与 module-configuration platform conflict 的有效状态与 resolver 分别按 §3.3/§5.4；领域 conflict 由所属模块定义，但同样禁止墙钟 LWW。
- 财务金额、账户、日期和 correction 冲突不得用墙钟 LWW 静默决胜。
- replay 只能从 canonical records 重建 projection，不从 JSONL 或 UI cache 补真相。

### 8.3 恢复与秘密边界

- 恢复短语授权恢复根；设备私钥独立随机生成，新增设备必须经控制链授权。
- `vaultSessionId`、口令、恢复短语和 token 不进入事件、projection、React Query cache、localStorage、日志或明文导出。
- 桌面生产秘密进入 Stronghold 或等价受审计 secret backend。`projection.sqlite`、全文 search index、relation/backlink index 与 renderer command outbox 是明确的 per-Vault 本地明文边界，可能包含名称、正文片段、实体关系或 command 参数；它们不得进入 Git packs、canonical objects、日志或自动导出。
- Vault 锁定时必须关闭这些明文库的查询句柄并清空相关内存 cache；文件权限、删除、整卷备份与敏感扫描必须覆盖所有索引。整卷备份若包含这些文件必须额外加密，但索引本身仍必须可从 canonical Vault 全量重建。
- recovery phrase + 完整 encrypted remote 应能在空设备恢复 canonical event/object set；projection 随后重建。该演练必须覆盖崩溃点和错误口令。
- 丢失恢复短语且无可用已授权设备时不可承诺恢复；文档与 UI 必须直说。

### 8.4 Schema evolution

以下版本轴必须分开：

1. envelope/protocol version；
2. module payload/resource schema version；
3. module semver；
4. host API version；
5. projection schema version。

模块 upcaster 必须纯函数、确定性、可重放；未知未来版本 fail closed 并保留 opaque bytes，不得删掉“不认识”的模块数据。旧客户端若不能安全重放必须拒绝写入或只读打开，不能静默 downgrade。

未知但验签、解密和 envelope 合法的模块记录属于 `unsupported/unprojected`；只有协议、签名、密文或结构验证失败才进入 ingest quarantine。

### 8.5 Checkpoint 与 compaction

**CURRENT POLICY**：无限 append，不做破坏性 GC。

只有同时具备以下合同后，才允许另立 ADR 设计 compaction：根签名 checkpoint、checkpoint 前事件集承诺、所有有效设备 ack 或明确撤权、从 checkpoint + tail 的空机恢复、旧离线设备回归行为、故障注入和远端截断检测。

### 8.6 导出与备份

- **Semantic export**：只覆盖当前已安装、兼容且宿主可语义解析的模块，生成用户主动触发的 JSONL/CSV/Markdown 明文副本。它用于查看或迁往外部工具，不保证完整恢复 Vault，也不允许自动 replay；回迁必须经过目标模块 command，形成新的 canonical history，不能伪造原 `EventId`、设备签名或控制链。
- **Opaque preservation package**：未知、未安装或不兼容模块不提供伪造的 semantic export。若用户显式制作保全包，只能 byte-identical 保存其认证密文 envelope/object bytes、`ModuleId`、object hash、control frontier 与 checksum，并标记 `representation=opaque-encrypted`；当前宿主不得声称内容可读、已 upcast 或可语义校验。保全包只能进入 canonical restore 验证链，不能通过普通模块 command 改写，也不能单独冒充完整 Vault 或新的同步真相。
- 两种输出都排除 password hash、session、token、恢复材料和 renderer outbox。Semantic export manifest 还必须列出 `excludedOpaqueModules`；保全包不得混入本地明文 projection/search/relation index。
- 每次输出附带版本化 manifest，至少记录 `formatVersion`、`representation`、`vaultId`、生成时间、module/export schema versions、canonical frontier 与文件 checksum；校验失败时不得继续导入或恢复。
- Canonical recovery 仍依赖恢复材料与完整加密 remote/local event-object store；本地整卷备份必须使用一致性 snapshot/quiesce，并通过 restore drill 验证 RPO/RTO。

## 9. 时间、日历与通知

**PHASE GATE**：Phase 4 出口通过前，面向用户数据、默认配置或可发布 runtime 的 schedule/reminder/calendar intent 只能被创建、查看或由用户显式 command 执行。为完成本阶段实现与对抗测试，只允许在隔离 test Vault 中、经显式 non-production feature gate 启用墙钟执行和通知投递；测试产物不得连接用户 Vault 或进入 release 配置。除此之外，GET、renderer timer、启动/解锁 hook、OS notification callback、Web adapter cron 或 headless process 均不得因墙钟到期而 append canonical event、自动记账或投递通知。本限制不影响不产生领域结果的 transport retry/backoff。只有全部 Phase 4 出口项通过后，production capability 才能默认保持关闭并按发布流程另行启用。

时间能力拆成四层：

1. 模块拥有 reminder/schedule intent 及领域含义。
2. Collections 可提供 time fields 与 calendar view；Calendar 是视图，不是第二份事件数据。
3. Platform Scheduler 持久化 due work、lease、retry 与 missed-occurrence 处理。
4. Notification adapter 在每台设备本地申请权限、调度、取消和投递 OS notification。

产生 Vault 写入的 occurrence identity 固定为以下语义元组；协议 ADR 再定义带 domain/version 前缀的具体 hash encoding：

```text
(VaultId, ModuleId, ScheduleId, OccurrenceInstantUtc)
```

- `scheduleRevision` 不进入 occurrence identity。每个 revision 必须持久化 `effectiveFromInstant`、recurrence rule、业务 payload fingerprint、IANA `calendarTimezone`、`tzdbVersion`、DST gap/fold policy 与 `missedRunPolicy`；其生效区间是 `[effectiveFromInstant, nextRevision.effectiveFromInstant)`，同一 occurrence instant 只能由一个 revision 解释。
- schedule revision command 必须携带 `expectedHeadEventId`。基于同一 parent 的合法并发 revision 形成 module-owned `schedule_conflict`；所属模块的显式 resolver 必须引用当前完整 head set。解决前该 schedule 的新 claim、领域副作用与通知都 fail closed，但同步、审计、导出和无副作用查询继续可用。
- Scheduler job 记录实际使用的 revision 供审计。编辑 schedule 只影响其生效点之后的 occurrence；新的 `effectiveFromInstant` 不得覆盖、重解释或换掉任何已有 terminal occurrence 的 command identity。追溯修改必须显式 cancel/supersede 尚未终结的 occurrence；已产生领域结果时必须走所属模块的 correction/reversal 合同。
- Scheduler 使用应用内固定、版本化 tzdb，不读取各设备可能漂移的 OS tzdb 来独立决定 canonical instant。设备不支持 revision 指定的 tzdb 时 fail closed；timezone/tzdb 或 recurrence 变化必须创建新 revision，不能回写过去的 occurrence。
- 对会产生 Vault write 的 occurrence，`executed | skipped | superseded | needs_attention` 必须成为 canonical 或可完全从 canonical command outcome 重建的 terminal disposition；Scheduler 不得仅推进本地 cursor、lease 或 next-run 时间而没有对应终态。`catch_up_last` 必须在同一原子 outcome 中把更早 missed occurrences 标为 `skipped`，并为选中的最后一次 occurrence 写入终态；任一步失败都不能推进 cursor。
- `missedRunPolicy` 的 V1 枚举为 `skip | catch_up_last | catch_up_all(maxOccurrences)`。提醒默认 `catch_up_last`；财务自动化只有用户明确选择时可使用有上限的 `catch_up_all`。超过上限进入 `needs_attention`，不得静默截断。模块禁用或 schedule 暂停区间不自动视为普通离线 missed interval，重新启用后必须由用户显式选择是否补跑。
- Scheduler 采用 at-least-once 投递，不承诺基础设施 exactly-once。自动 command ID 由 occurrence identity 派生；重复尝试以相同 ID/输入收敛为一个 canonical 结果，相同 identity 但 revision/payload 不同必须显式冲突，不能形成第二笔领域结果。
- 墙钟只用于判断持久化 occurrence 是否到期，不参与 canonical 全局排序。
- 多设备同时发现到期项时，最终只能形成一个 canonical 领域结果。
- 模块禁用后停止新的领域副作用；job 在 claim 与 commit 前都检查 effective module state。
- reminder intent 参与同步；每台设备的 OS pending notification 不参与同步，并由本地重新推导。设备投递去重使用 local identity `(OccurrenceIdentity, DeviceId, channel)`，不能把设备/渠道写入 canonical occurrence identity，也不能把“已通知”冒充领域执行成功。
- 桌面未运行或 Vault 锁定时不承诺后台执行；下次解锁按持久化 missed-run policy 处理。未来常驻 headless host 也必须复用同一 occurrence/command 合同。
- `lazy-settle-on-read` 只能作为显式“补处理”命令的内部实现，不能让 GET 列表产生隐藏写入。
- Finance replay 中的 fixed-point scheduler 与这里的 wall-clock Scheduler 是两个概念，代码和文档不得同名混用。

## 10. Runtime 与部署适配层

### 10.1 Tauri 桌面

Tauri 是 V1 canonical runtime：直接进入 `VaultGate`，使用 `vaultSessionId`、typed ports、Stronghold 与本地 event/projection stores，不经过邮箱登录或 HTTP `api.me`。Vault 打开后必须进入 §3.3 的 Desktop Shell，再由 Page Host 渲染平台页或模块页；不得继续把单个 Finance workbench 当作整个桌面应用根节点。

### 10.2 Web/API 兼容层

- CURRENT HTTP mode 继续服务旧 `/api/v1/*` 和旧 SQLite 数据；不得宣称与桌面 Vault 自动同步。
- CURRENT `HttpRuntime` 把“数据已在服务端”显示为 `synced` 只属于 legacy UI 语义；canonical HTTP host 必须报告真实 Vault transport 状态，不能保留该硬编码假设。
- HTTP OpenAPI 是 adapter-specific contract；Tauri IPC 由 Rust DTO → TypeScript bindings 及 drift check 管理。
- 内部模块化不得改动现有公共路径。尚未以独立 trusted `DeviceId` 加入目标控制链的 Web/self-host 不得解密、投影或查询 canonical Vault，canonical read/write endpoint 都必须 fail closed。`adapter_read_only` 只表示某个 adapter 不接受 append，绝不授予 canonical 解密或读取能力；它可以用于 legacy lane 的冻结写端点，或已经受信但 writer capability 关闭的 canonical host。若未来成为可写 Platform host，它必须使用相同 canonical protocol 与 secret backend，而不是做数据库 ↔ JSONL 双写。
- Turso/libSQL remote 仅是旧 HTTP adapter 的数据库部署选择，不是 local-first 同步主渠道。
- Web 壳可以复用 Desktop Shell 的无平台依赖组件，也可以显式分叉，但必须在 adapter 文档中选择其一；不得让 Web 底栏、邮箱 session 或 `HttpRuntime.synced` 语义改变 Desktop Shell、module descriptor 或 canonical sync 合同。

### 10.3 Self-host TARGET 门禁

**V1 默认边界**：未加入 canonical 控制链的 self-host 只能提供 legacy lane，或以显式 legacy snapshot 为来源的只读 importer；它不能提供 canonical projection/query。在 legacy cutover 前，旧 adapter writer 可以只写旧数据库，但不得同时写 canonical Vault。需要 canonical 只读能力的 headless host 也必须先在目标 Vault 中以独立 `DeviceId` 完成根签名加入，具备 Linux secret backend、独立设备密钥、最小撤权与 recovery drill，并把 writer capability 保持关闭。只有进一步具备 per-device sequence/ref、remote credential、canonical command dedup 及本节全部写门禁后，才可升级为 writable trusted headless device。任何形态都不得复用桌面 DeviceId、私钥或 renderer session；read 与 write capability 必须分开授权、探测和撤销。

在称为“生产级单容器自部署”前必须全部满足：

- production 不再无条件 bail，并装配真实、安全的认证模式。
- single-owner 模式使用首次管理员 bootstrap + 密码 + recovery key；无 SMTP 时关闭公开注册/邮件找回，禁止固定验证码或日志 OTP 作为公网生产方案。
- 若作为 canonical 只读 host，先满足受信加入、secret、撤权与 recovery 合同并禁用 writer；若作为可写 Platform host，再满足完整 trusted-writer 合同。已经受信但未获 writer capability 时 canonical write endpoint 必须保持 `adapter_read_only`；尚未受信时不得用该错误码暗示具备读取权限。
- production cookie/CSRF/origin 配置正确；容器不以 development 配置承载公网域名。
- 镜像包含实际启用 transport 所需组件；Compose 不依赖项目外私有网络才能启动。
- 数据卷权限、consistent snapshot、restore drill、升级回滚和 health/readiness 均有自动化验收。

## 11. Legacy Web/API 导入计划

### 11.1 迁移链处理

- 旧 SQL 迁移作为 adapter-specific legacy baseline 按原顺序执行，不按新 Platform 模块拆分。
- 0001 混合认证/债务，0003 同时创建账户并修改债务，0008 又依赖 0003；强拆模块会形成循环并破坏重放。
- 只有已提交并实际应用的迁移才进入 baseline。审计时 0009 尚未提交，因此不得提前宣布“0001–0009 永久冻结”。
- importer 必须读取 `schema_migrations` 与 schema fingerprint，显式支持已提交的 v1–v8，并把带 0009 的 dirty/候选 schema 当作单独、可检测的输入版本；不得仅按“最新表是否存在”猜测来源。
- “可检测”不等于“已接受或可自动应用”。0009 在提交、改写或冻结前，必须在独立 review branch/commit 上完成语义审查并记录候选 SQL checksum；至少覆盖 transaction archive/delete/restore 对余额与月度 summary 的影响、OpeningBalance 日期、debt principal/addition/repayment 的账户 movement、乐观锁与幂等、v1–v8 fresh upgrade/失败回滚，以及 OpenAPI/UI 的删除/归档/恢复措辞。审查后只能作为不可变 adapter migration 正式提交，或放弃进入运行迁移链；不得为了形成 baseline 直接升格已知有缺陷的 dirty SQL。importer 对已经出现过的精确候选 fingerprint 仍显式识别。
- baseline 建立后保存 SQL checksum/manifest，并以 fresh DB、每个历史版本升级、失败回滚和 schema fingerprint 测试禁止 shadow edit。
- 旧 `schema_migrations(version)` 继续属于 HTTP adapter。Platform/module schema 使用独立 registry；不得只给旧表加 `module` 列后让多个模块都从 v1 冲突。

### 11.2 导入流程

1. 对旧实例进入只读/quiesced 状态，创建一致性 SQLite snapshot，并记录 SHA-256、schema fingerprint 与逐表行数；importer 只读 snapshot，绝不就地迁移原数据库。
2. 检测来源 schema，盘点用户、账户、流水、债务、addition、repayment/reversal 与归档状态；认证秘密不进入 Vault 业务数据。
3. 每个旧 `user_id` 创建一个 single-owner Vault 和稳定 owner `PrincipalId`；多用户数据库拆为多个 Vault，不合并业务数据。
4. 在 staging Vault 中生成带稳定 `legacyId`/provenance 的 canonical Finance commands/events；command ID 由 snapshot hash、旧 `user_id`、实体类型和 legacy ID 确定性派生，多 event output 再加入稳定 role。旧 `ledger_accounts`、`ledger_transactions` 与 `opening_balance_cents` 没有币种字段，V1 importer 明确按 CNY 解释；绑定账户的 cash-movement debt/addition/repayment 必须也是 CNY，发现非 CNY 或同账户混币时 fail closed 并单列报告，不能沿用 0009 view 跨币种求和。未绑定账户的非 CNY debt 可按其原币种导入债务领域。若来源含非零 `opening_balance_cents`，生成 CNY OpeningBalance Journal；其 `effectiveOn` 固定为该账户最早一笔 legacy 经济变动业务日期的前一自然日。经济变动集合必须覆盖绑定该账户的 `ledger_transactions.occurred_on`，以及 `origin_kind = 'cash_movement'` 债务图中的 `debts.occurred_on`、`debt_addition_events.effective_on` 与 `repayment_events.effective_on`，包括之后会被 Reversal 的 archived 记录。若没有任何经济变动，则使用按旧 `users.timezone` 解释的 `ledger_accounts.created_at` 日期。日期/时区非法或减一日溢出时 fail closed；不得回退到导入当天、Unix Epoch 或任意远古日期。零期初余额不生成 Journal，旧 balance view 等派生结果不直接导入。
5. 对旧 archived transaction 按 §7.2 生成正式 `Reversal + ArchiveJournal`。
6. promote/replay 后比较实体计数、逐账户余额、债务本金/已还/剩余、日期、币种和引用完整性。
7. 重复导入同一 snapshot 必须幂等；任一差异使切换失败，旧数据库保持原样可回退。
8. 导出并扫描 canonical remote，确认没有 password hash、session、email token、恢复材料或明文业务 payload。

旧系统已经物理删除且没有审计记录的数据无法凭空恢复，导入报告必须单列这一不可逆缺口。

### 11.3 Cutover 耐久状态机

切换窗口禁止 legacy DB 与 canonical Vault 双写。legacy adapter 与目标 Vault 必须分别保存可交叉核验的耐久标记，不能依赖内存、部署顺序或人工口头状态。

Legacy 侧状态为 `legacy_writable → quiescing → snapshot_frozen → activation_pending → canonical_active`，标记至少包含 source instance/user、snapshot SHA-256、target `VaultId`、importer version、确定性 `activationCommandId`、activation event/frontier 与更新时间。每个 legacy 写请求都必须在同一数据库事务内、紧邻 commit 再确认状态仍为 `legacy_writable`。

Vault 侧在 staging 之外接受普通生产 command 前，必须存在签名加密的 canonical platform event `LegacyCutoverActivatedV1`，包含匹配的 source fingerprint、snapshot SHA-256、importer version 与 imported frontier。它属于 platform state，不未经 ADR 塞入只负责设备授权的根 control chain。

安全顺序固定为：

1. legacy 侧 CAS 到 `quiescing`，拒绝新写并排空在途事务；
2. 创建一致性 snapshot，持久化 `snapshot_frozen`；
3. 完成 staging import、projection rebuild、逐分对账与秘密扫描；
4. 在 append 前先 CAS 到 `activation_pending`，持久化由 source fingerprint、snapshot、target Vault 与 importer version 确定性派生的 `activationCommandId`；
5. 用该 command ID 幂等 append `LegacyCutoverActivatedV1`。命令只有在 imported frontier 全部存在、已 promote 且匹配对账 manifest 时才生效；成功后 Vault 成为唯一允许继续演进的 canonical store；
6. legacy 侧写入 `canonical_active`、activation event/frontier 并永久只读。

只有从未进入 `activation_pending` 的 `snapshot_frozen` 才可以显式 abort、丢弃 staging Vault 并恢复 legacy writer。进入 pending 后，无论 append 响应丢失、进程崩溃或远端暂时不可达，都只允许查询 exact command outcome、用相同 ID 幂等重试或完成 `canonical_active`，永远不得回到 `legacy_writable`。若第 5、6 步之间崩溃，legacy 保持只读，reconciler 核验两端相同 fingerprint/event 后补完第 6 步。未知、不匹配或部分写入状态一律 fail closed，并由 readiness 返回 `cutover_incomplete`；应用回滚只能继续使用同一 canonical event store，不能重开 legacy writer 或反向拼接数据。

## 12. 演进路线与门禁

### Phase 0 · 架构收口与证据基线

- 以本文替换旧 Finance-rooted 模块化/JSONL 双向同步主张。
- 新增并接受单文件 Platform Kernel V1 ADR，固化 D1–D10、owner、日期与 supersedes 关系；按文首权威序建立 `docs/architecture/index.md`，旧 Finance-rooted/JSONL 文档必须标记 `superseded_by`。
- 将本文、ADR、M1 architecture/threat-model 文档及其所描述的施工线状态放入可到达的具名 branch/commit，并记录 immutable commit SHA、base SHA 与审计日期。任何 CURRENT 依据不得只存在于 untracked/dirty working tree。
- 对 local-first M1、旧 HTTP dirty changes、0009 candidate、测试与打包状态分别建立验收清单；dirty candidate 可以继续研究，但不能成为 baseline 或交付证据。0009 必须在独立 review commit 上完成 §11.1 的语义审查。
- 后续 council 必须同时读取 Platform 规范与目标实现 worktree，不能只审窄化提示词；Council 报告是证据输入，不自动升级为规范或实现通过。
- 本阶段只固化事实、决策与门禁，不改公共 route、不切数据、不引入双写。

**出口**：从记录的 branch/commit fresh checkout 可以读取 ADR、本文和必要协议文档，并复核全部 CURRENT 断言；规范文件已经被版本控制，不存在只能靠 working tree 恢复的架构基线；不存在第二套 canonical truth，也没有“未提交 = 已交付”的状态升级。

### Phase 1 · 落地并独立验收 local-first 基础

- 收口 `finance-core`、crypto/event protocol、event/projection stores、sync、Tauri/Stronghold 与 Runtime ports。
- 复验 command dedup、deterministic replay、wire/staging、remote freeze、恢复事务和 renderer crash outbox。
- 用真实打包的 macOS `.app` 完成创建、确认恢复短语、锁定/解锁、记账、更正、同步、空机恢复与投影重建。
- M1 的 `crates/`、`apps/desktop/`、runtime 与实施文档必须进入记录的 immutable commit SHA，独立验证日志绑定同一 SHA；形成可审计分支不等于已合并或发布。
- 验证 control genesis、event envelope 与当前 Vault 的 `VaultId` 一致，并验证一 Vault 一 repository；本阶段不得实现任何墙钟触发的领域副作用。

**出口**：源码测试、构建、bundle、真实窗口流程分别通过；commit、merge 与发布状态单独报告。临时 Finance workbench 可以继续作为 M1 根页面，但必须明确报告 D10 尚未实现；main Web `AppShell`、Chromium 截图或仅能打出 `.app` 都不得升级为 Desktop Shell 通过。本阶段的 file/bare/mock 多设备验证也不得升级为 production multi-device ready。

### Phase 2 · 抽出 Platform Kernel，Finance 成为第一个模块

- 清除 Platform 对 Money/Account/Journal 的依赖；Finance 通过 scoped ports 使用 Vault。
- 将现有最小 ModuleManifest 演进为 §5 合同，删除 Sidebar 导航语义，落地稳定 page ID/`moduleHome` compatibility gate 与每 Vault descriptor/state。
- 落地 `PinnedModulesSet` 与 `ModuleConfigurationSet` 两个 Vault 级 full-set reducer、基于 competing head EventIds 的可重建 platform conflict、owner resolver 与默认 Finance onboarding pin；reducer 对抗测试必须覆盖相反 fetch 顺序、resolver 先于部分被引用 heads 到达、解决期间出现第三个 head，以及 stale resolver 重放，并证明最终候选与 effective state 收敛。将 renderer outbox 迁为 Vault-scoped schema，并完成 A → B → A 隔离测试。
- 落地 multi-vault host lifecycle；拒绝两个 Vault 绑定同一 repository，并验证并发空 remote bootstrap。M1 单 Vault 限制不再冒充产品目标。
- 实现最小 `DeviceRevoked` control record、cutoff disposition 与新 DeviceId 恢复路径；通过前不得宣称 multi-device production ready。
- 用 descriptor 驱动的 Desktop Shell 替换直接 `VaultWorkspace` 根节点，落地模块管理、固定/取消固定、排序、折叠、deep link 与 Page Host，并关闭全局 `⌘/Ctrl+B`、`Tab` 劫持缺陷。
- 固化归档不改余额、显式 correction/reversal，以及 module disable 行为矩阵。
- 本阶段仍不得接入墙钟触发的领域副作用；Finance subscriptions 只能手动执行或只读展示到期信息。

**出口**：依赖方向门禁通过，`vault-sqlite` 不再依赖 Finance 类型；禁用/启用 Finance 不丢数据、不改变余额；projection 可重建；宿主无 Finance ID 条件分支；相反 fetch 顺序产生相同 preference conflict/resolution。D10 只在真实打包 macOS Tauri `.app` 完成 §3.3 的 pin/order、展开/折叠、窄窗 overlay、pointer、Tab/Shift+Tab、快捷键隔离、deep link、禁用/恢复与 focus return 后通过。

### Phase 3 · 用非财务模块验证宿主

- 实现 `EntityRef`、全局 search/relation index、module-native object store 与贡献点 registry。
- 各取一个最小 Collections slice 和 Shard slice，验证结构化 records 与原生文件对象两种数据形态。
- 将真实非 Finance 模块固定到 Sidebar、排序、deep link 打开并取消固定，验证 Shell 不增加 module-ID 分支。
- 已安装但禁用的模块仍维护 upcast/projection，并可做语义导出与恢复。
- 未知、未安装或不兼容模块只做 byte-identical opaque 保存、同步、checksum 验证与 §8.6 的 opaque preservation；没有代码/schema 时不得声称可以 semantic export 或 upcast。
- 删除 search/relation index 后必须可重建；Git object、导出和日志的 plaintext scan 均不得发现索引内容。本阶段仍不得接入墙钟触发的领域副作用。

**出口**：增加第三个模块不修改 host 或 Shell 分支；真实非 Finance 模块可固定且未固定时仍可到达；删除全部 projection/index 后可从 Vault 重建；Finance/Shard/Collections 共享 change protocol 但不共享万能 schema。

### Phase 4 · Scheduler 与 Notification

- 实现 durable intent/job、lease/retry、时区/recurrence/missed occurrence、deterministic occurrence key。
- 实现 device-local notification 重算、权限、去重、点击回源与模块禁用竞态。
- 实现与对抗测试只能先在隔离 test Vault 和显式 non-production gate 下启用；出口通过前不得连接用户 Vault、进入默认配置或形成可发布的墙钟副作用能力。

**出口**：月底/闰年、tzdb 版本不一致、DST gap/fold、离线、多设备并发、schedule 编辑后补跑、暂停/禁用恢复与 catch-up cap 均有对抗测试；GET 无隐藏写入；同一 `scheduleId + occurrenceInstantUtc` 在任何 revision 和 missed-run 路径下只产生一个 canonical 结果。

### Phase 5A · Legacy importer 与 cutover

- 完成 §11 的 source detection、OpeningBalance 日期、Reversal + ArchiveJournal、双端 cutover 状态机与 crash matrix。
- 至少使用两份不同 schema/version 的真实脱敏快照 rehearsal。保留旧 `/api/v1` 的 route/schema 与读取兼容；cutover 后旧 DB writer 永久只读，写端点只能稳定返回 `adapter_read_only`，或在 Phase 5B trusted-device 门禁通过后映射到 canonical command，绝不能恢复 legacy writer。

**出口**：重复导入幂等、余额逐分一致、projection rebuild 一致、每个 cutover transition（包括 `activation_pending` 前后、append 响应丢失和重启）故障注入安全、秘密/明文扫描通过、旧库未被就地修改。5A 完成不依赖 Docker/self-host 可生产。

### Phase 5B · 受信 self-host adapter

- legacy lane 与 legacy snapshot importer 可在不接触 canonical keys 的情况下独立保留；未加入控制链的 self-host 不得提供 canonical query。
- canonical 只读 headless host 也必须以独立、可撤权的 trusted `DeviceId` 加入 Vault，满足 secret/recovery 门禁，并显式关闭 writer capability；此时写请求稳定返回 `adapter_read_only`。
- 若启用 canonical write，self-host 还必须满足完整 trusted-writer 门禁，并复用相同 command、sync、Scheduler、secret 与 recovery 合同。
- 按 §10.3 完成 production image、认证、凭据、backup/restore、升级回滚与真实 container restore drill，而非只通过 health endpoint。

**出口**：只读与可写能力显式可探测；trusted headless host 不复用桌面身份并可被最小撤权；5B 失败不得回滚或否定已经通过的 5A importer/cutover 验收。

### 远期

- key epoch rotation、历史密文读取撤销与密钥再分发；最小 DeviceId 后续写权限撤销已属于 multi-device production 门禁。
- 签名 checkpoint、离线设备 ack 与可证明安全的 compaction。
- 不可信第三方模块 runtime、签名分发与 Registry。
- 多 owner/shared Vault；该阶段必须重新审计密钥分发、权限撤销和并发语义。

## 13. 全局验收不变量

以下任一失败都阻止宣称“模块架构完成”：

1. Platform Kernel 不依赖任何 Finance/Shard/Collections 领域类型。
2. 新增模块不需要在 shell/kernel 中增加 module-ID 条件分支。
3. 所有 projection、search 与 relation index 可从 canonical Vault 重建。
4. 更换 Git transport 不改变领域 command、event 或冲突语义。
5. 禁用模块不删除、不失认、不停止同步其 canonical data。
6. 归档前后账户余额逐项相等；经济变化只能来自显式财务事件。
7. remote 从第一条业务记录起无明文；秘密与本地 projection/search/relation/outbox 明文不进入 Git、日志或自动导出。
8. 同一完整 fetched control/event/object 输入集在不同到达顺序下产生相同 disposition、canonical set、projection、余额、领域冲突与 ingest quarantine。
9. 未知未来协议或模块 schema fail closed 且保留原始数据。
10. Web/Tauri/self-host 的能力差异显式返回，不静默假成功。
11. 本地测试、提交/合并、bundle、部署与生产验收分别报告，不互相替代。
12. Legacy cutover 前后只有一个可写真相源；任何迁移阶段都不存在 DB ↔ Vault 双写。
13. Sidebar 固定列表只保存 `ModuleId` 与顺序；固定、启用、授权和数据所有权互不冒充，未固定模块仍可到达。
14. 桌面展开/折叠与窗口自适应不重挂载当前页面、不丢键盘焦点，也不把 Tauri 导航替换为移动端底栏；活动模块变为 `module_disabled`、`module_unavailable` 或 `forbidden` 时保留 route intent，但必须卸载模块页并安全替换为对应 Shell-owned 状态页。
15. 每个 V1 Vault 独占一个 Git repository；control genesis、refs、envelope 与当前 Vault 的 `VaultId` 必须一致，空 remote 并发 bootstrap 只能有一个胜者。
16. renderer 草稿按 Vault 隔离，不得跨 Vault 自动展示、恢复、ack、discard 或提交；同步后的 command dedup 必须能从 canonical records 重建。
17. `pinnedModules` 与 full-set module configuration 的合法离线分叉按 competing head EventIds 产生可重建 platform conflict，并只能由引用完整 head set 的显式 resolver 关闭；相反到达顺序或新 head 竞态不得改变候选、部分解决或按 revision 误认同一 head。
18. 同一 `scheduleId + occurrenceInstantUtc` 在任何 revision、设备与 missed-run 路径下最多形成一个 canonical 领域结果；Phase 4 出口前，除隔离 test Vault + 显式 non-production gate 外，不存在墙钟触发的领域写入或通知投递，且测试能力不得进入用户数据或 release 配置。
19. 对包含有效 `DeviceRevoked` 的同一完整输入集，cutoff 后记录的最终 disposition 必须为 `revoked_device`，最终 effective projection 中不得保留其领域影响；曾在不完整 control frontier 下临时应用的影响必须确定性撤回并保留审计报告。撤权不能被另一设备代写、复用密钥或本地 UI 状态绕过。
20. self-host 在作为独立、具备 secret backend 且可撤权的 trusted device 加入前，不得解密或查询 canonical Vault；加入后 read/write capability 分开授权，writer 未开时稳定只读。
21. legacy cutover 的每个崩溃点都必须保持至多一个 writer；`activation_pending` 允许暂时零 writer 并 fail closed。状态最终只能收敛到从未进入 pending 时的“legacy 唯一可写”，或进入 pending 后的“Vault 唯一可写”；任何未知/不匹配状态 fail closed，不存在 dual write 窗口。

## 14. 术语

- **Canonical Vault**：控制记录、platform/domain events 与模块原生对象组成的规范数据集合。
- **Event store**：本设备持久保存 canonical records 的 append-only store，不是缓存。
- **Projection**：从 canonical data 确定性派生的查询模型，可删除重建。
- **Module**：拥有领域不变量、数据 schema 与贡献点的顶层能力单元，不等于页面或数据库表。
- **Runtime adapter**：把宿主中立 ports 映射到 Tauri IPC、HTTP 或未来 headless host 的边界层。
- **Command outbox**：renderer 未完成写操作的 durable 草稿，不是复制队列。
- **Distributed command outcome**：从 canonical records 确定性派生的 command 逻辑结果；可随 projection 删除重建，是同步收敛后的 dedup 依据。
- **Platform conflict**：合法签名的 owner platform events 因并发父版本而产生的可重建分叉；不是安全 quarantine，必须由对应 resolver 显式关闭。
- **Ingest quarantine**：协议、签名、密文或 envelope 非法记录的安全隔离 disposition；不能接收正常的领域/platform 并发冲突。
- **Frozen**：检测到控制链或远端历史 rewind/rewrite 等异常后停止 promote 的 Vault 状态；不等于删除已接受历史。
- **Wire / staging / promote**：分别表示未信任输入的耐久留存、隔离验证区和进入 canonical set 的原子接受动作。
- **Replay scheduler**：决定事件重放顺序的确定性算法。
- **Time Scheduler**：按现实时间派发 durable occurrence 的平台能力。
- **Occurrence identity**：`VaultId + ModuleId + ScheduleId + OccurrenceInstantUtc` 的稳定语义身份；schedule revision 只决定该 instant 的 payload，不创造第二身份。
- **PageId**：`{ModuleId}/{localId}` 形式的永久页面协议身份；与 route path、title、icon 和 module version 解耦。
- **Archive**：可见性/可写性状态，不是财务撤销或物理删除。
- **Semantic export / Opaque preservation**：前者是当前模块可解释的明文副本，后者是未知模块 byte-identical 的认证密文保全；二者都不是新的同步真相。
- **Cutover activation**：由 legacy 侧耐久状态与 Vault 内 `LegacyCutoverActivatedV1` 共同证明唯一 writer 已切换的双端状态，不是一次部署或内存 flag。
- **Pinned module**：Sidebar 中指向模块 `moduleHome` 的 Vault 级有序快捷入口，不代表模块已授权或拥有 Shell。
- **Local shell state**：expanded/collapsed、宽度、overlay 和焦点等本机窗口状态；不进入 canonical Vault 或同步。

## 15. 审计记录

- 2026-08-06：两轮 council 对较窄的 Finance/Axum 模块化方案给出了 legacy migration、持久幂等、调度、墓碑和自部署方面的有效提醒；其输入未包含正在施工的 local-first worktree，因此不能作为整体 Platform 架构的通过证明。
- 2026-08-06：独立审计同时核对两个 worktree，发现 `Router<AppState> + JSONL sync` 与 `Vault/event protocol` 是不兼容主线，并发现旧 HTTP 归档流水会改变余额。
- 2026-08-06：所有者确认采用 D1–D9，本文据此改写；旧 council 结论只保留仍适用于新边界的工程门禁。
- 2026-08-06：所有者补充确认 D10：桌面端使用可折叠左侧边栏 + 页面，模块可以固定在左侧边栏。
- 2026-08-06 · council-20260806-115918 · 选手:codex,antigravity,grok,mimo,kimi · 主席codex裁决 MEDIUM / CONDITIONAL APPROVE：「4/5 有效意见一致认可 D1–D10 方向，按 Phase 关闭 preference、模块合同、multi-vault、Scheduler、迁移与 D10 实现门禁后继续」· 报告 /Users/panyuhang/.council/zhiyu/council-20260806-115918/viewer.html
- 2026-08-06：按 `council-20260806-115918` 主席推荐修订正文，补齐 preference resolver、manifest/页面身份、三层 command dedup、Vault-scoped outbox、一 Vault 一 repository、Scheduler、最小设备撤权、OpeningBalance、cutover 与分阶段验收合同；此记录只证明文档已修订，不证明 Phase 0、实现或发布已通过。
