use axum::{
    Json, Router,
    extract::{Request, State},
    http::{Method, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;
use serde_json::json;

use crate::{
    backup_config::{self, BackupConfig, ConfigValidation},
    backup_page,
    backup_runner::{self, BackupContext},
};

pub const BACKUP_PATH: &str = "/desktop/backup";

#[derive(Clone)]
pub struct BackupHttpState {
    pub backup: BackupContext,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfigResponse {
    config: BackupConfig,
    validation: ConfigValidation,
}

pub fn router(state: BackupHttpState, public_base_url: String) -> Router {
    Router::new()
        .route(BACKUP_PATH, get(page))
        .route(
            "/desktop/backup/api/config",
            get(get_config).put(put_config),
        )
        .route("/desktop/backup/api/status", get(get_status))
        .route("/desktop/backup/api/run", post(run_backup))
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
        Ok(config) => Json(ConfigResponse {
            validation: backup_config::validate(&config),
            config,
        })
        .into_response(),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("读取配置失败：{error:#}"),
        ),
    }
}

async fn put_config(
    State(state): State<BackupHttpState>,
    Json(config): Json<BackupConfig>,
) -> Response {
    match backup_config::save(&state.backup.config_dir, &config) {
        Ok(validation) => Json(ConfigResponse { config, validation }).into_response(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, &format!("保存失败：{error:#}")),
    }
}

async fn get_status(State(state): State<BackupHttpState>) -> Response {
    Json(state.backup.status.lock().await.clone()).into_response()
}

async fn run_backup(State(state): State<BackupHttpState>) -> Response {
    let context = state.backup;
    tokio::spawn(async move {
        if let Err(error) = backup_runner::run_once(&context).await {
            tracing::warn!(?error, "界面触发的备份失败");
        }
    });
    (StatusCode::ACCEPTED, Json(json!({ "started": true }))).into_response()
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
