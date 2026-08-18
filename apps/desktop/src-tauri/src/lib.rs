mod backup_client;
mod config;

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use backup_client::{BackupClient, HandoffTicket, get_backup_settings, save_backup_settings};
use tauri::{
    AppHandle, LogicalSize, Manager, PhysicalPosition, PhysicalSize, Url, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder,
    menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder},
};
use tracing_subscriber::EnvFilter;

const SETTINGS_MENU_ID: &str = "connection-settings";
const DISCONNECT_MENU_ID: &str = "disconnect-server";
const SESSION_COOKIE_NAMES: [&str; 2] = ["__Host-zhiyu_session", "zhiyu_session"];
const HANDOFF_PAGE_TIMEOUT: Duration = Duration::from_secs(20);
const DEFAULT_WINDOW_WIDTH: f64 = 1280.0;
const DEFAULT_WINDOW_HEIGHT: f64 = 840.0;

// 交通灯归侧边栏管：它不再浮在顶栏上，而是独占侧边栏顶上那一行（56pt，与顶栏等
// 高），品牌 lockup 和折叠按钮都排在它下面。尺寸也比 macOS 默认小一号——默认的
// 12pt 是按 28pt 标准标题栏配的，放进侧边栏这一行里显得粗壮。
#[cfg(target_os = "macos")]
mod traffic_lights {
    use anyhow::{Context, Result};
    use objc2_app_kit::{NSWindow, NSWindowButton};
    use objc2_quartz_core::CATransform3D;
    use tauri::WebviewWindow;

    /// 系统交通灯的排版，实测得来：按钮控件框 14pt 见方，中间画 12pt 的圆点，相邻
    /// 按钮中心距 23pt。tao 摆放按钮时也是照抄这个系统间距。
    const BUTTON_FRAME: f64 = 14.0;
    const DOT: f64 = 12.0;
    const PITCH: f64 = 23.0;
    /// 缩小一号：圆点 12 → 9.6pt，中心距同比收到 18.4pt。
    const SCALE: f64 = 0.8;
    /// 侧边栏折叠后的宽度。折叠态交通灯要在侧边栏里居中，所以这个宽度和灯组宽度是
    /// 一对：改一个就得改另一个（另一半在 window_chrome_script 注入的 CSS 里）。
    const COLLAPSED_SIDEBAR_WIDTH: f64 = 72.0;
    /// 侧边栏首格与顶栏等高 56px，交通灯与它共享中线。
    const SIDEBAR_HEADER_HEIGHT: f64 = 56.0;

    /// 缩放后整组灯的视觉宽度：首尾两个圆点外缘之间。
    const GROUP_WIDTH: f64 = PITCH * SCALE * 2.0 + DOT * SCALE;
    /// 折叠态居中定出灯组的视觉左缘（≈12.8pt）。展开态沿用同一个 x——它正好落在
    /// 侧边栏 12px 的内边距上，与导航项左对齐，折叠/展开切换时灯也不会横向跳。
    const VISUAL_LEFT: f64 = (COLLAPSED_SIDEBAR_WIDTH - GROUP_WIDTH) / 2.0;

    /// tao 摆的是按钮控件框的左缘，比圆点的视觉左缘往左多出半圈留白。
    pub const ORIGIN_X: f64 = VISUAL_LEFT - (BUTTON_FRAME - DOT * SCALE) / 2.0;
    /// tao 把交通灯容器的高度设成「按钮高 + y」，而按钮在容器里贴着底部 9pt，于是
    /// 圆点中心距窗口顶 = y - 2（实测）。要落在首格中线上，y 得把这 2pt 补回去。
    pub const ORIGIN_Y: f64 = SIDEBAR_HEADER_HEIGHT / 2.0 + 2.0;

    /// 缩放交通灯。窗口尺寸变化后 AppKit 会重排这三颗按钮，所以这个函数要能被反复
    /// 调用：它只写死值，不累积。
    pub fn shrink(window: &WebviewWindow) -> Result<()> {
        let address = window.ns_window().context("拿不到 macOS 原生窗口")? as usize;
        window
            .run_on_main_thread(move || {
                // SAFETY: 地址来自同一个存活窗口的 ns_window()，且只在主线程解引用。
                unsafe { shrink_on_main_thread(address) }
            })
            .context("在主线程缩放交通灯失败")
    }

