use std::collections::{HashMap, HashSet};

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use libsql::{Connection, TransactionBehavior, params};
use uuid::Uuid;

use crate::{
    AppState,
    auth::AuthUser,
    domain::{
        CategoryRuleConditionInput, CategoryRuleConditionView, CategoryRuleView, CategoryView,
        CreateCategoryRequest, CreateCategoryRuleRequest, RecategorizeResponse,
        RevertCategoryRuleResponse, UpdateCategoryRequest, UpdateCategoryRuleRequest,
        validate_note,
    },
    error::ApiError,
    idempotency::{idempotency_key, replay_idempotency, request_hash, store_idempotency},
    transactions::{TransactionPatch, update_transaction_row},
};

const MAX_RULE_CONDITIONS: usize = 10;
const MAX_MATCH_VALUE_CHARS: usize = 200;
const RECEIPT_FIELDS: [&str; 3] = ["merchant_order_id", "channel_category", "pay_method"];

#[utoipa::path(get, path = "/api/v1/categories", responses((status = 200, body = [CategoryView])), security(("cookieAuth" = [])))]
pub async fn list_categories(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<CategoryView>>, ApiError> {
    let conn = state.connection().await?;
    Ok(Json(load_category_tree(&conn, &user.id).await?))
}

#[utoipa::path(post, path = "/api/v1/categories", request_body = CreateCategoryRequest, responses((status = 201, body = CategoryView), (status = 409, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn create_category(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Json(input): Json<CreateCategoryRequest>,
) -> Result<Response, ApiError> {
    let name = validate_category_name(&input.name)?;
    validate_category_kind(&input.kind)?;
    let key = idempotency_key(&headers)?;
    let hash = request_hash(&input)?;
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) =
        replay_idempotency(&tx, &user.id, &key, "create_category", &hash).await?
    {
        return Ok(response);
    }
    if let Some(parent_id) = input.parent_id.as_deref() {
        ensure_category_parent(&tx, &user.id, parent_id, &input.kind).await?;
    }
    ensure_unique_category_name(&tx, &user.id, input.parent_id.as_deref(), &name, None).await?;
    let id = Uuid::now_v7().to_string();
    let now = Utc::now().to_rfc3339();
    tx.execute(
        "INSERT INTO categories(id,user_id,parent_id,name,normalized_name,kind,sort_order,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?8)",
        params![id.clone(), user.id.clone(), input.parent_id, name.clone(), normalize_name(&name), input.kind, input.sort_order, now],
    ).await?;
    let item = load_category(&tx, &user.id, &id).await?;
    store_idempotency(
        &tx,
        &user.id,
        &key,
        "create_category",
        &hash,
        StatusCode::CREATED,
        &item,
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(item)).into_response())
}

#[utoipa::path(patch, path = "/api/v1/categories/{id}", params(("id" = String, Path)), request_body = UpdateCategoryRequest, responses((status = 200, body = CategoryView), (status = 409, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn update_category(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<UpdateCategoryRequest>,
) -> Result<Response, ApiError> {
    let name = input
        .name
        .as_deref()
        .map(validate_category_name)
        .transpose()?;
    let key = idempotency_key(&headers)?;
    let hash = request_hash(&input)?;
    let operation = format!("update_category:{id}");
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) = replay_idempotency(&tx, &user.id, &key, &operation, &hash).await? {
        return Ok(response);
    }
    let existing = load_category(&tx, &user.id, &id).await?;
    let new_name = name.unwrap_or_else(|| existing.name.clone());
    ensure_unique_category_name(
        &tx,
        &user.id,
        existing.parent_id.as_deref(),
        &new_name,
        Some(&id),
    )
    .await?;
    let sort_order = input.sort_order.unwrap_or(existing.sort_order);
    let archived_at = match input.archived {
        Some(true) => Some(Utc::now().to_rfc3339()),
        Some(false) => None,
        None if existing.archived => Some(Utc::now().to_rfc3339()),
        None => None,
    };
    let changed = tx.execute(
        "UPDATE categories SET name=?1,normalized_name=?2,sort_order=?3,archived_at=?4,version=version+1,updated_at=?5 WHERE id=?6 AND user_id=?7 AND version=?8",
        params![new_name.clone(), normalize_name(&new_name), sort_order, archived_at, Utc::now().to_rfc3339(), id.clone(), user.id.clone(), input.version],
    ).await?;
    if changed == 0 {
        return Err(ApiError::conflict(
            "version_conflict",
            "该分类已在其他设备更新，请刷新后重试",
        ));
    }
    let item = load_category(&tx, &user.id, &id).await?;
    store_idempotency(
        &tx,
        &user.id,
        &key,
        &operation,
        &hash,
        StatusCode::OK,
        &item,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(item).into_response())
}

