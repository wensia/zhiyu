use std::collections::BTreeSet;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use libsql::{Connection, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    AppState,
    accounts::{ensure_active_ledger_account, ensure_active_ledger_account_if_present},
    auth::AuthUser,
    domain::{
        AccountType, CounterpartyBrief, CounterpartyView, CreateCounterpartyRequest,
        CreateDebtAdditionRequest, CreateDebtRequest, CreateRepaymentRequest, DashboardSummary,
        DebtAdditionEventView, DebtDirection, DebtListQuery, DebtListResponse, DebtOriginKind,
        DebtStatus, DebtView, LedgerAccountBrief, MAX_SAFE_CENTS, RepaymentEventView,
        ReverseRepaymentRequest, TransactionKind, TransactionLinkCandidate,
        TransactionLinkCandidatesQuery, UpdateCounterpartyRequest, UpdateDebtAdditionRequest,
        UpdateDebtRequest, UpdateRepaymentRequest, VersionRequest, debt_status, validate_amount,
        validate_date, validate_debt_origin, validate_note,
    },
    error::ApiError,
    idempotency::{idempotency_key, replay_idempotency, request_hash, store_idempotency},
    lifecycle::{DeletionHandleFuture, DeletionPrepareFuture, TransactionsDeleted},
    transactions::{
        NewTransactionRow, OnExternalConflict, TransactionPatch, archive_transaction_row,
        insert_transaction_row, update_transaction_row,
    },
};

#[utoipa::path(get, path = "/api/v1/debts", params(DebtListQuery), responses((status = 200, body = DebtListResponse)), security(("cookieAuth" = [])))]
pub async fn list_debts(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<DebtListQuery>,
) -> Result<Json<DebtListResponse>, ApiError> {
    let conn = state.connection().await?;
    reconcile_transaction_links(&conn, &user.id).await?;
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
    let start = (u64::from(page - 1) * u64::from(page_size)) as usize;
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
    reconcile_transaction_links(&conn, &user.id).await?;
    Ok(Json(load_debt(&conn, &user, &id, true).await?))
}

#[derive(Debug)]
struct ReconcileReference {
    source_kind: String,
    source_id: String,
    link_kind: String,
    debt_id: String,
    transaction_id: String,
    account_id: Option<String>,
    direction: String,
    currency: String,
    amount_cents: i64,
    occurred_on: String,
    event_kind: String,
    transaction_missing: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct DeletedDebtReference {
    source_kind: String,
    source_id: String,
    link_kind: String,
    debt_id: String,
    transaction_id: String,
    account_id: Option<String>,
    direction: String,
    currency: String,
    amount_cents: i64,
    occurred_on: String,
    event_kind: String,
}

#[derive(Debug)]
struct MissingTransactionLink {
    link_kind: String,
    ref_id: String,
    transaction_id: String,
    label: String,
}

/// Repairs the debts plugin's private references after core transaction archival/deletion.
///
/// Archived transactions are detached. Missing account-backed transactions are rebuilt, while
/// missing cashless references are cleared. Existing active transactions regain a missing link.
pub async fn reconcile_transaction_links(
    conn: &Connection,
    user_id: &str,
) -> Result<u64, ApiError> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    let mut rows = tx
        .query(
            "SELECT 'debts',d.id,'principal',d.id,d.account_id,d.direction,d.currency,d.principal_cents-COALESCE((SELECT SUM(a.amount_cents) FROM debt_addition_events a WHERE a.debt_id=d.id),0),d.occurred_on,'' FROM debts d WHERE d.user_id=?1 AND d.transaction_id IS NULL AND d.account_id IS NOT NULL UNION ALL SELECT 'debt_addition_events',e.id,'addition',e.debt_id,e.account_id,d.direction,d.currency,e.amount_cents,e.effective_on,'' FROM debt_addition_events e JOIN debts d ON d.id=e.debt_id AND d.user_id=e.user_id WHERE e.user_id=?1 AND e.transaction_id IS NULL AND e.account_id IS NOT NULL UNION ALL SELECT 'repayment_events',e.id,'repayment',e.debt_id,e.account_id,d.direction,d.currency,e.amount_cents,e.effective_on,e.kind FROM repayment_events e JOIN debts d ON d.id=e.debt_id AND d.user_id=e.user_id WHERE e.user_id=?1 AND e.transaction_id IS NULL AND e.account_id IS NOT NULL",
            [user_id],
        )
        .await?;
    let mut missing_transactions = Vec::new();
    while let Some(row) = rows.next().await? {
        missing_transactions.push(DeletedDebtReference {
            source_kind: row.get(0)?,
            source_id: row.get(1)?,
            link_kind: row.get(2)?,
            debt_id: row.get(3)?,
            transaction_id: String::new(),
            account_id: row.get(4)?,
            direction: row.get(5)?,
            currency: row.get(6)?,
            amount_cents: row.get(7)?,
            occurred_on: row.get(8)?,
            event_kind: row.get(9)?,
        });
    }
    drop(rows);
    for reference in &missing_transactions {
        repair_deleted_reference(&tx, user_id, reference, true).await?;
    }

    let mut rows = tx.query(
            "SELECT 'debts',d.id,'principal',d.id,d.transaction_id,d.account_id,d.direction,d.currency,d.principal_cents-COALESCE((SELECT SUM(a.amount_cents) FROM debt_addition_events a WHERE a.debt_id=d.id),0),d.occurred_on,'',CASE WHEN t.id IS NULL THEN 1 ELSE 0 END FROM debts d LEFT JOIN ledger_transactions t ON t.id=d.transaction_id AND t.user_id=d.user_id WHERE d.user_id=?1 AND d.transaction_id IS NOT NULL AND (t.id IS NULL OR t.archived_at IS NOT NULL) UNION ALL SELECT 'debt_addition_events',e.id,'addition',e.debt_id,e.transaction_id,e.account_id,d.direction,d.currency,e.amount_cents,e.effective_on,'',CASE WHEN t.id IS NULL THEN 1 ELSE 0 END FROM debt_addition_events e JOIN debts d ON d.id=e.debt_id AND d.user_id=e.user_id LEFT JOIN ledger_transactions t ON t.id=e.transaction_id AND t.user_id=e.user_id WHERE e.user_id=?1 AND e.transaction_id IS NOT NULL AND (t.id IS NULL OR t.archived_at IS NOT NULL) UNION ALL SELECT 'repayment_events',e.id,'repayment',e.debt_id,e.transaction_id,e.account_id,d.direction,d.currency,e.amount_cents,e.effective_on,e.kind,CASE WHEN t.id IS NULL THEN 1 ELSE 0 END FROM repayment_events e JOIN debts d ON d.id=e.debt_id AND d.user_id=e.user_id LEFT JOIN ledger_transactions t ON t.id=e.transaction_id AND t.user_id=e.user_id WHERE e.user_id=?1 AND e.transaction_id IS NOT NULL AND (t.id IS NULL OR t.archived_at IS NOT NULL)",
            [user_id],
        )
        .await?;
    let mut broken = Vec::new();
    while let Some(row) = rows.next().await? {
        broken.push(ReconcileReference {
            source_kind: row.get(0)?,
            source_id: row.get(1)?,
            link_kind: row.get(2)?,
            debt_id: row.get(3)?,
            transaction_id: row.get(4)?,
            account_id: row.get(5)?,
            direction: row.get(6)?,
            currency: row.get(7)?,
            amount_cents: row.get(8)?,
            occurred_on: row.get(9)?,
            event_kind: row.get(10)?,
            transaction_missing: row.get::<i64>(11)? != 0,
        });
    }
    drop(rows);

