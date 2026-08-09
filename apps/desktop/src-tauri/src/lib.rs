mod backup_config;
mod backup_github;
mod backup_http;
mod backup_page;
mod backup_runner;
mod backup_state;

use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, bail};
use axum::{
    extract::{Request, State},
    http::{Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get,
};
use tauri::{
    AppHandle, Manager, WebviewUrl, WebviewWindowBuilder,
    menu::{MenuBuilder, MenuItem, SubmenuBuilder},
};
use tracing_subscriber::EnvFilter;
use zhiyu_api::{AppState, config::Config, email::DevFileEmailSender, rate_limit::RateLimiter};

const LOCAL_USER_EMAIL: &str = "local@zhiyu.desktop";
const ENTER_PATH: &str = "/desktop/enter";
const HOME_PATH: &str = "/app/debts";
const BACKUP_NOW: &str = "backup-now";
const RESTORE_FROM_BACKUP: &str = "restore-from-backup";
const OPEN_BACKUP_SETTINGS: &str = "open-backup-settings";
const PENDING_RESTORE: &str = "pending-restore";
const AUTO_BACKUP_DEBOUNCE: std::time::Duration = std::time::Duration::from_secs(30);
const AUTO_BACKUP_WATCHDOG: std::time::Duration = std::time::Duration::from_secs(15 * 60);

struct BackupMenuItems {
    backup_now: MenuItem<tauri::Wry>,
    restore: MenuItem<tauri::Wry>,
}

struct DesktopServer {
    public_base_url: String,
}

/// 桌面窗口用无边框标题栏，页面内容一直铺到窗口顶端，于是左上角的红绿灯会压在
/// 侧栏上；侧栏顶部与折叠态的顶栏都要让出这块位置。
///
/// 之所以在这里注入而不是写进 `apps/web`：这纯粹是桌面外壳的需要，网页版没有
/// 红绿灯，不该为它留白。CSS 里的属性选择器用单引号，好安放在 JS 双引号字符串里。
///
/// 初始化脚本先于文档解析执行，`document.head` 未必已经存在，两种时机都要覆盖。
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

fn resolve_web_dist(app: &tauri::App) -> anyhow::Result<PathBuf> {
    let resource_web = app
        .path()
        .resource_dir()
        .context("无法确定应用资源目录")?
        .join("web");
    if resource_web.exists() {
        return Ok(resource_web);
    }

    let development_web = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../web/dist");
    if development_web.exists() {
        return Ok(development_web);
    }

    bail!("找不到前端构建产物，请先运行 pnpm --dir apps/web build");
}

async fn start_server(
    data_dir: PathBuf,
    web_dist_dir: PathBuf,
    backup_context: backup_runner::BackupContext,
    backup_now_item: MenuItem<tauri::Wry>,
    app_handle: AppHandle,
) -> anyhow::Result<u16> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let local_addr = listener.local_addr()?;
    let port = local_addr.port();
    let public_base_url = format!("http://127.0.0.1:{port}");
    let dev_mail_dir = data_dir.join("mail");
    let config = Config {
        // 本地 HTTP 环境必须使用开发模式，避免 Secure Cookie 被 webview 丢弃。
        app_env: "development".into(),
        bind_addr: local_addr,
        // CSRF 校验要求公开地址与窗口 origin 完全一致。
        public_base_url: public_base_url.clone(),
        database_url: format!("file:{}", data_dir.join("zhiyu.db").display()),
        turso_auth_token: None,
        dev_mail_dir: dev_mail_dir.clone(),
        web_dist_dir,
    };
    let db = zhiyu_api::db::connect(&config).await?;
    let state = AppState {
        db: Arc::new(db),
        config: Arc::new(config),
        email: Arc::new(DevFileEmailSender::new(dev_mail_dir)),
        rate_limiter: RateLimiter::default(),
    };
    let desktop_backup_router = backup_http::router(
        backup_http::BackupHttpState {
            backup: backup_context.clone(),
            backup_now: backup_now_item,
            app_handle,
            backup_settings_url: format!("{public_base_url}{}", backup_http::BACKUP_PATH),
        },
        public_base_url,
    );
    let router = zhiyu_api::app(state.clone())
        .route(
            ENTER_PATH,
            // 主路由已经消耗了状态，追加的 MethodRouter 必须单独绑定状态。
            get(enter).with_state(state),
        )
        .merge(desktop_backup_router)
        // 必须最后挂载，才能观察 API 主路由与之后追加的桌面路由。
        .layer(axum::middleware::from_fn_with_state(
            backup_context,
            observe_successful_write,
        ));

    tauri::async_runtime::spawn(async move {
        if let Err(error) = axum::serve(listener, router).await {
            tracing::error!(%error, "桌面内置服务异常退出");
        }
    });

    Ok(port)
}

