pub mod accounts;
pub mod auth;
pub mod backup;
pub mod categories;
pub mod categorize;
pub mod config;
pub mod dashboards;
pub mod db;
pub mod debts;
pub mod domain;
pub mod email;
pub mod error;
mod idempotency;
pub mod imports;
pub mod lifecycle;
pub mod plugins;
pub mod rate_limit;
pub mod self_transfer;
pub mod transactions;

use std::sync::Arc;

use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderValue, Method, StatusCode, Uri, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, patch, post, put},
};
use libsql::Database;
use serde_json::json;
use tower::ServiceBuilder;
use tower_http::{
    catch_panic::CatchPanicLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
    trace::TraceLayer,
};
use utoipa::{
    Modify, OpenApi,
    openapi::security::{ApiKey, ApiKeyValue, Http, HttpAuthScheme, SecurityScheme},
};

use crate::{config::Config, email::EmailSender, rate_limit::RateLimiter};

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Database>,
    pub config: Arc<Config>,
    pub email: Arc<dyn EmailSender>,
    pub rate_limiter: RateLimiter,
    pub backup_status: backup::BackupStatusStore,
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
            components.add_security_scheme(
                "bearerAuth",
                SecurityScheme::Http(Http::new(HttpAuthScheme::Bearer)),
            );
        }
    }
}