    unsafe fn shrink_on_main_thread(address: usize) {
        let window: &NSWindow = unsafe { &*(address as *const NSWindow) };
        let buttons = [
            NSWindowButton::CloseButton,
            NSWindowButton::MiniaturizeButton,
            NSWindowButton::ZoomButton,
        ];
        for (index, kind) in buttons.into_iter().enumerate() {
            let Some(button) = window.standardWindowButton(kind) else {
                continue;
            };
            // 只缩按钮的话中心距还是系统的 23pt，圆点小了、缝隙没变，一组灯就散了。
            // 收拢间距得挪按钮自己的 frame：热区跟着一起走，不会像 layer 变换那样
            // 让看得见的圆点和点得到的地方错开。窗口 resize 后 tao 会按系统间距重
            // 排，所以这里写的是绝对值，反复调用也不会越缩越紧。
            let mut origin = button.frame().origin;
            origin.x = ORIGIN_X + PITCH * SCALE * index as f64;
            button.setFrameOrigin(origin);

            button.setWantsLayer(true);
            let Some(layer) = button.layer() else {
                continue;
            };
            // 缩放绕 layer 的锚点发生，而 AppKit 给按钮配的锚点在左下角（实测），
            // 照搬缩放会把圆点往那个角拽。锚点是 AppKit 自己在维护的（改它就得连
            // position 一起补，之后每次重排还得再补一次），所以这里不动它，改为把
            // 「锚点到中心」的偏移折进变换里，效果等价而互不干扰。
            let bounds = layer.bounds();
            let anchor = layer.anchorPoint();
            let recenter_x = (0.5 - anchor.x) * bounds.size.width * (1.0 - SCALE);
            let recenter_y = (0.5 - anchor.y) * bounds.size.height * (1.0 - SCALE);
            layer.setTransform(scaled(SCALE, recenter_x, recenter_y));
        }
    }

    fn scaled(scale: f64, shift_x: f64, shift_y: f64) -> CATransform3D {
        CATransform3D {
            m11: scale,
            m12: 0.0,
            m13: 0.0,
            m14: 0.0,
            m21: 0.0,
            m22: scale,
            m23: 0.0,
            m24: 0.0,
            m31: 0.0,
            m32: 0.0,
            m33: 1.0,
            m34: 0.0,
            m41: shift_x,
            m42: shift_y,
            m43: 0.0,
            m44: 1.0,
        }
    }
}

fn centered_position(
    work_area_position: PhysicalPosition<i32>,
    work_area_size: PhysicalSize<u32>,
    window_size: PhysicalSize<u32>,
) -> PhysicalPosition<i32> {
    let centered_axis = |origin: i32, available: u32, window: u32| {
        let offset = (i64::from(available) - i64::from(window)).max(0) / 2;
        origin.saturating_add(offset.min(i64::from(i32::MAX)) as i32)
    };
    PhysicalPosition::new(
        centered_axis(
            work_area_position.x,
            work_area_size.width,
            window_size.width,
        ),
        centered_axis(
            work_area_position.y,
            work_area_size.height,
            window_size.height,
        ),
    )
}

fn center_window_in_current_display(window: &WebviewWindow) -> Result<()> {
    let monitor = match window.current_monitor()? {
        Some(monitor) => monitor,
        None => window
            .primary_monitor()?
            .context("找不到可用于居中窗口的显示器")?,
    };
    let work_area = monitor.work_area();
    let position = centered_position(work_area.position, work_area.size, window.outer_size()?);
    window.set_position(position)?;
    Ok(())
}

