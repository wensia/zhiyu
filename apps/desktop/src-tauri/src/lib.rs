mod backup_client;
mod config;

use std::panic::{AssertUnwindSafe, catch_unwind};

use anyhow::{Context, Result, anyhow, bail};
use backup_client::{BackupClient, get_backup_settings, save_backup_settings};
use tauri::{
    AppHandle, Manager, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder},
    webview::{Cookie, cookie},
};
use tracing_subscriber::EnvFilter;

const DEFAULT_DESKTOP_URL: &str = "https://zhiyu.askfish.net";
const SETTINGS_MENU_ID: &str = "backup-settings";
const WINDOW_CHROME_SCRIPT: &str = r#"(function () {
  var css = ".sidebar{padding-top:34px}.app-shell[data-sidebar='collapsed'] .topbar{padding-left:76px}";
  function inject() {
    if (document.getElementById("zhiyu-desktop-chrome")) return;
    var style = document.createElement("style");
    style.id = "zhiyu-desktop-chrome";
    style.textContent = css;
    document.head.appendChild(style);
  }
  if (document.head) inject();
  else document.addEventListener("DOMContentLoaded", inject);
})();"#;

fn open_settings_window(app: &tauri::AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("settings") {
        window.show()?;
        window.unminimize()?;
        window.set_focus()?;
        return Ok(());
    }
    WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
        .title("备份设置")
        .inner_size(640.0, 720.0)
        .min_inner_size(560.0, 620.0)
        .resizable(true)
        .build()?;
    Ok(())
}

fn fallback_desktop_url() -> Result<Url> {
    std::env::var("ZHIYU_DESKTOP_URL")
        .unwrap_or_else(|_| DEFAULT_DESKTOP_URL.to_owned())
        .parse()
        .context("桌面远程 URL 无效")
}

fn webview_session_cookie(session: &backup_client::SessionHandoff) -> Result<Cookie<'static>> {
    let secure = session.server_url.scheme() == "https";
    if session.cookie_name.starts_with("__Host-") && !secure {
        bail!("__Host- cookie 只能注入 HTTPS 地址");
    }
    let cookie = Cookie::build((session.cookie_name.clone(), session.cookie_value.clone()))
        .path("/")
        .http_only(true)
        .same_site(cookie::SameSite::Lax)
        .secure(secure)
        .max_age(cookie::time::Duration::seconds(session.max_age))
        .build();
    if cookie.domain().is_some() {
        bail!("会话 cookie 意外包含 Domain，拒绝注入");
    }
    Ok(cookie)
}

async fn inject_session_cookie(
    window: &WebviewWindow,
    session: &backup_client::SessionHandoff,
) -> Result<()> {
    let cookie = webview_session_cookie(session)?;
    tracing::info!(
        cookie_name = %session.cookie_name,
        secure = cookie.secure(),
        path = cookie.path(),
        has_domain = cookie.domain().is_some(),
        "injecting desktop session cookie before remote navigation"
    );
    match catch_unwind(AssertUnwindSafe(|| window.set_cookie(cookie))) {
        Ok(Ok(())) => tracing::info!(
            cookie_name = %session.cookie_name,
            "desktop session cookie injection returned success"
        ),
        Ok(Err(error)) => {
            tracing::error!(
                error = %error,
                cookie_name = %session.cookie_name,
                "desktop session cookie injection returned an error"
            );
            return Err(anyhow!(error).context("向 WebView 注入会话 cookie 失败"));
        }
        Err(_) => {
            tracing::error!(
                cookie_name = %session.cookie_name,
                "desktop session cookie injection panicked"
            );
            bail!("向 WebView 注入会话 cookie 时发生 panic");
        }
    }

    let cookies = match catch_unwind(AssertUnwindSafe(|| {
        window.cookies_for_url(session.server_url.clone())
    })) {
        Ok(Ok(cookies)) => cookies,
        Ok(Err(error)) => {
            tracing::error!(
                error = %error,
                cookie_name = %session.cookie_name,
                "reading desktop session cookie back returned an error"
            );
            return Err(anyhow!(error).context("从 WebView 回读会话 cookie 失败"));
        }
        Err(_) => {
            tracing::error!(
                cookie_name = %session.cookie_name,
                "reading desktop session cookie back panicked"
            );
            bail!("从 WebView 回读会话 cookie 时发生 panic");
        }
    };
    let cookie_names = cookies
        .iter()
        .map(|cookie| cookie.name().to_owned())
        .collect::<Vec<_>>();
    let found = cookies.iter().any(|cookie| {
        cookie.name() == session.cookie_name && cookie.value() == session.cookie_value
    });
    tracing::info!(
        cookie_name = %session.cookie_name,
        found,
        cookie_names = ?cookie_names,
        "desktop session cookie readback completed"
    );
    if !found {
        if session.cookie_name.starts_with("__Host-") {
            bail!("__Host- cookie 注入失败，已回退到邮箱登录");
        }
        bail!("session cookie 注入失败，已回退到邮箱登录");
    }
    Ok(())
}

