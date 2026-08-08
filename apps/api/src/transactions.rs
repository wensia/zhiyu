use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use libsql::{Connection, TransactionBehavior, params};
use uuid::Uuid;

use crate::{
    AppState,
    auth::AuthUser,
    debts::{
        ensure_active_ledger_account_if_present, idempotency_key, replay_idempotency, request_hash,
        store_idempotency, validate_note,
    },
    domain::{
        AccountType, CreateTransactionRequest, LedgerTransactionView, TransactionCategorySummary,
        TransactionDaySummary, TransactionKind, TransactionListQuery, TransactionListResponse,
        TransactionMonthSummary, TransactionSummaryQuery, UpdateTransactionRequest, VersionRequest,
        validate_amount, validate_category, validate_date, validate_month,
    },
    error::ApiError,
};

const TRANSACTION_SELECT: &str = "SELECT t.id, t.kind, t.amount_cents, t.occurred_on, t.category, t.note, t.archived_at, t.version, t.created_at, t.updated_at, a.id, a.name, a.account_type, a.archived_at FROM ledger_transactions t LEFT JOIN ledger_accounts a ON a.id = t.account_id";

#[utoipa::path(get, path = "/api/v1/transactions", params(TransactionListQuery), responses((status = 200, body = TransactionListResponse)), security(("cookieAuth" = [])))]
pub async fn list_transactions(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<TransactionListQuery>,
) -> Result<Json<TransactionListResponse>, ApiError> {
    let (month_from, month_to) = match query.month.as_deref() {
        Some(month) => {
            let (from, to) = validate_month(month)?;
            (from, to)
        }
        None => (String::new(), String::new()),
    };
    let kind = match query.kind.as_deref() {
        None | Some("") => String::new(),
        Some("income") => "income".to_owned(),
        Some("expense") => "expense".to_owned(),
        Some(_) => return Err(ApiError::validation("收支类型不正确")),
    };
    let category = query.category.clone().unwrap_or_default();
    let account_id = query.account_id.clone().unwrap_or_default();
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(50).clamp(1, 200);
    let month_enabled = if month_from.is_empty() { "" } else { "1" };
    let conn = state.connection().await?;

    let mut total_rows = conn
        .query(
            "SELECT COUNT(*) FROM ledger_transactions t WHERE t.user_id = ?1 AND t.archived_at IS NULL AND (?2 = '' OR (t.occurred_on >= ?3 AND t.occurred_on < ?4)) AND (?5 = '' OR t.kind = ?5) AND (?6 = '' OR t.category = ?6) AND (?7 = '' OR t.account_id = ?7)",
            params![
                user.id.clone(),
                month_enabled,
                month_from.clone(),
                month_to.clone(),
                kind.clone(),
                category.clone(),
                account_id.clone()
            ],
        )
        .await?;
    let total: i64 = total_rows
        .next()
        .await?
        .map(|row| row.get(0))
        .transpose()?
        .unwrap_or(0);
    drop(total_rows);

    let sql = format!(
        "{TRANSACTION_SELECT} WHERE t.user_id = ?1 AND t.archived_at IS NULL AND (?2 = '' OR (t.occurred_on >= ?3 AND t.occurred_on < ?4)) AND (?5 = '' OR t.kind = ?5) AND (?6 = '' OR t.category = ?6) AND (?7 = '' OR t.account_id = ?7) ORDER BY t.occurred_on DESC, t.id DESC LIMIT ?8 OFFSET ?9"
    );
    let mut rows = conn
        .query(
            &sql,
            params![
                user.id.clone(),
                month_enabled,
                month_from,
                month_to,
                kind,
                category,
                account_id,
                i64::from(page_size),
                i64::from((page - 1) * page_size)
            ],
        )
        .await?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().await? {
        items.push(transaction_from_row(&row)?);
    }
    Ok(Json(TransactionListResponse {
        items,
        page,
        page_size,
        total: total as u64,
    }))
}

#[utoipa::path(post, path = "/api/v1/transactions", request_body = CreateTransactionRequest, responses((status = 201, body = LedgerTransactionView), (status = 409, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn create_transaction(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Json(input): Json<CreateTransactionRequest>,
) -> Result<Response, ApiError> {
    validate_amount(input.amount_cents)?;
    validate_date(&input.occurred_on, "发生日期")?;
    let category = validate_category(&input.category)?;
    validate_note(&input.note)?;
    let key = idempotency_key(&headers)?;
    let request_hash = request_hash(&input)?;
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) =
        replay_idempotency(&tx, &user.id, &key, "create_transaction", &request_hash).await?
    {
        return Ok(response);
    }

    ensure_active_ledger_account_if_present(&tx, &user.id, input.account_id.as_deref()).await?;
    let id = Uuid::now_v7().to_string();
    let now = Utc::now().to_rfc3339();
    tx.execute(
        "INSERT INTO ledger_transactions(id, user_id, kind, amount_cents, occurred_on, category, account_id, note, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
        params![id.clone(), user.id.clone(), input.kind.as_str(), input.amount_cents, input.occurred_on, category, input.account_id, input.note.trim(), now],
    ).await?;
    let item = load_transaction(&tx, &user.id, &id).await?;
    store_idempotency(
        &tx,
        &user.id,
        &key,
        "create_transaction",
        &request_hash,
        StatusCode::CREATED,
        &item,
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(item)).into_response())
}

