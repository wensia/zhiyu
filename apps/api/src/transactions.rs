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
    accounts::ensure_active_ledger_account_if_present,
    auth::AuthUser,
    domain::{
        AccountType, CreateTransactionRequest, LedgerTransactionView, PnlScope,
        TransactionCategorySummary, TransactionDaySummary, TransactionKind, TransactionLinkView,
        TransactionListQuery, TransactionListResponse, TransactionMonthSummary,
        TransactionSummaryQuery, UpdateTransactionRequest, VersionRequest, normalize_counterparty,
        validate_amount, validate_category, validate_date, validate_month, validate_note,
    },
    error::ApiError,
    idempotency::{idempotency_key, replay_idempotency, request_hash, store_idempotency},
};

const TRANSACTION_SELECT: &str = "SELECT t.id, t.kind, t.amount_cents, t.occurred_on, t.category, t.note, t.archived_at, t.version, t.created_at, t.updated_at, a.id, a.name, a.account_type, a.archived_at, COALESCE((SELECT json_group_array(json_object('pluginId', link.plugin_id, 'kind', link.kind, 'refId', link.ref_id, 'label', link.label)) FROM (SELECT l.plugin_id, l.kind, l.ref_id, l.label FROM transaction_links l WHERE l.user_id = t.user_id AND l.transaction_id = t.id ORDER BY l.created_at, l.id) link), '[]'), transfer_from.id, transfer_from.name, transfer_from.account_type, transfer_from.archived_at, transfer_to.id, transfer_to.name, transfer_to.account_type, transfer_to.archived_at, t.payee_name, COALESCE(t.payee_key, ''), t.description, t.occurred_at, t.occurred_at_precision, t.currency, t.category_id, t.category_source, t.pnl_scope, t.created_by, t.category_rule_id, (SELECT NULLIF(r.note, '') FROM category_rules r WHERE r.id=t.category_rule_id AND r.user_id=t.user_id) FROM ledger_transactions t LEFT JOIN ledger_accounts a ON a.id = t.account_id AND a.user_id = t.user_id LEFT JOIN ledger_accounts transfer_from ON transfer_from.id = t.transfer_from_account_id AND transfer_from.user_id = t.user_id LEFT JOIN ledger_accounts transfer_to ON transfer_to.id = t.transfer_to_account_id AND transfer_to.user_id = t.user_id";

pub(crate) enum OnExternalConflict {
    Error,
    Ignore,
}

pub(crate) struct NewTransactionRow {
    pub(crate) id: String,
    pub(crate) user_id: String,
    pub(crate) kind: String,
    pub(crate) amount_cents: i64,
    pub(crate) currency: String,
    pub(crate) occurred_on: String,
    pub(crate) occurred_at: Option<String>,
    pub(crate) occurred_at_precision: String,
    pub(crate) category: String,
    pub(crate) category_id: Option<String>,
    pub(crate) category_source: String,
    pub(crate) category_rule_id: Option<String>,
    pub(crate) payee_name: String,
    pub(crate) payee_key: String,
    pub(crate) description: String,
    pub(crate) account_id: Option<String>,
    pub(crate) transfer_from_account_id: Option<String>,
    pub(crate) transfer_to_account_id: Option<String>,
    pub(crate) note: String,
    pub(crate) archived_at: Option<String>,
    pub(crate) version: i64,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    pub(crate) source_channel: String,
    pub(crate) external_id: String,
    pub(crate) import_batch_id: Option<String>,
    pub(crate) event_id: Option<String>,
    pub(crate) pnl_scope: String,
    pub(crate) created_by: String,
    pub(crate) on_external_conflict: OnExternalConflict,
}

pub(crate) enum TransactionPatch {
    BindAccountIfUnboundNonTransfer {
        account_id: String,
    },
    SetPayeeKey {
        payee_key: String,
    },
    RestoreAmountAccountsAndEvent {
        amount_cents: i64,
        account_id: Option<String>,
        transfer_to_account_id: Option<String>,
        event_id: Option<String>,
        expected_event_id: String,
        updated_at: String,
    },
    RestoreArchiveAndEvent {
        archived_at: Option<String>,
        event_id: Option<String>,
        expected_event_id: String,
        updated_at: String,
    },
    SetAmountAccountAndEvent {
        amount_cents: i64,
        account_id: String,
        event_id: String,
        updated_at: String,
    },
    SetAmountTransferDestinationAndEvent {
        amount_cents: i64,
        transfer_to_account_id: String,
        event_id: String,
        updated_at: String,
    },
    ArchiveAndSetEvent {
        archived_at: String,
        event_id: String,
    },
    ReplaceStandardFields {
        kind: String,
        amount_cents: i64,
        currency: String,
        occurred_on: String,
        description: String,
        account_id: String,
        updated_at: String,
    },
    SyncPnlScopeForPlugin {
        plugin_id: String,
    },
    ClearCategoryRule {
        rule_id: String,
        updated_at: String,
    },
}

