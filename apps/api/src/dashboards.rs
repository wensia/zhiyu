use std::collections::HashSet;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{NaiveDate, Utc};
use libsql::{Connection, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{
    AppState,
    auth::AuthUser,
    error::ApiError,
    idempotency::{idempotency_key, replay_idempotency, request_hash, store_idempotency},
    plugins::{self, BUILT_IN_PLUGINS, WidgetDefinition},
};

pub const CORE_WIDGETS: &[WidgetDefinition] = &[
    WidgetDefinition {
        id: "income-expense-trend",
        name: "收支趋势",
        description: "按日展示月度收入与支出趋势。",
        default_w: 8,
        default_h: 4,
        min_w: 4,
        min_h: 3,
    },
    WidgetDefinition {
        id: "category-share",
        name: "分类占比",
        description: "按分类展示收入与支出占比。",
        default_w: 4,
        default_h: 4,
        min_w: 3,
        min_h: 3,
    },
    WidgetDefinition {
        id: "account-balances",
        name: "账户余额",
        description: "展示当前账户余额。",
        default_w: 4,
        default_h: 3,
        min_w: 3,
        min_h: 2,
    },
    WidgetDefinition {
        id: "month-compare",
        name: "月度对比",
        description: "对比近六个月收入与支出。",
        default_w: 8,
        default_h: 3,
        min_w: 4,
        min_h: 2,
    },
];

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DashboardWidgetView {
    pub id: String,
    pub widget_type: String,
    pub plugin_id: Option<String>,
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    pub config: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DashboardView {
    pub id: String,
    pub name: String,
    pub position: i64,
    pub widgets: Vec<DashboardWidgetView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CreateDashboardRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct UpdateDashboardRequest {
    pub name: Option<String>,
    pub position: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DashboardWidgetInput {
    #[serde(default)]
    pub id: Option<String>,
    pub widget_type: String,
    pub plugin_id: Option<String>,
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    #[serde(default = "empty_config")]
    pub config: Value,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginWidgetTypes {
    pub plugin_id: &'static str,
    pub enabled: bool,
    pub widgets: Vec<WidgetDefinition>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WidgetTypesResponse {
    pub core: Vec<WidgetDefinition>,
    pub plugins: Vec<PluginWidgetTypes>,
}

#[derive(Debug, Clone, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct AggregateQuery {
    pub from: String,
    pub to: String,
    pub group_by: String,
    pub account_id: Option<String>,
    pub category_id: Option<String>,
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AggregateItem {
    pub key: String,
    pub label: String,
    pub income_cents: i64,
    pub expense_cents: i64,
    pub count: i64,
}

fn empty_config() -> Value {
    Value::Object(Default::default())
}

fn validate_name(name: &str) -> Result<String, ApiError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return Err(ApiError::validation("面板名称须为 1–60 个字符"));
    }
    Ok(name.to_owned())
}

fn validate_widget(input: &DashboardWidgetInput) -> Result<(), ApiError> {
    if input.x < 0
        || input.y < 0
        || !(1..=12).contains(&input.w)
        || input.h < 1
        || input.x + input.w > 12
    {
        return Err(ApiError::validation("组件超出 12 列网格范围"));
    }
    if let Some(id) = input.id.as_deref()
        && (id.trim().is_empty() || id.len() > 128)
    {
        return Err(ApiError::validation("组件 ID 无效"));
    }
    if let Some(plugin_id) = input.plugin_id.as_deref()
        && !BUILT_IN_PLUGINS.iter().any(|plugin| plugin.id == plugin_id)
    {
        return Err(ApiError::validation("组件所属插件不存在"));
    }

    let parts = input.widget_type.split(':').collect::<Vec<_>>();
    match parts.as_slice() {
        ["core", widget_id]
            if input.plugin_id.is_none()
                && CORE_WIDGETS.iter().any(|widget| widget.id == *widget_id) =>
        {
            Ok(())
        }
        ["plugin", plugin_id, widget_id]
            if input.plugin_id.as_deref() == Some(*plugin_id)
                && BUILT_IN_PLUGINS.iter().any(|plugin| {
                    plugin.id == *plugin_id
                        && plugin.widgets.iter().any(|widget| widget.id == *widget_id)
                }) =>
        {
            Ok(())
        }
        _ => Err(ApiError::validation("widgetType 或 pluginId 无效")),
    }
}

async fn load_dashboard(
    conn: &Connection,
    user_id: &str,
    dashboard_id: &str,
) -> Result<DashboardView, ApiError> {
    let mut rows = conn
        .query(
            "SELECT id,name,position FROM dashboards WHERE id=?1 AND user_id=?2",
            params![dashboard_id, user_id],
        )
        .await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| ApiError::not_found("面板不存在"))?;
    let id = row.get::<String>(0)?;
    let name = row.get::<String>(1)?;
    let position = row.get::<i64>(2)?;
    drop(rows);

    let mut widget_rows = conn
        .query(
            "SELECT id,widget_type,plugin_id,x,y,w,h,config FROM dashboard_widgets WHERE dashboard_id=?1 AND user_id=?2 ORDER BY position,id",
            params![dashboard_id, user_id],
        )
        .await?;
    let mut widgets = Vec::new();
    while let Some(row) = widget_rows.next().await? {
        let config = serde_json::from_str::<Value>(&row.get::<String>(7)?)
            .map_err(|_| ApiError::internal("dashboard widget config is invalid"))?;
        widgets.push(DashboardWidgetView {
            id: row.get(0)?,
            widget_type: row.get(1)?,
            plugin_id: row.get(2)?,
            x: row.get(3)?,
            y: row.get(4)?,
            w: row.get(5)?,
            h: row.get(6)?,
            config,
        });
    }
    Ok(DashboardView {
        id,
        name,
        position,
        widgets,
    })
}

async fn ordered_dashboard_ids(conn: &Connection, user_id: &str) -> Result<Vec<String>, ApiError> {
    let mut rows = conn
        .query(
            "SELECT id FROM dashboards WHERE user_id=?1 ORDER BY position,id",
            [user_id],
        )
        .await?;
    let mut ids = Vec::new();
    while let Some(row) = rows.next().await? {
        ids.push(row.get(0)?);
    }
    Ok(ids)
}

async fn rewrite_dashboard_positions(
    tx: &Transaction,
    user_id: &str,
    ids: &[String],
    now: &str,
) -> Result<(), ApiError> {
    tx.execute(
        "UPDATE dashboards SET position=position+1000000,updated_at=?1 WHERE user_id=?2",
        params![now, user_id],
    )
    .await?;
    for (position, id) in ids.iter().enumerate() {
        let changed = tx
            .execute(
                "UPDATE dashboards SET position=?1,updated_at=?2 WHERE id=?3 AND user_id=?4",
                params![position as i64, now, id.clone(), user_id],
            )
            .await?;
        if changed != 1 {
            return Err(ApiError::conflict(
                "version_conflict",
                "面板已变化，请刷新后重试",
            ));
        }
    }
    Ok(())
}

async fn insert_widget(
    tx: &Transaction,
    user_id: &str,
    dashboard_id: &str,
    position: i64,
    input: &DashboardWidgetInput,
    now: &str,
) -> Result<(), ApiError> {
    let id = input
        .id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| Uuid::now_v7().to_string());
    tx.execute(
        "INSERT INTO dashboard_widgets(id,dashboard_id,user_id,widget_type,plugin_id,x,y,w,h,config,position,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?12)",
        params![
            id,
            dashboard_id,
            user_id,
            input.widget_type.clone(),
            input.plugin_id.clone(),
            input.x,
            input.y,
            input.w,
            input.h,
            serde_json::to_string(&input.config).map_err(ApiError::internal)?,
            position,
            now
        ],
    )
    .await
    .map_err(|error| {
        if error.to_string().contains("UNIQUE constraint failed") {
            ApiError::validation("组件 ID 重复")
        } else {
            ApiError::from(error)
        }
    })?;
    Ok(())
}

#[utoipa::path(get, path = "/api/v1/dashboards", responses((status = 200, body = [DashboardView])), security(("cookieAuth" = [])))]
pub async fn list_dashboards(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<DashboardView>>, ApiError> {
    let conn = state.connection().await?;
    let ids = ordered_dashboard_ids(&conn, &user.id).await?;
    let mut dashboards = Vec::with_capacity(ids.len());
    for id in ids {
        dashboards.push(load_dashboard(&conn, &user.id, &id).await?);
    }
    Ok(Json(dashboards))
}

#[utoipa::path(post, path = "/api/v1/dashboards", request_body = CreateDashboardRequest, responses((status = 201, body = DashboardView), (status = 422, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn create_dashboard(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Json(input): Json<CreateDashboardRequest>,
) -> Result<Response, ApiError> {
    let name = validate_name(&input.name)?;
    let key = idempotency_key(&headers)?;
    let hash = request_hash(&input)?;
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) =
        replay_idempotency(&tx, &user.id, &key, "create_dashboard", &hash).await?
    {
        return Ok(response);
    }
    let position = ordered_dashboard_ids(&tx, &user.id).await?.len() as i64;
    let id = Uuid::now_v7().to_string();
    let now = Utc::now().to_rfc3339();
    tx.execute(
        "INSERT INTO dashboards(id,user_id,name,position,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?5)",
        params![id.clone(), user.id.clone(), name, position, now],
    )
    .await?;
    let body = load_dashboard(&tx, &user.id, &id).await?;
    store_idempotency(
        &tx,
        &user.id,
        &key,
        "create_dashboard",
        &hash,
        StatusCode::CREATED,
        &body,
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(body)).into_response())
}