    let mut affected_transactions = BTreeSet::new();
    for reference in &broken {
        tx.execute(
            "DELETE FROM transaction_links WHERE user_id = ?1 AND transaction_id = ?2 AND plugin_id = 'debts' AND kind = ?3 AND ref_id = ?4",
            params![
                user_id,
                reference.transaction_id.clone(),
                reference.link_kind.clone(),
                reference.debt_id.clone()
            ],
        )
        .await?;
        let deleted_reference = DeletedDebtReference::from(reference);
        let changed = if reference.transaction_missing {
            repair_deleted_reference(&tx, user_id, &deleted_reference, false).await?
        } else {
            clear_deleted_reference(&tx, user_id, &deleted_reference, false).await?
        };
        if changed == 1 {
            affected_transactions.insert(reference.transaction_id.clone());
        }
    }

    for transaction_id in &affected_transactions {
        sync_transaction_pnl_scope(&tx, user_id, Some(transaction_id)).await?;
    }

    let mut rows = tx
        .query(
            "SELECT 'principal', d.id, d.transaction_id, c.display_name FROM debts d JOIN ledger_transactions t ON t.id = d.transaction_id AND t.user_id = d.user_id AND t.archived_at IS NULL JOIN counterparties c ON c.id = d.counterparty_id AND c.user_id = d.user_id WHERE d.user_id = ?1 AND NOT EXISTS (SELECT 1 FROM transaction_links l WHERE l.user_id = d.user_id AND l.transaction_id = d.transaction_id AND l.plugin_id = 'debts' AND l.kind = 'principal' AND l.ref_id = d.id) UNION ALL SELECT 'addition', e.debt_id, e.transaction_id, c.display_name FROM debt_addition_events e JOIN ledger_transactions t ON t.id = e.transaction_id AND t.user_id = e.user_id AND t.archived_at IS NULL JOIN debts d ON d.id = e.debt_id AND d.user_id = e.user_id JOIN counterparties c ON c.id = d.counterparty_id AND c.user_id = d.user_id WHERE e.user_id = ?1 AND NOT EXISTS (SELECT 1 FROM transaction_links l WHERE l.user_id = e.user_id AND l.transaction_id = e.transaction_id AND l.plugin_id = 'debts' AND l.kind = 'addition' AND l.ref_id = e.debt_id) UNION ALL SELECT 'repayment', e.debt_id, e.transaction_id, c.display_name FROM repayment_events e JOIN ledger_transactions t ON t.id = e.transaction_id AND t.user_id = e.user_id AND t.archived_at IS NULL JOIN debts d ON d.id = e.debt_id AND d.user_id = e.user_id JOIN counterparties c ON c.id = d.counterparty_id AND c.user_id = d.user_id WHERE e.user_id = ?1 AND NOT EXISTS (SELECT 1 FROM transaction_links l WHERE l.user_id = e.user_id AND l.transaction_id = e.transaction_id AND l.plugin_id = 'debts' AND l.kind = 'repayment' AND l.ref_id = e.debt_id)",
            [user_id],
        )
        .await?;
    let mut missing_links = Vec::new();
    while let Some(row) = rows.next().await? {
        missing_links.push(MissingTransactionLink {
            link_kind: row.get(0)?,
            ref_id: row.get(1)?,
            transaction_id: row.get(2)?,
            label: row.get(3)?,
        });
    }
    drop(rows);
    for link in &missing_links {
        tx.execute(
            "INSERT INTO transaction_links(id, user_id, transaction_id, plugin_id, kind, ref_id, label, created_at) VALUES (?1, ?2, ?3, 'debts', ?4, ?5, ?6, ?7)",
            params![
                Uuid::now_v7().to_string(),
                user_id,
                link.transaction_id.clone(),
                link.link_kind.clone(),
                link.ref_id.clone(),
                link.label.clone(),
                Utc::now().to_rfc3339()
            ],
        )
        .await?;
        sync_transaction_pnl_scope(&tx, user_id, Some(&link.transaction_id)).await?;
    }
    tx.commit().await?;
    Ok((missing_transactions.len() + broken.len() + missing_links.len()) as u64)
}

impl From<&ReconcileReference> for DeletedDebtReference {
    fn from(value: &ReconcileReference) -> Self {
        Self {
            source_kind: value.source_kind.clone(),
            source_id: value.source_id.clone(),
            link_kind: value.link_kind.clone(),
            debt_id: value.debt_id.clone(),
            transaction_id: value.transaction_id.clone(),
            account_id: value.account_id.clone(),
            direction: value.direction.clone(),
            currency: value.currency.clone(),
            amount_cents: value.amount_cents,
            occurred_on: value.occurred_on.clone(),
            event_kind: value.event_kind.clone(),
        }
    }
}

pub(crate) fn prepare_deleted_transactions<'a>(
    tx: &'a Transaction,
    event: &'a TransactionsDeleted,
) -> DeletionPrepareFuture<'a> {
    Box::pin(async move {
        let references = load_deleted_debt_references(tx, event).await?;
        serde_json::to_value(references).map_err(ApiError::internal)
    })
}

pub(crate) fn handle_deleted_transactions<'a>(
    tx: &'a Transaction,
    event: &'a TransactionsDeleted,
    prepared: &'a serde_json::Value,
) -> DeletionHandleFuture<'a> {
    Box::pin(async move {
        let references: Vec<DeletedDebtReference> =
            serde_json::from_value(prepared.clone()).map_err(ApiError::internal)?;
        for reference in references {
            if !event
                .transaction_ids
                .iter()
                .any(|id| id == &reference.transaction_id)
            {
                return Err(ApiError::internal(
                    "debt deletion snapshot contains an unexpected transaction",
                ));
            }
            repair_deleted_reference(tx, &event.user_id, &reference, true).await?;
        }
        Ok(())
    })
}

async fn load_deleted_debt_references(
    conn: &Connection,
    event: &TransactionsDeleted,
) -> Result<Vec<DeletedDebtReference>, ApiError> {
    if event.transaction_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = (0..event.transaction_ids.len())
        .map(|index| format!("?{}", index + 2))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT 'debts',d.id,'principal',d.id,d.transaction_id,d.account_id,d.direction,d.currency,d.principal_cents-COALESCE((SELECT SUM(a.amount_cents) FROM debt_addition_events a WHERE a.debt_id=d.id),0),d.occurred_on,'' FROM debts d WHERE d.user_id=?1 AND d.transaction_id IN ({placeholders}) UNION ALL SELECT 'debt_addition_events',e.id,'addition',e.debt_id,e.transaction_id,e.account_id,d.direction,d.currency,e.amount_cents,e.effective_on,'' FROM debt_addition_events e JOIN debts d ON d.id=e.debt_id AND d.user_id=e.user_id WHERE e.user_id=?1 AND e.transaction_id IN ({placeholders}) UNION ALL SELECT 'repayment_events',e.id,'repayment',e.debt_id,e.transaction_id,e.account_id,d.direction,d.currency,e.amount_cents,e.effective_on,e.kind FROM repayment_events e JOIN debts d ON d.id=e.debt_id AND d.user_id=e.user_id WHERE e.user_id=?1 AND e.transaction_id IN ({placeholders}) ORDER BY 1,2"
    );
    let mut values = Vec::with_capacity(event.transaction_ids.len() + 1);
    values.push(event.user_id.clone());
    values.extend(event.transaction_ids.iter().cloned());
    let mut rows = conn.query(&sql, values).await?;
    let mut references = Vec::new();
    while let Some(row) = rows.next().await? {
        references.push(DeletedDebtReference {
            source_kind: row.get(0)?,
            source_id: row.get(1)?,
            link_kind: row.get(2)?,
            debt_id: row.get(3)?,
            transaction_id: row.get(4)?,
            account_id: row.get(5)?,
            direction: row.get(6)?,
            currency: row.get(7)?,
            amount_cents: row.get(8)?,
            occurred_on: row.get(9)?,
            event_kind: row.get(10)?,
        });
    }
    Ok(references)
}