// 这里注入的 CSS 只负责「桌面窗口才需要」的装饰：给交通灯让位、隐藏网页版的退出
// 登录、补一个还原窗口按钮。
//
// 交通灯的安置方式经历过三版。第一版让侧边栏躲：`.sidebar{margin-top:34px}` 把整根
// 侧边栏往下推，交通灯于是悬在一块不属于任何区域的空白里，logo 卡在半空。第二版让
// 顶栏通宽，交通灯落在顶栏地盘上，只需要 `.topbar{padding-left:96px}`——代价是侧边栏
// 被顶栏拦腰截断，窗口左上角归顶栏管，可交通灯明明长在侧边栏那一列上。
//
// 现在侧边栏通高（见 styles.css 的 .sidebar grid-area），窗口左上角是侧边栏自己的
// 地盘，交通灯就住在那儿，横向占 12.8..59.2px（几何见 traffic_lights 模块）：
//   侧边栏顶部空出与顶栏等高的 56px，这一行只归交通灯，品牌 lockup 和折叠按钮整体
//   下移一格。让品牌挤到交通灯右边也塞得下（展开态 248px 够宽），但那一行就成了
//   「半格系统控件 + 半格产品标识」的混排，且折叠到 72px 时必然要拆开重排——同一
//   块东西在两个状态里长得不一样，不如始终让它独占一行。
//   折叠态宽度从 64px 加到 72px，正是为了让交通灯在这一态左右各余 12.8px、居中落
//   在侧边栏上。
fn window_chrome_script(server_url: &Url) -> Result<String> {
    let origin = serde_json::to_string(&server_url.origin().ascii_serialization())?;
    Ok(format!(
        r##"(function () {{
  if (location.origin !== {origin}) return;
  var css = ".sidebar{{padding-top:56px}}.app-shell[data-sidebar='collapsed']{{grid-template-columns:72px minmax(0, 1fr)}}.sidebar-logout{{display:none}}.sidebar-window-reset{{width:32px;height:var(--nav-item-height);margin-top:auto;margin-left:auto;display:flex;align-items:center;justify-content:center;padding:0;border:0;border-radius:var(--radius-control);color:var(--sidebar-foreground);background:transparent;font:inherit;cursor:pointer}}.sidebar-window-reset:hover:not(:disabled){{background:var(--sidebar-accent)}}.sidebar-window-reset svg{{width:18px;height:18px;flex:0 0 18px}}.app-shell[data-sidebar='collapsed'] .sidebar-window-reset{{margin-right:auto}}";
  function markDragRegions() {{
    document.querySelectorAll(".topbar,.sidebar-header").forEach(function (element) {{
      element.setAttribute("data-tauri-drag-region", "");
      element.setAttribute("title", "拖动以移动窗口");
    }});
  }}
  function ensureWindowReset() {{
    if (document.querySelector(".sidebar-window-reset")) return;
    var logout = document.querySelector(".sidebar-logout");
    if (!logout || !logout.parentElement) return;
    var button = document.createElement("button");
    button.type = "button";
    button.className = "sidebar-window-reset";
    button.setAttribute("aria-label", "还原窗口大小并居中");
    button.title = "还原窗口大小并居中";
    button.innerHTML = '<svg aria-hidden="true" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 12a9 9 0 1 0 3-6.7L3 8"/><path d="M3 3v5h5"/></svg>';
    button.addEventListener("click", function () {{
      button.disabled = true;
      window.__TAURI_INTERNALS__.invoke("reset_main_window").catch(function (error) {{
        console.error("还原窗口失败", error);
      }}).finally(function () {{ button.disabled = false; }});
    }});
    logout.parentElement.insertBefore(button, logout);
  }}
  function inject() {{
    if (document.getElementById("zhiyu-desktop-chrome")) return;
    var style = document.createElement("style");
    style.id = "zhiyu-desktop-chrome";
    style.textContent = css;
    document.head.appendChild(style);
    markDragRegions();
    ensureWindowReset();
    new MutationObserver(function () {{ markDragRegions(); ensureWindowReset(); }}).observe(document.documentElement, {{ childList: true, subtree: true }});
  }}
  if (document.head) inject();
  else document.addEventListener("DOMContentLoaded", inject);
}})();"##
    ))
}

#[tauri::command]
fn reset_main_window(window: WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        return Err("只有主窗口可以执行还原操作".to_owned());
    }
    window
        .set_fullscreen(false)
        .map_err(|error| error.to_string())?;
    window.unmaximize().map_err(|error| error.to_string())?;
    window
        .set_size(LogicalSize::new(
            DEFAULT_WINDOW_WIDTH,
            DEFAULT_WINDOW_HEIGHT,
        ))
        .map_err(|error| error.to_string())?;
    center_window_in_current_display(&window).map_err(|error| format!("{error:#}"))?;
    Ok(())
}

#[derive(Default)]
struct HandoffGate {
    previous_session_cookies: Vec<(String, String)>,
    pending: bool,
}

fn open_settings_window(app: &tauri::AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("settings") {
        window.show()?;
        window.unminimize()?;
        window.set_focus()?;
        return Ok(());
    }
    let mut builder =
        WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
            .title("连接设置")
            .min_inner_size(820.0, 480.0)
            .resizable(true);

    // 相对主窗口定尺寸与位置。固定像素在不同大小的主界面上会忽大忽小，而系统默认
    // 位置会把窗口放在屏幕偏上、离主界面很远的地方——这两点作者都实际反馈过。
    //
    // 设置内容采用横向双列：左侧连接表单，右侧备份状态。窗口继续相对主窗口定尺寸
    // 与位置，并用上下限保证分屏和小显示器上仍有安全边距。
    match app.get_webview_window("main") {
        Some(main) => {
            let scale = main.scale_factor().unwrap_or(1.0);
            let outer = main.outer_size().map(|size| size.to_logical::<f64>(scale));
            let origin = main
                .outer_position()
                .map(|position| position.to_logical::<f64>(scale));
            match (outer, origin) {
                (Ok(outer), Ok(origin)) => {
                    let width = (outer.width * 0.72).clamp(820.0, 1000.0);
                    let height = (outer.height * 0.59).clamp(480.0, 560.0);
                    builder = builder.inner_size(width, height).position(
                        origin.x + (outer.width - width) / 2.0,
                        origin.y + (outer.height - height) / 2.0,
                    );
                }
                _ => builder = builder.inner_size(920.0, 500.0).center(),
            }
            // 附属于主窗口：设置窗口不应沉到主界面背后，否则用户以为它消失了。
            builder = builder.parent(&main)?;
        }
        // 首次启动尚无主窗口（还没配置连接），此时屏幕居中是唯一合理的落点。
        None => builder = builder.inner_size(920.0, 500.0).center(),
    }

    builder.build()?;
    Ok(())
}