pub(crate) async fn handoff_main_window_session(app: &AppHandle, client: &BackupClient) {
    let Some(window) = app.get_webview_window("main") else {
        let error = "找不到主窗口，无法完成网页登录会话交接".to_owned();
        tracing::error!(%error);
        client.record_session_handoff_error(Some(error)).await;
        return;
    };
    let target_url = client
        .connection_server_url()
        .or_else(|_| fallback_desktop_url());
    let target_url = match target_url {
        Ok(url) => url,
        Err(error) => {
            let readable = format!("无法确定桌面远程 URL：{error:#}");
            tracing::error!(error = %readable, "desktop session handoff failed");
            client.record_session_handoff_error(Some(readable)).await;
            return;
        }
    };

    let handoff_result = match client.exchange_session().await {
        Ok(session) => inject_session_cookie(&window, &session).await,
        Err(error) => Err(error.context("用 api-key 换取网页登录会话失败")),
    };
    match handoff_result {
        Ok(()) => {
            client.record_session_handoff_error(None).await;
            tracing::info!(url = %target_url, "desktop session handoff succeeded");
        }
        Err(error) => {
            let readable = format!("{error:#}");
            tracing::error!(
                error = %readable,
                url = %target_url,
                "desktop session handoff failed; falling back to email login"
            );
            client.record_session_handoff_error(Some(readable)).await;
        }
    }

    if let Err(error) = window.navigate(target_url.clone()) {
        let readable = format!("打开远程页面 {target_url} 失败：{error}");
        tracing::error!(error = %readable, "desktop remote navigation failed");
        client.record_session_handoff_error(Some(readable)).await;
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

            let settings_item = MenuItemBuilder::with_id(SETTINGS_MENU_ID, "备份设置…")
                .accelerator("CmdOrCtrl+Comma")
                .build(app)?;
            let app_menu = SubmenuBuilder::new(app, "知余")
                .about(None)
                .separator()
                .item(&settings_item)
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
            app.on_menu_event(|handle, event| {
                if event.id().as_ref() == SETTINGS_MENU_ID
                    && let Err(error) = open_settings_window(handle)
                {
                    tracing::error!(error = %error, "opening backup settings failed");
                }
            });

            let initial_url = if needs_settings {
                WebviewUrl::External(fallback_desktop_url()?)
            } else {
                WebviewUrl::App("index.html".into())
            };
            let mut window = WebviewWindowBuilder::new(app, "main", initial_url)
                .title("知余")
                .inner_size(1280.0, 840.0)
                .min_inner_size(960.0, 640.0)
                .background_color(tauri::window::Color(0xff, 0xff, 0xff, 0xff))
                .initialization_script(WINDOW_CHROME_SCRIPT);
            #[cfg(target_os = "macos")]
            {
                window = window
                    .title_bar_style(tauri::TitleBarStyle::Overlay)
                    .hidden_title(true);
            }
            window.build()?;
            if !needs_settings {
                let handle = app.handle().clone();
                let client = backup_client.clone();
                tauri::async_runtime::spawn(async move {
                    handoff_main_window_session(&handle, &client).await;
                });
            }
            if needs_settings {
                open_settings_window(app.handle())?;
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("启动知余桌面版失败");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_cookie_is_secure_path_scoped_and_has_no_domain() {
        let session = backup_client::SessionHandoff {
            server_url: "https://zhiyu.example.com".parse().unwrap(),
            cookie_name: "__Host-zhiyu_session".to_owned(),
            cookie_value: "secret".to_owned(),
            max_age: 2_592_000,
        };

        let cookie = webview_session_cookie(&session).unwrap();

        assert_eq!(cookie.path(), Some("/"));
        assert_eq!(cookie.secure(), Some(true));
        assert_eq!(cookie.http_only(), Some(true));
        assert_eq!(cookie.same_site(), Some(cookie::SameSite::Lax));
        assert_eq!(cookie.domain(), None);
        assert_eq!(cookie.max_age().unwrap().whole_seconds(), 2_592_000);
    }
}