#[utoipa::path(post, path = "/api/v1/dashboards/default", responses((status = 200, body = DashboardView), (status = 201, body = DashboardView)), security(("cookieAuth" = [])))]
pub async fn create_default_dashboard(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let key = idempotency_key(&headers)?;
    let hash = request_hash(&Value::Null)?;
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) =
        replay_idempotency(&tx, &user.id, &key, "create_default_dashboard", &hash).await?
    {
        return Ok(response);
    }

    let ids = ordered_dashboard_ids(&tx, &user.id).await?;
    let (status, body) = if let Some(id) = ids.first() {
        (StatusCode::OK, load_dashboard(&tx, &user.id, id).await?)
    } else {
        let id = Uuid::now_v7().to_string();
        let now = Utc::now().to_rfc3339();
        tx.execute(
            "INSERT INTO dashboards(id,user_id,name,position,created_at,updated_at) VALUES (?1,?2,'月度',0,?3,?3)",
            params![id.clone(), user.id.clone(), now.clone()],
        )
        .await?;
        let defaults = [
            ("core:income-expense-trend", 0, 0, 8, 4),
            ("core:category-share", 8, 0, 4, 4),
            ("core:account-balances", 0, 4, 4, 3),
            ("core:month-compare", 4, 4, 8, 3),
        ];
        for (position, (widget_type, x, y, w, h)) in defaults.into_iter().enumerate() {
            let input = DashboardWidgetInput {
                id: None,
                widget_type: widget_type.to_owned(),
                plugin_id: None,
                x,
                y,
                w,
                h,
                config: empty_config(),
            };
            insert_widget(&tx, &user.id, &id, position as i64, &input, &now).await?;
        }
        (
            StatusCode::CREATED,
            load_dashboard(&tx, &user.id, &id).await?,
        )
    };
    store_idempotency(
        &tx,
        &user.id,
        &key,
        "create_default_dashboard",
        &hash,
        status,
        &body,
    )
    .await?;
    tx.commit().await?;
    Ok((status, Json(body)).into_response())
}