async fn repair_deleted_reference(
    conn: &Connection,
    user_id: &str,
    reference: &DeletedDebtReference,
    reference_was_cleared_by_delete: bool,
) -> Result<u64, ApiError> {
    let Some(account_id) = reference.account_id.as_deref() else {
        return clear_deleted_reference(conn, user_id, reference, reference_was_cleared_by_delete)
            .await;
    };
    let direction = DebtDirection::try_from(reference.direction.clone())?;
    let link_kind = reference_link_kind(reference)?;
    let transaction_id = create_auto_transaction(
        conn,
        user_id,
        &direction,
        link_kind,
        reference.amount_cents,
        &reference.currency,
        &reference.occurred_on,
        account_id,
    )
    .await?;
    let changed = update_deleted_reference(
        conn,
        user_id,
        reference,
        Some(&transaction_id),
        true,
        reference_was_cleared_by_delete,
    )
    .await?;
    if changed != 1 {
        return Err(ApiError::internal(
            "debt deletion repair affected unexpected row count",
        ));
    }
    sync_debt_transaction_link(
        conn,
        user_id,
        &reference.debt_id,
        link_kind,
        None,
        link_kind,
        Some(&transaction_id),
    )
    .await?;
    sync_transaction_pnl_scope(conn, user_id, Some(&transaction_id)).await?;
    Ok(changed)
}

async fn clear_deleted_reference(
    conn: &Connection,
    user_id: &str,
    reference: &DeletedDebtReference,
    reference_was_cleared_by_delete: bool,
) -> Result<u64, ApiError> {
    update_deleted_reference(
        conn,
        user_id,
        reference,
        None,
        false,
        reference_was_cleared_by_delete,
    )
    .await
}

async fn update_deleted_reference(
    conn: &Connection,
    user_id: &str,
    reference: &DeletedDebtReference,
    transaction_id: Option<&str>,
    transaction_auto_created: bool,
    reference_was_cleared_by_delete: bool,
) -> Result<u64, ApiError> {
    let table = match reference.source_kind.as_str() {
        "debts" => "debts",
        "debt_addition_events" => "debt_addition_events",
        "repayment_events" => "repayment_events",
        _ => return Err(ApiError::internal("债务插件自检遇到未知引用类型")),
    };
    if reference_was_cleared_by_delete {
        Ok(conn
            .execute(
                &format!(
                    "UPDATE {table} SET transaction_id=?1,transaction_auto_created=?2 WHERE id=?3 AND user_id=?4 AND transaction_id IS NULL"
                ),
                params![
                    transaction_id,
                    i64::from(transaction_auto_created),
                    reference.source_id.clone(),
                    user_id
                ],
            )
            .await?)
    } else {
        Ok(conn
            .execute(
                &format!(
                    "UPDATE {table} SET transaction_id=?1,transaction_auto_created=?2 WHERE id=?3 AND user_id=?4 AND transaction_id=?5"
                ),
                params![
                    transaction_id,
                    i64::from(transaction_auto_created),
                    reference.source_id.clone(),
                    user_id,
                    reference.transaction_id.clone()
                ],
            )
            .await?)
    }
}

fn reference_link_kind(reference: &DeletedDebtReference) -> Result<LinkKind, ApiError> {
    match reference.link_kind.as_str() {
        "principal" => Ok(LinkKind::Principal),
        "addition" => Ok(LinkKind::Addition),
        "repayment" if reference.event_kind == "reversal" => Ok(LinkKind::Reversal),
        "repayment" => Ok(LinkKind::Repayment),
        _ => Err(ApiError::internal("债务插件删除处理遇到未知流水关联类型")),
    }
}

pub async fn reconcile_all_transaction_links(conn: &Connection) -> Result<u64, ApiError> {
    let mut rows = conn.query("SELECT id FROM users ORDER BY id", ()).await?;
    let mut user_ids = Vec::new();
    while let Some(row) = rows.next().await? {
        user_ids.push(row.get::<String>(0)?);
    }
    drop(rows);
    let mut repaired = 0;
    for user_id in user_ids {
        repaired += reconcile_transaction_links(conn, &user_id).await?;
    }
    Ok(repaired)
}

