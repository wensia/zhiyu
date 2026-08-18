use std::{future::Future, pin::Pin};

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use libsql::{Connection, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    AppState,
    auth::AuthUser,
    categorize, debts,
    error::ApiError,
    idempotency::{idempotency_key, replay_idempotency, request_hash, store_idempotency},
    lifecycle::{DeletionHandler, SuggestionProvider},
};

#[derive(Debug, Clone, Copy)]
pub struct PluginDefinition {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub owns_transactions: bool,
    pub route_prefixes: &'static [&'static str],
    pub widgets: &'static [WidgetDefinition],
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WidgetDefinition {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub default_w: i64,
    pub default_h: i64,
    pub min_w: i64,
    pub min_h: i64,
}

const DEBT_WIDGETS: &[WidgetDefinition] = &[WidgetDefinition {
    id: "overview",
    name: "谁欠我多少",
    description: "汇总当前债务余额。",
    default_w: 4,
    default_h: 3,
    min_w: 3,
    min_h: 2,
}];

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginView {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub enabled: bool,
    pub owns_transactions: bool,
    pub route_prefixes: &'static [&'static str],
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct UpdatePluginRequest {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePluginResponse {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub enabled: bool,
    pub owns_transactions: bool,
    pub route_prefixes: &'static [&'static str],
    pub reconciled: u64,
}

pub const BUILT_IN_PLUGINS: &[PluginDefinition] = &[
    PluginDefinition {
        id: "debts",
        name: "债务",
        description: "记录借入、借出及还款进度。",
        owns_transactions: true,
        route_prefixes: &[
            "/api/v1/debts",
            "/api/v1/debt-additions",
            "/api/v1/repayments",
            "/api/v1/counterparties",
            "/api/v1/dashboard/summary",
        ],
        widgets: DEBT_WIDGETS,
    },
    PluginDefinition {
        id: "bill-imports",
        name: "账单导入",
        description: "从受支持的账单来源导入流水。",
        owns_transactions: false,
        route_prefixes: &["/api/v1/imports", "/api/v1/duplicate-suspicions"],
        widgets: &[],
    },
    PluginDefinition {
        id: "auto-categorize",
        name: "自动分类",
        description: "按规则为流水自动匹配分类。",
        owns_transactions: false,
        route_prefixes: &[
            "/api/v1/category-rules",
            "/api/v1/categories/recategorize",
            "/api/v1/categories/rules",
        ],
        widgets: &[],
    },
];

#[derive(Clone, Copy)]
struct SuggestionProviderRegistration {
    plugin_id: &'static str,
    provider: SuggestionProvider,
}

#[derive(Clone, Copy)]
struct DeletionHandlerRegistration {
    plugin_id: &'static str,
    handler: DeletionHandler,
}

type SelfCheckFuture<'a> = Pin<Box<dyn Future<Output = Result<u64, ApiError>> + Send + 'a>>;
type SelfCheck = for<'a> fn(&'a Connection, &'a str) -> SelfCheckFuture<'a>;

#[derive(Clone, Copy)]
struct SelfCheckRegistration {
    plugin_id: &'static str,
    run: SelfCheck,
}

const SUGGESTION_PROVIDERS: &[SuggestionProviderRegistration] = &[SuggestionProviderRegistration {
    plugin_id: "auto-categorize",
    provider: categorize::suggest_categories,
}];
const DELETION_HANDLERS: &[DeletionHandlerRegistration] = &[DeletionHandlerRegistration {
    plugin_id: "debts",
    handler: DeletionHandler {
        prepare: debts::prepare_deleted_transactions,
        handle: debts::handle_deleted_transactions,
    },
}];
const SELF_CHECKS: &[SelfCheckRegistration] = &[SelfCheckRegistration {
    plugin_id: "debts",
    run: debts_self_check,
}];

fn debts_self_check<'a>(conn: &'a Connection, user_id: &'a str) -> SelfCheckFuture<'a> {
    Box::pin(debts::reconcile_transaction_links(conn, user_id))
}

fn definition(id: &str) -> Option<&'static PluginDefinition> {
    BUILT_IN_PLUGINS.iter().find(|plugin| plugin.id == id)
}

fn matches_prefix(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|remainder| remainder.starts_with('/'))
}

pub fn plugin_for_api_path(path: &str) -> Option<&'static PluginDefinition> {
    BUILT_IN_PLUGINS.iter().find(|plugin| {
        plugin
            .route_prefixes
            .iter()
            .any(|prefix| matches_prefix(path, prefix))
    })
}

pub async fn is_enabled(
    conn: &Connection,
    user_id: &str,
    plugin_id: &str,
) -> Result<bool, ApiError> {
    let mut rows = conn
        .query(
            "SELECT enabled FROM plugin_settings WHERE user_id=?1 AND plugin_id=?2",
            params![user_id, plugin_id],
        )
        .await?;
    Ok(rows
        .next()
        .await?
        .map(|row| row.get::<i64>(0))
        .transpose()?
        .is_none_or(|enabled| enabled != 0))
}

