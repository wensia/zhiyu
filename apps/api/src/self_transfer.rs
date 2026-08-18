use axum::{
    Json,
    extract::State,
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
        CreateSelfTransferAliasRequest, DeleteSelfTransferAliasRequest, SelfTransferAliasView,
        normalize_counterparty, validate_note,
    },
    error::ApiError,
    idempotency::{idempotency_key, replay_idempotency, request_hash, store_idempotency},
};

#[utoipa::path(get, path = "/api/v1/self-transfer-aliases", responses((status = 200, body = [SelfTransferAliasView])), security(("cookieAuth" = [])))]
pub async fn list_self_transfer_aliases(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<SelfTransferAliasView>>, ApiError> {
    let conn = state.connection().await?;
    Ok(Json(load_aliases(&conn, &user.id).await?))
}

#[utoipa::path(post, path = "/api/v1/self-transfer-aliases", request_body = CreateSelfTransferAliasRequest, responses((status = 201, body = SelfTransferAliasView), (status = 409, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn create_self_transfer_alias(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Json(input): Json<CreateSelfTransferAliasRequest>,
) -> Result<Response, ApiError> {
    let alias = validate_alias(&input.alias)?;
    validate_note(&input.note)?;
    let normalized_alias = normalize_counterparty("manual", &alias);
    if normalized_alias.trim().is_empty() {
        return Err(ApiError::validation("自有账户标识归一化后不能为空"));
    }
    let key = idempotency_key(&headers)?;
    let hash = request_hash(&input)?;
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) =
        replay_idempotency(&tx, &user.id, &key, "create_self_transfer_alias", &hash).await?
    {
        return Ok(response);
    }
    let mut duplicates = tx
        .query(
            "SELECT id FROM self_transfer_aliases WHERE user_id=?1 AND normalized_alias=?2",
            params![user.id.clone(), normalized_alias.clone()],
        )
        .await?;
    if duplicates.next().await?.is_some() {
        return Err(ApiError::conflict(
            "self_transfer_alias_exists",
            "该自有账户标识已存在",
        ));
    }
    drop(duplicates);

    let id = Uuid::now_v7().to_string();
    let now = Utc::now().to_rfc3339();
    tx.execute(
        "INSERT INTO self_transfer_aliases(id,user_id,alias,normalized_alias,note,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?6)",
        params![id.clone(), user.id.clone(), alias, normalized_alias, input.note.trim(), now],
    )
    .await?;
    let item = load_alias(&tx, &user.id, &id).await?;
    store_idempotency(
        &tx,
        &user.id,
        &key,
        "create_self_transfer_alias",
        &hash,
        StatusCode::CREATED,
        &item,
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(item)).into_response())
}

#[utoipa::path(delete, path = "/api/v1/self-transfer-aliases", request_body = DeleteSelfTransferAliasRequest, responses((status = 204), (status = 404, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn delete_self_transfer_alias(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Json(input): Json<DeleteSelfTransferAliasRequest>,
) -> Result<Response, ApiError> {
    let key = idempotency_key(&headers)?;
    let hash = request_hash(&input)?;
    let operation = format!("delete_self_transfer_alias:{}", input.id);
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) = replay_idempotency(&tx, &user.id, &key, &operation, &hash).await? {
        return Ok(response);
    }
    let changed = tx
        .execute(
            "DELETE FROM self_transfer_aliases WHERE id=?1 AND user_id=?2",
            params![input.id, user.id.clone()],
        )
        .await?;
    if changed == 0 {
        return Err(ApiError::not_found("找不到该自有账户标识"));
    }
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

async fn load_aliases(
    conn: &Connection,
    user_id: &str,
) -> Result<Vec<SelfTransferAliasView>, ApiError> {
    let mut rows = conn
        .query(
            "SELECT id,alias,normalized_alias,note,created_at,updated_at FROM self_transfer_aliases WHERE user_id=?1 ORDER BY created_at,id",
            [user_id],
        )
        .await?;
    let mut aliases = Vec::new();
    while let Some(row) = rows.next().await? {
        aliases.push(SelfTransferAliasView {
            id: row.get(0)?,
            alias: row.get(1)?,
            normalized_alias: row.get(2)?,
            note: row.get(3)?,
            created_at: row.get(4)?,
            updated_at: row.get(5)?,
        });
    }
    Ok(aliases)
}

async fn load_alias(
    conn: &Connection,
    user_id: &str,
    id: &str,
) -> Result<SelfTransferAliasView, ApiError> {
    let mut rows = conn
        .query(
            "SELECT id,alias,normalized_alias,note,created_at,updated_at FROM self_transfer_aliases WHERE id=?1 AND user_id=?2",
            params![id, user_id],
        )
        .await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| ApiError::not_found("找不到该自有账户标识"))?;
    Ok(SelfTransferAliasView {
        id: row.get(0)?,
        alias: row.get(1)?,
        normalized_alias: row.get(2)?,
        note: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

fn validate_alias(value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 60 {
        return Err(ApiError::validation("自有账户标识须为 1–60 个字符"));
    }
    Ok(value.to_owned())
}