#[utoipa::path(get, path = "/api/v1/debts/{id}/link-candidates", params(("id" = String, Path), TransactionLinkCandidatesQuery), responses((status = 200, body = [TransactionLinkCandidate])), security(("cookieAuth" = [])))]
pub async fn list_link_candidates(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Query(query): Query<TransactionLinkCandidatesQuery>,
) -> Result<Json<Vec<TransactionLinkCandidate>>, ApiError> {
    if let Some(amount) = query.amount_cents {
        validate_amount(amount)?;
    }
    let conn = state.connection().await?;
    let debt = load_debt(&conn, &user, &id, false).await?;
    let mut rows = conn.query(
        "SELECT t.id, t.kind, t.amount_cents, t.occurred_on, t.note, a.id, a.name, a.account_type, a.archived_at FROM ledger_transactions t LEFT JOIN ledger_accounts a ON a.id = t.account_id AND a.user_id = t.user_id WHERE t.user_id = ?1 AND t.archived_at IS NULL AND t.kind <> 'transfer' AND NOT EXISTS (SELECT 1 FROM transaction_links l WHERE l.user_id = t.user_id AND l.transaction_id = t.id AND l.plugin_id = 'debts') AND (?2 IS NULL OR t.amount_cents = ?2)",
        params![user.id.clone(), query.amount_cents],
    ).await?;
    let counterparty = debt.counterparty.display_name.to_lowercase();
    let mut candidates = Vec::new();
    while let Some(row) = rows.next().await? {
        let note: String = row.get(4)?;
        let occurred_on: String = row.get(3)?;
        let amount_cents: i64 = row.get(2)?;
        let name_match = note.to_lowercase().contains(&counterparty);
        candidates.push((
            (name_match, occurred_on.clone()),
            TransactionLinkCandidate {
                id: row.get(0)?,
                kind: TransactionKind::from_db(&row.get::<String>(1)?)?,
                amount_cents,
                occurred_on,
                note,
                account: ledger_account_brief(&row, 5, 6, 7, 8)?,
            },
        ));
    }
    // 联系人匹配优先，其余一律时间从新到旧——金额只做过滤不做排序，
    // 否则表单未填金额时权重无的放矢，产出伪随机序（用户实测反馈）。
    candidates.sort_by(|left, right| {
        right
            .0
            .0
            .cmp(&left.0.0)
            .then_with(|| right.0.1.cmp(&left.0.1))
    });
    Ok(Json(
        candidates
            .into_iter()
            .take(20)
            .map(|(_, item)| item)
            .collect(),
    ))
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
    let account_id = if let Some(transaction_id) = input.transaction_id.as_deref() {
        if origin_kind != DebtOriginKind::CashMovement {
            return Err(ApiError::validation(
                "只有实际发生资金往来的债务才能关联流水",
            ));
        }
        validate_transaction_link(
            &tx,
            &user.id,
            transaction_id,
            input.principal_cents,
            &input.occurred_on,
            expected_transaction_kind(&input.direction, LinkKind::Principal),
            None,
        )
        .await?
    } else {
        input.account_id.clone()
    };
    if input.transaction_id.is_none() {
        validate_debt_origin(origin_kind, account_id.as_deref(), None)?;
    }
    ensure_active_ledger_account_if_present(&tx, &user.id, account_id.as_deref()).await?;

    let counterparty_id = if let Some(id) = input.counterparty_id.as_deref() {
        ensure_counterparty(&tx, &user.id, id).await?;
        id.to_owned()
    } else {
        let name = validate_name(input.counterparty_name.as_deref().unwrap_or_default())?;
        create_counterparty_row(&tx, &user.id, &name, "").await?
    };

    let transaction_auto_created = input.transaction_id.is_none()
        && origin_kind == DebtOriginKind::CashMovement
        && account_id.is_some();
    let transaction_id = if transaction_auto_created {
        Some(
            create_auto_transaction(
                &tx,
                &user.id,
                &input.direction,
                LinkKind::Principal,
                input.principal_cents,
                "CNY",
                &input.occurred_on,
                account_id
                    .as_deref()
                    .expect("cash movement account was validated"),
            )
            .await?,
        )
    } else {
        input.transaction_id.clone()
    };
    let id = Uuid::now_v7().to_string();
    let now = Utc::now().to_rfc3339();
    tx.execute(
        "INSERT INTO debts(id, user_id, counterparty_id, direction, principal_cents, currency, occurred_on, due_on, note, created_at, updated_at, account_id, origin_kind, transaction_id, transaction_auto_created) VALUES (?1, ?2, ?3, ?4, ?5, 'CNY', ?6, ?7, ?8, ?9, ?9, ?10, ?11, ?12, ?13)",
        params![id.clone(), user.id.clone(), counterparty_id, input.direction.as_str(), input.principal_cents, input.occurred_on, input.due_on, input.note.trim(), now, account_id, origin_kind.as_str(), transaction_id.clone(), i64::from(transaction_auto_created)],
    ).await?;
    sync_debt_transaction_link(
        &tx,
        &user.id,
        &id,
        LinkKind::Principal,
        None,
        LinkKind::Principal,
        transaction_id.as_deref(),
    )
    .await?;
    sync_transaction_pnl_scope(&tx, &user.id, transaction_id.as_deref()).await?;
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
        "SELECT d.principal_cents, b.event_count, d.origin_kind, d.account_id, d.transaction_id, d.transaction_auto_created, d.direction, d.currency, COALESCE((SELECT SUM(a.amount_cents) FROM debt_addition_events a WHERE a.debt_id = d.id), 0) FROM debts d JOIN debt_balances b ON b.debt_id = d.id WHERE d.id = ?1 AND d.user_id = ?2",
        params![id.clone(), user.id.clone()],
    ).await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| ApiError::not_found("找不到该笔债务"))?;
    let existing_principal: i64 = row.get(0)?;
    let event_count: i64 = row.get(1)?;
    let existing_origin: String = row.get(2)?;
    let existing_account_id: Option<String> = row.get(3)?;
    let existing_transaction_id: Option<String> = row.get(4)?;
    let existing_transaction_auto_created: i64 = row.get(5)?;
    let direction = DebtDirection::try_from(row.get::<String>(6)?)?;
    let currency: String = row.get(7)?;
    let addition_total: i64 = row.get(8)?;
    drop(rows);
    let existing_origin = DebtOriginKind::from_db(&existing_origin)?;
    // 旧客户端不传 originKind：给了账户视为有实际收付款，否则保持原类型
    let origin_kind = input.origin_kind.unwrap_or(if input.account_id.is_some() {
        DebtOriginKind::CashMovement
    } else {
        existing_origin
    });
    let principal_transaction_amount = input
        .principal_cents
        .checked_sub(addition_total)
        .filter(|amount| *amount > 0)
        .ok_or_else(|| ApiError::validation("债务本金流水金额必须大于零"))?;
    let requested_transaction_id = input.transaction_id.clone();
    let mut transaction_auto_created = existing_transaction_auto_created != 0;
    let mut transaction_id = existing_transaction_id.clone();
    let account_id = match requested_transaction_id.as_ref() {
        Some(Some(requested_id)) => {
            let linked_account_id = validate_transaction_link(
                &tx,
                &user.id,
                requested_id,
                principal_transaction_amount,
                &input.occurred_on,
                expected_transaction_kind(&direction, LinkKind::Principal),
                Some(("principal", &id)),
            )
            .await?;
            if transaction_auto_created
                && existing_transaction_id.as_deref() != Some(requested_id.as_str())
                && let Some(existing_id) = existing_transaction_id.as_deref()
            {
                archive_auto_transaction(&tx, existing_id, &user.id).await?;
            }
            transaction_id = Some(requested_id.clone());
            transaction_auto_created = false;
            linked_account_id
        }
        Some(None) | None if transaction_auto_created => {
            let desired_account_id = input.account_id.clone();
            if let (Some(existing_id), Some(account_id)) = (
                existing_transaction_id.as_deref(),
                desired_account_id.as_deref(),
            ) {
                update_auto_transaction(
                    &tx,
                    existing_id,
                    &user.id,
                    &direction,
                    LinkKind::Principal,
                    principal_transaction_amount,
                    &currency,
                    &input.occurred_on,
                    account_id,
                )
                .await?;
                transaction_id = Some(existing_id.to_owned());
            } else if let Some(existing_id) = existing_transaction_id.as_deref() {
                archive_auto_transaction(&tx, existing_id, &user.id).await?;
                transaction_id = None;
                transaction_auto_created = false;
            }
            desired_account_id
        }
        Some(None) => {
            let desired_account_id = input.account_id.clone();
            if let Some(account_id) = desired_account_id.as_deref() {
                transaction_id = Some(
                    create_auto_transaction(
                        &tx,
                        &user.id,
                        &direction,
                        LinkKind::Principal,
                        principal_transaction_amount,
                        &currency,
                        &input.occurred_on,
                        account_id,
                    )
                    .await?,
                );
                transaction_auto_created = true;
            } else {
                transaction_id = None;
            }
            desired_account_id
        }
        None if transaction_id.is_none() && input.account_id.is_some() => {
            let desired_account_id = input.account_id.clone();
            transaction_id = Some(
                create_auto_transaction(
                    &tx,
                    &user.id,
                    &direction,
                    LinkKind::Principal,
                    principal_transaction_amount,
                    &currency,
                    &input.occurred_on,
                    desired_account_id
                        .as_deref()
                        .expect("account presence was checked"),
                )
                .await?,
            );
            transaction_auto_created = true;
            desired_account_id
        }
        None => input.account_id.clone().or(existing_account_id),
    };
    validate_debt_origin(origin_kind, account_id.as_deref(), Some(existing_origin))?;
    ensure_active_ledger_account_if_present(&tx, &user.id, account_id.as_deref()).await?;
    if event_count > 0 && existing_principal != input.principal_cents {
        return Err(ApiError::conflict(
            "principal_locked",
            "已有还款或追加记录，本金不可修改",
        ));
    }
    let changed = tx.execute(
        "UPDATE debts SET counterparty_id = ?1, account_id = ?2, origin_kind = ?3, principal_cents = ?4, occurred_on = ?5, due_on = ?6, note = ?7, transaction_id = ?8, transaction_auto_created = ?9, version = version + 1, updated_at = ?10 WHERE id = ?11 AND user_id = ?12 AND version = ?13",
        params![input.counterparty_id, account_id, origin_kind.as_str(), input.principal_cents, input.occurred_on, input.due_on, input.note.trim(), transaction_id.clone(), i64::from(transaction_auto_created), Utc::now().to_rfc3339(), id.clone(), user.id.clone(), input.version],
    ).await?;
    if changed == 0 {
        return Err(ApiError::conflict(
            "version_conflict",
            "记录已在其他设备更新，请刷新后重试",
        ));
    }
    sync_debt_transaction_link(
        &tx,
        &user.id,
        &id,
        LinkKind::Principal,
        existing_transaction_id.as_deref(),
        LinkKind::Principal,
        transaction_id.as_deref(),
    )
    .await?;
    refresh_debt_transaction_link_labels(&tx, &user.id, &id).await?;
    sync_changed_transaction_pnl_scopes(
        &tx,
        &user.id,
        existing_transaction_id.as_deref(),
        transaction_id.as_deref(),
    )
    .await?;
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
        "SELECT b.event_count, d.transaction_id, d.transaction_auto_created FROM debts d JOIN debt_balances b ON b.debt_id = d.id WHERE d.id = ?1 AND d.user_id = ?2",
        params![id.clone(), user.id.clone()],
    ).await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| ApiError::not_found("找不到该笔债务"))?;
    let event_count: i64 = row.get(0)?;
    let transaction_id: Option<String> = row.get(1)?;
    let transaction_auto_created: i64 = row.get(2)?;
    drop(rows);
    if event_count > 0 {
        return Err(ApiError::conflict(
            "debt_has_history",
            "已有还款或追加历史，只能归档",
        ));
    }
    if transaction_auto_created != 0
        && let Some(transaction_id) = transaction_id.as_deref()
    {
        archive_auto_transaction(&tx, transaction_id, &user.id).await?;
    }
    let changed = tx
        .execute(
            "DELETE FROM debts WHERE id = ?1 AND user_id = ?2 AND version = ?3",
            params![id.clone(), user.id.clone(), input.version],
        )
        .await?;
    if changed == 0 {
        return Err(ApiError::conflict(
            "version_conflict",
            "记录已变化，请刷新后重试",
        ));
    }
    sync_debt_transaction_link(
        &tx,
        &user.id,
        &id,
        LinkKind::Principal,
        transaction_id.as_deref(),
        LinkKind::Principal,
        None,
    )
    .await?;
    sync_transaction_pnl_scope(&tx, &user.id, transaction_id.as_deref()).await?;
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
    let account_id = if let Some(transaction_id) = input.transaction_id.as_deref() {
        validate_transaction_link(
            &tx,
            &user.id,
            transaction_id,
            input.amount_cents,
            &input.effective_on,
            expected_transaction_kind(
                &DebtDirection::try_from(debt.direction.clone())?,
                LinkKind::Repayment,
            ),
            None,
        )
        .await?
    } else {
        input.account_id.clone()
    };
    ensure_active_ledger_account_if_present(&tx, &user.id, account_id.as_deref()).await?;
    if input.amount_cents > debt.remaining_cents {
        return Err(ApiError::conflict(
            "overpayment",
            "还款金额不能超过剩余金额",
        ));
    }
    let direction = DebtDirection::try_from(debt.direction.clone())?;
    let transaction_auto_created = input.transaction_id.is_none() && account_id.is_some();
    let transaction_id = if transaction_auto_created {
        Some(
            create_auto_transaction(
                &tx,
                &user.id,
                &direction,
                LinkKind::Repayment,
                input.amount_cents,
                &debt.currency,
                &input.effective_on,
                account_id.as_deref().expect("repayment account is present"),
            )
            .await?,
        )
    } else {
        input.transaction_id.clone()
    };
    tx.execute(
        "INSERT INTO repayment_events(id, user_id, debt_id, kind, amount_cents, effective_on, note, created_at, account_id, transaction_id, transaction_auto_created) VALUES (?1, ?2, ?3, 'payment', ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![Uuid::now_v7().to_string(), user.id.clone(), id.clone(), input.amount_cents, input.effective_on, input.note.trim(), Utc::now().to_rfc3339(), account_id, transaction_id.clone(), i64::from(transaction_auto_created)],
    ).await?;
    sync_debt_transaction_link(
        &tx,
        &user.id,
        &id,
        LinkKind::Repayment,
        None,
        LinkKind::Repayment,
        transaction_id.as_deref(),
    )
    .await?;
    sync_transaction_pnl_scope(&tx, &user.id, transaction_id.as_deref()).await?;
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
    let account_id = if let Some(transaction_id) = input.transaction_id.as_deref() {
        validate_transaction_link(
            &tx,
            &user.id,
            transaction_id,
            input.amount_cents,
            &input.effective_on,
            expected_transaction_kind(
                &DebtDirection::try_from(debt.direction.clone())?,
                LinkKind::Addition,
            ),
            None,
        )
        .await?
    } else {
        input.account_id.clone()
    };
    ensure_active_ledger_account_if_present(&tx, &user.id, account_id.as_deref()).await?;
    let principal_cents = debt
        .principal_cents
        .checked_add(input.amount_cents)
        .filter(|value| *value <= MAX_SAFE_CENTS)
        .ok_or_else(|| ApiError::validation("追加后本金超出安全范围"))?;
    let event_id = Uuid::now_v7().to_string();
    let now = Utc::now().to_rfc3339();
    let direction = DebtDirection::try_from(debt.direction.clone())?;
    let transaction_auto_created = input.transaction_id.is_none() && account_id.is_some();
    let transaction_id = if transaction_auto_created {
        Some(
            create_auto_transaction(
                &tx,
                &user.id,
                &direction,
                LinkKind::Addition,
                input.amount_cents,
                &debt.currency,
                &input.effective_on,
                account_id.as_deref().expect("addition account is present"),
            )
            .await?,
        )
    } else {
        input.transaction_id.clone()
    };
    tx.execute(
        "INSERT INTO debt_addition_events(id, user_id, debt_id, amount_cents, effective_on, note, created_at, account_id, transaction_id, transaction_auto_created) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![event_id, user.id.clone(), id.clone(), input.amount_cents, input.effective_on, input.note.trim(), now.clone(), account_id, transaction_id.clone(), i64::from(transaction_auto_created)],
    ).await?;
    sync_debt_transaction_link(
        &tx,
        &user.id,
        &id,
        LinkKind::Addition,
        None,
        LinkKind::Addition,
        transaction_id.as_deref(),
    )
    .await?;
    sync_transaction_pnl_scope(&tx, &user.id, transaction_id.as_deref()).await?;
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
        "SELECT e.debt_id, e.amount_cents, d.principal_cents, d.archived_at, b.paid_cents, e.account_id, e.transaction_id, d.direction, e.transaction_auto_created, d.currency FROM debt_addition_events e JOIN debts d ON d.id = e.debt_id AND d.user_id = e.user_id JOIN debt_balances b ON b.debt_id = d.id WHERE e.id = ?1 AND e.user_id = ?2",
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
    let current_transaction_id: Option<String> = row.get(6)?;
    let direction = DebtDirection::try_from(row.get::<String>(7)?)?;
    let current_transaction_auto_created = row.get::<i64>(8)? != 0;
    let currency: String = row.get(9)?;
    drop(rows);
    if archived_at.is_some() {
        return Err(ApiError::conflict(
            "debt_archived",
            "归档债务不能编辑追加记录",
        ));
    }
    let link_kind = if converts_to_repayment {
        LinkKind::Repayment
    } else {
        LinkKind::Addition
    };
    let (account_id, transaction_id, transaction_auto_created) = resolve_event_transaction(
        &tx,
        &user.id,
        &direction,
        link_kind,
        input.amount_cents,
        &currency,
        &input.effective_on,
        &input.transaction_id,
        input.account_id.clone(),
        current_transaction_id.clone(),
        current_account_id.clone(),
        current_transaction_auto_created,
        ("addition", &debt_id),
    )
    .await?;
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
            "INSERT INTO repayment_events(id, user_id, debt_id, kind, amount_cents, effective_on, note, created_at, account_id, transaction_id, transaction_auto_created) VALUES (?1, ?2, ?3, 'payment', ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![id.clone(), user.id.clone(), debt_id.clone(), input.amount_cents, input.effective_on, input.note.trim(), Utc::now().to_rfc3339(), account_id, transaction_id.clone(), i64::from(transaction_auto_created)],
        ).await?;
    } else {
        tx.execute(
            "UPDATE debt_addition_events SET amount_cents = ?1, effective_on = ?2, note = ?3, account_id = ?4, transaction_id = ?5, transaction_auto_created = ?6 WHERE id = ?7 AND user_id = ?8",
            params![input.amount_cents, input.effective_on, input.note.trim(), account_id, transaction_id.clone(), i64::from(transaction_auto_created), id.clone(), user.id.clone()],
        ).await?;
    }
    sync_debt_transaction_link(
        &tx,
        &user.id,
        &debt_id,
        LinkKind::Addition,
        current_transaction_id.as_deref(),
        link_kind,
        transaction_id.as_deref(),
    )
    .await?;
    sync_changed_transaction_pnl_scopes(
        &tx,
        &user.id,
        current_transaction_id.as_deref(),
        transaction_id.as_deref(),
    )
    .await?;
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
        "SELECT e.debt_id, e.amount_cents, d.principal_cents, d.archived_at, b.remaining_cents, EXISTS(SELECT 1 FROM repayment_events reversal WHERE reversal.reverses_event_id = e.id), e.account_id, e.transaction_id, d.direction, e.transaction_auto_created, d.currency FROM repayment_events e JOIN debts d ON d.id = e.debt_id AND d.user_id = e.user_id JOIN debt_balances b ON b.debt_id = d.id WHERE e.id = ?1 AND e.user_id = ?2 AND e.kind = 'payment'",
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
    let current_transaction_id: Option<String> = row.get(7)?;
    let direction = DebtDirection::try_from(row.get::<String>(8)?)?;
    let current_transaction_auto_created = row.get::<i64>(9)? != 0;
    let currency: String = row.get(10)?;
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
    let link_kind = if converts_to_addition {
        LinkKind::Addition
    } else {
        LinkKind::Repayment
    };
    let (account_id, transaction_id, transaction_auto_created) = resolve_event_transaction(
        &tx,
        &user.id,
        &direction,
        link_kind,
        input.amount_cents,
        &currency,
        &input.effective_on,
        &input.transaction_id,
        input.account_id.clone(),
        current_transaction_id.clone(),
        current_account_id.clone(),
        current_transaction_auto_created,
        ("repayment", &debt_id),
    )
    .await?;
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
            "INSERT INTO debt_addition_events(id, user_id, debt_id, amount_cents, effective_on, note, created_at, account_id, transaction_id, transaction_auto_created) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![id.clone(), user.id.clone(), debt_id.clone(), input.amount_cents, input.effective_on, input.note.trim(), Utc::now().to_rfc3339(), account_id, transaction_id.clone(), i64::from(transaction_auto_created)],
        ).await?;
    } else {
        tx.execute(
            "UPDATE repayment_events SET amount_cents = ?1, effective_on = ?2, note = ?3, account_id = ?4, transaction_id = ?5, transaction_auto_created = ?6 WHERE id = ?7 AND user_id = ?8 AND kind = 'payment'",
            params![input.amount_cents, input.effective_on, input.note.trim(), account_id, transaction_id.clone(), i64::from(transaction_auto_created), id.clone(), user.id.clone()],
        ).await?;
    }
    sync_debt_transaction_link(
        &tx,
        &user.id,
        &debt_id,
        LinkKind::Repayment,
        current_transaction_id.as_deref(),
        link_kind,
        transaction_id.as_deref(),
    )
    .await?;
    sync_changed_transaction_pnl_scopes(
        &tx,
        &user.id,
        current_transaction_id.as_deref(),
        transaction_id.as_deref(),
    )
    .await?;
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
        "SELECT e.debt_id, e.amount_cents, d.archived_at, EXISTS(SELECT 1 FROM repayment_events r WHERE r.reverses_event_id = e.id), e.account_id, d.direction, d.currency FROM repayment_events e JOIN debts d ON d.id = e.debt_id WHERE e.id = ?1 AND e.user_id = ?2 AND e.kind = 'payment'",
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
    let direction = DebtDirection::try_from(row.get::<String>(5)?)?;
    let currency: String = row.get(6)?;
    drop(rows);
    if archived_at.is_some() {
        return Err(ApiError::conflict("debt_archived", "归档债务不能撤销还款"));
    }
    if already_reversed != 0 {
        return Err(ApiError::conflict("already_reversed", "该还款已撤销"));
    }
    let transaction_auto_created = account_id.is_some();
    let transaction_id = if let Some(account_id) = account_id.as_deref() {
        Some(
            create_auto_transaction(
                &tx,
                &user.id,
                &direction,
                LinkKind::Reversal,
                amount_cents,
                &currency,
                &input.effective_on,
                account_id,
            )
            .await?,
        )
    } else {
        None
    };
    tx.execute(
        "INSERT INTO repayment_events(id, user_id, debt_id, kind, amount_cents, effective_on, note, reverses_event_id, created_at, account_id, transaction_id, transaction_auto_created) VALUES (?1, ?2, ?3, 'reversal', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![Uuid::now_v7().to_string(), user.id.clone(), debt_id.clone(), amount_cents, input.effective_on, input.note.trim(), id, Utc::now().to_rfc3339(), account_id, transaction_id.clone(), i64::from(transaction_auto_created)],
    ).await?;
    sync_debt_transaction_link(
        &tx,
        &user.id,
        &debt_id,
        LinkKind::Repayment,
        None,
        LinkKind::Repayment,
        transaction_id.as_deref(),
    )
    .await?;
    sync_transaction_pnl_scope(&tx, &user.id, transaction_id.as_deref()).await?;
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
    tx.execute(
        "UPDATE transaction_links SET label = ?1 WHERE user_id = ?2 AND plugin_id = 'debts' AND ref_id IN (SELECT d.id FROM debts d WHERE d.user_id = ?2 AND d.counterparty_id = ?3)",
        params![name, user.id.clone(), id.clone()],
    )
    .await?;
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

