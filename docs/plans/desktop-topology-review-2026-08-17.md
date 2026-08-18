# 桌面端与后端形态复审（2026-08-17）

- 状态：**已拍板暂缓（2026-08-17）**——作者决定先不做本机版，ADR-0001 的「部署版 + 瘦客户端」现状不变。本文保留为将来做本机版时的施工图；届时送达方式为「后端随安装包一起装 + 首次引导页选住哪」。
- 起因：作者提出「纯 web 后端 + Tauri；服务器部署给 MCP/hermes/openclaw 用；纯本地把后端与 Tauri 打进同一个 dmg、改配置即可」
- 过程：Claude 构思 → 多模型 council（codex/grok/kimi/antigravity，session `council-20260817-074525`）→ ChatGPT Pro 两轮复审（会话 https://chatgpt.com/c/6a82c08f-3d54-83e8-ab4f-583db30ee342 ，基线 commit `ee67a78`）
- 本文只记**经本地源码核实**的结论；外部意见中「依赖未读内容」的部分已逐条对照代码，未核实的不写。

## 一、裁决

**修正后接受 A：一份 dmg，`zhiyu-api` 作 signed externalBin（sidecar），Tauri 永远只是「连一个 URL 的瘦客户端」，本机账本 = 由壳托管的 loopback `zhiyu-api`。**

前提是作者先回答两个需求问题（ChatGPT 的 Phase -1，council 的 grok/kimi 异议同旨）：

- Q1 本机权威库是不是真需求？若一年用不到几次 → 不做（方案 C），把 loopback 收成正式连接目标即可。
- Q2 GUI 退出后本机 agent 是否还必须能用？若「否」→ 内嵌同进程（D1，同样一份 dmg）与 sidecar 差距缩小；若「是」→ sidecar，且未来演进为 launchd 常驻（届时删除父死子死）。

## 二、对第一轮 council 裁决的修正（已核实）

| 类型 | 修正 |
|---|---|
| 推理过强 | 「sidecar 成本趋零」不成立：externalBin 按 target triple 进构建/签名/公证/更新面。正确表述：**接受额外二进制的生命周期成本，换 API 部署形态同构（Docker/本机/测试/agent 全是同一个 `zhiyu-api`）+ 进程隔离**。 |
| 遗漏 | 内嵌 Axum 也能做成一份发行版（D1）；「sidecar vs 内嵌」不能用「一发行版 vs 两发行版」裁决，真正分水岭是 Q2。 |
| **推翻** | **临时 shell API key 取消。** `handoff_tickets`（0011）本身就是完整 capability（user_id/hash/expires_at/consumed_at，与 `api_keys` 无耦合）；把 shell 启动凭证塞进 3650 天的 `api_keys` 会污染 `last_used_at`/撤销/审计/备份语义。 |
| **推翻** | **Rust `cookies_for_url` 回读不再是认证判据。** 改服务端 ready barrier（见 §三 D），整段 `session_cookies_for_url`/`confirm_new_session_cookie`/dev 放行删除。 |
| 事实修正 | 「poppler GPL + 仓库 UNLICENSED ⇒ 不能打包」推不出；shell out 独立可执行文件与链接库的法律分析不同，属许可问题非架构问题。**但**「靠用户 PATH」也不行：macOS GUI 不继承 shell `$PATH`，`Command::new("pdftotext")` 在 dmg 安装态找不到 Homebrew。短期显式探测 `/opt/homebrew/bin`、`/usr/local/bin`、`/usr/bin`、PATH；中期用真实招行/民生 corpus 做 PDFium spike（字符级 bbox，需自建 word/line 重建，不能直接替换 `-bbox-layout`）。 |
| 事实修正 | 「本机不用 `__Host-`」是兼容性策略，不是规范定理；仍照做（http loopback → `zhiyu_session`）。 |
| 事实修正 | MCP：SSE 已是历史；当前是 stdio / Streamable HTTP。hermes、openclaw 两者都支持。选独立 `zhiyu-mcp` stdio 的理由是职责清晰（agent host 管进程、复用 API key、可连本机或服务器），不是规范偏好。 |
| 心智模型 | 「两本账」→ **「权威账本实例（stable instance_id）+ 部署位置」**；一次性迁移 = **authority transfer**（源变只读），禁止默认从服务器快照恢复到本机（无同步系统下的 silent fork）。「不做双向同步」原样保留。 |
| 表述 | 「服务器行为逐字节不变」改为「账本 API、普通 Web session、self-host 部署语义不变；desktop handoff 协议升级」。 |
| 补充坑 | `tracing_subscriber::fmt().init()`（`main.rs`）默认写 stdout；stdout 做控制协议前**必须**切 stderr。 |
| 补充坑（本地核实） | 领域表按 `user_id` 隔离；`issue_api_key` 会为不同邮箱建 verified 机器用户，服务器库可能不止一个 verified user。**本地 principal 必须显式持久化选择，不能按「恰好一个 verified user」推断。** |

## 三、实施规格（源码级）

