use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use libsql::{Connection, TransactionBehavior, params};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    AppState,
    auth::AuthUser,
    domain::{
        AccountType, CounterpartyBrief, CounterpartyView, CreateCounterpartyRequest,
        CreateDebtAdditionRequest, CreateDebtRequest, CreateRepaymentRequest, DashboardSummary,
        DebtAdditionEventView, DebtListQuery, DebtListResponse, DebtOriginKind, DebtStatus,
        DebtView, LedgerAccountBrief, MAX_SAFE_CENTS, RepaymentEventView, ReverseRepaymentRequest,
        UpdateCounterpartyRequest, UpdateDebtAdditionRequest, UpdateDebtRequest,
        UpdateRepaymentRequest, VersionRequest, debt_status, validate_amount, validate_date,
        validate_debt_origin,
    },
    error::ApiError,
};

#[utoipa::path(get, path = "/api/v1/debts", params(DebtListQuery), responses((status = 200, body = DebtListResponse)), security(("cookieAuth" = [])))]
pub async fn list_debts(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<DebtListQuery>,
) -> Result<Json<DebtListResponse>, ApiError> {
    let conn = state.connection().await?;
    let mut items = load_all_debts(&conn, &user, false).await?;
    let search = query
        .query
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_lowercase);
    items.retain(|debt| {
        let search_matches = search.as_ref().is_none_or(|term| {
            debt.counterparty.display_name.to_lowercase().contains(term)
                || debt.note.to_lowercase().contains(term)
        });
        let direction_matches = query
            .direction
            .as_ref()
            .is_none_or(|value| value == &debt.direction);
        let status_matches = query.status.as_ref().is_none_or(|value| {
            serde_json::to_value(&debt.status)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .is_some_and(|status| &status == value)
        });
        let counterparty_matches = query
            .counterparty_id
            .as_ref()
            .is_none_or(|value| value == &debt.counterparty.id);
        let archived_matches = query.archived.is_none_or(|value| value == debt.archived);
        search_matches
            && direction_matches
            && status_matches
            && counterparty_matches
            && archived_matches
    });
    let total = items.len() as u64;
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).clamp(1, 100);
    let start = ((page - 1) * page_size) as usize;
    let items = items
        .into_iter()
        .skip(start)
        .take(page_size as usize)
        .collect();
    Ok(Json(DebtListResponse {
        items,
        page,
        page_size,
        total,
    }))
}

#[utoipa::path(get, path = "/api/v1/debts/{id}", params(("id" = String, Path)), responses((status = 200, body = DebtView), (status = 404, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn get_debt(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<DebtView>, ApiError> {
    let conn = state.connection().await?;
    Ok(Json(load_debt(&conn, &user, &id, true).await?))
}

#[utoipa::path(post, path = "/api/v1/debts", request_body = CreateDebtRequest, responses((status = 201, body = DebtView), (status = 409, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn create_debt(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Json(input): Json<CreateDebtRequest>,
) -> Result<Response, ApiError> {
    validate_amount(input.principal_cents)?;
    validate_date(&input.occurred_on, "发生日期")?;
    if let Some(due) = input.due_on.as_deref() {
        validate_date(due, "到期日")?;
    }
    validate_note(&input.note)?;
    let key = idempotency_key(&headers)?;
    let request_hash = request_hash(&input)?;
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) =
        replay_idempotency(&tx, &user.id, &key, "create_debt", &request_hash).await?
    {
        return Ok(response);
    }

    let origin_kind = input.origin_kind.unwrap_or(DebtOriginKind::CashMovement);
    validate_debt_origin(origin_kind, input.account_id.as_deref(), None)?;
    ensure_active_ledger_account_if_present(&tx, &user.id, input.account_id.as_deref()).await?;

    let counterparty_id = if let Some(id) = input.counterparty_id.as_deref() {
        ensure_counterparty(&tx, &user.id, id).await?;
        id.to_owned()
    } else {
        let name = validate_name(input.counterparty_name.as_deref().unwrap_or_default())?;
        create_counterparty_row(&tx, &user.id, &name, "").await?
    };

    let id = Uuid::now_v7().to_string();
    let now = Utc::now().to_rfc3339();
    tx.execute(
        "INSERT INTO debts(id, user_id, counterparty_id, direction, principal_cents, currency, occurred_on, due_on, note, created_at, updated_at, account_id, origin_kind) VALUES (?1, ?2, ?3, ?4, ?5, 'CNY', ?6, ?7, ?8, ?9, ?9, ?10, ?11)",
        params![id.clone(), user.id.clone(), counterparty_id, input.direction.as_str(), input.principal_cents, input.occurred_on, input.due_on, input.note.trim(), now, input.account_id, origin_kind.as_str()],
    ).await?;
    let debt = load_debt(&tx, &user, &id, true).await?;
    store_idempotency(
        &tx,
        &user.id,
        &key,
        "create_debt",
        &request_hash,
        StatusCode::CREATED,
        &debt,
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(debt)).into_response())
}