async fn is_enabled_in_transaction(
    tx: &Transaction,
    user_id: &str,
    plugin_id: &str,
) -> Result<bool, ApiError> {
    let mut rows = tx
        .query(
            "SELECT enabled FROM plugin_settings WHERE user_id=?1 AND plugin_id=?2",
            params![user_id, plugin_id],
        )
        .await?;
    Ok(rows
        .next()
        .await?
        .map(|row| row.get::<i64>(0))
        .transpose()?
        .is_none_or(|enabled| enabled != 0))
}

pub async fn suggestion_providers(
    tx: &Transaction,
    user_id: &str,
) -> Result<Vec<SuggestionProvider>, ApiError> {
    let mut providers = Vec::new();
    for registration in SUGGESTION_PROVIDERS {
        if is_enabled_in_transaction(tx, user_id, registration.plugin_id).await? {
            providers.push(registration.provider);
        }
    }
    Ok(providers)
}

pub async fn deletion_handlers(
    tx: &Transaction,
    user_id: &str,
) -> Result<Vec<DeletionHandler>, ApiError> {
    let mut handlers = Vec::new();
    for registration in DELETION_HANDLERS {
        if is_enabled_in_transaction(tx, user_id, registration.plugin_id).await? {
            handlers.push(registration.handler);
        }
    }
    Ok(handlers)
}

pub fn owning_plugin_for_created_by(created_by: &str) -> Option<&'static PluginDefinition> {
    let id = created_by.strip_prefix("plugin:")?;
    BUILT_IN_PLUGINS
        .iter()
        .find(|plugin| plugin.id == id && plugin.owns_transactions)
}

async fn plugin_view(
    conn: &Connection,
    user_id: &str,
    plugin: &'static PluginDefinition,
) -> Result<PluginView, ApiError> {
    Ok(PluginView {
        id: plugin.id,
        name: plugin.name,
        description: plugin.description,
        enabled: is_enabled(conn, user_id, plugin.id).await?,
        owns_transactions: plugin.owns_transactions,
        route_prefixes: plugin.route_prefixes,
    })
}

async fn run_self_check(
    conn: &Connection,
    user_id: &str,
    plugin_id: &str,
) -> Result<u64, ApiError> {
    match SELF_CHECKS
        .iter()
        .find(|registration| registration.plugin_id == plugin_id)
    {
        Some(registration) => (registration.run)(conn, user_id).await,
        None => Ok(0),
    }
}

pub async fn run_startup_self_checks(conn: &Connection) -> Result<u64, ApiError> {
    let mut rows = conn.query("SELECT id FROM users ORDER BY id", ()).await?;
    let mut user_ids = Vec::new();
    while let Some(row) = rows.next().await? {
        user_ids.push(row.get::<String>(0)?);
    }
    drop(rows);

    let mut reconciled = 0;
    for user_id in user_ids {
        for registration in SELF_CHECKS {
            if is_enabled(conn, &user_id, registration.plugin_id).await? {
                reconciled += (registration.run)(conn, &user_id).await?;
            }
        }
    }
    Ok(reconciled)
}

#[utoipa::path(get, path = "/api/v1/plugins", responses((status = 200, body = [PluginView])), security(("cookieAuth" = [])))]
pub async fn list_plugins(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<PluginView>>, ApiError> {
    let conn = state.connection().await?;
    let mut plugins = Vec::with_capacity(BUILT_IN_PLUGINS.len());
    for plugin in BUILT_IN_PLUGINS {
        plugins.push(plugin_view(&conn, &user.id, plugin).await?);
    }
    Ok(Json(plugins))
}

#[utoipa::path(patch, path = "/api/v1/plugins/{id}", params(("id" = String, Path)), request_body = UpdatePluginRequest, responses((status = 200, body = UpdatePluginResponse), (status = 404, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn update_plugin(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<UpdatePluginRequest>,
) -> Result<Response, ApiError> {
    let plugin = definition(&id).ok_or_else(|| ApiError::not_found("插件不存在"))?;
    let key = idempotency_key(&headers)?;
    let hash = request_hash(&input)?;
    let operation = format!("update_plugin:{id}");
    let conn = state.connection().await?;
    if let Some(response) = replay_idempotency(&conn, &user.id, &key, &operation, &hash).await? {
        return Ok(response);
    }

    let was_enabled = is_enabled(&conn, &user.id, plugin.id).await?;
    let reconciled = if input.enabled && !was_enabled {
        run_self_check(&conn, &user.id, plugin.id).await?
    } else {
        0
    };

    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    tx.execute(
        "INSERT INTO plugin_settings(user_id,plugin_id,enabled,updated_at) VALUES (?1,?2,?3,?4) ON CONFLICT(user_id,plugin_id) DO UPDATE SET enabled=excluded.enabled,updated_at=excluded.updated_at",
        params![
            user.id.clone(),
            plugin.id,
            i64::from(input.enabled),
            Utc::now().to_rfc3339()
        ],
    )
    .await?;
    let body = UpdatePluginResponse {
        id: plugin.id,
        name: plugin.name,
        description: plugin.description,
        enabled: input.enabled,
        owns_transactions: plugin.owns_transactions,
        route_prefixes: plugin.route_prefixes,
        reconciled,
    };
    store_idempotency(
        &tx,
        &user.id,
        &key,
        &operation,
        &hash,
        StatusCode::OK,
        &body,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(body).into_response())
}
