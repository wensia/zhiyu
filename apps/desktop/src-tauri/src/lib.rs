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
    AppHandle, Manager, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder},
};
use tracing_subscriber::EnvFilter;

const SETTINGS_MENU_ID: &str = "connection-settings";
const DISCONNECT_MENU_ID: &str = "disconnect-server";
const SESSION_COOKIE_NAMES: [&str; 2] = ["__Host-zhiyu_session", "zhiyu_session"];
const HANDOFF_PAGE_TIMEOUT: Duration = Duration::from_secs(20);
fn window_chrome_script(server_url: &Url) -> Result<String> {
    let origin = serde_json::to_string(&server_url.origin().ascii_serialization())?;
    Ok(format!(
        r##"(function () {{
  if (location.origin !== {origin}) return;
  var css = ".sidebar{{padding-top:34px}}.app-shell[data-sidebar='collapsed'] .topbar{{padding-left:76px}}.sidebar-logout{{display:none}}";
  function inject() {{
    if (document.getElementById("zhiyu-desktop-chrome")) return;
    var style = document.createElement("style");
    style.id = "zhiyu-desktop-chrome";
    style.textContent = css;
    document.head.appendChild(style);
  }}
  if (document.head) inject();
  else document.addEventListener("DOMContentLoaded", inject);
}})();"##
    ))
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
    WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
        .title("连接设置")
        .inner_size(640.0, 720.0)
        .min_inner_size(560.0, 620.0)
        .resizable(true)
        .build()?;
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
        builder = builder
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true);
    }
    builder
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
        Err(error) => Err(error.context("用 api-key 获取桌面交接票据失败")),
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
            save_backup_settings
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
        assert!(chrome_script.contains(".sidebar-logout{display:none}"));
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