#[utoipa::path(patch, path = "/api/v1/debts/{id}", request_body = UpdateDebtRequest, responses((status = 200, body = DebtView), (status = 409, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn update_debt(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<UpdateDebtRequest>,
) -> Result<Response, ApiError> {
    validate_amount(input.principal_cents)?;
    validate_date(&input.occurred_on, "发生日期")?;
    if let Some(due) = input.due_on.as_deref() {
        validate_date(due, "到期日")?;
    }
    validate_note(&input.note)?;
    let key = idempotency_key(&headers)?;
    let request_hash = request_hash(&input)?;
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    let operation = format!("update_debt:{id}");
    if let Some(response) =
        replay_idempotency(&tx, &user.id, &key, &operation, &request_hash).await?
    {
        return Ok(response);
    }
    ensure_counterparty(&tx, &user.id, &input.counterparty_id).await?;
    let mut rows = tx.query(
        "SELECT d.principal_cents, b.event_count, d.origin_kind FROM debts d JOIN debt_balances b ON b.debt_id = d.id WHERE d.id = ?1 AND d.user_id = ?2",
        params![id.clone(), user.id.clone()],
    ).await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| ApiError::not_found("找不到该笔债务"))?;
    let existing_principal: i64 = row.get(0)?;
    let event_count: i64 = row.get(1)?;
    let existing_origin: String = row.get(2)?;
    drop(rows);
    let existing_origin = DebtOriginKind::from_db(&existing_origin)?;
    // 旧客户端不传 originKind：给了账户视为有实际收付款，否则保持原类型
    let origin_kind = input.origin_kind.unwrap_or(if input.account_id.is_some() {
        DebtOriginKind::CashMovement
    } else {
        existing_origin
    });
    validate_debt_origin(
        origin_kind,
        input.account_id.as_deref(),
        Some(existing_origin),
    )?;
    ensure_active_ledger_account_if_present(&tx, &user.id, input.account_id.as_deref()).await?;
    if event_count > 0 && existing_principal != input.principal_cents {
        return Err(ApiError::conflict(
            "principal_locked",
            "已有还款或追加记录，本金不可修改",
        ));
    }
    let changed = tx.execute(
        "UPDATE debts SET counterparty_id = ?1, account_id = ?2, origin_kind = ?3, principal_cents = ?4, occurred_on = ?5, due_on = ?6, note = ?7, version = version + 1, updated_at = ?8 WHERE id = ?9 AND user_id = ?10 AND version = ?11",
        params![input.counterparty_id, input.account_id.clone(), origin_kind.as_str(), input.principal_cents, input.occurred_on, input.due_on, input.note.trim(), Utc::now().to_rfc3339(), id.clone(), user.id.clone(), input.version],
    ).await?;
    if changed == 0 {
        return Err(ApiError::conflict(
            "version_conflict",
            "记录已在其他设备更新，请刷新后重试",
        ));
    }
    let debt = load_debt(&tx, &user, &id, true).await?;
    store_idempotency(
        &tx,
        &user.id,
        &key,
        &operation,
        &request_hash,
        StatusCode::OK,
        &debt,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(debt).into_response())
}

#[utoipa::path(post, path = "/api/v1/debts/{id}/archive", request_body = VersionRequest, responses((status = 200, body = DebtView)), security(("cookieAuth" = [])))]
pub async fn archive_debt(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<VersionRequest>,
) -> Result<Response, ApiError> {
    let key = idempotency_key(&headers)?;
    set_archive(&state, &user, &id, &input, true, &key).await
}

#[utoipa::path(post, path = "/api/v1/debts/{id}/restore", request_body = VersionRequest, responses((status = 200, body = DebtView)), security(("cookieAuth" = [])))]
pub async fn restore_debt(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<VersionRequest>,
) -> Result<Response, ApiError> {
    let key = idempotency_key(&headers)?;
    set_archive(&state, &user, &id, &input, false, &key).await
}

async fn set_archive(
    state: &AppState,
    user: &AuthUser,
    id: &str,
    input: &VersionRequest,
    archived: bool,
    key: &str,
) -> Result<Response, ApiError> {
    let operation = format!("{}_debt:{id}", if archived { "archive" } else { "restore" });
    let request_hash = request_hash(input)?;
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) =
        replay_idempotency(&tx, &user.id, key, &operation, &request_hash).await?
    {
        return Ok(response);
    }
    let value = archived.then(|| Utc::now().to_rfc3339());
    let changed = tx.execute(
        "UPDATE debts SET archived_at = ?1, version = version + 1, updated_at = ?2 WHERE id = ?3 AND user_id = ?4 AND version = ?5",
        params![value, Utc::now().to_rfc3339(), id, user.id.clone(), input.version],
    ).await?;
    if changed == 0 {
        return Err(ApiError::conflict(
            "version_conflict",
            "记录已变化，请刷新后重试",
        ));
    }
    let debt = load_debt(&tx, user, id, true).await?;
    store_idempotency(
        &tx,
        &user.id,
        key,
        &operation,
        &request_hash,
        StatusCode::OK,
        &debt,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(debt).into_response())
}