#[utoipa::path(delete, path = "/api/v1/categories/{id}", params(("id" = String, Path)), responses((status = 204), (status = 409, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn delete_category(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let key = idempotency_key(&headers)?;
    let hash = request_hash(&())?;
    let operation = format!("delete_category:{id}");
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) = replay_idempotency(&tx, &user.id, &key, &operation, &hash).await? {
        return Ok(response);
    }
    load_category(&tx, &user.id, &id).await?;
    let (rule_refs, transaction_refs) = category_reference_counts(&tx, &user.id, &id).await?;
    if rule_refs > 0 || transaction_refs > 0 {
        return Err(ApiError::conflict(
            "category_in_use",
            format!("该分类仍被 {rule_refs} 条规则和 {transaction_refs} 笔交易引用，请改用归档"),
        ));
    }
    tx.execute(
        "DELETE FROM categories WHERE id=?1 AND user_id=?2",
        params![id, user.id.clone()],
    )
    .await?;
    store_idempotency(
        &tx,
        &user.id,
        &key,
        &operation,
        &hash,
        StatusCode::NO_CONTENT,
        &(),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[utoipa::path(get, path = "/api/v1/category-rules", responses((status = 200, body = [CategoryRuleView])), security(("cookieAuth" = [])))]
pub async fn list_category_rules(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<CategoryRuleView>>, ApiError> {
    let conn = state.connection().await?;
    Ok(Json(load_rules(&conn, &user.id).await?))
}

#[utoipa::path(post, path = "/api/v1/category-rules", request_body = CreateCategoryRuleRequest, responses((status = 201, body = CategoryRuleView), (status = 422, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn create_category_rule(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Json(input): Json<CreateCategoryRuleRequest>,
) -> Result<Response, ApiError> {
    validate_source_channel(&input.source_channel)?;
    validate_note(&input.note)?;
    validate_conditions(&input.conditions)?;
    let key = idempotency_key(&headers)?;
    let hash = request_hash(&input)?;
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) =
        replay_idempotency(&tx, &user.id, &key, "create_category_rule", &hash).await?
    {
        return Ok(response);
    }
    ensure_rule_category(&tx, &user.id, &input.category_id).await?;
    let id = Uuid::now_v7().to_string();
    let now = Utc::now().to_rfc3339();
    tx.execute(
        "INSERT INTO category_rules(id,user_id,priority,enabled,source_channel,category_id,note,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?8)",
        params![id.clone(), user.id.clone(), input.priority, i64::from(input.enabled), input.source_channel, input.category_id, input.note.trim(), now],
    ).await?;
    insert_conditions(&tx, &id, &input.conditions).await?;
    let item = load_rule(&tx, &user.id, &id).await?;
    store_idempotency(
        &tx,
        &user.id,
        &key,
        "create_category_rule",
        &hash,
        StatusCode::CREATED,
        &item,
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(item)).into_response())
}

