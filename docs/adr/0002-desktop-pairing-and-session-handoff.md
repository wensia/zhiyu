# ADR-0002：桌面端配对与一次性 session 交接

- 状态：**Proposed**（2026-08-10）——待作者决策后转 Accepted
- 相关：`docs/adr/0001-server-authoritative-thin-client.md`
- 外部意见：ChatGPT Pro（会话 https://chatgpt.com/c/6a78b9e0-5c7c-83e8-9d15-de68237bd517）
  结论经本地独立核实，核实结果见文末

## 问题

ADR-0001 把桌面端定为纯 WebView 壳，指向远程 URL。但 WebView 加载远程页面时不会带
`Authorization` 头，页面里的 JS 靠 cookie 认证。于是桌面端如何取得身份成了问题。

初版设想是：用 `initialization_script` 把 api-key 注入页面 JS 上下文，让前端改用 Bearer。
**该方案已否决**，原因：

1. 把十年有效期的长期凭证放进远程页面的 JS 环境，页面上任何脚本（未来引入的第三方
   依赖、或服务器被替换后的恶意页面）都能读到，且无法按设备撤销。
2. `initialization_script` 会在**所有顶层导航**执行（Windows 上还会进入子 frame），
   靠 Origin 判断来兜底不值得把安全性押在上面。
3. 桌面走 Bearer、浏览器走 cookie，前端要写两套认证分支并感知自己跑在哪个宿主里。

## 决定（提案）

**远程页面只持有普通、短期、HttpOnly 的 Web session；桌面设备的长期身份留在 Rust
进程和系统凭据库里。**

采用「一次性配对码 → 每设备独立凭证 → 一次性 session 交接票据 → 服务端通过正常
HTTPS 响应写入现有 `__Host-zhiyu_session`」。

### 凭证层次

| 层 | 生命周期 | 存放 | 用途 |
|---|---|---|---|
| enrollment code | 5–15 分钟，单次 | CLI 输出，用户复制 | 只用于注册一台设备 |
| device secret | 长期至撤销 | 桌面端系统凭据库（Rust 侧） | 换取 handoff ticket、拉备份 |
| handoff ticket | 30–60 秒，单次 | 仅在本地 bootstrap 页短暂存在 | 换取 session cookie |
| session cookie | 30 天 | WebView cookie store | 现有 `__Host-zhiyu_session`，前端零改动 |
| **api-key** | 十年 | 用户自行保管 | **保留给 CLI / 自动化 / 外部集成，不再作为桌面日常身份** |

### 流程

```
首次配对
────────
服务端 CLI               docker exec zhiyu zhiyu pair
  └─ 输出连接码           ZY1.<base64: base_url + server_id + enrollment_code + expires_at>

用户粘贴到桌面端
  └─ Rust: POST /desktop/devices/enroll { enrollment_code, device_name }
     ←  { device_id, device_secret }        device_secret 只返回一次
  └─ 写入系统凭据库

每次启动
────────
Rust  ──device_secret──▶  POST /desktop/session-handoffs
                          ←  { ticket }   30–60 秒、单次使用

WebView ──▶ 本地打包的 bootstrap 页（不加载任何远程资源）
              └─ 顶层表单 POST 到 /desktop/session-handoffs/consume

服务端  ──▶ 原子消费 ticket，创建带 device_id 的 session
            Set-Cookie: __Host-zhiyu_session=...; Secure; HttpOnly; Path=/; SameSite=Lax
            303 Location: /

WebView ──▶ 跟随跳转进入远程 React 应用，此后 credentials: "include" 照旧
```

关键点：进入本地 bootstrap 页上下文的只是一张 30–60 秒的单次票据，不是设备长期凭证；
**远程 React 页面两者都接触不到**。

让服务端通过正常 HTTPS 响应写 cookie，走的是浏览器引擎最标准的路径，不必去赌各平台
native cookie 注入对 `__Host-` 前缀的语义是否一致。

## 必须一并修的既有缺陷

### csrf_guard 用字符串包含判断 cookie 存在性

`apps/api/src/lib.rs:231-236`：

```rust
let has_session_cookie = request.headers().get(header::COOKIE)
    .and_then(|value| value.to_str().ok())
    .is_some_and(|value| value.contains(state.config.cookie_name()));
if unsafe_method && has_session_cookie { /* 校验 Origin */ }
```