async fn observe_successful_write(
    State(backup): State<backup_runner::BackupContext>,
    request: Request,
    next: Next,
) -> Response {
    let should_observe = !matches!(*request.method(), Method::GET | Method::HEAD)
        && request.uri().path().starts_with("/api/v1");
    let response = next.run(request).await;
    if should_observe && response.status().is_success() {
        // 响应已经成功形成，dirty 持久化也放到旁路，不能拖慢或改变记账结果。
        tokio::spawn(async move { backup.mark_dirty().await });
    }
    response
}

async fn backup_now(
    context: backup_runner::BackupContext,
    item: MenuItem<tauri::Wry>,
) -> anyhow::Result<()> {
    backup_runner::run_once(&context).await?;
    let status = context.status.lock().await;
    if status.running {
        item.set_text("立即备份（正在执行…）")?;
    } else if let Some(error) = &status.last_error {
        anyhow::bail!(error.clone());
    } else {
        item.set_text("立即备份（远端已确认）")?;
    }
    Ok(())
}

fn run_pending_restore(
    data_dir: &std::path::Path,
    config_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let marker = data_dir.join(PENDING_RESTORE);
    if !marker.exists() {
        return Ok(());
    }
    let config =
        backup_config::load(config_dir)?.context("检测到待恢复标记，但备份仓库尚未配置")?;
    let report = tauri::async_runtime::block_on(zhiyu_api::backup::restore(
        &config.repo_path,
        &data_dir.join("zhiyu.db"),
        &data_dir.join("quarantine"),
    ))?;
    backup_config::mark_restore_completed(config_dir)
        .context("数据库已经恢复，但无法解除同步恢复门禁")?;
    std::fs::remove_file(&marker).context("恢复成功，但无法删除 pending-restore 标记")?;
    tracing::info!(
        quarantine = %report.quarantined_to.display(),
        migrated = report.migrated,
        "离线恢复完成"
    );
    Ok(())
}

async fn enter(State(state): State<AppState>) -> Response {
    match zhiyu_api::auth::ensure_local_session(&state, LOCAL_USER_EMAIL).await {
        Ok(cookie) => (
            StatusCode::SEE_OTHER,
            [(header::LOCATION, HOME_PATH), (header::SET_COOKIE, &cookie)],
        )
            .into_response(),
        Err(error) => {
            tracing::error!(?error, "创建桌面本地会话失败");
            (StatusCode::INTERNAL_SERVER_ERROR, "无法创建本地会话").into_response()
        }
    }
}