#[utoipa::path(patch, path = "/api/v1/category-rules/{id}", params(("id" = String, Path)), request_body = UpdateCategoryRuleRequest, responses((status = 200, body = CategoryRuleView), (status = 404, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn update_category_rule(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<UpdateCategoryRuleRequest>,
) -> Result<Response, ApiError> {
    if let Some(source_channel) = input.source_channel.as_deref() {
        validate_source_channel(source_channel)?;
    }
    if let Some(note) = input.note.as_deref() {
        validate_note(note)?;
    }
    if let Some(conditions) = input.conditions.as_deref() {
        validate_conditions(conditions)?;
    }
    let key = idempotency_key(&headers)?;
    let hash = request_hash(&input)?;
    let operation = format!("update_category_rule:{id}");
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) = replay_idempotency(&tx, &user.id, &key, &operation, &hash).await? {
        return Ok(response);
    }
    let existing = load_rule(&tx, &user.id, &id).await?;
    let category_id = input
        .category_id
        .as_deref()
        .unwrap_or(&existing.category_id);
    ensure_rule_category(&tx, &user.id, category_id).await?;
    tx.execute(
        "UPDATE category_rules SET priority=?1,enabled=?2,source_channel=?3,category_id=?4,note=?5,updated_at=?6 WHERE id=?7 AND user_id=?8",
        params![input.priority.unwrap_or(existing.priority), i64::from(input.enabled.unwrap_or(existing.enabled)), input.source_channel.as_deref().unwrap_or(&existing.source_channel), category_id, input.note.as_deref().unwrap_or(&existing.note).trim(), Utc::now().to_rfc3339(), id.clone(), user.id.clone()],
    ).await?;
    if let Some(conditions) = input.conditions.as_deref() {
        tx.execute(
            "DELETE FROM category_rule_conditions WHERE rule_id=?1",
            [id.as_str()],
        )
        .await?;
        insert_conditions(&tx, &id, conditions).await?;
    }
    let item = load_rule(&tx, &user.id, &id).await?;
    store_idempotency(
        &tx,
        &user.id,
        &key,
        &operation,
        &hash,
        StatusCode::OK,
        &item,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(item).into_response())
}

/// Deleting a rule preserves its existing assignments and only clears their trace reference.
/// Call the rule revert endpoint before deletion when those assignments should be removed.
#[utoipa::path(delete, path = "/api/v1/category-rules/{id}", params(("id" = String, Path)), responses((status = 204), (status = 404, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn delete_category_rule(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let key = idempotency_key(&headers)?;
    let hash = request_hash(&())?;
    let operation = format!("delete_category_rule:{id}");
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) = replay_idempotency(&tx, &user.id, &key, &operation, &hash).await? {
        return Ok(response);
    }
    load_rule(&tx, &user.id, &id).await?;
    tx.execute(
        "DELETE FROM category_rules WHERE id=?1 AND user_id=?2",
        params![id, user.id.clone()],
    )
    .await?;
    store_idempotency(
        &tx,
        &user.id,
        &key,
        &operation,
        &hash,
        StatusCode::NO_CONTENT,
        &(),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[utoipa::path(post, path = "/api/v1/categories/recategorize", responses((status = 200, body = RecategorizeResponse)), security(("cookieAuth" = [])))]
pub async fn recategorize(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let key = idempotency_key(&headers)?;
    let hash = request_hash(&())?;
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) = replay_idempotency(&tx, &user.id, &key, "recategorize", &hash).await? {
        return Ok(response);
    }
    let stats = crate::lifecycle::reapply_category_rules(&tx, &user.id).await?;
    let response = RecategorizeResponse {
        eligible: stats.eligible,
        matched: stats.matched,
        changed: stats.changed,
    };
    store_idempotency(
        &tx,
        &user.id,
        &key,
        "recategorize",
        &hash,
        StatusCode::OK,
        &response,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(response).into_response())
}