**只看 Cookie 头里有没有这个名字，不看 cookie 是否有效。** 后果：任何带着过期
session cookie 的 Bearer 写请求都会被要求 Origin 匹配，Origin 不符即 403，哪怕认证
实际走的是 Bearer。这是当前代码就存在的缺陷，也会挡住本 ADR 的 handoff POST
（其 Origin 是 Tauri 本地应用 Origin，不等于 `public_base_url`）。

修法：让认证中间件产出显式的机制标记，CSRF 依据**实际选中的认证机制**判断，而不是
猜测请求里有没有 cookie。

```rust
AuthContext { user, mechanism: Session | ApiKey | DeviceCredential | HandoffTicket }
```

`AuthUser` 已经优先读 request extensions，上游中间件写入该上下文即可。

`/desktop/session-handoffs/consume` 必须：位于 session-CSRF 层之外；不接受 session
cookie 作为认证依据；即使请求意外携带旧 cookie 也忽略；只凭不可预测、短时、单次的
ticket 完成认证。

### capability 绑在加载远程内容的窗口上

`apps/desktop/src-tauri/capabilities/default.json` 把 `core:default` 授予 `"main"`
窗口，而 main 加载的正是远程 URL。`withGlobalTauri` 未启用，远程页面拿不到
`window.__TAURI__`，但 IPC 通道本身是开的。引入 bootstrap 页后应把能力按窗口拆分，
远程窗口的权限收到最小。

`tauri.conf.json` 的 `security.csp` 当前是 `null`，bootstrap 页需要自己的严格 CSP：
`default-src 'none'`、`form-action` 只允许当前配置的服务器、`base-uri 'none'`、
`frame-ancestors 'none'`。

## 首次连接的输入形态

**不再让用户分别填「域名或 IP:端口」和 api-key**，改为粘贴一段完整连接码：

```
ZY1.eyJ2IjoxLCJiYXNlX3VybCI6Imh0dHBzOi8vLi4uIn0
   └─ { v, base_url, server_id, enrollment_code, expires_at }
```

高级设置里保留「手动输入完整 URL + 一次性配对码」作为 fallback。

要求完整 URL（`https://ledger.example.com`），不接受裸域名或 `ip:port`——后者在
默认协议、路径前缀、IPv6 解析、默认端口、自签证书上都有歧义。生产模式要求 HTTPS 且
证书校验正常，不提供「一键忽略证书错误」；开发模式单独允许 `http://127.0.0.1`。

可增加无认证探测端点 `GET /.well-known/zhiyu` 返回 `{ product, protocol_version,
server_id, canonical_base_url }`，桌面端在消费 enrollment code 前核对 server_id
与协议版本。

同类项目的做法：Immich 输入完整 endpoint 后正常登录；Home Assistant 优先局域网发现、
失败回退手动完整 URL；Tailscale 区分单次与可复用 auth key；Syncthing 交换的 Device ID
是公钥派生的非秘密标识，属于对等模型，不适合直接照搬到服务端账户认证。

## 备份职责划分

```
服务器            桌面端
──────            ──────
生成一致快照  ──▶  下载服务器已生成的快照
发布 manifest      校验 SHA-256 + 长度，可选 PRAGMA quick_check
自己保留 30 天     写 .partial → fsync → 原子 rename
                   自己保留 30 天
```

**快照生成逻辑只实现一次（服务端），复制、调度、清理各自实现。** 桌面端不再自己对
数据库做快照。

保留策略抽成共享的纯函数模块（输入 snapshots + now + 30 天，输出 keep/delete），
两端调用同一套规则但**分别执行删除**。这种重复应当接受，因为两端失败模式不同：
服务器磁盘满不应删掉桌面唯一副本；桌面时钟错误不应改变服务器保留结果。

语义定为：保留 `created_at >= now - 30 天` 的全部成功快照，且**无论如何额外保留最新
一个成功快照**。

桌面端按 manifest 比对本地已有快照 ID，补齐所有服务器仍保留但本地缺失的快照，而不只
下载最新一个——这样关闭几天后重开能补上历史。

**调度必须在 Tauri Rust 层**，不能交给远程 React 页面：页面刷新会中断任务；WebView
在最小化时可能暂停 timer；前端触发意味着要给远程页面敏感 Tauri 能力。触发点为
启动时、重获前台时、Rust timer 周期检查。应用完全退出时不执行——除非另装系统级计划
任务，否则产品文案不能承诺「不开应用也每天备份」。