#[cfg(test)]
fn is_local_page(url: &Url, page: &str) -> bool {
    let local_origin = (url.scheme() == "tauri" && url.host_str() == Some("localhost"))
        || (url.scheme() == "http" && url.host_str() == Some("tauri.localhost"));
    local_origin && url.path() == format!("/{page}")
}

fn local_sibling_url(window: &WebviewWindow, page: &str) -> Result<Url> {
    let mut url = window.url().context("读取桌面本地页面 URL 失败")?;
    url.set_path(&format!("/{page}"));
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.origin() == right.origin()
}

fn error_url_with_code(url: &Url, code: &str) -> Url {
    let mut target = url.clone();
    target.set_fragment(Some(code));
    target
}

fn handoff_csp(server_url: &Url) -> String {
    format!(
        "default-src 'none'; form-action {}",
        server_url.origin().ascii_serialization()
    )
}

fn handoff_initialization_script(ticket: &HandoffTicket, failure_reason: &str) -> Result<String> {
    let endpoint = ticket
        .server_url
        .join("desktop/handoff")
        .context("无法构造桌面交接地址")?;
    let endpoint = serde_json::to_string(endpoint.as_str())?;
    let ticket = serde_json::to_string(&ticket.ticket)?;
    let failure_reason = serde_json::to_string(failure_reason)?;
    Ok(format!(
        r##"(function () {{
  function isLocal(page) {{
    var local = (location.protocol === "tauri:" && location.hostname === "localhost") ||
      (location.protocol === "http:" && location.hostname === "tauri.localhost");
    return local && location.pathname === "/" + page;
  }}
  function whenReady(run) {{
    if (document.readyState === "loading") {{
      document.addEventListener("DOMContentLoaded", run, {{ once: true }});
    }} else {{
      run();
    }}
  }}
  if (isLocal("handoff.html")) {{
    whenReady(function () {{
      var form = document.getElementById("handoff-form");
      form.action = {endpoint};
      form.elements.ticket.value = {ticket};
      form.submit();
    }});
    return;
  }}
  if (isLocal("error.html")) {{
    whenReady(function () {{
      var reasons = {{
        "#cookie-readback": {failure_reason},
        "#session-expired": "桌面会话已失效；已阻止显示邮箱登录页。请重新保存连接设置。",
        "#timeout": "桌面交接页在 20 秒内没有完成表单提交与 cookie 确认；已阻止加载邮箱登录页。"
      }};
      document.getElementById("failure-reason").textContent = reasons[location.hash] || {failure_reason};
    }});
  }}
}})();"##
    ))
}

fn error_initialization_script(reason: &str) -> Result<String> {
    let reason = serde_json::to_string(reason)?;
    Ok(format!(
        r#"(function () {{
  var local = (location.protocol === "tauri:" && location.hostname === "localhost") ||
    (location.protocol === "http:" && location.hostname === "tauri.localhost");
  if (!local || location.pathname !== "/error.html") return;
  document.addEventListener("DOMContentLoaded", function () {{
    document.getElementById("failure-reason").textContent = {reason};
  }}, {{ once: true }});
}})();"#
    ))
}

fn session_cookies_for_url(
    window: &WebviewWindow,
    server_url: &Url,
) -> Result<Vec<(String, String)>> {
    let cookies = match catch_unwind(AssertUnwindSafe(|| {
        window.cookies_for_url(server_url.clone())
    })) {
        Ok(Ok(cookies)) => cookies,
        Ok(Err(error)) => {
            tracing::error!(error = %error, "reading desktop session cookie back returned an error");
            return Err(anyhow!(error).context("从 WebView 回读会话 cookie 失败"));
        }
        Err(_) => {
            tracing::error!("reading desktop session cookie back panicked");
            bail!("从 WebView 回读会话 cookie 时发生 panic");
        }
    };
    Ok(cookies
        .into_iter()
        .filter(|cookie| SESSION_COOKIE_NAMES.contains(&cookie.name()))
        .map(|cookie| (cookie.name().to_owned(), cookie.value().to_owned()))
        .collect())
}

fn confirm_new_session_cookie(
    window: &WebviewWindow,
    server_url: &Url,
    previous: &[(String, String)],
) -> Result<()> {
    let cookies = session_cookies_for_url(window, server_url)?;
    let cookie_names = cookies
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    let found = cookies.iter().any(|current| !previous.contains(current));
    tracing::info!(
        found,
        cookie_names = ?cookie_names,
        "desktop session cookie readback completed"
    );
    if !found {
        #[cfg(debug_assertions)]
        if config::is_development_frontend(server_url) {
            tracing::warn!("allowing loopback dev navigation after WKWebView cookie readback lag");
            return Ok(());
        }
        bail!("交接响应未写入新的会话 cookie；已阻止加载邮箱登录页");
    }
    Ok(())
}