#[utoipa::path(patch, path = "/api/v1/transactions/{id}", params(("id" = String, Path)), request_body = UpdateTransactionRequest, responses((status = 200, body = LedgerTransactionView), (status = 409, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn update_transaction(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<UpdateTransactionRequest>,
) -> Result<Response, ApiError> {
    validate_amount(input.amount_cents)?;
    validate_date(&input.occurred_on, "发生日期")?;
    let category = validate_category(&input.category)?;
    validate_note(&input.note)?;
    let key = idempotency_key(&headers)?;
    let request_hash = request_hash(&input)?;
    let operation = format!("update_transaction:{id}");
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) =
        replay_idempotency(&tx, &user.id, &key, &operation, &request_hash).await?
    {
        return Ok(response);
    }

    load_transaction(&tx, &user.id, &id).await?;
    ensure_active_ledger_account_if_present(&tx, &user.id, input.account_id.as_deref()).await?;
    let changed = tx.execute(
        "UPDATE ledger_transactions SET kind = ?1, amount_cents = ?2, occurred_on = ?3, category = ?4, account_id = ?5, note = ?6, version = version + 1, updated_at = ?7 WHERE id = ?8 AND user_id = ?9 AND version = ?10",
        params![input.kind.as_str(), input.amount_cents, input.occurred_on, category, input.account_id, input.note.trim(), Utc::now().to_rfc3339(), id.clone(), user.id.clone(), input.version],
    ).await?;
    if changed == 0 {
        return Err(ApiError::conflict(
            "version_conflict",
            "这条记账已在其他设备更新，请刷新后重试",
        ));
    }
    let item = load_transaction(&tx, &user.id, &id).await?;
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