pub fn run() {
    tracing_subscriber::fmt()
        .compact()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("zhiyu_api=info,zhiyu_desktop=info")),
        )
        .init();

    tauri::Builder::default()
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let config_dir = app.path().app_config_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            if let Err(error) = backup_config::load(&config_dir) {
                // 配置错误不应让应用无法启动，否则用户也无法通过菜单打开文件修正。
                tracing::warn!(?error, "备份配置尚不可用");
            }

            // 必须先离线恢复，再启动会持有数据库连接的内嵌 Axum server。
            run_pending_restore(&data_dir, &config_dir)?;
            let mut initial_status = backup_state::load(&data_dir)?;
            // 进程异常退出时可能持久化了 running=true，新进程启动后锁实际并未占用。
            initial_status.running = false;
            let backup_context = backup_runner::BackupContext::new(
                data_dir.clone(),
                config_dir.clone(),
                Arc::new(tokio::sync::Mutex::new(initial_status)),
            );
            backup_runner::start_automation(
                backup_context.clone(),
                AUTO_BACKUP_DEBOUNCE,
                AUTO_BACKUP_WATCHDOG,
            );

            let sync_ready =
                tauri::async_runtime::block_on(backup_runner::setup_state(&backup_context))
                    .sync_enabled;
            let backup_now_item =
                MenuItem::with_id(app, BACKUP_NOW, "立即备份", sync_ready, None::<&str>)?;
            let restore_item =
                MenuItem::with_id(app, RESTORE_FROM_BACKUP, "从备份恢复…", true, None::<&str>)?;
            let open_settings_item =
                MenuItem::with_id(app, OPEN_BACKUP_SETTINGS, "备份设置…", true, None::<&str>)?;
            let backup_menu = SubmenuBuilder::new(app, "备份")
                .items(&[&open_settings_item, &backup_now_item, &restore_item])
                .build()?;
            let menu = MenuBuilder::new(app).item(&backup_menu).build()?;
            app.set_menu(menu)?;
            app.manage(BackupMenuItems {
                backup_now: backup_now_item.clone(),
                restore: restore_item,
            });
            app.manage(backup_context.clone());

            let web_dist_dir = resolve_web_dist(app)?;
            let port = tauri::async_runtime::block_on(start_server(
                data_dir,
                web_dist_dir,
                backup_context,
                backup_now_item.clone(),
                app.handle().clone(),
            ))?;
            let public_base_url = format!("http://127.0.0.1:{port}");
            app.manage(DesktopServer {
                public_base_url: public_base_url.clone(),
            });
            let url = format!("{public_base_url}{ENTER_PATH}").parse()?;

            #[allow(unused_mut)]
            let mut window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("知余")
                .inner_size(1280.0, 840.0)
                .min_inner_size(960.0, 640.0)
                // 侧栏是纯白，页面衬底是近白暖白，而 macOS 默认标题栏自带一层灰调材质，
                // 横在窗口顶端就成了一道割裂的边。
                .background_color(tauri::window::Color(0xff, 0xff, 0xff, 0xff))
                .initialization_script(WINDOW_CHROME_SCRIPT);

            // 让内容铺满整个窗口、只留下浮在上面的红绿灯；标题文字交给页面自己表达。
            #[cfg(target_os = "macos")]
            {
                window = window
                    .title_bar_style(tauri::TitleBarStyle::Overlay)
                    .hidden_title(true);
            }

            window.build()?;
            Ok(())
        })
        .on_menu_event(|app, event| {
            let data_dir = match app.path().app_data_dir() {
                Ok(path) => path,
                Err(error) => {
                    tracing::error!(?error, "无法确定应用数据目录");
                    return;
                }
            };
            match event.id().as_ref() {
                BACKUP_NOW => {
                    let item = app.state::<BackupMenuItems>().backup_now.clone();
                    let context = app.state::<backup_runner::BackupContext>().inner().clone();
                    item.set_text("立即备份（正在生成快照…）").ok();
                    tauri::async_runtime::spawn(async move {
                        if let Err(error) = backup_now(context, item.clone()).await {
                            tracing::error!(?error, "手动备份失败");
                            item.set_text(format!("立即备份（失败：{error}）")).ok();
                        }
                    });
                }
                RESTORE_FROM_BACKUP => {
                    let marker = data_dir.join(PENDING_RESTORE);
                    let item = app.state::<BackupMenuItems>().restore.clone();
                    match std::fs::write(&marker, b"restart to restore\n") {
                        Ok(()) => item.set_text("从备份恢复…（请重启应用）").ok(),
                        Err(error) => {
                            tracing::error!(?error, "写入待恢复标记失败");
                            item.set_text(format!("从备份恢复…（失败：{error}）")).ok()
                        }
                    };
                }
                OPEN_BACKUP_SETTINGS => {
                    let server = app.state::<DesktopServer>();
                    let url = format!("{}{}", server.public_base_url, backup_http::BACKUP_PATH);
                    if let Err(error) = open_backup_settings(app, &url) {
                        tracing::error!(?error, "无法打开备份设置窗口");
                    }
                }
                _ => {}
            }
        })
        .run(tauri::generate_context!())
        .expect("启动知余桌面版失败");
}

pub(crate) fn open_backup_settings(app: &AppHandle, url: &str) -> anyhow::Result<()> {
    if let Some(window) = app.get_webview_window("backup-settings") {
        window.show()?;
        window.set_focus()?;
        return Ok(());
    }
    let url = url.parse()?;
    WebviewWindowBuilder::new(app, "backup-settings", WebviewUrl::External(url))
        .title("知余 · 备份设置")
        .inner_size(720.0, 640.0)
        .min_inner_size(620.0, 520.0)
        .build()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use axum::{Router, body::Body, routing::get};
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn only_successful_api_writes_mark_backup_dirty() {
        let root = tempfile::tempdir().unwrap();
        let status = Arc::new(tokio::sync::Mutex::new(
            backup_state::BackupStatus::default(),
        ));
        let context = backup_runner::BackupContext::new(
            root.path().join("data"),
            root.path().join("config"),
            status.clone(),
        );
        let app = Router::new()
            .route(
                "/api/v1/success",
                axum::routing::post(|| async { StatusCode::CREATED }),
            )
            .route(
                "/api/v1/failure",
                axum::routing::post(|| async { StatusCode::BAD_REQUEST }),
            )
            .route("/api/v1/read", get(|| async { StatusCode::OK }))
            .layer(axum::middleware::from_fn_with_state(
                context,
                observe_successful_write,
            ));

        let failed = app
            .clone()
            .oneshot(
                Request::post("/api/v1/failure")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(failed.status(), StatusCode::BAD_REQUEST);
        tokio::task::yield_now().await;
        assert!(!status.lock().await.dirty);

        app.oneshot(
            Request::post("/api/v1/success")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !status.lock().await.dirty {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}