pub(crate) async fn insert_transaction_row(
    tx: &Connection,
    row: NewTransactionRow,
) -> Result<u64, ApiError> {
    let sql = match row.on_external_conflict {
        OnExternalConflict::Error => {
            "INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,currency,occurred_on,occurred_at,occurred_at_precision,category,category_id,category_source,category_rule_id,payee_name,payee_key,description,account_id,transfer_from_account_id,transfer_to_account_id,note,archived_at,version,created_at,updated_at,source_channel,external_id,import_batch_id,event_id,pnl_scope,created_by) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29)"
        }
        OnExternalConflict::Ignore => {
            "INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,currency,occurred_on,occurred_at,occurred_at_precision,category,category_id,category_source,category_rule_id,payee_name,payee_key,description,account_id,transfer_from_account_id,transfer_to_account_id,note,archived_at,version,created_at,updated_at,source_channel,external_id,import_batch_id,event_id,pnl_scope,created_by) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29) ON CONFLICT(user_id,source_channel,external_id) WHERE external_id != '' DO NOTHING"
        }
    };
    Ok(tx
        .execute(
            sql,
            params![
                row.id,
                row.user_id,
                row.kind,
                row.amount_cents,
                row.currency,
                row.occurred_on,
                row.occurred_at,
                row.occurred_at_precision,
                row.category,
                row.category_id,
                row.category_source,
                row.category_rule_id,
                row.payee_name,
                row.payee_key,
                row.description,
                row.account_id,
                row.transfer_from_account_id,
                row.transfer_to_account_id,
                row.note,
                row.archived_at,
                row.version,
                row.created_at,
                row.updated_at,
                row.source_channel,
                row.external_id,
                row.import_batch_id,
                row.event_id,
                row.pnl_scope,
                row.created_by
            ],
        )
        .await?)
}