#[derive(OpenApi)]
#[openapi(
    paths(
        auth::register, auth::verify_email, auth::resend_verification, auth::login,
        auth::session_from_key, auth::create_handoff_ticket,
        auth::logout, auth::me, auth::forgot_password, auth::reset_password,
        debts::list_debts, debts::get_debt, debts::list_link_candidates, debts::create_debt, debts::update_debt,
        debts::archive_debt, debts::restore_debt, debts::delete_debt,
        debts::create_repayment, debts::create_debt_addition, debts::update_debt_addition,
        debts::update_repayment, debts::reverse_repayment, debts::list_counterparties,
        debts::create_counterparty, debts::update_counterparty, debts::dashboard_summary,
        accounts::list_ledger_accounts, accounts::create_ledger_account, accounts::update_ledger_account,
        accounts::archive_ledger_account, accounts::restore_ledger_account,
        transactions::list_transactions, transactions::create_transaction, transactions::update_transaction,
        transactions::delete_transaction, transactions::restore_transaction, transactions::transaction_summary,
        transactions::list_transaction_categories,
        categories::list_categories, categories::create_category, categories::update_category,
        categories::delete_category, categories::list_category_rules,
        categories::create_category_rule, categories::update_category_rule,
        categories::delete_category_rule, categories::recategorize, categories::revert_category_rule,
        backup::list_backups, backup::backup_status, backup::download_backup
        ,imports::list_imports, imports::get_import, imports::upload_import, imports::commit_import,
        imports::discard_import, imports::bind_import_account, imports::upsert_import_account_mapping,
        imports::duplicates::list_duplicate_suspicions, imports::duplicates::update_duplicate_suspicion,
        imports::duplicates::confirm_duplicate_suspicion,
        imports::duplicates::dismiss_duplicate_suspicion,
        imports::duplicates::revert_duplicate_suspicion,
        self_transfer::list_self_transfer_aliases, self_transfer::create_self_transfer_alias,
        self_transfer::delete_self_transfer_alias,
        plugins::list_plugins, plugins::update_plugin
        ,dashboards::list_dashboards, dashboards::create_dashboard,
        dashboards::create_default_dashboard, dashboards::update_dashboard,
        dashboards::delete_dashboard, dashboards::replace_dashboard_widgets,
        dashboards::list_widget_types, dashboards::statistics_aggregate
    ),
    components(schemas(
        domain::UserView, auth::SessionFromKeyResponse, auth::SessionCookieView,
        auth::HandoffTicketResponse,
        domain::RegisterRequest, domain::LoginRequest, domain::EmailRequest,
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
        domain::TransactionKind, domain::PnlScope, domain::TransactionLinkView, domain::TransactionLinkCandidate, domain::LedgerTransactionView, domain::TransactionListResponse,
        domain::CreateTransactionRequest, domain::UpdateTransactionRequest,
        domain::CategoryView, domain::CreateCategoryRequest, domain::UpdateCategoryRequest,
        domain::CategoryRuleConditionView, domain::CategoryRuleView,
        domain::CategoryRuleConditionInput, domain::CreateCategoryRuleRequest,
        domain::UpdateCategoryRuleRequest, domain::RecategorizeResponse,
        domain::RevertCategoryRuleResponse,
        domain::TransactionDaySummary, domain::TransactionCategorySummary, domain::TransactionMonthSummary,
        backup::BackupListItem, backup::BackupRuntimeStatus,
        imports::ImportListResponse, imports::ImportBatchListItem, imports::ImportDetailResponse,
        imports::ImportRecordView, imports::ImportSummary, imports::ImportSummaryItem,
        imports::UnknownIssue, imports::CommitImportResponse,
        imports::DiscardImportResponse, imports::CommitImportRequest,
        imports::BindImportAccountRequest, imports::BindImportAccountResponse
        ,imports::UpsertImportAccountMappingRequest, imports::ImportAccountMappingResponse,
        imports::ImportPayMethodSummary,
        imports::duplicates::DuplicateSuspicionListResponse,
        imports::duplicates::DuplicateSuspicionClusterView,
        imports::duplicates::DuplicateSuspicionView,
        imports::duplicates::DuplicateTransactionView,
        imports::duplicates::UpdateDuplicateSuspicionRequest,
        imports::duplicates::DuplicateSuspicionActionResponse,
        imports::duplicates::DuplicateActionTransactionView,
        imports::duplicates::TransactionEventView
        ,domain::SelfTransferAliasView, domain::CreateSelfTransferAliasRequest,
        domain::DeleteSelfTransferAliasRequest,
        plugins::PluginView, plugins::UpdatePluginRequest, plugins::UpdatePluginResponse,
        plugins::WidgetDefinition,
        dashboards::DashboardWidgetView, dashboards::DashboardView,
        dashboards::CreateDashboardRequest, dashboards::UpdateDashboardRequest,
        dashboards::DashboardWidgetInput, dashboards::PluginWidgetTypes,
        dashboards::WidgetTypesResponse, dashboards::AggregateItem
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
        .route("/auth/session-from-key", post(auth::session_from_key))
        .route("/auth/handoff-tickets", post(auth::create_handoff_ticket))
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
        .route(
            "/debts/{id}/link-candidates",
            get(debts::list_link_candidates),
        )
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
        .route("/plugins", get(plugins::list_plugins))
        .route("/plugins/{id}", patch(plugins::update_plugin))
        .route(
            "/dashboards",
            get(dashboards::list_dashboards).post(dashboards::create_dashboard),
        )
        .route(
            "/dashboards/default",
            post(dashboards::create_default_dashboard),
        )
        .route(
            "/dashboards/widget-types",
            get(dashboards::list_widget_types),
        )
        .route(
            "/dashboards/{id}",
            patch(dashboards::update_dashboard).delete(dashboards::delete_dashboard),
        )
        .route(
            "/dashboards/{id}/widgets",
            put(dashboards::replace_dashboard_widgets),
        )
        .route(
            "/statistics/aggregate",
            get(dashboards::statistics_aggregate),
        )
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
        )
        .route(
            "/categories",
            get(categories::list_categories).post(categories::create_category),
        )
        .route("/categories/recategorize", post(categories::recategorize))
        .route(
            "/categories/rules/{id}/revert",
            post(categories::revert_category_rule),
        )
        .route(
            "/categories/{id}",
            patch(categories::update_category).delete(categories::delete_category),
        )
        .route(
            "/category-rules",
            get(categories::list_category_rules).post(categories::create_category_rule),
        )
        .route(
            "/category-rules/{id}",
            patch(categories::update_category_rule).delete(categories::delete_category_rule),
        )
        .route("/backups", get(backup::list_backups))
        .route("/backups/status", get(backup::backup_status))
        .route("/backups/{id}", get(backup::download_backup))
        .route(
            "/self-transfer-aliases",
            get(self_transfer::list_self_transfer_aliases)
                .post(self_transfer::create_self_transfer_alias)
                .delete(self_transfer::delete_self_transfer_alias),
        )
        .route(
            "/imports",
            get(imports::list_imports)
                .post(imports::upload_import)
                .layer(DefaultBodyLimit::max(11 * 1024 * 1024)),
        )
        .route(
            "/imports/{id}",
            get(imports::get_import).delete(imports::discard_import),
        )
        .route(
            "/imports/mappings",
            axum::routing::post(imports::upsert_import_account_mapping),
        )
        .route(
            "/imports/{id}/commit",
            axum::routing::post(imports::commit_import),
        )
        .route(
            "/imports/{id}/account",
            axum::routing::post(imports::bind_import_account),
        )
        .route(
            "/duplicate-suspicions",
            get(imports::duplicates::list_duplicate_suspicions),
        )
        .route(
            "/duplicate-suspicions/{id}",
            patch(imports::duplicates::update_duplicate_suspicion),
        )
        .route(
            "/duplicate-suspicions/{id}/confirm",
            post(imports::duplicates::confirm_duplicate_suspicion),
        )
        .route(
            "/duplicate-suspicions/{id}/dismiss",
            post(imports::duplicates::dismiss_duplicate_suspicion),
        )
        .route(
            "/duplicate-suspicions/{id}/revert",
            post(imports::duplicates::revert_duplicate_suspicion),
        );

    // 静态资源必须显式声明缓存策略。tower-http 默认什么都不设，客户端于是退回
    // 启发式缓存——按 (now - last_modified) * 10% 自行推算过期时间。实测中 WKWebView
    // 因此长期持有旧 index.html，服务端换了新 bundle 也不会被发现，每次部署都要手动
    // 清缓存才能生效。
    //
    // 两类资源的策略相反，必须分开挂载：
    //   assets/*  文件名带内容 hash，内容变则文件名变，可以永久缓存
    //   其余      index.html 是新 bundle 的唯一入口，必须每次回源验证
    // no-cache 不是「不缓存」，而是「可缓存但每次需重新验证」，配合 last-modified
    // 仍能返回 304，不会浪费带宽。
    let index = state.config.web_dist_dir.join("index.html");
    let hashed_assets = ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        ))
        .service(ServeDir::new(state.config.web_dist_dir.join("assets")));
    let static_files = ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        ))
        .service(ServeDir::new(state.config.web_dist_dir.clone()).fallback(ServeFile::new(index)));

    Router::new()
        .route(
            "/health/live",
            get(|| async { Json(json!({ "status": "ok" })) }),
        )
        .route("/health/ready", get(readiness))
        .route("/desktop/handoff", post(auth::desktop_handoff))
        .route(
            "/desktop/handoff/{ticket}",
            get(auth::desktop_handoff_by_path),
        )
        .route(
            "/api/openapi.json",
            get(|| async { Json(ApiDoc::openapi()) }),
        )
        .nest("/api/v1", api)
        .nest(
            "/api",
            Router::new()
                .fallback(|| async { crate::error::ApiError::not_found("API 端点不存在") }),
        )
        .nest_service("/assets", hashed_assets)
        .fallback_service(static_files)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            plugin_access_guard,
        ))
        .layer(middleware::from_fn_with_state(state.clone(), csrf_guard))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(TraceLayer::new_for_http())
        .layer(CatchPanicLayer::new())
        .with_state(state)
}