fn close_main_window(app: &AppHandle) -> Result<()> {
    if let Some(window) = app.get_webview_window("main") {
        window.close().context("关闭旧主窗口失败")?;
    }
    Ok(())
}

fn show_native_alert(window: &WebviewWindow, message: &str) {
    let Ok(message) = serde_json::to_string(message) else {
        return;
    };
    if let Err(error) = window.eval(format!("window.alert({message})")) {
        tracing::error!(error = %error, "showing native alert failed");
    }
}

fn disconnect_from_server(app: &AppHandle) -> Result<()> {
    let client = app.state::<BackupClient>().inner().clone();
    let (server_url, credential_warning) = client.clear_connection()?;

    if let (Some(window), Some(server_url)) = (app.get_webview_window("main"), server_url) {
        match session_cookies_for_url(&window, &server_url) {
            Ok(cookies) => {
                tauri::async_runtime::spawn(async move {
                    if let Err(error) = client.logout_session(server_url, cookies).await {
                        tracing::warn!(error = %error, "best-effort desktop session logout failed");
                    }
                });
            }
            Err(error) => {
                tracing::warn!(error = %error, "best-effort desktop session cookie readback failed");
            }
        }
    }

    let close_result = close_main_window(app);
    let open_result = open_settings_window(app);
    close_result?;
    open_result?;
    if let Some(warning) = credential_warning
        && let Some(window) = app.get_webview_window("settings")
    {
        show_native_alert(&window, &format!("连接配置已清除，但{warning}"));
    }
    Ok(())
}

fn confirm_disconnect(app: &AppHandle) -> Result<()> {
    let window = if let Some(window) = app.get_webview_window("main") {
        window
    } else {
        open_settings_window(app)?;
        app.get_webview_window("settings")
            .context("连接设置窗口创建后不可用")?
    };
    let handle = app.clone();
    let error_window = window.clone();
    window.eval_with_callback(
        "window.confirm('断开与服务器的连接？\\n这会清除本机保存的服务器配置和 api-key。')",
        move |value| {
            if serde_json::from_str::<bool>(&value).unwrap_or(false)
                && let Err(error) = disconnect_from_server(&handle)
            {
                let readable = format!("断开连接失败：{error:#}");
                tracing::error!(error = %readable, "disconnecting desktop from server failed");
                show_native_alert(&error_window, &readable);
            }
        },
    )?;
    Ok(())
}

fn main_window_builder<'a>(
    app: &'a AppHandle,
    initial_url: WebviewUrl,
) -> WebviewWindowBuilder<'a, tauri::Wry, AppHandle<tauri::Wry>> {
    let mut builder = WebviewWindowBuilder::new(app, "main", initial_url)
        .title("知余")
        .inner_size(1280.0, 840.0)
        .min_inner_size(960.0, 640.0)
        .background_color(tauri::window::Color(0xff, 0xff, 0xff, 0xff));
    #[cfg(target_os = "macos")]
    {
        // 交通灯的默认位置是按 macOS 标准 28pt 标题栏算的，整组中心落在 y≈19pt。
        // 这里的窗口没有标题栏，交通灯坐在侧边栏顶格里，位置得跟着侧边栏走——
        // 具体数值见 traffic_lights 模块。
        builder = builder
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true)
            .traffic_light_position(tauri::LogicalPosition::new(
                traffic_lights::ORIGIN_X,
                traffic_lights::ORIGIN_Y,
            ));
    }
    builder
}