#[derive(Clone, Copy)]
enum LinkKind {
    Principal,
    Addition,
    Repayment,
    Reversal,
}

impl LinkKind {
    fn transaction_link_kind(self) -> &'static str {
        match self {
            Self::Principal => "principal",
            Self::Addition => "addition",
            Self::Repayment | Self::Reversal => "repayment",
        }
    }
}

fn expected_transaction_kind(direction: &DebtDirection, link_kind: LinkKind) -> TransactionKind {
    match (direction, link_kind) {
        (DebtDirection::LendOut, LinkKind::Repayment)
        | (DebtDirection::BorrowIn, LinkKind::Reversal)
        | (DebtDirection::BorrowIn, LinkKind::Addition)
        | (DebtDirection::BorrowIn, LinkKind::Principal) => TransactionKind::Income,
        (DebtDirection::BorrowIn, LinkKind::Repayment)
        | (DebtDirection::LendOut, LinkKind::Reversal)
        | (DebtDirection::LendOut, LinkKind::Addition)
        | (DebtDirection::LendOut, LinkKind::Principal) => TransactionKind::Expense,
    }
}

fn auto_transaction_description(direction: &DebtDirection, link_kind: LinkKind) -> &'static str {
    match (direction, link_kind) {
        (DebtDirection::BorrowIn, LinkKind::Principal) => "债务本金（借入）",
        (DebtDirection::LendOut, LinkKind::Principal) => "债务本金（借出）",
        (DebtDirection::BorrowIn, LinkKind::Addition) => "追加借款（借入）",
        (DebtDirection::LendOut, LinkKind::Addition) => "追加借款（借出）",
        (DebtDirection::BorrowIn, LinkKind::Repayment) => "还款（借入）",
        (DebtDirection::LendOut, LinkKind::Repayment) => "还款（借出）",
        (DebtDirection::BorrowIn, LinkKind::Reversal) => "撤销还款（借入）",
        (DebtDirection::LendOut, LinkKind::Reversal) => "撤销还款（借出）",
    }
}