async fn plugin_access_guard(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let Some(plugin) = plugins::plugin_for_api_path(request.uri().path()) else {
        return next.run(request).await;
    };
    let Some(context) = request.extensions().get::<auth::AuthContext>() else {
        return next.run(request).await;
    };
    let conn = match state.connection().await {
        Ok(conn) => conn,
        Err(error) => return error.into_response(),
    };
    match plugins::is_enabled(&conn, &context.user.id, plugin.id).await {
        Ok(true) => next.run(request).await,
        Ok(false) => crate::error::ApiError::conflict(
            "plugin_disabled",
            format!("{}插件已关闭", plugin.name),
        )
        .into_response(),
        Err(error) => error.into_response(),
    }
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
    let path = request.uri().path();
    if path == "/desktop/handoff" || path.starts_with("/desktop/handoff/") {
        // 该请求来自 Tauri 本地跳板页，Origin 天然不等于 public_base_url。
        // 60 秒、不可预测且单次消费的票据就是此路径的防伪凭证。这里也必须完全
        // 忽略旧 session cookie，避免无效票据被 cookie 认证或续期响应掩盖。
        return next.run(request).await;
    }
    let bearer_only = matches!(
        path,
        "/api/v1/auth/session-from-key" | "/api/v1/auth/handoff-tickets"
    );
    let session_token = (!bearer_only)
        .then(|| auth::cookie_value(request.headers(), state.config.cookie_name()))
        .flatten();
    let mut auth_context = None;
    let authenticated_token = if let Some(token) = session_token {
        match auth::authenticate_session_token(&state, &token).await {
            Ok(user) => {
                auth_context = Some(auth::AuthContext {
                    user,
                    mechanism: auth::AuthMechanism::Session,
                });
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
    if auth_context.is_none()
        && let Some(token) = auth::bearer_token(request.headers())
    {
        match auth::authenticate_api_key(&state, token).await {
            Ok(user) => {
                auth_context = Some(auth::AuthContext {
                    user,
                    mechanism: auth::AuthMechanism::ApiKey,
                });
            }
            Err(error) => {
                tracing::debug!(?error, "bearer API key was not valid");
            }
        }
    }
    let unsafe_method = !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    let uses_session = auth_context
        .as_ref()
        .is_some_and(|context| context.mechanism == auth::AuthMechanism::Session);
    if unsafe_method && uses_session {
        let request_origin = request
            .headers()
            .get(header::ORIGIN)
            .and_then(|value| value.to_str().ok());
        if !request_origin
            .is_some_and(|origin| origins_match(origin, &state.config.public_base_url))
        {
            let request_origin = request_origin.unwrap_or("（缺失或非法）");
            return crate::error::ApiError::forbidden(format!(
                "请求来源 {request_origin} 与服务端配置的 {} 不一致",
                state.config.public_base_url
            ))
            .into_response();
        }
    }
    if let Some(context) = auth_context {
        request.extensions_mut().insert(context);
    }
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

fn origins_match(left: &str, right: &str) -> bool {
    let Some((left_scheme, left_host, left_port)) = parse_origin(left) else {
        return false;
    };
    let Some((right_scheme, right_host, right_port)) = parse_origin(right) else {
        return false;
    };

    left_scheme == right_scheme
        && left_port == right_port
        && (left_host == right_host
            || (is_loopback_host(&left_host) && is_loopback_host(&right_host)))
}

fn parse_origin(value: &str) -> Option<(String, String, u16)> {
    let uri = value.parse::<Uri>().ok()?;
    let scheme = uri.scheme_str()?.to_ascii_lowercase();
    let authority = uri.authority()?;
    if authority.as_str().contains('@')
        || uri
            .path_and_query()
            .is_some_and(|path_and_query| path_and_query.as_str() != "/")
    {
        return None;
    }

    let port = authority.port_u16().or(match scheme.as_str() {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    })?;
    let host = authority
        .host()
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or_else(|| authority.host())
        .to_ascii_lowercase();
    Some((scheme, host, port))
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}