#[utoipa::path(delete, path = "/api/v1/debts/{id}", request_body = VersionRequest, responses((status = 204), (status = 409, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn delete_debt(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<VersionRequest>,
) -> Result<Response, ApiError> {
    let key = idempotency_key(&headers)?;
    let request_hash = request_hash(&input)?;
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    let operation = format!("delete_debt:{id}");
    if let Some(response) =
        replay_idempotency(&tx, &user.id, &key, &operation, &request_hash).await?
    {
        return Ok(response);
    }
    let mut rows = tx.query(
        "SELECT b.event_count FROM debts d JOIN debt_balances b ON b.debt_id = d.id WHERE d.id = ?1 AND d.user_id = ?2",
        params![id.clone(), user.id.clone()],
    ).await?;
    let event_count: i64 = rows
        .next()
        .await?
        .ok_or_else(|| ApiError::not_found("找不到该笔债务"))?
        .get(0)?;
    drop(rows);
    if event_count > 0 {
        return Err(ApiError::conflict(
            "debt_has_history",
            "已有还款或追加历史，只能归档",
        ));
    }
    let changed = tx
        .execute(
            "DELETE FROM debts WHERE id = ?1 AND user_id = ?2 AND version = ?3",
            params![id, user.id.clone(), input.version],
        )
        .await?;
    if changed == 0 {
        return Err(ApiError::conflict(
            "version_conflict",
            "记录已变化，请刷新后重试",
        ));
    }
    store_idempotency(
        &tx,
        &user.id,
        &key,
        &operation,
        &request_hash,
        StatusCode::NO_CONTENT,
        &(),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[utoipa::path(post, path = "/api/v1/debts/{id}/repayments", request_body = CreateRepaymentRequest, responses((status = 201, body = DebtView), (status = 409, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn create_repayment(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<CreateRepaymentRequest>,
) -> Result<Response, ApiError> {
    validate_amount(input.amount_cents)?;
    validate_date(&input.effective_on, "还款日期")?;
    validate_note(&input.note)?;
    let key = idempotency_key(&headers)?;
    let request_hash = request_hash(&input)?;
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    let operation = format!("repayment:{id}");
    if let Some(response) =
        replay_idempotency(&tx, &user.id, &key, &operation, &request_hash).await?
    {
        return Ok(response);
    }
    let debt = load_debt(&tx, &user, &id, false).await?;
    if debt.archived {
        return Err(ApiError::conflict("debt_archived", "归档债务不能登记还款"));
    }
    ensure_active_ledger_account(&tx, &user.id, &input.account_id).await?;
    if input.amount_cents > debt.remaining_cents {
        return Err(ApiError::conflict(
            "overpayment",
            "还款金额不能超过剩余金额",
        ));
    }
    tx.execute(
        "INSERT INTO repayment_events(id, user_id, debt_id, kind, amount_cents, effective_on, note, created_at, account_id) VALUES (?1, ?2, ?3, 'payment', ?4, ?5, ?6, ?7, ?8)",
        params![Uuid::now_v7().to_string(), user.id.clone(), id.clone(), input.amount_cents, input.effective_on, input.note.trim(), Utc::now().to_rfc3339(), input.account_id],
    ).await?;
    tx.execute(
        "UPDATE debts SET version = version + 1, updated_at = ?1 WHERE id = ?2",
        params![Utc::now().to_rfc3339(), id.clone()],
    )
    .await?;
    let debt = load_debt(&tx, &user, &id, true).await?;
    store_idempotency(
        &tx,
        &user.id,
        &key,
        &operation,
        &request_hash,
        StatusCode::CREATED,
        &debt,
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(debt)).into_response())
}

#[utoipa::path(post, path = "/api/v1/debts/{id}/additions", request_body = CreateDebtAdditionRequest, responses((status = 201, body = DebtView), (status = 409, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn create_debt_addition(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<CreateDebtAdditionRequest>,
) -> Result<Response, ApiError> {
    validate_amount(input.amount_cents)?;
    validate_date(&input.effective_on, "追加日期")?;
    validate_note(&input.note)?;
    let key = idempotency_key(&headers)?;
    let request_hash = request_hash(&input)?;
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    let operation = format!("debt_addition:{id}");
    if let Some(response) =
        replay_idempotency(&tx, &user.id, &key, &operation, &request_hash).await?
    {
        return Ok(response);
    }

    let debt = load_debt(&tx, &user, &id, false).await?;
    if debt.archived {
        return Err(ApiError::conflict("debt_archived", "归档债务不能追加"));
    }
    ensure_active_ledger_account(&tx, &user.id, &input.account_id).await?;
    let principal_cents = debt
        .principal_cents
        .checked_add(input.amount_cents)
        .filter(|value| *value <= MAX_SAFE_CENTS)
        .ok_or_else(|| ApiError::validation("追加后本金超出安全范围"))?;
    let event_id = Uuid::now_v7().to_string();
    let now = Utc::now().to_rfc3339();
    tx.execute(
        "INSERT INTO debt_addition_events(id, user_id, debt_id, amount_cents, effective_on, note, created_at, account_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![event_id, user.id.clone(), id.clone(), input.amount_cents, input.effective_on, input.note.trim(), now.clone(), input.account_id],
    ).await?;
    let changed = tx.execute(
        "UPDATE debts SET principal_cents = ?1, version = version + 1, updated_at = ?2 WHERE id = ?3 AND user_id = ?4 AND version = ?5",
        params![principal_cents, now, id.clone(), user.id.clone(), debt.version],
    ).await?;
    if changed == 0 {
        return Err(ApiError::conflict(
            "version_conflict",
            "记录已变化，请刷新后重试",
        ));
    }
    let debt = load_debt(&tx, &user, &id, true).await?;
    store_idempotency(
        &tx,
        &user.id,
        &key,
        &operation,
        &request_hash,
        StatusCode::CREATED,
        &debt,
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(debt)).into_response())
}

#[utoipa::path(patch, path = "/api/v1/debt-additions/{id}", request_body = UpdateDebtAdditionRequest, responses((status = 200, body = DebtView), (status = 409, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn update_debt_addition(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<UpdateDebtAdditionRequest>,
) -> Result<Response, ApiError> {
    validate_amount(input.amount_cents)?;
    validate_date(&input.effective_on, "追加日期")?;
    validate_note(&input.note)?;
    let converts_to_repayment = matches!(input.movement_type.as_deref(), Some("repayment"));
    if !matches!(
        input.movement_type.as_deref(),
        None | Some("addition") | Some("repayment")
    ) {
        return Err(ApiError::validation("无效的往来类型"));
    }
    let key = idempotency_key(&headers)?;
    let request_hash = request_hash(&input)?;
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    let operation = format!("update_debt_addition:{id}");
    if let Some(response) =
        replay_idempotency(&tx, &user.id, &key, &operation, &request_hash).await?
    {
        return Ok(response);
    }
    let mut rows = tx.query(
        "SELECT e.debt_id, e.amount_cents, d.principal_cents, d.archived_at, b.paid_cents, e.account_id FROM debt_addition_events e JOIN debts d ON d.id = e.debt_id AND d.user_id = e.user_id JOIN debt_balances b ON b.debt_id = d.id WHERE e.id = ?1 AND e.user_id = ?2",
        params![id.clone(), user.id.clone()],
    ).await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| ApiError::not_found("找不到该笔追加记录"))?;
    let debt_id: String = row.get(0)?;
    let previous_amount: i64 = row.get(1)?;
    let principal_cents: i64 = row.get(2)?;
    let archived_at: Option<String> = row.get(3)?;
    let paid_cents: i64 = row.get(4)?;
    let current_account_id: Option<String> = row.get(5)?;
    drop(rows);
    if archived_at.is_some() {
        return Err(ApiError::conflict(
            "debt_archived",
            "归档债务不能编辑追加记录",
        ));
    }
    let account_id = input.account_id.clone().or(current_account_id.clone());
    if account_id != current_account_id
        && let Some(account_id) = account_id.as_deref()
    {
        ensure_active_ledger_account(&tx, &user.id, account_id).await?;
    }
    let updated_principal = principal_cents
        .checked_sub(previous_amount)
        .and_then(|value| {
            if converts_to_repayment {
                Some(value)
            } else {
                value.checked_add(input.amount_cents)
            }
        })
        .filter(|value| (1..=MAX_SAFE_CENTS).contains(value))
        .ok_or_else(|| ApiError::validation("编辑后的本金超出安全范围"))?;
    if updated_principal < paid_cents {
        return Err(ApiError::conflict(
            "addition_amount_below_paid",
            "追加金额不能使本金低于已还金额",
        ));
    }
    if converts_to_repayment {
        let remaining_after_removing_addition = updated_principal
            .checked_sub(paid_cents)
            .ok_or_else(|| ApiError::conflict("overpayment", "还款金额不能超过剩余金额"))?;
        if input.amount_cents > remaining_after_removing_addition {
            return Err(ApiError::conflict(
                "overpayment",
                "还款金额不能超过剩余金额",
            ));
        }
        tx.execute(
            "DELETE FROM debt_addition_events WHERE id = ?1 AND user_id = ?2",
            params![id.clone(), user.id.clone()],
        )
        .await?;
        tx.execute(
            "INSERT INTO repayment_events(id, user_id, debt_id, kind, amount_cents, effective_on, note, created_at, account_id) VALUES (?1, ?2, ?3, 'payment', ?4, ?5, ?6, ?7, ?8)",
            params![id.clone(), user.id.clone(), debt_id.clone(), input.amount_cents, input.effective_on, input.note.trim(), Utc::now().to_rfc3339(), account_id],
        ).await?;
    } else {
        tx.execute(
            "UPDATE debt_addition_events SET amount_cents = ?1, effective_on = ?2, note = ?3, account_id = ?4 WHERE id = ?5 AND user_id = ?6",
            params![input.amount_cents, input.effective_on, input.note.trim(), account_id, id.clone(), user.id.clone()],
        ).await?;
    }
    let changed = tx.execute(
        "UPDATE debts SET principal_cents = ?1, version = version + 1, updated_at = ?2 WHERE id = ?3 AND user_id = ?4 AND version = ?5",
        params![updated_principal, Utc::now().to_rfc3339(), debt_id.clone(), user.id.clone(), input.version],
    ).await?;
    if changed == 0 {
        return Err(ApiError::conflict(
            "version_conflict",
            "记录已在其他设备更新，请刷新后重试",
        ));
    }
    let debt = load_debt(&tx, &user, &debt_id, true).await?;
    store_idempotency(
        &tx,
        &user.id,
        &key,
        &operation,
        &request_hash,
        StatusCode::OK,
        &debt,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(debt).into_response())
}

#[utoipa::path(patch, path = "/api/v1/repayments/{id}", request_body = UpdateRepaymentRequest, responses((status = 200, body = DebtView), (status = 409, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn update_repayment(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<UpdateRepaymentRequest>,
) -> Result<Response, ApiError> {
    validate_amount(input.amount_cents)?;
    validate_date(&input.effective_on, "还款日期")?;
    validate_note(&input.note)?;
    let converts_to_addition = matches!(input.movement_type.as_deref(), Some("addition"));
    if !matches!(
        input.movement_type.as_deref(),
        None | Some("repayment") | Some("addition")
    ) {
        return Err(ApiError::validation("无效的往来类型"));
    }
    let key = idempotency_key(&headers)?;
    let request_hash = request_hash(&input)?;
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    let operation = format!("update_repayment:{id}");
    if let Some(response) =
        replay_idempotency(&tx, &user.id, &key, &operation, &request_hash).await?
    {
        return Ok(response);
    }
    let mut rows = tx.query(
        "SELECT e.debt_id, e.amount_cents, d.principal_cents, d.archived_at, b.remaining_cents, EXISTS(SELECT 1 FROM repayment_events reversal WHERE reversal.reverses_event_id = e.id), e.account_id FROM repayment_events e JOIN debts d ON d.id = e.debt_id AND d.user_id = e.user_id JOIN debt_balances b ON b.debt_id = d.id WHERE e.id = ?1 AND e.user_id = ?2 AND e.kind = 'payment'",
        params![id.clone(), user.id.clone()],
    ).await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| ApiError::not_found("找不到该笔还款"))?;
    let debt_id: String = row.get(0)?;
    let previous_amount: i64 = row.get(1)?;
    let principal_cents: i64 = row.get(2)?;
    let archived_at: Option<String> = row.get(3)?;
    let remaining_cents: i64 = row.get(4)?;
    let has_reversal: i64 = row.get(5)?;
    let current_account_id: Option<String> = row.get(6)?;
    drop(rows);
    if archived_at.is_some() {
        return Err(ApiError::conflict(
            "debt_archived",
            "归档债务不能编辑还款记录",
        ));
    }
    if has_reversal != 0 {
        return Err(ApiError::conflict(
            "repayment_reversed",
            "已撤销的还款记录不能编辑",
        ));
    }
    let account_id = input.account_id.clone().or(current_account_id.clone());
    if account_id != current_account_id
        && let Some(account_id) = account_id.as_deref()
    {
        ensure_active_ledger_account(&tx, &user.id, account_id).await?;
    }
    let maximum_amount = remaining_cents
        .checked_add(previous_amount)
        .ok_or_else(|| ApiError::validation("还款金额超出安全范围"))?;
    if !converts_to_addition && input.amount_cents > maximum_amount {
        return Err(ApiError::conflict(
            "overpayment",
            "还款金额不能超过剩余金额",
        ));
    }
    let updated_principal = if converts_to_addition {
        principal_cents
            .checked_add(input.amount_cents)
            .filter(|value| *value <= MAX_SAFE_CENTS)
            .ok_or_else(|| ApiError::validation("编辑后的本金超出安全范围"))?
    } else {
        principal_cents
    };
    if converts_to_addition {
        tx.execute(
            "DELETE FROM repayment_events WHERE id = ?1 AND user_id = ?2 AND kind = 'payment'",
            params![id.clone(), user.id.clone()],
        )
        .await?;
        tx.execute(
            "INSERT INTO debt_addition_events(id, user_id, debt_id, amount_cents, effective_on, note, created_at, account_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id.clone(), user.id.clone(), debt_id.clone(), input.amount_cents, input.effective_on, input.note.trim(), Utc::now().to_rfc3339(), account_id],
        ).await?;
    } else {
        tx.execute(
            "UPDATE repayment_events SET amount_cents = ?1, effective_on = ?2, note = ?3, account_id = ?4 WHERE id = ?5 AND user_id = ?6 AND kind = 'payment'",
            params![input.amount_cents, input.effective_on, input.note.trim(), account_id, id.clone(), user.id.clone()],
        ).await?;
    }
    let changed = tx.execute(
        "UPDATE debts SET principal_cents = ?1, version = version + 1, updated_at = ?2 WHERE id = ?3 AND user_id = ?4 AND version = ?5",
        params![updated_principal, Utc::now().to_rfc3339(), debt_id.clone(), user.id.clone(), input.version],
    ).await?;
    if changed == 0 {
        return Err(ApiError::conflict(
            "version_conflict",
            "记录已在其他设备更新，请刷新后重试",
        ));
    }
    let debt = load_debt(&tx, &user, &debt_id, true).await?;
    store_idempotency(
        &tx,
        &user.id,
        &key,
        &operation,
        &request_hash,
        StatusCode::OK,
        &debt,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(debt).into_response())
}