#[utoipa::path(post, path = "/api/v1/categories/rules/{id}/revert", params(("id" = String, Path)), responses((status = 200, body = RevertCategoryRuleResponse), (status = 404, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn revert_category_rule(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let key = idempotency_key(&headers)?;
    let hash = request_hash(&())?;
    let operation = format!("revert_category_rule:{id}");
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) = replay_idempotency(&tx, &user.id, &key, &operation, &hash).await? {
        return Ok(response);
    }
    load_rule(&tx, &user.id, &id).await?;
    let mut rows = tx
        .query(
            "SELECT id FROM ledger_transactions WHERE user_id=?1 AND category_rule_id=?2 AND category_source='rule' ORDER BY id",
            params![user.id.clone(), id.clone()],
        )
        .await?;
    let mut transaction_ids = Vec::new();
    while let Some(row) = rows.next().await? {
        transaction_ids.push(row.get::<String>(0)?);
    }
    drop(rows);
    let now = Utc::now().to_rfc3339();
    let mut reverted_count = 0_i64;
    for transaction_id in transaction_ids {
        reverted_count += update_transaction_row(
            &tx,
            &user.id,
            &transaction_id,
            TransactionPatch::ClearCategoryRule {
                rule_id: id.clone(),
                updated_at: now.clone(),
            },
        )
        .await? as i64;
    }
    let response = RevertCategoryRuleResponse { id, reverted_count };
    store_idempotency(
        &tx,
        &user.id,
        &key,
        &operation,
        &hash,
        StatusCode::OK,
        &response,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(response).into_response())
}

async fn load_category_tree(
    conn: &Connection,
    user_id: &str,
) -> Result<Vec<CategoryView>, ApiError> {
    let mut rows = conn.query(
        "SELECT id,parent_id,name,kind,sort_order,archived_at,version FROM categories WHERE user_id=?1 ORDER BY sort_order ASC,name ASC,id ASC",
        [user_id],
    ).await?;
    let mut nodes = HashMap::new();
    let mut ordered_ids = Vec::new();
    while let Some(row) = rows.next().await? {
        let id: String = row.get(0)?;
        ordered_ids.push(id.clone());
        nodes.insert(
            id.clone(),
            CategoryView {
                id,
                parent_id: row.get(1)?,
                name: row.get(2)?,
                kind: row.get(3)?,
                sort_order: row.get(4)?,
                archived: row.get::<Option<String>>(5)?.is_some(),
                version: row.get(6)?,
                children: Vec::new(),
            },
        );
    }
    let known_ids: HashSet<String> = nodes.keys().cloned().collect();
    let mut child_ids: HashMap<String, Vec<String>> = HashMap::new();
    let mut root_ids = Vec::new();
    for id in ordered_ids {
        let parent_id = nodes.get(&id).and_then(|item| item.parent_id.clone());
        if let Some(parent_id) = parent_id.filter(|parent_id| known_ids.contains(parent_id)) {
            child_ids.entry(parent_id).or_default().push(id);
        } else {
            root_ids.push(id);
        }
    }
    Ok(root_ids
        .into_iter()
        .filter_map(|id| take_category_node(&id, &mut nodes, &child_ids))
        .collect())
}

fn take_category_node(
    id: &str,
    nodes: &mut HashMap<String, CategoryView>,
    child_ids: &HashMap<String, Vec<String>>,
) -> Option<CategoryView> {
    let mut node = nodes.remove(id)?;
    if let Some(ids) = child_ids.get(id) {
        node.children = ids
            .iter()
            .filter_map(|child_id| take_category_node(child_id, nodes, child_ids))
            .collect();
    }
    Some(node)
}

async fn load_category(
    conn: &Connection,
    user_id: &str,
    id: &str,
) -> Result<CategoryView, ApiError> {
    let mut rows = conn.query(
        "SELECT id,parent_id,name,kind,sort_order,archived_at,version FROM categories WHERE id=?1 AND user_id=?2",
        params![id, user_id],
    ).await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| ApiError::not_found("找不到该分类"))?;
    Ok(CategoryView {
        id: row.get(0)?,
        parent_id: row.get(1)?,
        name: row.get(2)?,
        kind: row.get(3)?,
        sort_order: row.get(4)?,
        archived: row.get::<Option<String>>(5)?.is_some(),
        version: row.get(6)?,
        children: Vec::new(),
    })
}