pub(crate) async fn update_transaction_row(
    tx: &Connection,
    user_id: &str,
    id: &str,
    patch: TransactionPatch,
) -> Result<u64, ApiError> {
    let changed = match patch {
        TransactionPatch::BindAccountIfUnboundNonTransfer { account_id } => {
            tx.execute(
                "UPDATE ledger_transactions SET account_id=?1 WHERE id=?2 AND user_id=?3 AND account_id IS NULL AND kind<>'transfer'",
                params![account_id, id, user_id],
            )
            .await?
        }
        TransactionPatch::SetPayeeKey { payee_key } => {
            tx.execute(
                "UPDATE ledger_transactions SET payee_key=?1 WHERE id=?2 AND user_id=?3",
                params![payee_key, id, user_id],
            )
            .await?
        }
        TransactionPatch::RestoreAmountAccountsAndEvent {
            amount_cents,
            account_id,
            transfer_to_account_id,
            event_id,
            expected_event_id,
            updated_at,
        } => {
            tx.execute(
                "UPDATE ledger_transactions SET amount_cents=?1,account_id=?2,transfer_to_account_id=?3,event_id=?4,updated_at=?5,version=version+1 WHERE id=?6 AND user_id=?7 AND event_id=?8",
                params![amount_cents, account_id, transfer_to_account_id, event_id, updated_at, id, user_id, expected_event_id],
            )
            .await?
        }
        TransactionPatch::RestoreArchiveAndEvent {
            archived_at,
            event_id,
            expected_event_id,
            updated_at,
        } => {
            tx.execute(
                "UPDATE ledger_transactions SET archived_at=?1,event_id=?2,updated_at=?3,version=version+1 WHERE id=?4 AND user_id=?5 AND event_id=?6",
                params![archived_at, event_id, updated_at, id, user_id, expected_event_id],
            )
            .await?
        }
        TransactionPatch::SetAmountAccountAndEvent {
            amount_cents,
            account_id,
            event_id,
            updated_at,
        } => {
            tx.execute(
                "UPDATE ledger_transactions SET amount_cents=?1,account_id=?2,event_id=?3,updated_at=?4,version=version+1 WHERE id=?5 AND user_id=?6 AND archived_at IS NULL",
                params![amount_cents, account_id, event_id, updated_at, id, user_id],
            )
            .await?
        }
        TransactionPatch::SetAmountTransferDestinationAndEvent {
            amount_cents,
            transfer_to_account_id,
            event_id,
            updated_at,
        } => {
            tx.execute(
                "UPDATE ledger_transactions SET amount_cents=?1,transfer_to_account_id=?2,event_id=?3,updated_at=?4,version=version+1 WHERE id=?5 AND user_id=?6 AND archived_at IS NULL",
                params![amount_cents, transfer_to_account_id, event_id, updated_at, id, user_id],
            )
            .await?
        }
        TransactionPatch::ArchiveAndSetEvent {
            archived_at,
            event_id,
        } => {
            tx.execute(
                "UPDATE ledger_transactions SET archived_at=?1,event_id=?2,updated_at=?1,version=version+1 WHERE id=?3 AND user_id=?4 AND archived_at IS NULL",
                params![archived_at, event_id, id, user_id],
            )
            .await?
        }
        TransactionPatch::ReplaceStandardFields {
            kind,
            amount_cents,
            currency,
            occurred_on,
            description,
            account_id,
            updated_at,
        } => {
            tx.execute(
                "UPDATE ledger_transactions SET kind=?1,amount_cents=?2,currency=?3,occurred_on=?4,occurred_at=NULL,occurred_at_precision='day',description=?5,account_id=?6,transfer_from_account_id=NULL,transfer_to_account_id=NULL,note=?5,version=version+1,updated_at=?7 WHERE id=?8 AND user_id=?9 AND archived_at IS NULL",
                params![kind, amount_cents, currency, occurred_on, description, account_id, updated_at, id, user_id],
            )
            .await?
        }
        TransactionPatch::SyncPnlScopeForPlugin { plugin_id } => {
            tx.execute(
                "UPDATE ledger_transactions SET pnl_scope=CASE WHEN EXISTS (SELECT 1 FROM transaction_links l WHERE l.transaction_id=?1 AND l.user_id=?2 AND l.plugin_id=?3) THEN 'excluded' ELSE 'counted' END WHERE id=?1 AND user_id=?2",
                params![id, user_id, plugin_id],
            )
            .await?
        }
        TransactionPatch::ClearCategoryRule {
            rule_id,
            updated_at,
        } => {
            tx.execute(
                "UPDATE ledger_transactions SET category_id=NULL,category_source='none',category_rule_id=NULL,updated_at=?1 WHERE id=?2 AND user_id=?3 AND category_rule_id=?4 AND category_source='rule'",
                params![updated_at, id, user_id, rule_id],
            )
            .await?
        }
    };
    Ok(changed)
}

pub(crate) async fn archive_transaction_row(
    tx: &Connection,
    user_id: &str,
    id: &str,
) -> Result<u64, ApiError> {
    let now = Utc::now().to_rfc3339();
    let changed = tx
        .execute(
            "UPDATE ledger_transactions SET archived_at=?1,version=version+1,updated_at=?1 WHERE id=?2 AND user_id=?3 AND archived_at IS NULL",
            params![now, id, user_id],
        )
        .await?;
    tx.execute(
        "DELETE FROM transaction_links WHERE transaction_id=?1 AND user_id=?2",
        params![id, user_id],
    )
    .await?;
    Ok(changed)
}