#[utoipa::path(post, path = "/api/v1/repayments/{id}/reversals", request_body = ReverseRepaymentRequest, responses((status = 201, body = DebtView), (status = 409, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn reverse_repayment(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<ReverseRepaymentRequest>,
) -> Result<Response, ApiError> {
    validate_date(&input.effective_on, "撤销日期")?;
    validate_note(&input.note)?;
    let key = idempotency_key(&headers)?;
    let request_hash = request_hash(&input)?;
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    let operation = format!("reverse:{id}");
    if let Some(response) =
        replay_idempotency(&tx, &user.id, &key, &operation, &request_hash).await?
    {
        return Ok(response);
    }
    let mut rows = tx.query(
        "SELECT e.debt_id, e.amount_cents, d.archived_at, EXISTS(SELECT 1 FROM repayment_events r WHERE r.reverses_event_id = e.id), e.account_id FROM repayment_events e JOIN debts d ON d.id = e.debt_id WHERE e.id = ?1 AND e.user_id = ?2 AND e.kind = 'payment'",
        params![id.clone(), user.id.clone()],
    ).await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| ApiError::not_found("找不到该笔还款"))?;
    let debt_id: String = row.get(0)?;
    let amount_cents: i64 = row.get(1)?;
    let archived_at: Option<String> = row.get(2)?;
    let already_reversed: i64 = row.get(3)?;
    let account_id: Option<String> = row.get(4)?;
    drop(rows);
    if archived_at.is_some() {
        return Err(ApiError::conflict("debt_archived", "归档债务不能撤销还款"));
    }
    if already_reversed != 0 {
        return Err(ApiError::conflict("already_reversed", "该还款已撤销"));
    }
    tx.execute(
        "INSERT INTO repayment_events(id, user_id, debt_id, kind, amount_cents, effective_on, note, reverses_event_id, created_at, account_id) VALUES (?1, ?2, ?3, 'reversal', ?4, ?5, ?6, ?7, ?8, ?9)",
        params![Uuid::now_v7().to_string(), user.id.clone(), debt_id.clone(), amount_cents, input.effective_on, input.note.trim(), id, Utc::now().to_rfc3339(), account_id],
    ).await?;
    tx.execute(
        "UPDATE debts SET version = version + 1, updated_at = ?1 WHERE id = ?2",
        params![Utc::now().to_rfc3339(), debt_id.clone()],
    )
    .await?;
    let debt = load_debt(&tx, &user, &debt_id, true).await?;
    store_idempotency(
        &tx,
        &user.id,
        &key,
        &operation,
        &request_hash,
        StatusCode::CREATED,
        &debt,
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(debt)).into_response())
}