async fn ensure_category_parent(
    conn: &Connection,
    user_id: &str,
    id: &str,
    kind: &str,
) -> Result<(), ApiError> {
    let mut rows = conn
        .query(
            "SELECT kind,archived_at FROM categories WHERE id=?1 AND user_id=?2",
            params![id, user_id],
        )
        .await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| ApiError::validation("父分类不存在"))?;
    let parent_kind: String = row.get(0)?;
    if parent_kind != kind {
        return Err(ApiError::validation("父分类与子分类的收支类型必须一致"));
    }
    if row.get::<Option<String>>(1)?.is_some() {
        return Err(ApiError::validation("不能在已归档分类下创建子分类"));
    }
    Ok(())
}

async fn ensure_unique_category_name(
    conn: &Connection,
    user_id: &str,
    parent_id: Option<&str>,
    name: &str,
    excluding_id: Option<&str>,
) -> Result<(), ApiError> {
    let mut rows = conn.query(
        "SELECT id FROM categories WHERE user_id=?1 AND COALESCE(parent_id,'')=COALESCE(?2,'') AND normalized_name=?3 AND (?4 IS NULL OR id<>?4) LIMIT 1",
        params![user_id, parent_id, normalize_name(name), excluding_id],
    ).await?;
    if rows.next().await?.is_some() {
        return Err(ApiError::conflict(
            "category_name_conflict",
            "同级分类名称已存在",
        ));
    }
    Ok(())
}

async fn category_reference_counts(
    conn: &Connection,
    user_id: &str,
    id: &str,
) -> Result<(i64, i64), ApiError> {
    let mut rows = conn.query(
        "SELECT (SELECT COUNT(*) FROM category_rules WHERE user_id=?1 AND category_id=?2),(SELECT COUNT(*) FROM ledger_transactions WHERE user_id=?1 AND category_id=?2)",
        params![user_id, id],
    ).await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| ApiError::internal("分类引用计数失败"))?;
    Ok((row.get(0)?, row.get(1)?))
}

async fn load_rules(conn: &Connection, user_id: &str) -> Result<Vec<CategoryRuleView>, ApiError> {
    let mut rows = conn.query(
        "SELECT r.id,r.priority,r.enabled,r.source_channel,r.category_id,r.note,c.id,c.match_field,c.match_kind,c.match_value FROM category_rules r LEFT JOIN category_rule_conditions c ON c.rule_id=r.id WHERE r.user_id=?1 ORDER BY r.priority ASC,r.created_at ASC,r.id ASC,c.created_at ASC,c.id ASC",
        [user_id],
    ).await?;
    let mut rules: Vec<CategoryRuleView> = Vec::new();
    while let Some(row) = rows.next().await? {
        let id: String = row.get(0)?;
        if rules.last().is_none_or(|rule| rule.id != id) {
            rules.push(CategoryRuleView {
                id: id.clone(),
                priority: row.get(1)?,
                enabled: row.get::<i64>(2)? != 0,
                source_channel: row.get(3)?,
                category_id: row.get(4)?,
                note: row.get(5)?,
                conditions: Vec::new(),
                warnings: Vec::new(),
            });
        }
        if let Some(condition_id) = row.get::<Option<String>>(6)? {
            rules
                .last_mut()
                .expect("rule exists")
                .conditions
                .push(CategoryRuleConditionView {
                    id: condition_id,
                    match_field: row.get(7)?,
                    match_kind: row.get(8)?,
                    match_value: row.get(9)?,
                });
        }
    }
    for rule in &mut rules {
        rule.warnings = rule_warnings(
            &rule.source_channel,
            rule.conditions
                .iter()
                .map(|condition| condition.match_field.as_str()),
        );
    }
    Ok(rules)
}

async fn load_rule(
    conn: &Connection,
    user_id: &str,
    id: &str,
) -> Result<CategoryRuleView, ApiError> {
    load_rules(conn, user_id)
        .await?
        .into_iter()
        .find(|rule| rule.id == id)
        .ok_or_else(|| ApiError::not_found("找不到该分类规则"))
}