#[utoipa::path(patch, path = "/api/v1/dashboards/{id}", params(("id" = String, Path)), request_body = UpdateDashboardRequest, responses((status = 200, body = DashboardView), (status = 404, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn update_dashboard(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<UpdateDashboardRequest>,
) -> Result<Response, ApiError> {
    if input.name.is_none() && input.position.is_none() {
        return Err(ApiError::validation("至少提供一个要修改的字段"));
    }
    let name = input.name.as_deref().map(validate_name).transpose()?;
    if input.position.is_some_and(|position| position < 0) {
        return Err(ApiError::validation("面板位置无效"));
    }
    let key = idempotency_key(&headers)?;
    let hash = request_hash(&input)?;
    let operation = format!("update_dashboard:{id}");
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) = replay_idempotency(&tx, &user.id, &key, &operation, &hash).await? {
        return Ok(response);
    }
    load_dashboard(&tx, &user.id, &id).await?;
    let now = Utc::now().to_rfc3339();
    if let Some(name) = name {
        let changed = tx
            .execute(
                "UPDATE dashboards SET name=?1,updated_at=?2 WHERE id=?3 AND user_id=?4",
                params![name, now.clone(), id.clone(), user.id.clone()],
            )
            .await?;
        if changed != 1 {
            return Err(ApiError::conflict(
                "version_conflict",
                "面板已变化，请刷新后重试",
            ));
        }
    }
    if let Some(position) = input.position {
        let mut ids = ordered_dashboard_ids(&tx, &user.id).await?;
        if position as usize >= ids.len() {
            return Err(ApiError::validation("面板位置超出范围"));
        }
        let current = ids
            .iter()
            .position(|candidate| candidate == &id)
            .ok_or_else(|| ApiError::conflict("version_conflict", "面板已变化，请刷新后重试"))?;
        let moved = ids.remove(current);
        ids.insert(position as usize, moved);
        rewrite_dashboard_positions(&tx, &user.id, &ids, &now).await?;
    }
    let body = load_dashboard(&tx, &user.id, &id).await?;
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