/// 窗口建好后才能摸到 AppKit 的按钮，缩放因此不能写在 builder 里。resize 会让
/// AppKit 重排按钮（全屏进出尤其明显），所以这里顺带挂上事件重新缩放一次。
fn apply_window_chrome(window: &WebviewWindow) {
    #[cfg(target_os = "macos")]
    {
        if let Err(error) = traffic_lights::shrink(window) {
            tracing::error!(error = %format!("{error:#}"), "shrinking traffic lights failed");
        }
        let resized_window = window.clone();
        window.on_window_event(move |event| {
            if !matches!(event, tauri::WindowEvent::Resized(_)) {
                return;
            }
            if let Err(error) = traffic_lights::shrink(&resized_window) {
                tracing::error!(
                    error = %format!("{error:#}"),
                    "re-shrinking traffic lights after resize failed"
                );
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

fn show_handoff_error(app: &AppHandle, reason: &str) -> Result<()> {
    close_main_window(app)?;
    let script = error_initialization_script(reason)?;
    let navigation_app = app.clone();
    let window = main_window_builder(app, WebviewUrl::App("error.html".into()))
        .initialization_script(script)
        .on_navigation(move |url| {
            if url.scheme() == "zhiyu" && url.host_str() == Some("open-settings") {
                if let Err(error) = open_settings_window(&navigation_app) {
                    tracing::error!(error = %error, "opening connection settings failed");
                }
                return false;
            }
            true
        })
        .build()?;
    apply_window_chrome(&window);
    center_window_in_current_display(&window)?;
    window.show()?;
    Ok(())
}

fn open_handoff_window(
    app: &AppHandle,
    client: &BackupClient,
    ticket: HandoffTicket,
) -> Result<()> {
    close_main_window(app)?;
    let server_url = ticket.server_url.clone();
    let failure_reason =
        "交接响应没有写入新的会话 cookie。请检查服务器地址与部署配置后重新保存连接设置。";
    let initialization_script = format!(
        "{}\n{}",
        handoff_initialization_script(&ticket, failure_reason)?,
        window_chrome_script(&server_url)?
    );
    let gate = Arc::new(Mutex::new(HandoffGate::default()));
    let error_url = Arc::new(Mutex::new(None::<Url>));
    let navigation_app = app.clone();
    let navigation_client = client.clone();
    let navigation_server_url = server_url.clone();
    let navigation_gate = gate.clone();
    let navigation_error_url = error_url.clone();
    let csp_server_url = server_url.clone();

    let window = main_window_builder(app, WebviewUrl::App("index.html".into()))
        .visible(false)
        // 交接窗口在 cookie 确认前保持隐藏，而 Tauri 默认的 Suspend 策略会把不在
        // 窗口里的 webview 任务完全挂起——跳板页的表单提交脚本因此根本不执行，
        // 表现为交接 20 秒超时而服务端从未收到请求。
        .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled)
        .initialization_script(initialization_script)
        .on_web_resource_request(move |request, response| {
            if request.uri().path() == "/handoff.html" {
                let csp = handoff_csp(&csp_server_url);
                if let Ok(value) = csp.parse() {
                    response.headers_mut().insert("Content-Security-Policy", value);
                }
            }
        })
        .on_navigation(move |url| {
            if url.scheme() == "zhiyu" && url.host_str() == Some("open-settings") {
                if let Err(error) = open_settings_window(&navigation_app) {
                    tracing::error!(error = %error, "opening connection settings failed");
                }
                return false;
            }
            if !same_origin(url, &navigation_server_url) {
                return true;
            }

            let Some(window) = navigation_app.get_webview_window("main") else {
                tracing::error!("main window disappeared during desktop session handoff");
                return false;
            };
            if url.path() == "/login" {
                let reason = "桌面会话已失效；已阻止显示邮箱登录页。请重新保存连接设置。";
                tracing::error!(error = reason, "desktop email login navigation blocked");
                let client = navigation_client.clone();
                let reason_owned = reason.to_owned();
                tauri::async_runtime::spawn(async move {
                    client.record_session_handoff_error(Some(reason_owned)).await;
                });
                if let Some(target) = navigation_error_url.lock().ok().and_then(|url| url.clone()) {
                    if let Err(error) =
                        window.navigate(error_url_with_code(&target, "session-expired"))
                    {
                        tracing::error!(error = %error, "opening local handoff error page failed");
                    }
                    let _ = window.show();
                }
                return false;
            }

            // 交接请求自身要放行：cookie 由它的 303 响应种下，此刻检查必然为空。
            // 等浏览器跟随重定向回到应用页面时，下面的 gate 才做确认。
            if url.path().starts_with("/desktop/handoff") {
                return true;
            }

            let mut gate = match navigation_gate.lock() {
                Ok(gate) => gate,
                Err(_) => {
                    tracing::error!("desktop handoff gate lock was poisoned");
                    return false;
                }
            };
            if !gate.pending {
                return true;
            }
            let result = confirm_new_session_cookie(
                &window,
                &navigation_server_url,
                &gate.previous_session_cookies,
            );
            gate.pending = false;
            drop(gate);
            match result {
                Ok(()) => {
                    tracing::info!(url = %url, "desktop session handoff succeeded");
                    let client = navigation_client.clone();
                    tauri::async_runtime::spawn(async move {
                        client.record_session_handoff_error(None).await;
                    });
                    if let Err(error) = window.show() {
                        tracing::error!(error = %error, "showing authenticated main window failed");
                    }
                    true
                }
                Err(error) => {
                    let readable = format!("{error:#}");
                    tracing::error!(error = %readable, "desktop session handoff failed");
                    let client = navigation_client.clone();
                    let recorded = readable.clone();
                    tauri::async_runtime::spawn(async move {
                        client.record_session_handoff_error(Some(recorded)).await;
                    });
                    if let Some(target) = navigation_error_url.lock().ok().and_then(|url| url.clone()) {
                        if let Err(error) =
                            window.navigate(error_url_with_code(&target, "cookie-readback"))
                        {
                            tracing::error!(error = %error, "opening local handoff error page failed");
                        }
                        let _ = window.show();
                    }
                    false
                }
            }
        })
        .build()?;
    apply_window_chrome(&window);
    center_window_in_current_display(&window)?;

    let previous_session_cookies = session_cookies_for_url(&window, &server_url)?;
    let handoff_url = ticket
        .server_url
        .join(&format!("desktop/handoff/{}", ticket.ticket))
        .context("无法构造桌面交接地址")?;
    let local_error_url = local_sibling_url(&window, "error.html")?;
    *error_url
        .lock()
        .map_err(|_| anyhow!("桌面错误页状态锁已损坏"))? = Some(local_error_url);
    {
        let mut gate = gate.lock().map_err(|_| anyhow!("桌面交接状态锁已损坏"))?;
        gate.previous_session_cookies = previous_session_cookies;
        gate.pending = true;
    }
    window
        .navigate(handoff_url)
        .context("打开本地交接跳板页失败")?;
    let timeout_gate = gate.clone();
    let timeout_app = app.clone();
    let timeout_client = client.clone();
    let timeout_error_url = error_url.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(HANDOFF_PAGE_TIMEOUT).await;
        let timed_out = timeout_gate.lock().is_ok_and(|mut gate| {
            if gate.pending {
                gate.pending = false;
                true
            } else {
                false
            }
        });
        if !timed_out {
            return;
        }
        let reason = "桌面交接页在 20 秒内没有完成表单提交与 cookie 确认；已阻止加载邮箱登录页。";
        tracing::error!(error = reason, "desktop session handoff timed out");
        timeout_client
            .record_session_handoff_error(Some(reason.to_owned()))
            .await;
        if let Some(window) = timeout_app.get_webview_window("main")
            && let Some(target) = timeout_error_url.lock().ok().and_then(|url| url.clone())
        {
            if let Err(error) = window.navigate(error_url_with_code(&target, "timeout")) {
                tracing::error!(error = %error, "opening local timeout error page failed");
            }
            let _ = window.show();
        }
    });
    Ok(())
}

pub(crate) async fn handoff_main_window_session(app: &AppHandle, client: &BackupClient) {
    let result = match client.create_handoff_ticket().await {
        Ok(ticket) => open_handoff_window(app, client, ticket),
        // context 保持中性：连不上前端/服务器时，「api-key 获取失败」会把人往凭证方向带。
        Err(error) => Err(error.context("建立桌面会话失败")),
    };
    if let Err(error) = result {
        let readable = format!("{error:#}");
        tracing::error!(error = %readable, "desktop session handoff failed");
        client
            .record_session_handoff_error(Some(readable.clone()))
            .await;
        if let Err(page_error) = show_handoff_error(app, &readable) {
            tracing::error!(error = %page_error, "showing local handoff error page failed");
        }
    }
}

pub fn run() {
    tracing_subscriber::fmt()
        .compact()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("zhiyu_desktop=info")),
        )
        .init();

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_backup_settings,
            save_backup_settings,
            reset_main_window
        ])
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            let data_dir = app.path().app_data_dir()?;
            let backup_client = BackupClient::new(config_dir, data_dir)?;
            let needs_settings = !backup_client.has_connection_config();
            backup_client.spawn_scheduler();
            app.manage(backup_client.clone());

            let settings_item = MenuItemBuilder::with_id(SETTINGS_MENU_ID, "连接设置…")
                .accelerator("CmdOrCtrl+Comma")
                .build(app)?;
            let disconnect_item =
                MenuItemBuilder::with_id(DISCONNECT_MENU_ID, "断开连接").build(app)?;
            let app_menu = SubmenuBuilder::new(app, "知余")
                .about(None)
                .separator()
                .item(&settings_item)
                .separator()
                .item(&disconnect_item)
                .separator()
                .quit()
                .build()?;
            let edit_menu = SubmenuBuilder::new(app, "编辑")
                .undo()
                .redo()
                .separator()
                .cut()
                .copy()
                .paste()
                .select_all()
                .build()?;
            let window_menu = SubmenuBuilder::new(app, "窗口")
                .minimize()
                .close_window()
                .build()?;
            app.set_menu(
                MenuBuilder::new(app)
                    .items(&[&app_menu, &edit_menu, &window_menu])
                    .build()?,
            )?;
            app.on_menu_event(|handle, event| match event.id().as_ref() {
                SETTINGS_MENU_ID => {
                    if let Err(error) = open_settings_window(handle) {
                        tracing::error!(error = %error, "opening connection settings failed");
                    }
                }
                DISCONNECT_MENU_ID => {
                    if let Err(error) = confirm_disconnect(handle) {
                        tracing::error!(error = %error, "opening disconnect confirmation failed");
                    }
                }
                _ => {}
            });

            if needs_settings {
                // 未配置时不创建主窗口，更不会加载远程 URL；连接设置是唯一入口。
                open_settings_window(app.handle())?;
            } else {
                let handle = app.handle().clone();
                let client = backup_client.clone();
                tauri::async_runtime::spawn(async move {
                    handoff_main_window_session(&handle, &client).await;
                });
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("构建知余桌面版失败")
        .run(|app, event| match event {
            tauri::RunEvent::ExitRequested {
                code: None, api, ..
            } => {
                #[cfg(target_os = "macos")]
                api.prevent_exit();
            }
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen {
                has_visible_windows: false,
                ..
            } => {
                let client = app.state::<BackupClient>().inner().clone();
                if client.has_connection_config() {
                    if app.get_webview_window("main").is_none() {
                        let handle = app.clone();
                        tauri::async_runtime::spawn(async move {
                            handoff_main_window_session(&handle, &client).await;
                        });
                    }
                } else if let Err(error) = open_settings_window(app) {
                    tracing::error!(error = %error, "reopening connection settings failed");
                }
            }
            _ => {}
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centers_window_inside_display_work_area() {
        assert_eq!(
            centered_position(
                PhysicalPosition::new(0, 25),
                PhysicalSize::new(1728, 1055),
                PhysicalSize::new(1280, 840),
            ),
            PhysicalPosition::new(224, 132)
        );
        assert_eq!(
            centered_position(
                PhysicalPosition::new(-1920, 0),
                PhysicalSize::new(1920, 1080),
                PhysicalSize::new(1280, 840),
            ),
            PhysicalPosition::new(-1600, 120)
        );
    }

    #[test]
    fn settings_window_uses_horizontal_layout_and_single_password_icon() {
        let html = include_str!("../../placeholder/settings.html");
        let css = include_str!("../../placeholder/settings.css");
        let script = include_str!("../../placeholder/settings.js");

        assert!(html.contains("class=\"settings-grid\""));
        assert_eq!(html.matches("<svg").count(), 1);
        assert!(html.contains("id=\"key-visibility-icon\""));
        assert!(!html.contains("id=\"icon-hide\""));
        assert!(css.contains("grid-template-columns: minmax(0, 1.65fr)"));
        assert!(script.contains("EYE_ICON"));
        assert!(script.contains("EYE_OFF_ICON"));
        assert!(!script.contains("iconHide.hidden"));
    }

    #[test]
    fn handoff_script_contains_ticket_only_behind_local_page_check() {
        let ticket = HandoffTicket {
            server_url: "https://zhiyu.example.com".parse().unwrap(),
            ticket: "single-use-secret".to_owned(),
        };
        let script = handoff_initialization_script(&ticket, "failure").unwrap();
        let check = script.find("isLocal(\"handoff.html\")").unwrap();
        let secret = script.find("single-use-secret").unwrap();
        assert!(check < secret);
        assert!(script.contains("https://zhiyu.example.com/desktop/handoff"));
        assert!(!script.contains("?ticket="));
        assert_eq!(
            handoff_csp(&ticket.server_url),
            "default-src 'none'; form-action https://zhiyu.example.com"
        );
        let chrome_script = window_chrome_script(&ticket.server_url).unwrap();
        assert!(chrome_script.contains("location.origin !== \"https://zhiyu.example.com\""));
        // 交通灯独占侧边栏顶上那一行：两个状态都留 56px，宽度只在折叠态改。
        assert!(chrome_script.contains(".sidebar{padding-top:56px}"));
        assert!(
            chrome_script
                .contains(".app-shell[data-sidebar='collapsed']{grid-template-columns:72px")
        );
        assert!(!chrome_script.contains(".topbar{padding-left"));
        assert!(!chrome_script.contains(".sidebar-header{padding-left"));
        assert!(chrome_script.contains(".sidebar-logout{display:none}"));
        assert!(chrome_script.contains("data-tauri-drag-region"));
        assert!(chrome_script.contains("reset_main_window"));
        let html = include_str!("../../placeholder/handoff.html");
        assert_eq!(html.matches("<form").count(), 1);
        assert!(!html.contains("http://"));
        assert!(!html.contains("https://"));
        assert!(!html.contains("<script"));
    }

    #[test]
    fn local_page_detection_rejects_remote_lookalikes() {
        assert!(is_local_page(
            &"tauri://localhost/handoff.html".parse().unwrap(),
            "handoff.html"
        ));
        assert!(is_local_page(
            &"http://tauri.localhost/handoff.html".parse().unwrap(),
            "handoff.html"
        ));
        assert!(!is_local_page(
            &"https://zhiyu.example.com/handoff.html".parse().unwrap(),
            "handoff.html"
        ));
    }
}