#[utoipa::path(delete, path = "/api/v1/transactions/{id}", params(("id" = String, Path)), request_body = VersionRequest, responses((status = 204), (status = 409, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn delete_transaction(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<VersionRequest>,
) -> Result<Response, ApiError> {
    let key = idempotency_key(&headers)?;
    let request_hash = request_hash(&input)?;
    let operation = format!("delete_transaction:{id}");
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) =
        replay_idempotency(&tx, &user.id, &key, &operation, &request_hash).await?
    {
        return Ok(response);
    }

    let changed = tx.execute(
        "UPDATE ledger_transactions SET archived_at = ?1, version = version + 1, updated_at = ?1 WHERE id = ?2 AND user_id = ?3 AND version = ?4 AND archived_at IS NULL",
        params![Utc::now().to_rfc3339(), id.clone(), user.id.clone(), input.version],
    ).await?;
    if changed == 0 {
        return Err(ApiError::conflict(
            "version_conflict",
            "这条记账已变化，请刷新后重试",
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

#[utoipa::path(post, path = "/api/v1/transactions/{id}/restore", params(("id" = String, Path)), request_body = VersionRequest, responses((status = 200, body = LedgerTransactionView), (status = 409, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn restore_transaction(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<VersionRequest>,
) -> Result<Response, ApiError> {
    let key = idempotency_key(&headers)?;
    let request_hash = request_hash(&input)?;
    let operation = format!("restore_transaction:{id}");
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) =
        replay_idempotency(&tx, &user.id, &key, &operation, &request_hash).await?
    {
        return Ok(response);
    }

    let changed = tx.execute(
        "UPDATE ledger_transactions SET archived_at = NULL, version = version + 1, updated_at = ?1 WHERE id = ?2 AND user_id = ?3 AND version = ?4 AND archived_at IS NOT NULL",
        params![Utc::now().to_rfc3339(), id.clone(), user.id.clone(), input.version],
    ).await?;
    if changed == 0 {
        return Err(ApiError::conflict(
            "version_conflict",
            "这条记账已变化，请刷新后重试",
        ));
    }
    let item = load_transaction(&tx, &user.id, &id).await?;
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

#[utoipa::path(get, path = "/api/v1/transactions/summary", params(TransactionSummaryQuery), responses((status = 200, body = TransactionMonthSummary), (status = 422, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn transaction_summary(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<TransactionSummaryQuery>,
) -> Result<Json<TransactionMonthSummary>, ApiError> {
    let (month_from, month_to) = validate_month(&query.month)?;
    let conn = state.connection().await?;

    let mut day_rows = conn
        .query(
            "SELECT substr(occurred_on, 1, 10) AS day, SUM(CASE WHEN kind = 'income' THEN amount_cents ELSE 0 END) AS income_cents, SUM(CASE WHEN kind = 'expense' THEN amount_cents ELSE 0 END) AS expense_cents, COUNT(*) AS entry_count FROM ledger_transactions WHERE user_id = ?1 AND archived_at IS NULL AND occurred_on >= ?2 AND occurred_on < ?3 GROUP BY day ORDER BY day",
            params![user.id.clone(), month_from.clone(), month_to.clone()],
        )
        .await?;
    let mut days = Vec::new();
    let mut income_cents = 0_i64;
    let mut expense_cents = 0_i64;
    let mut transaction_count = 0_i64;
    while let Some(row) = day_rows.next().await? {
        let day_income: i64 = row.get(1)?;
        let day_expense: i64 = row.get(2)?;
        income_cents += day_income;
        expense_cents += day_expense;
        transaction_count += row.get::<i64>(3)?;
        days.push(TransactionDaySummary {
            date: row.get(0)?,
            income_cents: day_income,
            expense_cents: day_expense,
        });
    }
    drop(day_rows);

    let mut category_rows = conn
        .query(
            "SELECT category, SUM(CASE WHEN kind = 'income' THEN amount_cents ELSE 0 END) AS income_cents, SUM(CASE WHEN kind = 'expense' THEN amount_cents ELSE 0 END) AS expense_cents, COUNT(*) AS entry_count FROM ledger_transactions WHERE user_id = ?1 AND archived_at IS NULL AND occurred_on >= ?2 AND occurred_on < ?3 GROUP BY category ORDER BY expense_cents DESC, category",
            params![user.id.clone(), month_from, month_to],
        )
        .await?;
    let mut by_category = Vec::new();
    while let Some(row) = category_rows.next().await? {
        by_category.push(TransactionCategorySummary {
            category: row.get(0)?,
            income_cents: row.get(1)?,
            expense_cents: row.get(2)?,
            count: row.get(3)?,
        });
    }

    Ok(Json(TransactionMonthSummary {
        month: query.month.clone(),
        days,
        by_category,
        income_cents,
        expense_cents,
        net_cents: income_cents - expense_cents,
        transaction_count,
    }))
}

#[utoipa::path(get, path = "/api/v1/transactions/categories", responses((status = 200, body = [String])), security(("cookieAuth" = [])))]
pub async fn list_transaction_categories(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<String>>, ApiError> {
    let conn = state.connection().await?;
    let mut rows = conn
        .query(
            "SELECT DISTINCT category FROM ledger_transactions WHERE user_id = ?1 AND archived_at IS NULL AND category <> '' ORDER BY category",
            [user.id.clone()],
        )
        .await?;
    let mut categories = Vec::new();
    while let Some(row) = rows.next().await? {
        categories.push(row.get::<String>(0)?);
    }
    Ok(Json(categories))
}

async fn load_transaction(
    conn: &Connection,
    user_id: &str,
    id: &str,
) -> Result<LedgerTransactionView, ApiError> {
    let mut rows = conn
        .query(
            &format!("{TRANSACTION_SELECT} WHERE t.id = ?1 AND t.user_id = ?2"),
            params![id, user_id],
        )
        .await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| ApiError::not_found("找不到这条记账"))?;
    transaction_from_row(&row)
}

fn transaction_from_row(row: &libsql::Row) -> Result<LedgerTransactionView, ApiError> {
    let kind: String = row.get(1)?;
    let archived_at: Option<String> = row.get(6)?;
    let account_id: Option<String> = row.get(10)?;
    let account = match account_id {
        Some(id) => {
            let name: String = row.get(11)?;
            let account_type: String = row.get(12)?;
            let account_archived_at: Option<String> = row.get(13)?;
            Some(crate::domain::LedgerAccountBrief {
                id,
                name,
                account_type: AccountType::from_db(&account_type)?,
                archived: account_archived_at.is_some(),
            })
        }
        None => None,
    };
    Ok(LedgerTransactionView {
        id: row.get(0)?,
        kind: TransactionKind::from_db(&kind)?,
        amount_cents: row.get(2)?,
        occurred_on: row.get(3)?,
        category: row.get(4)?,
        account,
        note: row.get(5)?,
        archived: archived_at.is_some(),
        version: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}
