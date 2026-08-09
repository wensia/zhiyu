use axum::{
    Json, Router,
    extract::{Request, State},
    http::{Method, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, menu::MenuItem};

use crate::{
    backup_config::{self, BackupConfig, ConfigValidation},
    backup_github::SetupState,
    backup_page,
    backup_runner::{self, BackupContext},
};

pub const BACKUP_PATH: &str = "/desktop/backup";

#[derive(Clone)]
pub struct BackupHttpState {
    pub backup: BackupContext,
    pub backup_now: MenuItem<tauri::Wry>,
    pub app_handle: AppHandle,
    pub backup_settings_url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigResponse {
    config: BackupConfig,
    validation: ConfigValidation,
    #[serde(flatten)]
    setup: SetupState,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateRepositoryRequest {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BindRepositoryRequest {
    name_with_owner: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutoBackupRequest {
    enabled: bool,
}

pub fn router(state: BackupHttpState, public_base_url: String) -> Router {
    Router::new()
        .route(BACKUP_PATH, get(page))
        .route("/desktop/backup/api/config", get(get_config))
        .route(
            "/desktop/backup/api/github/repositories",
            get(list_repositories),
        )
        .route("/desktop/backup/api/github/create", post(create_repository))
        .route("/desktop/backup/api/github/bind", post(bind_repository))
        .route("/desktop/backup/api/restore", post(request_restore))
        .route("/desktop/backup/api/auto-backup", put(set_auto_backup))
        .route("/desktop/backup/api/status", get(get_status))
        .route("/desktop/backup/api/run", post(run_backup))
        .route(
            "/desktop/backup/api/open-settings",
            post(open_backup_settings),
        )
        .with_state(state)
        .layer(middleware::from_fn_with_state(
            public_base_url,
            origin_guard,
        ))
}

async fn page() -> Html<&'static str> {
    Html(backup_page::HTML)
}

async fn get_config(State(state): State<BackupHttpState>) -> Response {
    match backup_config::read_for_display(&state.backup.config_dir) {
        Ok(config) => {
            let setup = backup_runner::setup_state(&state.backup).await;
            state.backup_now.set_enabled(setup.sync_enabled).ok();
            Json(ConfigResponse {
                validation: backup_config::validate(&config),
                config,
                setup,
            })
            .into_response()
        }
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("读取配置失败：{error:#}"),
        ),
    }
}

async fn list_repositories(State(state): State<BackupHttpState>) -> Response {
    match state.backup.github.list_repositories().await {
        Ok(repositories) => Json(json!({ "repositories": repositories })).into_response(),
        Err(error) => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            &format!("无法读取仓库：{error:#}"),
        ),
    }
}

async fn create_repository(
    State(state): State<BackupHttpState>,
    Json(request): Json<CreateRepositoryRequest>,
) -> Response {
    match state
        .backup
        .github
        .create_repository(
            &state.backup.data_dir,
            &state.backup.config_dir,
            request.name.trim(),
        )
        .await
    {
        Ok(setup) => {
            state.backup_now.set_enabled(setup.sync_enabled).ok();
            Json(setup).into_response()
        }
        Err(error) => api_error(StatusCode::BAD_REQUEST, &format!("创建失败：{error:#}")),
    }
}

async fn bind_repository(
    State(state): State<BackupHttpState>,
    Json(request): Json<BindRepositoryRequest>,
) -> Response {
    match state
        .backup
        .github
        .bind_repository(
            &state.backup.data_dir,
            &state.backup.config_dir,
            request.name_with_owner.trim(),
        )
        .await
    {
        Ok(setup) => {
            state.backup_now.set_enabled(setup.sync_enabled).ok();
            Json(setup).into_response()
        }
        Err(error) => api_error(StatusCode::BAD_REQUEST, &format!("绑定失败：{error:#}")),
    }
}

async fn request_restore(State(state): State<BackupHttpState>) -> Response {
    let setup = backup_runner::setup_state(&state.backup).await;
    if setup.repository_binding.state
        != crate::backup_github::RepositoryBindingState::RestoreRequired
    {
        return api_error(StatusCode::CONFLICT, "当前仓库不需要恢复");
    }
    let marker = state.backup.data_dir.join(crate::PENDING_RESTORE);
    match tokio::fs::write(&marker, b"restart to restore\n").await {
        Ok(()) => Json(json!({
            "restartRequired": true,
            "message": "恢复请求已保存。请完全退出并重新打开知余；恢复成功后才能启用同步。"
        }))
        .into_response(),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("无法保存恢复请求：{error}"),
        ),
    }
}

async fn set_auto_backup(
    State(state): State<BackupHttpState>,
    Json(request): Json<AutoBackupRequest>,
) -> Response {
    if request.enabled {
        let setup = backup_runner::setup_state(&state.backup).await;
        if !setup.sync_enabled {
            return api_error(
                StatusCode::CONFLICT,
                "GitHub CLI、登录态或仓库绑定尚未就绪，不能启用自动同步",
            );
        }
    }
    match backup_config::set_auto_backup(&state.backup.config_dir, request.enabled) {
        Ok(config) => Json(json!({ "autoBackup": config.auto_backup })).into_response(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, &format!("保存失败：{error:#}")),
    }
}

async fn get_status(State(state): State<BackupHttpState>) -> Response {
    Json(state.backup.status.lock().await.clone()).into_response()
}

async fn run_backup(State(state): State<BackupHttpState>) -> Response {
    let setup = backup_runner::setup_state(&state.backup).await;
    if !setup.sync_enabled {
        return api_error(
            StatusCode::CONFLICT,
            "云同步尚未就绪；请先安装并登录 GitHub CLI，再绑定私有仓库",
        );
    }
    let context = state.backup;
    tokio::spawn(async move {
        if let Err(error) = backup_runner::run_once(&context).await {
            tracing::warn!(?error, "界面触发的备份失败");
        }
    });
    (StatusCode::ACCEPTED, Json(json!({ "started": true }))).into_response()
}

async fn open_backup_settings(State(state): State<BackupHttpState>) -> Response {
    match crate::open_backup_settings(&state.app_handle, &state.backup_settings_url) {
        Ok(()) => Json(json!({ "opened": true })).into_response(),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("无法打开备份设置：{error:#}"),
        ),
    }
}

fn api_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

/// 桌面专属写端点不经过 API 自带的 csrf_guard，必须在这组路由上单独做严格同源校验。
async fn origin_guard(
    State(public_base_url): State<String>,
    request: Request,
    next: Next,
) -> Response {
    if request.method() != Method::GET {
        let matches = request
            .headers()
            .get(header::ORIGIN)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|origin| origin == public_base_url);
        if !matches {
            return api_error(StatusCode::FORBIDDEN, "请求来源校验失败");
        }
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, routing::post};
    use tower::ServiceExt;

    use super::*;

    fn protected_router() -> Router {
        Router::new()
            .route("/write", post(|| async { StatusCode::NO_CONTENT }))
            .layer(middleware::from_fn_with_state(
                "http://127.0.0.1:4567".to_owned(),
                origin_guard,
            ))
    }

    #[tokio::test]
    async fn correct_origin_is_allowed() {
        let response = protected_router()
            .oneshot(
                Request::post("/write")
                    .header(header::ORIGIN, "http://127.0.0.1:4567")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn wrong_or_missing_origin_is_forbidden() {
        for origin in [Some("http://evil.invalid"), None] {
            let mut request = Request::post("/write");
            if let Some(origin) = origin {
                request = request.header(header::ORIGIN, origin);
            }
            let response = protected_router()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
    }
}