#[utoipa::path(delete, path = "/api/v1/dashboards/{id}", params(("id" = String, Path)), responses((status = 204), (status = 404, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn delete_dashboard(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let key = idempotency_key(&headers)?;
    let hash = request_hash(&Value::Null)?;
    let operation = format!("delete_dashboard:{id}");
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) = replay_idempotency(&tx, &user.id, &key, &operation, &hash).await? {
        return Ok(response);
    }
    let ids = ordered_dashboard_ids(&tx, &user.id).await?;
    if !ids.iter().any(|candidate| candidate == &id) {
        return Err(ApiError::not_found("面板不存在"));
    }
    if ids.len() == 1 {
        return Err(ApiError::conflict("last_dashboard", "至少保留一个统计面板"));
    }
    let changed = tx
        .execute(
            "DELETE FROM dashboards WHERE id=?1 AND user_id=?2",
            params![id, user.id.clone()],
        )
        .await?;
    if changed != 1 {
        return Err(ApiError::conflict(
            "version_conflict",
            "面板已变化，请刷新后重试",
        ));
    }
    let remaining = ordered_dashboard_ids(&tx, &user.id).await?;
    rewrite_dashboard_positions(&tx, &user.id, &remaining, &Utc::now().to_rfc3339()).await?;
    let empty = Value::Null;
    store_idempotency(
        &tx,
        &user.id,
        &key,
        &operation,
        &hash,
        StatusCode::NO_CONTENT,
        &empty,
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[utoipa::path(put, path = "/api/v1/dashboards/{id}/widgets", params(("id" = String, Path)), request_body = [DashboardWidgetInput], responses((status = 200, body = DashboardView), (status = 404, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn replace_dashboard_widgets(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(inputs): Json<Vec<DashboardWidgetInput>>,
) -> Result<Response, ApiError> {
    let mut ids = HashSet::new();
    for input in &inputs {
        validate_widget(input)?;
        if let Some(widget_id) = input.id.as_deref()
            && !ids.insert(widget_id.trim().to_owned())
        {
            return Err(ApiError::validation("组件 ID 重复"));
        }
    }
    let key = idempotency_key(&headers)?;
    let hash = request_hash(&inputs)?;
    let operation = format!("replace_dashboard_widgets:{id}");
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) = replay_idempotency(&tx, &user.id, &key, &operation, &hash).await? {
        return Ok(response);
    }
    load_dashboard(&tx, &user.id, &id).await?;
    tx.execute(
        "DELETE FROM dashboard_widgets WHERE dashboard_id=?1 AND user_id=?2",
        params![id.clone(), user.id.clone()],
    )
    .await?;
    let now = Utc::now().to_rfc3339();
    for (position, input) in inputs.iter().enumerate() {
        insert_widget(&tx, &user.id, &id, position as i64, input, &now).await?;
    }
    let changed = tx
        .execute(
            "UPDATE dashboards SET updated_at=?1 WHERE id=?2 AND user_id=?3",
            params![now, id.clone(), user.id.clone()],
        )
        .await?;
    if changed != 1 {
        return Err(ApiError::conflict(
            "version_conflict",
            "面板已变化，请刷新后重试",
        ));
    }
    let body = load_dashboard(&tx, &user.id, &id).await?;
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

#[utoipa::path(get, path = "/api/v1/dashboards/widget-types", responses((status = 200, body = WidgetTypesResponse)), security(("cookieAuth" = [])))]
pub async fn list_widget_types(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<WidgetTypesResponse>, ApiError> {
    let conn = state.connection().await?;
    let mut plugin_types = Vec::with_capacity(BUILT_IN_PLUGINS.len());
    for plugin in BUILT_IN_PLUGINS {
        plugin_types.push(PluginWidgetTypes {
            plugin_id: plugin.id,
            enabled: plugins::is_enabled(&conn, &user.id, plugin.id).await?,
            widgets: plugin.widgets.to_vec(),
        });
    }
    Ok(Json(WidgetTypesResponse {
        core: CORE_WIDGETS.to_vec(),
        plugins: plugin_types,
    }))
}

fn parse_aggregate_query(query: &AggregateQuery) -> Result<(NaiveDate, NaiveDate), ApiError> {
    let from = NaiveDate::parse_from_str(&query.from, "%Y-%m-%d")
        .map_err(|_| ApiError::validation("from 必须是 YYYY-MM-DD"))?;
    let to = NaiveDate::parse_from_str(&query.to, "%Y-%m-%d")
        .map_err(|_| ApiError::validation("to 必须是 YYYY-MM-DD"))?;
    let days = (to - from).num_days();
    if !(1..=366).contains(&days) {
        return Err(ApiError::validation("统计范围须为 1–366 天"));
    }
    if !matches!(
        query.group_by.as_str(),
        "day" | "month" | "category" | "account"
    ) {
        return Err(ApiError::validation(
            "groupBy 必须是 day、month、category 或 account",
        ));
    }
    if query
        .kind
        .as_deref()
        .is_some_and(|kind| !matches!(kind, "income" | "expense"))
    {
        return Err(ApiError::validation("kind 必须是 income 或 expense"));
    }
    Ok((from, to))
}

#[utoipa::path(get, path = "/api/v1/statistics/aggregate", params(AggregateQuery), responses((status = 200, body = [AggregateItem]), (status = 422, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn statistics_aggregate(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<AggregateQuery>,
) -> Result<Json<Vec<AggregateItem>>, ApiError> {
    let (from, to) = parse_aggregate_query(&query)?;
    let grouping = match query.group_by.as_str() {
        "day" => "substr(t.occurred_on,1,10), substr(t.occurred_on,1,10)",
        "month" => "substr(t.occurred_on,1,7), substr(t.occurred_on,1,7)",
        "category" => {
            "COALESCE(c.id,NULLIF(t.category,''),''), COALESCE(c.name,NULLIF(t.category,''),'')"
        }
        "account" => "COALESCE(a.id,''), COALESCE(a.name,'')",
        _ => unreachable!("groupBy was validated"),
    };
    let sql = format!(
        "SELECT {grouping}, SUM(CASE WHEN t.kind='income' THEN t.amount_cents ELSE 0 END), SUM(CASE WHEN t.kind='expense' THEN t.amount_cents ELSE 0 END), COUNT(*) FROM ledger_transactions t LEFT JOIN categories c ON c.id=t.category_id AND c.user_id=t.user_id LEFT JOIN ledger_accounts a ON a.id=t.account_id AND a.user_id=t.user_id WHERE t.user_id=?1 AND t.occurred_on>=?2 AND t.occurred_on<?3 AND t.archived_at IS NULL AND t.pnl_scope='counted' AND t.kind IN ('income','expense') AND (?4 IS NULL OR t.account_id=?4) AND (?5 IS NULL OR t.category_id=?5) AND (?6 IS NULL OR t.kind=?6) GROUP BY 1,2 ORDER BY 1,2"
    );
    let conn = state.connection().await?;
    let mut rows = conn
        .query(
            &sql,
            params![
                user.id,
                from.format("%Y-%m-%d").to_string(),
                to.format("%Y-%m-%d").to_string(),
                query.account_id,
                query.category_id,
                query.kind
            ],
        )
        .await?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().await? {
        items.push(AggregateItem {
            key: row.get(0)?,
            label: row.get(1)?,
            income_cents: row.get(2)?,
            expense_cents: row.get(3)?,
            count: row.get(4)?,
        });
    }
    Ok(Json(items))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{AggregateQuery, DashboardWidgetInput, parse_aggregate_query, validate_widget};

    #[test]
    fn aggregate_range_is_capped_at_366_days() {
        let query = AggregateQuery {
            from: "2024-01-01".into(),
            to: "2025-01-02".into(),
            group_by: "day".into(),
            account_id: None,
            category_id: None,
            kind: None,
        };
        assert!(parse_aggregate_query(&query).is_err());
    }

    #[test]
    fn widget_validation_rejects_unknown_plugin_and_grid_overflow() {
        let unknown = DashboardWidgetInput {
            id: None,
            widget_type: "plugin:missing:overview".into(),
            plugin_id: Some("missing".into()),
            x: 0,
            y: 0,
            w: 4,
            h: 3,
            config: json!({}),
        };
        assert!(validate_widget(&unknown).is_err());
        let overflow = DashboardWidgetInput {
            id: None,
            widget_type: "core:category-share".into(),
            plugin_id: None,
            x: 10,
            y: 0,
            w: 4,
            h: 3,
            config: json!({}),
        };
        assert!(validate_widget(&overflow).is_err());
    }
}