#[allow(clippy::too_many_arguments)]
async fn create_auto_transaction(
    conn: &Connection,
    user_id: &str,
    direction: &DebtDirection,
    link_kind: LinkKind,
    amount_cents: i64,
    currency: &str,
    occurred_on: &str,
    account_id: &str,
) -> Result<String, ApiError> {
    let id = Uuid::now_v7().to_string();
    let now = Utc::now().to_rfc3339();
    let description = auto_transaction_description(direction, link_kind);
    insert_transaction_row(
        conn,
        NewTransactionRow {
            id: id.clone(),
            user_id: user_id.to_owned(),
            kind: expected_transaction_kind(direction, link_kind)
                .as_str()
                .to_owned(),
            amount_cents,
            currency: currency.to_owned(),
            occurred_on: occurred_on.to_owned(),
            occurred_at: None,
            occurred_at_precision: "day".to_owned(),
            category: String::new(),
            category_id: None,
            category_source: "none".to_owned(),
            category_rule_id: None,
            payee_name: String::new(),
            payee_key: String::new(),
            description: description.to_owned(),
            account_id: Some(account_id.to_owned()),
            transfer_from_account_id: None,
            transfer_to_account_id: None,
            note: description.to_owned(),
            archived_at: None,
            version: 1,
            created_at: now.clone(),
            updated_at: now,
            source_channel: String::new(),
            external_id: String::new(),
            import_batch_id: None,
            event_id: None,
            pnl_scope: "excluded".to_owned(),
            created_by: "plugin:debts".to_owned(),
            on_external_conflict: OnExternalConflict::Error,
        },
    )
    .await?;
    Ok(id)
}