#[utoipa::path(get, path = "/api/v1/counterparties", responses((status = 200, body = [CounterpartyView])), security(("cookieAuth" = [])))]
pub async fn list_counterparties(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<CounterpartyView>>, ApiError> {
    let conn = state.connection().await?;
    Ok(Json(load_counterparties(&conn, &user).await?))
}

#[utoipa::path(post, path = "/api/v1/counterparties", request_body = CreateCounterpartyRequest, responses((status = 201, body = CounterpartyView)), security(("cookieAuth" = [])))]
pub async fn create_counterparty(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Json(input): Json<CreateCounterpartyRequest>,
) -> Result<Response, ApiError> {
    let name = validate_name(&input.display_name)?;
    validate_note(&input.note)?;
    let key = idempotency_key(&headers)?;
    let request_hash = request_hash(&input)?;
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) =
        replay_idempotency(&tx, &user.id, &key, "create_counterparty", &request_hash).await?
    {
        return Ok(response);
    }
    let id = create_counterparty_row(&tx, &user.id, &name, input.note.trim()).await?;
    let item = load_counterparties(&tx, &user)
        .await?
        .into_iter()
        .find(|item| item.id == id)
        .ok_or_else(|| ApiError::internal("created counterparty missing"))?;
    store_idempotency(
        &tx,
        &user.id,
        &key,
        "create_counterparty",
        &request_hash,
        StatusCode::CREATED,
        &item,
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(item)).into_response())
}

