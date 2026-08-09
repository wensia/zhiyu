use tauri::{
    WebviewUrl, WebviewWindowBuilder,
    menu::{MenuBuilder, SubmenuBuilder},
};
use tracing_subscriber::EnvFilter;

const DEFAULT_DESKTOP_URL: &str = "https://zhiyu.askfish.net";
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

pub fn run() {
    tracing_subscriber::fmt()
        .compact()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("zhiyu_desktop=info")),
        )
        .init();

    tauri::Builder::default()
        .setup(|app| {
            let app_menu = SubmenuBuilder::new(app, "知余")
                .about(None)
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
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("启动知余桌面版失败");
}