### A. `desktop-local` 启动
1. `APP_ENV=desktop-local`：`is_production()` 仍 false（`zhiyu_session`、无 Secure）；`email_delivery_available()` 对 `self-host | desktop-local` 为 false。
2. 强制 `127.0.0.1:0`，其它 `BIND_ADDR` 报错；`PUBLIC_BASE_URL` 必须为空，不引入 `auto-loopback` 哨兵。
3. 小型二阶段配置：`UnboundConfig::from_env()` → `TcpListener::bind` → `local_addr()` → `finalize(addr) -> Config`（`public_base_url = http://127.0.0.1:<port>`）。`main.rs` 顺序：unbound → bind → finalize → `db::connect` → `AppState` → scheduler → `app(state)`。router 本就在 bind 后构造，`app()` 接口不动。
4. 拒绝非本地 `DATABASE_URL` / 非空 `TURSO_AUTH_TOKEN`。
5. 数据目录 flock（第二个实例硬失败）、迁移前 `VACUUM INTO` 快照、目录排除 Time Machine/iCloud。

### B. 票据
6. 从 `create_handoff_ticket` 抽 `mint_handoff_ticket_for_user(state, user_id)`（清过期 → `new_token` → INSERT，TTL 60s）；HTTP 端点改为 Bearer 认证后调用它，外部行为不变。**无 migration。**
7. 本地 principal：首次由迁移/初始化流程显式写入本地元数据；启动时校验其存在且 verified，否则拒绝启动。

### C. 父子私有控制通道（stdin/stdout NDJSON）
8. stdout 只走协议，stderr 走日志（`with_writer(std::io::stderr)`）；每行 flush；≤ 8 KiB；未知 `v/op` 返回 protocol error；stdin EOF = 父进程消失 → 退出。
9. 启动只发 ready，不预铸票：`{"v":1,"event":"ready","boot_id":…,"base_url":"http://127.0.0.1:P","pid":…}`。
10. 按需铸票：请求 `{"v":1,"id":…,"op":"mint_handoff_ticket"}` → `{"v":1,"id":…,"ok":true,"ticket":…,"expires_at":…}`；另有 `{"op":"shutdown"}`。
11. Tauri 侧用 `tauri-plugin-shell`（仅 Rust 调用，不暴露给页面）或 `std::process::Command` 管道均可。

### D. 服务端 ready barrier
12. 新路由 `GET /desktop/session-ready`、`/desktop/handoff-complete`、`/desktop/handoff-failed`。
13. `consume_desktop_handoff` 成功 → Set-Cookie + 303 `/desktop/session-ready`；票据无效/过期/已消费 → 303 `/desktop/handoff-failed`（不再 303 `/`）。
14. `session-ready` 只认 cookie session（`csrf_guard` 对该路径禁止 Bearer fallback）；有效 → 303 `/desktop/handoff-complete`，否则 303 `/desktop/handoff-failed`。普通浏览器访问 complete → 303 `/`，failed → 303 `/login`。三者均 `no-store` + `no-referrer`。

### E. 壳侧状态机
15. `ProbingExistingSession → AcquiringTicket → PerformingHandoff → AwaitingReady → Established | Failed`。
16. 首次启动与 Reopen 第一步都是隐藏窗口 GET `/desktop/session-ready`（session-first）：到 `handoff-complete` → show → `/`，**零新票**；到 `handoff-failed` → 远程走 `POST /api/v1/auth/handoff-tickets`、本机走 stdin 铸票 → 现有 `GET /desktop/handoff/{ticket}` → 再到 ready；第二次仍 failed → 终止，不循环。
17. `/login` 拦截保留，降级为运行期 session 失效兜底；20 秒 deadline 覆盖整条 probe→mint→handoff→ready。
18. `LocalBackendSupervisor`：spawn、握手 deadline、health、崩溃重启一次（带抖动）后停在设置页、`Exit` 收割、macOS `ExitRequested`/`prevent_exit`/Reopen 五条路径各自验证；single-instance。

### F. 修改点
`apps/api/src/config.rs`、`main.rs`、`auth.rs`、`lib.rs`（3 路由 + csrf session-only）、新增 `apps/api/src/desktop_local.rs`；`apps/desktop/src-tauri/{Cargo.toml, tauri.conf.json(externalBin), src/lib.rs, src/config.rs(连接目标 LocalThisMac|Remote)}`；构建脚本产出 `zhiyu-api-<triple>`。migrations 不改。

## 四、落地顺序与风险
- Phase -1：答 Q1/Q2。
- Phase 0：ready barrier + 删 cookie 回读/dev 放行 + loopback 成正式连接目标（不碰 sidecar）。
- Phase 1：API 侧 desktop-local 全套（二阶段 config、instance_id、flock、principal、控制通道、迁移前快照），CLI 集成测试验证。
- Phase 2：Tauri supervisor + externalBin + 签名公证。
- Phase 3：authority transfer（服务器 ↔ Mac）。
- Phase 4：`zhiyu-mcp` stdio。
- 风险排序：sidecar/打包态生命周期 > authority 身份与防 fork > dmg 安装态 PATH/资源/签名差异 > cookie handoff > 公证。

## 五、待本地验证的假设
1. 公证后的 dmg 里 Set-Cookie→303 后 `/auth/me` 是否已成功而 `cookies_for_url` 仍看不到（证实 native 回读是错误抽象）。
2. macOS 关窗 / Cmd+Q / Dock Reopen / 更新重启 / panic 五条路径的 sidecar 清理。
3. API 在页面运行中被 kill 后 GUI 的恢复状态机是否确定。
4. `/Applications` 安装态的实际 PATH 与 `/opt/homebrew/bin/pdftotext` 可见性。
5. 现有真实招行/民生样本建立 golden transaction 输出（PDFium spike 的比对基线）。
6. 当前 DB 是否有适合承载 `ledger_instance_id` 的元数据位置。
