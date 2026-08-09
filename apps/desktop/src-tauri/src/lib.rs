mod backup_client;
mod config;

use backup_client::{BackupClient, get_backup_settings, save_backup_settings};
use tauri::{
    Manager, WebviewUrl, WebviewWindowBuilder,
    menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder},
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
            app.manage(backup_client);

            let settings_item =
                MenuItemBuilder::with_id(SETTINGS_MENU_ID, "备份设置…").build(app)?;
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

            let configured_url = std::env::var("ZHIYU_DESKTOP_URL")
                .unwrap_or_else(|_| DEFAULT_DESKTOP_URL.to_owned());
            let url = configured_url.parse()?;
            let mut window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
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
            if needs_settings {
                open_settings_window(app.handle())?;
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("启动知余桌面版失败");
}
