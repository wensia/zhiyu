pub mod accounts;
pub mod auth;
pub mod config;
pub mod db;
pub mod debts;
pub mod domain;
pub mod email;
pub mod error;
pub mod rate_limit;
pub mod transactions;

use std::sync::Arc;

use axum::{
    Json, Router,
    body::Body,
    extract::{Request, State},
    http::{Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
};
use libsql::Database;
use serde_json::json;
use tower_http::{
    catch_panic::CatchPanicLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use utoipa::{
    Modify, OpenApi,
    openapi::security::{ApiKey, ApiKeyValue, SecurityScheme},
};

use crate::{config::Config, email::EmailSender, rate_limit::RateLimiter};

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Database>,
    pub config: Arc<Config>,
    pub email: Arc<dyn EmailSender>,
    pub rate_limiter: RateLimiter,
}

impl AppState {
    pub async fn connection(&self) -> Result<libsql::Connection, crate::error::ApiError> {
        let conn = self.db.connect().map_err(crate::error::ApiError::from)?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")
            .await
            .map_err(crate::error::ApiError::from)?;
        Ok(conn)
    }
}

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "cookieAuth",
                SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new("zhiyu_session"))),
            );
        }
    }
}

#[derive(OpenApi)]
#[openapi(
    paths(
        auth::register, auth::verify_email, auth::resend_verification, auth::login,
        auth::logout, auth::me, auth::forgot_password, auth::reset_password,
        debts::list_debts, debts::get_debt, debts::create_debt, debts::update_debt,
        debts::archive_debt, debts::restore_debt, debts::delete_debt,
        debts::create_repayment, debts::create_debt_addition, debts::update_debt_addition,
        debts::update_repayment, debts::reverse_repayment, debts::list_counterparties,
        debts::create_counterparty, debts::update_counterparty, debts::dashboard_summary,
        accounts::list_ledger_accounts, accounts::create_ledger_account, accounts::update_ledger_account,
        accounts::archive_ledger_account, accounts::restore_ledger_account,
        transactions::list_transactions, transactions::create_transaction, transactions::update_transaction,
        transactions::delete_transaction, transactions::restore_transaction, transactions::transaction_summary,
        transactions::list_transaction_categories
    ),
    components(schemas(
        domain::UserView, domain::RegisterRequest, domain::LoginRequest, domain::EmailRequest,
        domain::TokenRequest, domain::ResetPasswordRequest, domain::MessageResponse,
        domain::DebtDirection, domain::DebtStatus, domain::DebtOriginKind, domain::CounterpartyView,
        domain::RepaymentEventView, domain::DebtAdditionEventView, domain::DebtView, domain::CounterpartyBrief,
        domain::DebtListResponse, domain::CreateDebtRequest, domain::UpdateDebtRequest,
        domain::VersionRequest, domain::CreateRepaymentRequest, domain::CreateDebtAdditionRequest,
        domain::UpdateDebtAdditionRequest, domain::UpdateRepaymentRequest, domain::ReverseRepaymentRequest,
        domain::CreateCounterpartyRequest, domain::UpdateCounterpartyRequest,
        domain::AccountType, domain::AccountNameSource, domain::LedgerAccountBrief, domain::LedgerAccountView,
        domain::CreateLedgerAccountRequest, domain::UpdateLedgerAccountRequest,
        domain::DashboardSummary, error::ErrorBody,
        domain::TransactionKind, domain::LedgerTransactionView, domain::TransactionListResponse,
        domain::CreateTransactionRequest, domain::UpdateTransactionRequest,
        domain::TransactionDaySummary, domain::TransactionCategorySummary, domain::TransactionMonthSummary
    )),
    modifiers(&SecurityAddon),
    tags((name = "知余", description = "个人债务管理 API"))
)]
pub struct ApiDoc;