#[utoipa::path(patch, path = "/api/v1/counterparties/{id}", request_body = UpdateCounterpartyRequest, responses((status = 200, body = CounterpartyView)), security(("cookieAuth" = [])))]
pub async fn update_counterparty(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<UpdateCounterpartyRequest>,
) -> Result<Response, ApiError> {
    let name = validate_name(&input.display_name)?;
    validate_note(&input.note)?;
    let key = idempotency_key(&headers)?;
    let request_hash = request_hash(&input)?;
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    let operation = format!("update_counterparty:{id}");
    if let Some(response) =
        replay_idempotency(&tx, &user.id, &key, &operation, &request_hash).await?
    {
        return Ok(response);
    }
    let changed = tx.execute(
        "UPDATE counterparties SET display_name = ?1, normalized_name = ?2, note = ?3, version = version + 1, updated_at = ?4 WHERE id = ?5 AND user_id = ?6 AND version = ?7",
        params![name.clone(), name.to_lowercase(), input.note.trim(), Utc::now().to_rfc3339(), id.clone(), user.id.clone(), input.version],
    ).await?;
    if changed == 0 {
        return Err(ApiError::conflict(
            "version_conflict",
            "联系人已变化，请刷新后重试",
        ));
    }
    let item = load_counterparties(&tx, &user)
        .await?
        .into_iter()
        .find(|item| item.id == id)
        .ok_or_else(|| ApiError::not_found("找不到联系人"))?;
    store_idempotency(
        &tx,
        &user.id,
        &key,
        &operation,
        &request_hash,
        StatusCode::OK,
        &item,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(item).into_response())
}

#[utoipa::path(get, path = "/api/v1/dashboard/summary", responses((status = 200, body = DashboardSummary)), security(("cookieAuth" = [])))]
pub async fn dashboard_summary(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<DashboardSummary>, ApiError> {
    let conn = state.connection().await?;
    let debts = load_all_debts(&conn, &user, false).await?;
    let lend = debts
        .iter()
        .filter(|debt| !debt.archived && debt.direction == "lend_out")
        .map(|debt| debt.remaining_cents)
        .sum();
    let borrow = debts
        .iter()
        .filter(|debt| !debt.archived && debt.direction == "borrow_in")
        .map(|debt| debt.remaining_cents)
        .sum();
    let overdue_count = debts
        .iter()
        .filter(|debt| debt.status == DebtStatus::Overdue)
        .count() as i64;
    Ok(Json(DashboardSummary {
        lend_out_remaining_cents: lend,
        borrow_in_remaining_cents: borrow,
        net_cents: lend - borrow,
        overdue_count,
    }))
}

async fn load_all_debts(
    conn: &Connection,
    user: &AuthUser,
    include_events: bool,
) -> Result<Vec<DebtView>, ApiError> {
    let mut rows = conn
        .query(
            "SELECT d.id FROM debts d WHERE d.user_id = ?1 ORDER BY d.updated_at DESC, d.id DESC",
            [user.id.clone()],
        )
        .await?;
    let mut ids = Vec::new();
    while let Some(row) = rows.next().await? {
        ids.push(row.get::<String>(0)?);
    }
    let mut debts = Vec::with_capacity(ids.len());
    for id in ids {
        debts.push(load_debt(conn, user, &id, include_events).await?);
    }
    Ok(debts)
}

async fn load_debt(
    conn: &Connection,
    user: &AuthUser,
    id: &str,
    include_events: bool,
) -> Result<DebtView, ApiError> {
    let mut rows = conn.query(
        "SELECT d.id, d.direction, d.principal_cents, d.currency, d.occurred_on, d.due_on, d.note, d.archived_at, d.version, d.created_at, d.updated_at, c.id, c.display_name, b.paid_cents, b.remaining_cents, a.id, a.name, a.account_type, a.archived_at, d.origin_kind FROM debts d JOIN counterparties c ON c.id = d.counterparty_id JOIN debt_balances b ON b.debt_id = d.id LEFT JOIN ledger_accounts a ON a.id = d.account_id AND a.user_id = d.user_id WHERE d.id = ?1 AND d.user_id = ?2",
        params![id, user.id.clone()],
    ).await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| ApiError::not_found("找不到该笔债务"))?;
    let due_on: Option<String> = row.get(5)?;
    let archived_at: Option<String> = row.get(7)?;
    let remaining_cents: i64 = row.get(14)?;
    let origin_kind: String = row.get(19)?;
    let mut debt = DebtView {
        id: row.get(0)?,
        direction: row.get(1)?,
        principal_cents: row.get(2)?,
        currency: row.get(3)?,
        occurred_on: row.get(4)?,
        due_on,
        note: row.get(6)?,
        archived: archived_at.is_some(),
        version: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
        counterparty: CounterpartyBrief {
            id: row.get(11)?,
            display_name: row.get(12)?,
        },
        paid_cents: row.get(13)?,
        remaining_cents,
        status: DebtStatus::Open,
        account: ledger_account_brief(&row, 15, 16, 17, 18)?,
        origin_kind: DebtOriginKind::from_db(&origin_kind)?,
        repayments: Vec::new(),
        additions: Vec::new(),
    };
    debt.status = debt_status(
        debt.archived,
        debt.remaining_cents,
        debt.due_on.as_deref(),
        &user.timezone,
        Utc::now(),
    );
    drop(rows);
    if include_events {
        debt.repayments = load_events(conn, &user.id, id).await?;
        debt.additions = load_additions(conn, &user.id, id).await?;
    }
    Ok(debt)
}