#[allow(clippy::too_many_arguments)]
async fn update_auto_transaction(
    conn: &Connection,
    transaction_id: &str,
    user_id: &str,
    direction: &DebtDirection,
    link_kind: LinkKind,
    amount_cents: i64,
    currency: &str,
    occurred_on: &str,
    account_id: &str,
) -> Result<(), ApiError> {
    let description = auto_transaction_description(direction, link_kind);
    let changed = update_transaction_row(
        conn,
        user_id,
        transaction_id,
        TransactionPatch::ReplaceStandardFields {
            kind: expected_transaction_kind(direction, link_kind)
                .as_str()
                .to_owned(),
            amount_cents,
            currency: currency.to_owned(),
            occurred_on: occurred_on.to_owned(),
            description: description.to_owned(),
            account_id: account_id.to_owned(),
            updated_at: Utc::now().to_rfc3339(),
        },
    )
    .await?;
    if changed == 0 {
        return Err(ApiError::internal("自动创建的债务流水不存在或已归档"));
    }
    Ok(())
}

async fn archive_auto_transaction(
    conn: &Connection,
    transaction_id: &str,
    user_id: &str,
) -> Result<(), ApiError> {
    archive_transaction_row(conn, user_id, transaction_id).await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn sync_debt_transaction_link(
    conn: &Connection,
    user_id: &str,
    debt_id: &str,
    previous_kind: LinkKind,
    previous_transaction_id: Option<&str>,
    kind: LinkKind,
    transaction_id: Option<&str>,
) -> Result<(), ApiError> {
    if let Some(previous_transaction_id) = previous_transaction_id {
        conn.execute(
            "DELETE FROM transaction_links WHERE user_id = ?1 AND transaction_id = ?2 AND plugin_id = 'debts' AND kind = ?3 AND ref_id = ?4",
            params![
                user_id,
                previous_transaction_id,
                previous_kind.transaction_link_kind(),
                debt_id
            ],
        )
        .await?;
    }
    let Some(transaction_id) = transaction_id else {
        return Ok(());
    };
    let changed = conn
        .execute(
            "INSERT INTO transaction_links(id, user_id, transaction_id, plugin_id, kind, ref_id, label, created_at) SELECT ?1, d.user_id, ?2, 'debts', ?3, d.id, c.display_name, ?4 FROM debts d JOIN counterparties c ON c.id = d.counterparty_id AND c.user_id = d.user_id WHERE d.id = ?5 AND d.user_id = ?6 ON CONFLICT(transaction_id, plugin_id, kind, ref_id) DO UPDATE SET label = excluded.label",
            params![
                Uuid::now_v7().to_string(),
                transaction_id,
                kind.transaction_link_kind(),
                Utc::now().to_rfc3339(),
                debt_id,
                user_id
            ],
        )
        .await?;
    if changed == 0 {
        return Err(ApiError::internal("债务关联缺少对应债务或联系人"));
    }
    Ok(())
}

async fn refresh_debt_transaction_link_labels(
    conn: &Connection,
    user_id: &str,
    debt_id: &str,
) -> Result<(), ApiError> {
    conn.execute(
        "UPDATE transaction_links SET label = (SELECT c.display_name FROM debts d JOIN counterparties c ON c.id = d.counterparty_id AND c.user_id = d.user_id WHERE d.id = transaction_links.ref_id AND d.user_id = transaction_links.user_id) WHERE user_id = ?1 AND plugin_id = 'debts' AND ref_id = ?2",
        params![user_id, debt_id],
    )
    .await?;
    Ok(())
}

async fn sync_transaction_pnl_scope(
    conn: &Connection,
    user_id: &str,
    transaction_id: Option<&str>,
) -> Result<(), ApiError> {
    let Some(transaction_id) = transaction_id else {
        return Ok(());
    };
    update_transaction_row(
        conn,
        user_id,
        transaction_id,
        TransactionPatch::SyncPnlScopeForPlugin {
            plugin_id: "debts".to_owned(),
        },
    )
    .await?;
    Ok(())
}

async fn sync_changed_transaction_pnl_scopes(
    conn: &Connection,
    user_id: &str,
    previous_transaction_id: Option<&str>,
    transaction_id: Option<&str>,
) -> Result<(), ApiError> {
    sync_transaction_pnl_scope(conn, user_id, previous_transaction_id).await?;
    if transaction_id != previous_transaction_id {
        sync_transaction_pnl_scope(conn, user_id, transaction_id).await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn resolve_event_transaction(
    conn: &Connection,
    user_id: &str,
    direction: &DebtDirection,
    link_kind: LinkKind,
    amount_cents: i64,
    currency: &str,
    occurred_on: &str,
    requested_transaction_id: &Option<Option<String>>,
    requested_account_id: Option<String>,
    current_transaction_id: Option<String>,
    current_account_id: Option<String>,
    current_auto_created: bool,
    exclude: (&str, &str),
) -> Result<(Option<String>, Option<String>, bool), ApiError> {
    if let Some(Some(requested_id)) = requested_transaction_id {
        let account_id = validate_transaction_link(
            conn,
            user_id,
            requested_id,
            amount_cents,
            occurred_on,
            expected_transaction_kind(direction, link_kind),
            Some(exclude),
        )
        .await?;
        if current_auto_created
            && current_transaction_id.as_deref() != Some(requested_id.as_str())
            && let Some(current_id) = current_transaction_id.as_deref()
        {
            archive_auto_transaction(conn, current_id, user_id).await?;
        }
        return Ok((account_id, Some(requested_id.clone()), false));
    }

    let account_id = requested_account_id.or(current_account_id);
    if current_auto_created {
        if let (Some(transaction_id), Some(account_id_value)) =
            (current_transaction_id.as_deref(), account_id.as_deref())
        {
            update_auto_transaction(
                conn,
                transaction_id,
                user_id,
                direction,
                link_kind,
                amount_cents,
                currency,
                occurred_on,
                account_id_value,
            )
            .await?;
            return Ok((account_id, current_transaction_id, true));
        }
        if let Some(transaction_id) = current_transaction_id.as_deref() {
            archive_auto_transaction(conn, transaction_id, user_id).await?;
        }
        return Ok((account_id, None, false));
    }

    if requested_transaction_id.is_none() && current_transaction_id.is_some() {
        return Ok((account_id, current_transaction_id, false));
    }
    if let Some(account_id_value) = account_id.as_deref() {
        let transaction_id = create_auto_transaction(
            conn,
            user_id,
            direction,
            link_kind,
            amount_cents,
            currency,
            occurred_on,
            account_id_value,
        )
        .await?;
        return Ok((account_id, Some(transaction_id), true));
    }
    Ok((account_id, None, false))
}

async fn validate_transaction_link(
    conn: &Connection,
    user_id: &str,
    transaction_id: &str,
    amount_cents: i64,
    occurred_on: &str,
    expected_kind: TransactionKind,
    exclude: Option<(&str, &str)>,
) -> Result<Option<String>, ApiError> {
    let mut rows = conn.query(
        "SELECT kind, amount_cents, occurred_on, account_id FROM ledger_transactions WHERE id = ?1 AND user_id = ?2 AND archived_at IS NULL",
        params![transaction_id, user_id],
    ).await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| ApiError::validation("流水不存在、已删除或不属于当前用户"))?;
    let kind: String = row.get(0)?;
    let transaction_amount: i64 = row.get(1)?;
    let transaction_date: String = row.get(2)?;
    let account_id: Option<String> = row.get(3)?;
    drop(rows);
    let (excluded_kind, excluded_ref_id) = exclude.unwrap_or(("", ""));
    let mut linked = conn
        .query(
            "SELECT 1 FROM transaction_links WHERE user_id = ?1 AND transaction_id = ?2 AND plugin_id = 'debts' AND NOT (kind = ?3 AND ref_id = ?4) LIMIT 1",
            params![user_id, transaction_id, excluded_kind, excluded_ref_id],
        )
        .await?;
    if linked.next().await?.is_some() {
        return Err(ApiError::validation("该流水已关联其他债务往来记录"));
    }
    if transaction_amount != amount_cents {
        return Err(ApiError::validation("关联流水金额与债务往来金额不一致"));
    }
    if transaction_date != occurred_on {
        return Err(ApiError::validation("关联流水日期与债务往来日期不一致"));
    }
    if kind != expected_kind.as_str() {
        return Err(ApiError::validation("关联流水的收支方向与债务往来方向不符"));
    }
    Ok(account_id)
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
        "SELECT d.id, d.direction, d.principal_cents, d.currency, d.occurred_on, d.due_on, d.note, d.archived_at, d.version, d.created_at, d.updated_at, c.id, c.display_name, b.paid_cents, b.remaining_cents, a.id, a.name, a.account_type, a.archived_at, d.origin_kind, d.transaction_id, d.transaction_auto_created FROM debts d JOIN counterparties c ON c.id = d.counterparty_id JOIN debt_balances b ON b.debt_id = d.id LEFT JOIN ledger_accounts a ON a.id = d.account_id AND a.user_id = d.user_id WHERE d.id = ?1 AND d.user_id = ?2",
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
        transaction_id: row.get(20)?,
        transaction_auto_created: row.get::<i64>(21)? != 0,
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
        "SELECT e.id, e.amount_cents, e.effective_on, e.note, e.created_at, a.id, a.name, a.account_type, a.archived_at, e.transaction_id, e.transaction_auto_created FROM debt_addition_events e LEFT JOIN ledger_accounts a ON a.id = e.account_id AND a.user_id = e.user_id WHERE e.user_id = ?1 AND e.debt_id = ?2 ORDER BY e.effective_on DESC, e.created_at DESC",
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
            transaction_id: row.get(9)?,
            transaction_auto_created: row.get::<i64>(10)? != 0,
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
        "SELECT e.id, e.kind, e.amount_cents, e.effective_on, e.note, e.reverses_event_id, e.created_at, EXISTS(SELECT 1 FROM repayment_events r WHERE r.reverses_event_id = e.id), a.id, a.name, a.account_type, a.archived_at, e.transaction_id, e.transaction_auto_created FROM repayment_events e LEFT JOIN ledger_accounts a ON a.id = e.account_id AND a.user_id = e.user_id WHERE e.user_id = ?1 AND e.debt_id = ?2 ORDER BY e.effective_on DESC, e.created_at DESC",
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
            transaction_id: row.get(12)?,
            transaction_auto_created: row.get::<i64>(13)? != 0,
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