pub(crate) async fn hard_delete_transaction_row(
    tx: &Connection,
    user_id: &str,
    id: &str,
) -> Result<u64, ApiError> {
    tx.execute(
        "DELETE FROM transaction_links WHERE transaction_id=?1 AND user_id=?2",
        params![id, user_id],
    )
    .await?;
    Ok(tx
        .execute(
            "DELETE FROM ledger_transactions WHERE id=?1 AND user_id=?2",
            params![id, user_id],
        )
        .await?)
}

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
        Some("transfer") => "transfer".to_owned(),
        Some(_) => return Err(ApiError::validation("收支类型不正确")),
    };
    let category = query.category.clone().unwrap_or_default();
    let account_id = query.account_id.clone().unwrap_or_default();
    let search = query.q.as_deref().unwrap_or_default().trim();
    if search.chars().count() > 100 {
        return Err(ApiError::validation("搜索词不能超过 100 个字符"));
    }
    let search_pattern = if search.is_empty() {
        String::new()
    } else {
        format!(
            "%{}%",
            search
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        )
    };
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(50).clamp(1, 200);
    let month_enabled = if month_from.is_empty() { "" } else { "1" };
    let conn = state.connection().await?;

    let mut total_rows = conn
        .query(
            "SELECT COUNT(*) FROM ledger_transactions t WHERE t.user_id = ?1 AND t.archived_at IS NULL AND (?2 = '' OR (t.occurred_on >= ?3 AND t.occurred_on < ?4)) AND (?5 = '' OR t.kind = ?5) AND (?6 = '' OR t.category_id = ?6 OR EXISTS (SELECT 1 FROM categories filter_category WHERE filter_category.id = t.category_id AND filter_category.name = ?6) OR (t.category_id IS NULL AND t.category = ?6)) AND (?7 = '' OR t.account_id = ?7) AND (?8 = '' OR LOWER(t.payee_name) LIKE LOWER(?8) ESCAPE '\\' OR LOWER(t.payee_key) LIKE LOWER(?8) ESCAPE '\\' OR LOWER(t.description) LIKE LOWER(?8) ESCAPE '\\' OR LOWER(t.note) LIKE LOWER(?8) ESCAPE '\\')",
            params![
                user.id.clone(),
                month_enabled,
                month_from.clone(),
                month_to.clone(),
                kind.clone(),
                category.clone(),
                account_id.clone(),
                search_pattern.clone()
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
        "{TRANSACTION_SELECT} WHERE t.user_id = ?1 AND t.archived_at IS NULL AND (?2 = '' OR (t.occurred_on >= ?3 AND t.occurred_on < ?4)) AND (?5 = '' OR t.kind = ?5) AND (?6 = '' OR t.category_id = ?6 OR EXISTS (SELECT 1 FROM categories filter_category WHERE filter_category.id = t.category_id AND filter_category.name = ?6) OR (t.category_id IS NULL AND t.category = ?6)) AND (?7 = '' OR t.account_id = ?7) AND (?8 = '' OR LOWER(t.payee_name) LIKE LOWER(?8) ESCAPE '\\' OR LOWER(t.payee_key) LIKE LOWER(?8) ESCAPE '\\' OR LOWER(t.description) LIKE LOWER(?8) ESCAPE '\\' OR LOWER(t.note) LIKE LOWER(?8) ESCAPE '\\') ORDER BY t.occurred_on DESC, t.id DESC LIMIT ?9 OFFSET ?10"
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
                search_pattern,
                i64::from(page_size),
                i64::from(page - 1) * i64::from(page_size)
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

#[utoipa::path(post, path = "/api/v1/transactions", request_body = CreateTransactionRequest, responses((status = 201, body = LedgerTransactionView), (status = 400, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
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
    validate_transaction_accounts(
        input.kind,
        input.account_id.as_deref(),
        input.transfer_from_account_id.as_deref(),
        input.transfer_to_account_id.as_deref(),
    )?;
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
    ensure_active_ledger_account_if_present(
        &tx,
        &user.id,
        input.transfer_from_account_id.as_deref(),
    )
    .await?;
    ensure_active_ledger_account_if_present(&tx, &user.id, input.transfer_to_account_id.as_deref())
        .await?;
    let id = Uuid::now_v7().to_string();
    let now = Utc::now().to_rfc3339();
    let payee_key = normalize_counterparty("manual", "");
    tx.execute(
        "INSERT INTO ledger_transactions(id, user_id, kind, amount_cents, occurred_on, category, account_id, transfer_from_account_id, transfer_to_account_id, note, payee_key, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
        params![id.clone(), user.id.clone(), input.kind.as_str(), input.amount_cents, input.occurred_on, category, input.account_id, input.transfer_from_account_id, input.transfer_to_account_id, input.note.trim(), payee_key, now],
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

#[utoipa::path(patch, path = "/api/v1/transactions/{id}", params(("id" = String, Path)), request_body = UpdateTransactionRequest, responses((status = 200, body = LedgerTransactionView), (status = 400, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
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
    validate_transaction_accounts(
        input.kind,
        input.account_id.as_deref(),
        input.transfer_from_account_id.as_deref(),
        input.transfer_to_account_id.as_deref(),
    )?;
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

    let existing = load_transaction(&tx, &user.id, &id).await?;
    let mut source_rows = tx
        .query(
            "SELECT source_channel FROM ledger_transactions WHERE id=?1 AND user_id=?2",
            params![id.clone(), user.id.clone()],
        )
        .await?;
    let source_channel = source_rows
        .next()
        .await?
        .ok_or_else(|| ApiError::not_found("找不到该记账"))?
        .get::<String>(0)?;
    drop(source_rows);
    let payee_key = normalize_counterparty(
        if source_channel.is_empty() {
            "manual"
        } else {
            &source_channel
        },
        &existing.payee_name,
    );
    ensure_active_ledger_account_if_present(&tx, &user.id, input.account_id.as_deref()).await?;
    ensure_active_ledger_account_if_present(
        &tx,
        &user.id,
        input.transfer_from_account_id.as_deref(),
    )
    .await?;
    ensure_active_ledger_account_if_present(&tx, &user.id, input.transfer_to_account_id.as_deref())
        .await?;
    if let Some(category_id) = input.category_id.as_deref() {
        ensure_active_category(&tx, &user.id, category_id, input.kind.as_str()).await?;
    }
    let changed = tx.execute(
        "UPDATE ledger_transactions SET kind = ?1, amount_cents = ?2, occurred_on = ?3, category = ?4, account_id = ?5, transfer_from_account_id = ?6, transfer_to_account_id = ?7, note = ?8, payee_key = ?9, category_id=COALESCE(?10,category_id), category_source=CASE WHEN ?10 IS NOT NULL THEN 'user' ELSE category_source END, category_rule_id=CASE WHEN ?10 IS NOT NULL THEN NULL ELSE category_rule_id END, version = version + 1, updated_at = ?11 WHERE id = ?12 AND user_id = ?13 AND version = ?14",
        params![input.kind.as_str(), input.amount_cents, input.occurred_on, category, input.account_id, input.transfer_from_account_id, input.transfer_to_account_id, input.note.trim(), payee_key, input.category_id, Utc::now().to_rfc3339(), id.clone(), user.id.clone(), input.version],
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

#[utoipa::path(delete, path = "/api/v1/transactions/{id}", params(("id" = String, Path)), request_body = VersionRequest, responses((status = 200, body = LedgerTransactionView), (status = 409, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
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

    let mut creator_rows = tx
        .query(
            "SELECT created_by FROM ledger_transactions WHERE id=?1 AND user_id=?2",
            params![id.clone(), user.id.clone()],
        )
        .await?;
    let created_by = creator_rows
        .next()
        .await?
        .ok_or_else(|| ApiError::not_found("找不到这条记账"))?
        .get::<String>(0)?;
    drop(creator_rows);
    if let Some(plugin) = crate::plugins::owning_plugin_for_created_by(&created_by) {
        return Err(ApiError::conflict(
            "plugin_owned_transaction",
            format!(
                "这笔由{}创建，请在{}里删除对应记录",
                plugin.name, plugin.name
            ),
        ));
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
    tx.execute(
        "DELETE FROM transaction_links WHERE transaction_id = ?1 AND user_id = ?2",
        params![id.clone(), user.id.clone()],
    )
    .await?;
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
            "SELECT substr(t.occurred_on, 1, 10) AS day, SUM(CASE WHEN t.kind = 'income' THEN t.amount_cents ELSE 0 END) AS income_cents, SUM(CASE WHEN t.kind = 'expense' THEN t.amount_cents ELSE 0 END) AS expense_cents, SUM(CASE WHEN t.kind IN ('income', 'expense') THEN 1 ELSE 0 END) AS entry_count FROM ledger_transactions t WHERE t.user_id = ?1 AND t.archived_at IS NULL AND t.occurred_on >= ?2 AND t.occurred_on < ?3 AND t.pnl_scope = 'counted' GROUP BY day HAVING entry_count > 0 ORDER BY day",
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
            "SELECT CASE WHEN c.id IS NOT NULL THEN c.name ELSE COALESCE(NULLIF(t.category, ''), '') END AS category, SUM(CASE WHEN t.kind = 'income' THEN t.amount_cents ELSE 0 END) AS income_cents, SUM(CASE WHEN t.kind = 'expense' THEN t.amount_cents ELSE 0 END) AS expense_cents, SUM(CASE WHEN t.kind IN ('income', 'expense') THEN 1 ELSE 0 END) AS entry_count FROM ledger_transactions t LEFT JOIN categories c ON c.id = t.category_id WHERE t.user_id = ?1 AND t.archived_at IS NULL AND t.occurred_on >= ?2 AND t.occurred_on < ?3 AND t.pnl_scope = 'counted' GROUP BY c.id, CASE WHEN c.id IS NULL THEN NULLIF(t.category, '') END HAVING entry_count > 0 ORDER BY expense_cents DESC, category",
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
            "SELECT DISTINCT CASE WHEN c.id IS NOT NULL THEN c.name ELSE t.category END AS category_name FROM ledger_transactions t LEFT JOIN categories c ON c.id = t.category_id WHERE t.user_id = ?1 AND t.archived_at IS NULL AND (c.id IS NOT NULL OR t.category <> '') ORDER BY category_name",
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
    let account = ledger_account_brief(row, 10, 11, 12, 13)?;
    Ok(LedgerTransactionView {
        id: row.get(0)?,
        kind: TransactionKind::from_db(&kind)?,
        amount_cents: row.get(2)?,
        occurred_on: row.get(3)?,
        occurred_at: row.get(26)?,
        occurred_at_precision: row.get(27)?,
        currency: row.get(28)?,
        category: row.get(4)?,
        category_id: row.get(29)?,
        category_source: row.get(30)?,
        category_rule_id: row.get(33)?,
        category_rule_name: row.get(34)?,
        payee_name: row.get(23)?,
        payee_key: row.get(24)?,
        description: row.get(25)?,
        account,
        transfer_from_account: ledger_account_brief(row, 15, 16, 17, 18)?,
        transfer_to_account: ledger_account_brief(row, 19, 20, 21, 22)?,
        note: row.get(5)?,
        archived: archived_at.is_some(),
        version: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        pnl_scope: PnlScope::from_db(&row.get::<String>(31)?)?,
        created_by: row.get(32)?,
        links: serde_json::from_str::<Vec<TransactionLinkView>>(&row.get::<String>(14)?)
            .map_err(|_| ApiError::internal("transaction links are invalid"))?,
    })
}

async fn ensure_active_category(
    conn: &Connection,
    user_id: &str,
    category_id: &str,
    transaction_kind: &str,
) -> Result<(), ApiError> {
    let mut rows = conn
        .query(
            "SELECT kind,archived_at FROM categories WHERE id=?1 AND user_id=?2",
            params![category_id, user_id],
        )
        .await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| ApiError::validation("分类不存在"))?;
    if row.get::<Option<String>>(1)?.is_some() {
        return Err(ApiError::validation("不能使用已归档分类"));
    }
    if transaction_kind != "transfer" && row.get::<String>(0)? != transaction_kind {
        return Err(ApiError::validation("分类与交易的收支类型不一致"));
    }
    Ok(())
}

fn ledger_account_brief(
    row: &libsql::Row,
    id_index: i32,
    name_index: i32,
    type_index: i32,
    archived_index: i32,
) -> Result<Option<crate::domain::LedgerAccountBrief>, ApiError> {
    let Some(id) = row.get::<Option<String>>(id_index)? else {
        return Ok(None);
    };
    let account_type: String = row.get(type_index)?;
    Ok(Some(crate::domain::LedgerAccountBrief {
        id,
        name: row.get(name_index)?,
        account_type: AccountType::from_db(&account_type)?,
        archived: row.get::<Option<String>>(archived_index)?.is_some(),
    }))
}

fn validate_transaction_accounts(
    kind: TransactionKind,
    account_id: Option<&str>,
    transfer_from_account_id: Option<&str>,
    transfer_to_account_id: Option<&str>,
) -> Result<(), ApiError> {
    match kind {
        TransactionKind::Transfer => {
            if account_id.is_some() {
                return Err(ApiError::bad_request(
                    "validation_error",
                    "转账不能指定普通账户",
                ));
            }
            if transfer_from_account_id.is_none() && transfer_to_account_id.is_none() {
                return Err(ApiError::bad_request(
                    "validation_error",
                    "转账至少需要一个转出或转入账户",
                ));
            }
            if transfer_from_account_id.is_some()
                && transfer_from_account_id == transfer_to_account_id
            {
                return Err(ApiError::bad_request(
                    "validation_error",
                    "转出账户和转入账户不能相同",
                ));
            }
        }
        TransactionKind::Income | TransactionKind::Expense => {
            if transfer_from_account_id.is_some() || transfer_to_account_id.is_some() {
                return Err(ApiError::bad_request(
                    "validation_error",
                    "非转账交易不能指定转出或转入账户",
                ));
            }
        }
    }
    Ok(())
}