async fn ensure_rule_category(conn: &Connection, user_id: &str, id: &str) -> Result<(), ApiError> {
    let mut rows = conn
        .query(
            "SELECT archived_at FROM categories WHERE id=?1 AND user_id=?2",
            params![id, user_id],
        )
        .await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| ApiError::validation("规则目标分类不存在"))?;
    if row.get::<Option<String>>(0)?.is_some() {
        return Err(ApiError::validation("规则目标分类已归档"));
    }
    Ok(())
}

async fn insert_conditions(
    conn: &Connection,
    rule_id: &str,
    conditions: &[CategoryRuleConditionInput],
) -> Result<(), ApiError> {
    let now = Utc::now().to_rfc3339();
    for condition in conditions {
        conn.execute(
            "INSERT INTO category_rule_conditions(id,rule_id,match_field,match_kind,match_value,created_at) VALUES (?1,?2,?3,?4,?5,?6)",
            params![Uuid::now_v7().to_string(), rule_id, condition.match_field.as_str(), condition.match_kind.as_str(), condition.match_value.trim(), now.clone()],
        ).await?;
    }
    Ok(())
}

fn validate_category_name(value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 60 {
        return Err(ApiError::validation("分类名称须为 1–60 个字符"));
    }
    Ok(value.to_owned())
}

fn normalize_name(value: &str) -> String {
    value.trim().to_lowercase()
}

fn validate_category_kind(value: &str) -> Result<(), ApiError> {
    if matches!(value, "income" | "expense") {
        Ok(())
    } else {
        Err(ApiError::validation("分类类型只能是 income 或 expense"))
    }
}

fn validate_source_channel(value: &str) -> Result<(), ApiError> {
    if matches!(value, "" | "alipay" | "wechat" | "cmb" | "cmbc") {
        Ok(())
    } else {
        Err(ApiError::validation("规则来源渠道不正确"))
    }
}

fn validate_conditions(conditions: &[CategoryRuleConditionInput]) -> Result<(), ApiError> {
    if conditions.is_empty() || conditions.len() > MAX_RULE_CONDITIONS {
        return Err(ApiError::validation(format!(
            "规则条件须为 1–{MAX_RULE_CONDITIONS} 条"
        )));
    }
    for condition in conditions {
        let field = condition.match_field.as_str();
        let kind = condition.match_kind.as_str();
        if !matches!(
            field,
            "payee_key"
                | "payee_name"
                | "description"
                | "note"
                | "channel_category"
                | "pay_method"
                | "merchant_order_id"
                | "amount_cents"
                | "kind"
        ) {
            return Err(ApiError::validation("规则匹配字段不正确"));
        }
        let valid_combination = if field == "amount_cents" {
            matches!(kind, "exact" | "gte" | "lte")
        } else {
            matches!(kind, "exact" | "contains" | "prefix")
        };
        if !valid_combination {
            return Err(ApiError::validation(format!(
                "match_kind {kind} 不能用于 match_field {field}"
            )));
        }
        let value = condition.match_value.trim();
        if value.is_empty() || value.chars().count() > MAX_MATCH_VALUE_CHARS {
            return Err(ApiError::validation(format!(
                "match_value 须为 1–{MAX_MATCH_VALUE_CHARS} 个字符"
            )));
        }
        if field == "amount_cents" && value.parse::<i64>().is_err() {
            return Err(ApiError::validation(
                "amount_cents 的 match_value 必须是整数",
            ));
        }
    }
    Ok(())
}

fn rule_warnings<'a>(source_channel: &str, fields: impl Iterator<Item = &'a str>) -> Vec<String> {
    if !source_channel.is_empty() {
        return Vec::new();
    }
    let mut warned = Vec::new();
    for field in fields {
        if RECEIPT_FIELDS.contains(&field) && !warned.contains(&field) {
            warned.push(field);
        }
    }
    warned
        .into_iter()
        .map(|field| format!("条件字段 {field} 仅在特定渠道有语义，建议限定 source_channel"))
        .collect()
}