pub fn app(state: AppState) -> Router {
    let api = Router::new()
        .route("/auth/register", post(auth::register))
        .route("/auth/verify-email", post(auth::verify_email))
        .route("/auth/resend-verification", post(auth::resend_verification))
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/me", get(auth::me))
        .route("/auth/forgot-password", post(auth::forgot_password))
        .route("/auth/reset-password", post(auth::reset_password))
        .route("/debts", get(debts::list_debts).post(debts::create_debt))
        .route(
            "/debts/{id}",
            get(debts::get_debt)
                .patch(debts::update_debt)
                .delete(debts::delete_debt),
        )
        .route("/debts/{id}/archive", post(debts::archive_debt))
        .route("/debts/{id}/restore", post(debts::restore_debt))
        .route("/debts/{id}/repayments", post(debts::create_repayment))
        .route("/debts/{id}/additions", post(debts::create_debt_addition))
        .route("/debt-additions/{id}", patch(debts::update_debt_addition))
        .route("/repayments/{id}", patch(debts::update_repayment))
        .route("/repayments/{id}/reversals", post(debts::reverse_repayment))
        .route(
            "/ledger-accounts",
            get(accounts::list_ledger_accounts).post(accounts::create_ledger_account),
        )
        .route(
            "/ledger-accounts/{id}",
            patch(accounts::update_ledger_account),
        )
        .route(
            "/ledger-accounts/{id}/archive",
            post(accounts::archive_ledger_account),
        )
        .route(
            "/ledger-accounts/{id}/restore",
            post(accounts::restore_ledger_account),
        )
        .route(
            "/counterparties",
            get(debts::list_counterparties).post(debts::create_counterparty),
        )
        .route("/counterparties/{id}", patch(debts::update_counterparty))
        .route("/dashboard/summary", get(debts::dashboard_summary))
        .route(
            "/transactions",
            get(transactions::list_transactions).post(transactions::create_transaction),
        )
        .route(
            "/transactions/summary",
            get(transactions::transaction_summary),
        )
        .route(
            "/transactions/categories",
            get(transactions::list_transaction_categories),
        )
        .route(
            "/transactions/{id}",
            patch(transactions::update_transaction).delete(transactions::delete_transaction),
        )
        .route(
            "/transactions/{id}/restore",
            post(transactions::restore_transaction),
        );

    let index = state.config.web_dist_dir.join("index.html");
    let static_files =
        ServeDir::new(state.config.web_dist_dir.clone()).fallback(ServeFile::new(index));

    Router::new()
        .route(
            "/health/live",
            get(|| async { Json(json!({ "status": "ok" })) }),
        )
        .route("/health/ready", get(readiness))
        .route(
            "/api/openapi.json",
            get(|| async { Json(ApiDoc::openapi()) }),
        )
        .nest("/api/v1", api)
        .fallback_service(static_files)
        .layer(middleware::from_fn_with_state(state.clone(), csrf_guard))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(TraceLayer::new_for_http())
        .layer(CatchPanicLayer::new())
        .with_state(state)
}

async fn readiness(State(state): State<AppState>) -> Response {
    match state.connection().await {
        Ok(conn) => match conn.query("SELECT 1", ()).await {
            Ok(_) => (StatusCode::OK, Json(json!({ "status": "ready" }))).into_response(),
            Err(error) => {
                tracing::error!(%error, "readiness query failed");
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({ "status": "unavailable" })),
                )
                    .into_response()
            }
        },
        Err(error) => {
            tracing::error!(?error, "readiness connection failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "status": "unavailable" })),
            )
                .into_response()
        }
    }
}

async fn csrf_guard(
    State(state): State<AppState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let unsafe_method = !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    let has_session_cookie = request
        .headers()
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains(state.config.cookie_name()));
    if unsafe_method && has_session_cookie {
        let origin_matches = request
            .headers()
            .get(header::ORIGIN)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|origin| origin.trim_end_matches('/') == state.config.public_base_url);
        if !origin_matches {
            return crate::error::ApiError::forbidden("请求来源校验失败").into_response();
        }
    }
    let session_token = request
        .headers()
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookies| {
            cookies
                .split(';')
                .filter_map(|part| part.trim().split_once('='))
                .find_map(|(name, value)| {
                    (name == state.config.cookie_name()).then(|| value.to_owned())
                })
        });
    let authenticated_token = if let Some(token) = session_token {
        match auth::authenticate_token(&state, &token).await {
            Ok(user) => {
                request.extensions_mut().insert(user);
                Some(token)
            }
            Err(error) => {
                tracing::debug!(?error, "session cookie was not valid");
                None
            }
        }
    } else {
        None
    };
    let mut response = next.run(request).await;
    if response.status().is_success()
        && !response.headers().contains_key(header::SET_COOKIE)
        && let Some(token) = authenticated_token
        && let Ok(value) = axum::http::HeaderValue::from_str(&auth::session_cookie_header(
            &state.config,
            &token,
            false,
        ))
    {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}