async fn load_additions(
    conn: &Connection,
    user_id: &str,
    debt_id: &str,
) -> Result<Vec<DebtAdditionEventView>, ApiError> {
    let mut rows = conn.query(
        "SELECT e.id, e.amount_cents, e.effective_on, e.note, e.created_at, a.id, a.name, a.account_type, a.archived_at FROM debt_addition_events e LEFT JOIN ledger_accounts a ON a.id = e.account_id AND a.user_id = e.user_id WHERE e.user_id = ?1 AND e.debt_id = ?2 ORDER BY e.effective_on DESC, e.created_at DESC",
        params![user_id, debt_id],
    ).await?;
    let mut events = Vec::new();
    while let Some(row) = rows.next().await? {
        events.push(DebtAdditionEventView {
            id: row.get(0)?,
            amount_cents: row.get(1)?,
            effective_on: row.get(2)?,
            note: row.get(3)?,
            created_at: row.get(4)?,
            account: ledger_account_brief(&row, 5, 6, 7, 8)?,
        });
    }
    Ok(events)
}

async fn load_events(
    conn: &Connection,
    user_id: &str,
    debt_id: &str,
) -> Result<Vec<RepaymentEventView>, ApiError> {
    let mut rows = conn.query(
        "SELECT e.id, e.kind, e.amount_cents, e.effective_on, e.note, e.reverses_event_id, e.created_at, EXISTS(SELECT 1 FROM repayment_events r WHERE r.reverses_event_id = e.id), a.id, a.name, a.account_type, a.archived_at FROM repayment_events e LEFT JOIN ledger_accounts a ON a.id = e.account_id AND a.user_id = e.user_id WHERE e.user_id = ?1 AND e.debt_id = ?2 ORDER BY e.effective_on DESC, e.created_at DESC",
        params![user_id, debt_id],
    ).await?;
    let mut events = Vec::new();
    while let Some(row) = rows.next().await? {
        let reversed: i64 = row.get(7)?;
        events.push(RepaymentEventView {
            id: row.get(0)?,
            kind: row.get(1)?,
            amount_cents: row.get(2)?,
            effective_on: row.get(3)?,
            note: row.get(4)?,
            reverses_event_id: row.get(5)?,
            created_at: row.get(6)?,
            reversed: reversed != 0,
            account: ledger_account_brief(&row, 8, 9, 10, 11)?,
        });
    }
    Ok(events)
}

async fn load_counterparties(
    conn: &Connection,
    user: &AuthUser,
) -> Result<Vec<CounterpartyView>, ApiError> {
    let debts = load_all_debts(conn, user, false).await?;
    let mut rows = conn.query("SELECT id, display_name, note, archived_at, version FROM counterparties WHERE user_id = ?1 ORDER BY normalized_name, id", [user.id.clone()]).await?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().await? {
        let id: String = row.get(0)?;
        let scoped: Vec<_> = debts
            .iter()
            .filter(|debt| debt.counterparty.id == id && !debt.archived)
            .collect();
        let lend: i64 = scoped
            .iter()
            .filter(|debt| debt.direction == "lend_out")
            .map(|debt| debt.remaining_cents)
            .sum();
        let borrow: i64 = scoped
            .iter()
            .filter(|debt| debt.direction == "borrow_in")
            .map(|debt| debt.remaining_cents)
            .sum();
        let archived_at: Option<String> = row.get(3)?;
        items.push(CounterpartyView {
            id,
            display_name: row.get(1)?,
            note: row.get(2)?,
            archived: archived_at.is_some(),
            version: row.get(4)?,
            lend_out_remaining_cents: lend,
            borrow_in_remaining_cents: borrow,
            net_cents: lend - borrow,
            active_debt_count: scoped
                .iter()
                .filter(|debt| debt.remaining_cents > 0)
                .count() as i64,
            overdue_count: scoped
                .iter()
                .filter(|debt| debt.status == DebtStatus::Overdue)
                .count() as i64,
        });
    }
    items.sort_by_key(|item| -item.net_cents.abs());
    Ok(items)
}

async fn ensure_counterparty(conn: &Connection, user_id: &str, id: &str) -> Result<(), ApiError> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM counterparties WHERE id = ?1 AND user_id = ?2 AND archived_at IS NULL",
            params![id, user_id],
        )
        .await?;
    if rows.next().await?.is_none() {
        return Err(ApiError::validation("联系人不存在或已归档"));
    }
    Ok(())
}

pub(crate) async fn ensure_active_ledger_account_if_present(
    conn: &Connection,
    user_id: &str,
    id: Option<&str>,
) -> Result<(), ApiError> {
    if let Some(id) = id {
        ensure_active_ledger_account(conn, user_id, id).await?;
    }
    Ok(())
}

async fn ensure_active_ledger_account(
    conn: &Connection,
    user_id: &str,
    id: &str,
) -> Result<(), ApiError> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM ledger_accounts WHERE id = ?1 AND user_id = ?2 AND archived_at IS NULL",
            params![id, user_id],
        )
        .await?;
    if rows.next().await?.is_none() {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "account_unavailable",
            "账户不存在或已归档",
        ));
    }
    Ok(())
}

fn ledger_account_brief(
    row: &libsql::Row,
    id_index: i32,
    name_index: i32,
    account_type_index: i32,
    archived_at_index: i32,
) -> Result<Option<LedgerAccountBrief>, ApiError> {
    let Some(id) = row.get::<Option<String>>(id_index)? else {
        return Ok(None);
    };
    let name = row
        .get::<Option<String>>(name_index)?
        .ok_or_else(|| ApiError::internal("ledger account name missing"))?;
    let account_type = row
        .get::<Option<String>>(account_type_index)?
        .ok_or_else(|| ApiError::internal("ledger account type missing"))?;
    let archived_at: Option<String> = row.get(archived_at_index)?;
    Ok(Some(LedgerAccountBrief {
        id,
        name,
        account_type: AccountType::from_db(&account_type)?,
        archived: archived_at.is_some(),
    }))
}