## 本地核实结果

| 断言 | 核实 |
|---|---|
| 需 Tauri ≥ 2.8 才有 `set_cookie` / `delete_cookie` | **确认**。`Cargo.lock` 解析到 tauri **2.11.5**，方案 B 在版本上可行 |
| `initialization_script` 在所有顶层导航执行 | **未实测**，采信官方文档。当前 `lib.rs` 只用它注入 CSS，无 secret，风险可接受；但据此否决用它承载凭证 |
| capabilities 需审查 | **确认且更糟**：`core:default` 授予的 `"main"` 就是加载远程 URL 的窗口 |
| 交接 POST 会被 CSRF 拒绝 | **确认且更精确**：`lib.rs:235` 用 `contains(cookie_name)` 判断，过期 cookie 也会触发 Origin 校验 |
| `withGlobalTauri` 是否启用 | **确认未启用**（`tauri.conf.json` 中为 None） |
| native 注入 `__Host-` cookie 三平台一致 | **未验证，把握低**。这正是不选方案 B 的理由 |
| 本地 bootstrap POST → Set-Cookie → 303 三平台可用 | **未验证，把握中高**。落地前必须三平台实测 |

## 被否决的替代方案

- **方案 B：Tauri `set_cookie` 原生注入** —— 版本上可行（2.11.5），服务端改动最少，
  React 零改动。但 WebView2 的 `AddOrUpdateCookie` 要求指定 domain，而 `__Host-`
  规范要求不得含 `Domain` 属性，native API 的 domain 字段是否会破坏 host-only 属性
  没有官方保证；WKWebView / WebKitGTK 对 host-only、前缀校验、HttpOnly+Secure+SameSite
  完整保留、重启后持久化均无 Tauri 层承诺。**留作方案 A 实测失败后的备选。**
- **方案 C：本地 loopback 反向代理** —— 需要删除浏览器 Cookie、重写 `Set-Cookie` 与
  `Location`、处理 CSP/WebSocket/SSE/Range/Service Worker，且代理本身是个
  confused deputy（任何浏览器页面都能访问 `127.0.0.1:<port>`），要防 DNS rebinding。
  对单用户账本越过合理复杂度边界。
- **自定义 URI scheme 拦截** —— `register_uri_scheme_protocol` 是注册新协议由 Rust
  返回响应，不是拦截现有 HTTPS 加头；各平台 Origin 不一致（macOS/iOS/Linux 是
  `<scheme>://localhost`，Windows/Android 是 `http://<scheme>.localhost`），会重新
  引入 CORS/CSP/Origin 问题。`on_web_resource_request` 只处理 Tauri 自有协议。

## 安全边界（必须说清）

HttpOnly session 防的是**持久凭证被 JS 读走**，挡不住同源恶意 JS 在页面打开期间替
用户发起已认证请求。所以：

- 第三方脚本 / XSS 拿不走设备长期凭证 ✓
- 但它仍可利用当前 session 操作账本
- 前端修复后，攻击者无法继续使用已窃取的凭证（因为它从未拿到设备凭证）
- 若服务端被完全接管，服务端权威架构本身无法保护账本完整性

若威胁模型要求「即使服务器能替换前端静态资源，桌面应用也不应执行这些资源」，那需要
把 React 前端重新打包进 Tauri，或引入资源签名/版本固定——改认证方式解决不了这个级别
的问题。

## 落地前必须实测

1. 三平台（Windows 11 + WebView2 Evergreen、目标 macOS、目标 Linux + WebKitGTK）
   跑通 bootstrap POST → Set-Cookie → 303 完整链路，验证 `/api/v1/auth/me` 与重启后
   cookie 持久化
2. handoff POST 在「携带旧 session cookie」「无 cookie」「ticket 已消费」三种情况下的行为
3. 撤销设备后该设备所有 session 立即失效
4. handoff ticket 并发提交只能成功一次
5. 连接码 / enrollment code / ticket 不进入反向代理 access log
6. 远程页面无法调用任何 Tauri 能力（文件、shell、HTTP client）
7. `on_navigation` 严格限制服务器 Origin，外链交给系统浏览器
8. 系统凭据库在目标 Linux 桌面可用（无 Secret Service / keyring 锁定 / 无图形会话时的行为）
9. 桌面备份在休眠、断网、磁盘满、下载中断后可恢复