async fn create_counterparty_row(
    conn: &Connection,
    user_id: &str,
    name: &str,
    note: &str,
) -> Result<String, ApiError> {
    let id = Uuid::now_v7().to_string();
    let now = Utc::now().to_rfc3339();
    conn.execute("INSERT INTO counterparties(id, user_id, display_name, normalized_name, note, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)", params![id.clone(), user_id, name, name.to_lowercase(), note, now]).await?;
    Ok(id)
}

fn validate_name(value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 80 {
        return Err(ApiError::validation("联系人名称须为 1–80 个字符"));
    }
    Ok(value.to_owned())
}

pub(crate) fn validate_note(value: &str) -> Result<(), ApiError> {
    if value.chars().count() > 2_000 {
        return Err(ApiError::validation("备注不能超过 2000 个字符"));
    }
    Ok(())
}

pub(crate) fn idempotency_key(headers: &HeaderMap) -> Result<String, ApiError> {
    let value = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .unwrap_or_default();
    if !(8..=128).contains(&value.len()) || !value.is_ascii() {
        return Err(ApiError::bad_request(
            "idempotency_key_required",
            "请提供 8–128 位 Idempotency-Key",
        ));
    }
    Ok(value.to_owned())
}

pub(crate) fn request_hash<T: Serialize>(value: &T) -> Result<String, ApiError> {
    let json = serde_json::to_vec(value).map_err(ApiError::internal)?;
    Ok(format!("{:x}", Sha256::digest(json)))
}

pub(crate) async fn replay_idempotency(
    conn: &Connection,
    user_id: &str,
    key: &str,
    operation: &str,
    hash: &str,
) -> Result<Option<Response>, ApiError> {
    let mut rows = conn.query("SELECT request_hash, response_status, response_body FROM idempotency_records WHERE user_id = ?1 AND idempotency_key = ?2 AND operation = ?3", params![user_id, key, operation]).await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    let existing_hash: String = row.get(0)?;
    if existing_hash != hash {
        return Err(ApiError::conflict(
            "idempotency_mismatch",
            "该 Idempotency-Key 已用于不同请求",
        ));
    }
    let status: i64 = row.get(1)?;
    let body: String = row.get(2)?;
    let status = StatusCode::from_u16(status as u16).map_err(ApiError::internal)?;
    let mut body: serde_json::Value = serde_json::from_str(&body).map_err(ApiError::internal)?;
    if let Some(object) = body.as_object_mut()
        && object.contains_key("repayments")
    {
        object
            .entry("additions".to_owned())
            .or_insert_with(|| serde_json::Value::Array(Vec::new()));
        object
            .entry("account".to_owned())
            .or_insert(serde_json::Value::Null);
        for key in ["repayments", "additions"] {
            if let Some(events) = object
                .get_mut(key)
                .and_then(serde_json::Value::as_array_mut)
            {
                for event in events {
                    if let Some(event) = event.as_object_mut() {
                        event
                            .entry("account".to_owned())
                            .or_insert(serde_json::Value::Null);
                    }
                }
            }
        }
    }
    backfill_account_types_in_idempotency_response(&mut body);
    Ok(Some((status, Json(body)).into_response()))
}

fn backfill_account_types_in_idempotency_response(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                backfill_account_types_in_idempotency_response(item);
            }
        }
        serde_json::Value::Object(object) => {
            for child in object.values_mut() {
                backfill_account_types_in_idempotency_response(child);
            }
            if !object.contains_key("accountType")
                && object.contains_key("archived")
                && let Some(name) = object.get("name").and_then(serde_json::Value::as_str)
            {
                object.insert(
                    "accountType".to_owned(),
                    serde_json::Value::String(infer_legacy_account_type(name).to_owned()),
                );
            }
            if object.contains_key("usageCount") && object.contains_key("name") {
                object
                    .entry("nameSource".to_owned())
                    .or_insert_with(|| serde_json::Value::String("custom".to_owned()));
            }
        }
        _ => {}
    }
}

fn infer_legacy_account_type(name: &str) -> &'static str {
    if name.starts_with("微信") {
        "wechat_balance"
    } else if name.starts_with("支付宝") {
        "alipay_balance"
    } else if name == "现金" {
        "cash"
    } else if name.starts_with("数字人民币") {
        "digital_cny"
    } else {
        "other"
    }
}

pub(crate) async fn store_idempotency<T: Serialize>(
    conn: &Connection,
    user_id: &str,
    key: &str,
    operation: &str,
    hash: &str,
    status: StatusCode,
    body: &T,
) -> Result<(), ApiError> {
    conn.execute("INSERT INTO idempotency_records(user_id, idempotency_key, operation, request_hash, response_status, response_body, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", params![user_id, key, operation, hash, status.as_u16() as i64, serde_json::to_string(body).map_err(ApiError::internal)?, Utc::now().to_rfc3339()]).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::backfill_account_types_in_idempotency_response;

    #[test]
    fn legacy_account_idempotency_responses_default_to_custom_name_source() {
        let mut account = json!({
            "id": "account-1",
            "name": "旧账户",
            "accountType": "other",
            "archived": false,
            "usageCount": 0
        });
        backfill_account_types_in_idempotency_response(&mut account);
        assert_eq!(account["nameSource"], "custom");

        let mut brief = json!({
            "id": "account-1",
            "name": "旧账户",
            "accountType": "other",
            "archived": false
        });
        backfill_account_types_in_idempotency_response(&mut brief);
        assert!(brief.get("nameSource").is_none());
    }
}
