pub mod alipay;
pub mod cmb;
pub mod cmbc;
pub mod duplicates;
pub mod model;
pub(crate) mod pdf;
pub mod wechat;

pub use alipay::parse_alipay_csv;
pub use cmb::parse_cmb_pdf;
pub use cmbc::parse_cmbc_pdf;
pub(crate) use model::NORMALIZATION_VERSION;
pub use model::{
    BaseDisposition, Direction, ImportParseError, ParsedRecord, SourceChannel, StoredDisposition,
};
pub use wechat::parse_wechat_xlsx;

use axum::{
    Json,
    extract::{Multipart, Path, Query, State, multipart::MultipartRejection},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use libsql::{Connection, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{
    AppState,
    auth::AuthUser,
    domain::{
        MAX_SAFE_CENTS, normalize_counterparty, validate_amount, validate_date, validate_note,
    },
    error::ApiError,
    idempotency::{idempotency_key, replay_idempotency, request_hash, store_idempotency},
    transactions::{
        NewTransactionRow, OnExternalConflict, TransactionPatch, hard_delete_transaction_row,
        insert_transaction_row, update_transaction_row,
    },
};

const PARSER_VERSION: i64 = 1;

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ImportListQuery {
    pub page: Option<u32>,
    pub page_size: Option<u32>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ImportDetailQuery {
    pub page: Option<u32>,
    pub page_size: Option<u32>,
    pub disposition: Option<String>,
    pub direction: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportListResponse {
    pub items: Vec<ImportBatchListItem>,
    pub total: i64,
    pub page: u32,
    pub page_size: u32,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportBatchListItem {
    pub id: String,
    pub status: String,
    pub channel: String,
    pub file_name: String,
    pub period_start: String,
    pub period_end: String,
    pub total_count: i64,
    pub committed_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummaryItem {
    pub count: i64,
    pub amount_cents: i64,
}

#[derive(Debug, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    pub import_income: ImportSummaryItem,
    pub import_expense: ImportSummaryItem,
    pub pending: ImportSummaryItem,
    pub neutral: ImportSummaryItem,
    pub closed: ImportSummaryItem,
    pub zero_amount: ImportSummaryItem,
    pub unknown: ImportSummaryItem,
    pub duplicate: ImportSummaryItem,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CommitImportRequest {
    pub account_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BindImportAccountRequest {
    pub account_id: String,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpsertImportAccountMappingRequest {
    pub source_channel: String,
    pub pay_method: String,
    pub account_id: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportAccountMappingResponse {
    pub source_channel: String,
    pub pay_method: String,
    pub account_id: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportPayMethodSummary {
    pub pay_method: String,
    pub count: i64,
    pub account_id: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UnknownIssue {
    pub row_index: i64,
    pub status: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportRecordView {
    pub id: String,
    pub row_index: i64,
    pub external_id: String,
    pub merchant_order_id: String,
    pub occurred_at: String,
    pub occurred_on: String,
    pub direction: String,
    pub amount_cents: i64,
    pub channel_category: String,
    pub counterparty: String,
    pub product: String,
    pub pay_method: String,
    pub channel_status: String,
    pub source_note: String,
    pub disposition: String,
    pub transaction_id: Option<String>,
    pub outcome: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportDetailResponse {
    pub id: String,
    pub status: String,
    pub channel: String,
    pub parser_version: i64,
    pub file_name: String,
    pub period_start: String,
    pub period_end: String,
    pub total_count: i64,
    pub committed_at: Option<String>,
    pub created_at: String,
    pub summary: ImportSummary,
    pub account_id: Option<String>,
    pub pay_methods: Vec<ImportPayMethodSummary>,
    pub issues: Vec<UnknownIssue>,
    pub records: Vec<ImportRecordView>,
    pub filtered_count: i64,
    pub page: u32,
    pub page_size: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_committed_batch_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_committed_at: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CommitImportResponse {
    pub id: String,
    pub status: String,
    pub imported_count: i64,
    pub duplicate_count: i64,
    pub diagnostics: Vec<String>,
    pub committed_at: String,
    pub summary: ImportSummary,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiscardImportResponse {
    pub id: String,
    pub status: String,
    pub deleted_count: i64,
    pub retained_modified_count: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BindImportAccountResponse {
    pub id: String,
    pub account_id: String,
    pub updated_count: i64,
}

#[derive(Debug)]
struct CommitCandidate {
    id: String,
    row_index: i64,
    external_id: String,
    direction: String,
    amount_cents: i64,
    occurred_at: String,
    occurred_on: String,
    occurred_at_precision: String,
    currency: String,
    counterparty: String,
    counterparty_normalized: String,
    product: String,
    pay_method: String,
    source_note: String,
    raw_json: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NeutralTransferKind {
    Withdrawal,
    Recharge,
    Other,
}

#[derive(Debug)]
enum HeaderIdentity {
    AlipayEmail(String),
    WechatNickname(String),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UploadHash<'a> {
    file_sha256: &'a str,
    normalized_file_name: &'a str,
    requested_channel: &'a str,
}

#[utoipa::path(get, path = "/api/v1/imports", params(ImportListQuery), responses((status = 200, body = ImportListResponse)), security(("cookieAuth" = [])))]
pub async fn list_imports(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<ImportListQuery>,
) -> Result<Json<ImportListResponse>, ApiError> {
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(50).clamp(1, 200);
    let conn = state.connection().await?;
    let total: i64 = conn
        .query(
            "SELECT count(*) FROM import_batches WHERE user_id=?1",
            params![user.id.clone()],
        )
        .await?
        .next()
        .await?
        .unwrap()
        .get(0)?;
    let mut rows = conn.query("SELECT id,status,source_channel,file_name,period_start,period_end,total_count,committed_at,created_at FROM import_batches WHERE user_id=?1 ORDER BY created_at DESC,id DESC LIMIT ?2 OFFSET ?3", params![user.id, i64::from(page_size), i64::from(page - 1) * i64::from(page_size)]).await?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().await? {
        items.push(ImportBatchListItem {
            id: row.get(0)?,
            status: row.get(1)?,
            channel: row.get(2)?,
            file_name: row.get(3)?,
            period_start: row.get(4)?,
            period_end: row.get(5)?,
            total_count: row.get(6)?,
            committed_at: row.get(7)?,
            created_at: row.get(8)?,
        });
    }
    Ok(Json(ImportListResponse {
        items,
        total,
        page,
        page_size,
    }))
}

#[utoipa::path(get, path = "/api/v1/imports/{id}", params(("id" = String, Path), ImportDetailQuery), responses((status = 200, body = ImportDetailResponse), (status = 404, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn get_import(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Query(query): Query<ImportDetailQuery>,
) -> Result<Json<ImportDetailResponse>, ApiError> {
    validate_filters(&query)?;
    let conn = state.connection().await?;
    Ok(Json(
        load_detail(&conn, &user.id, &id, &query, None, None).await?,
    ))
}

#[utoipa::path(post, path = "/api/v1/imports/mappings", request_body = UpsertImportAccountMappingRequest, responses((status = 200, body = ImportAccountMappingResponse), (status = 422, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn upsert_import_account_mapping(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Json(request): Json<UpsertImportAccountMappingRequest>,
) -> Result<Response, ApiError> {
    let key = idempotency_key(&headers)?;
    if !matches!(request.source_channel.as_str(), "alipay" | "wechat") {
        return Err(ApiError::validation(
            "sourceChannel 仅支持 alipay 或 wechat",
        ));
    }
    let pay_method = normalize_pay_method(&request.pay_method);
    if pay_method.is_empty() {
        return Err(ApiError::validation("payMethod 不能为空"));
    }
    let normalized_request = UpsertImportAccountMappingRequest {
        source_channel: request.source_channel,
        pay_method: pay_method.clone(),
        account_id: request.account_id,
    };
    let hash = request_hash(&normalized_request)?;
    let operation = "upsert_import_account_mapping";
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) = replay_idempotency(&tx, &user.id, &key, operation, &hash).await? {
        return Ok(response);
    }
    validate_account(&tx, &user.id, Some(&normalized_request.account_id)).await?;
    let now = Utc::now().to_rfc3339();
    tx.execute(
        "INSERT INTO import_account_mappings(id,user_id,source_channel,pay_method,account_id,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?6) ON CONFLICT(user_id,source_channel,pay_method) DO UPDATE SET account_id=excluded.account_id,updated_at=excluded.updated_at",
        params![Uuid::now_v7().to_string(), user.id.clone(), normalized_request.source_channel.clone(), pay_method.clone(), normalized_request.account_id.clone(), now],
    ).await?;
    let body = ImportAccountMappingResponse {
        source_channel: normalized_request.source_channel,
        pay_method,
        account_id: normalized_request.account_id,
    };
    store_idempotency(&tx, &user.id, &key, operation, &hash, StatusCode::OK, &body).await?;
    tx.commit().await?;
    Ok(Json(body).into_response())
}

#[utoipa::path(post, path = "/api/v1/imports", request_body(content_type = "multipart/form-data"), responses((status = 201, body = ImportDetailResponse), (status = 409, body = crate::error::ErrorBody), (status = 413, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn upload_import(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    multipart: Result<Multipart, MultipartRejection>,
) -> Result<Response, ApiError> {
    let key = idempotency_key(&headers)?;
    let mut multipart = multipart.map_err(multipart_error)?;
    let mut file: Option<(String, Vec<u8>)> = None;
    let mut requested_channel: Option<String> = None;
    while let Some(field) = multipart.next_field().await.map_err(multipart_error)? {
        match field.name() {
            Some("file") if file.is_none() => {
                let name = normalize_file_name(field.file_name().unwrap_or("import-file"));
                let bytes = field.bytes().await.map_err(multipart_error)?;
                if bytes.len() > model::MAX_IMPORT_BYTES {
                    return Err(payload_too_large());
                }
                file = Some((name, bytes.to_vec()));
            }
            Some("channel") if requested_channel.is_none() => {
                let value = field.text().await.map_err(multipart_error)?;
                let value = value.trim();
                if !matches!(value, "alipay" | "wechat" | "cmb" | "cmbc") {
                    return Err(invalid_multipart(
                        "channel 仅支持 alipay、wechat、cmb 或 cmbc",
                    ));
                }
                requested_channel = Some(value.to_owned());
            }
            Some("file" | "channel") => return Err(invalid_multipart("multipart 字段重复")),
            _ => return Err(invalid_multipart("multipart 包含未知字段")),
        }
    }
    let (file_name, bytes) =
        file.ok_or_else(|| invalid_multipart("必须提供且仅提供一个 file 字段"))?;
    let file_sha256 = format!("{:x}", Sha256::digest(&bytes));
    let requested = requested_channel.as_deref().unwrap_or("auto");
    let hash = request_hash(&UploadHash {
        file_sha256: &file_sha256,
        normalized_file_name: &file_name,
        requested_channel: requested,
    })?;
    let (channel, parsed, header_identity) = parse_upload(&bytes, requested_channel.as_deref())?;

    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) = replay_idempotency(&tx, &user.id, &key, "create_import", &hash).await? {
        tx.rollback().await?;
        return Ok(response);
    }
    let previous = previous_committed(&tx, &user.id, &file_sha256).await?;
    let mut stored = Vec::with_capacity(parsed.len());
    for record in parsed {
        let disposition = duplicate_disposition(&tx, &user.id, channel, &record).await?;
        stored.push((record, disposition));
    }
    let status = if stored.iter().any(|(_, d)| *d == "unknown") {
        "blocked"
    } else {
        "preview"
    };
    let period_start = stored
        .iter()
        .map(|(r, _)| r.occurred_on.as_str())
        .min()
        .ok_or_else(|| ApiError::internal("parser returned no records"))?
        .to_owned();
    let period_end = stored
        .iter()
        .map(|(r, _)| r.occurred_on.as_str())
        .max()
        .unwrap()
        .to_owned();
    let id = Uuid::now_v7().to_string();
    let now = Utc::now().to_rfc3339();
    tx.execute("INSERT INTO import_batches(id,user_id,source_channel,parser_version,file_name,file_sha256,period_start,period_end,total_count,status,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?11)", params![id.clone(), user.id.clone(), channel_name(channel), PARSER_VERSION, file_name, file_sha256, period_start, period_end, stored.len() as i64, status, now.clone()]).await?;
    for (record, disposition) in stored {
        let counterparty_normalized =
            normalize_counterparty(channel_name(channel), &record.counterparty);
        tx.execute("INSERT INTO import_records(id,batch_id,row_index,external_id,merchant_order_id,occurred_at,occurred_on,direction,amount_cents,channel_category,counterparty,product,pay_method,channel_status,source_note,counterparty_account_raw,occurred_at_precision,currency,external_id_source,counter_channel_raw,balance_after_cents,raw_json,disposition,counterparty_normalized,normalization_version,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26)", params![Uuid::now_v7().to_string(), id.clone(), record.row_index, record.external_id, record.merchant_order_id, record.occurred_at, record.occurred_on, direction_name(record.direction), record.amount_cents, record.channel_category, record.counterparty, record.product, record.pay_method, record.channel_status, record.source_note, record.counterparty_account_raw, record.occurred_at_precision, record.currency, record.external_id_source, record.counter_channel_raw, record.balance_after_cents, record.raw_json, disposition, counterparty_normalized, NORMALIZATION_VERSION, now.clone()]).await?;
    }
    let query = ImportDetailQuery {
        page: Some(1),
        page_size: Some(50),
        disposition: None,
        direction: None,
    };
    let body = load_detail(
        &tx,
        &user.id,
        &id,
        &query,
        previous,
        header_identity.as_ref(),
    )
    .await?;
    store_idempotency(
        &tx,
        &user.id,
        &key,
        "create_import",
        &hash,
        StatusCode::CREATED,
        &body,
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(body)).into_response())
}

#[utoipa::path(post, path = "/api/v1/imports/{id}/commit", params(("id" = String, Path)), request_body = CommitImportRequest, responses((status = 200, body = CommitImportResponse), (status = 404, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn commit_import(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<CommitImportRequest>,
) -> Result<Response, ApiError> {
    let key = idempotency_key(&headers)?;
    let hash = request_hash(&request)?;
    let operation = format!("commit_import:{id}");
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) = replay_idempotency(&tx, &user.id, &key, &operation, &hash).await? {
        return Ok(response);
    }

    let mut batches = tx
        .query(
            "SELECT status,source_channel,period_start,period_end FROM import_batches WHERE id=?1 AND user_id=?2",
            params![id.clone(), user.id.clone()],
        )
        .await?;
    let batch = batches
        .next()
        .await?
        .ok_or_else(|| ApiError::not_found("找不到该导入批次"))?;
    let status: String = batch.get(0)?;
    let source_channel: String = batch.get(1)?;
    let period_start: String = batch.get(2)?;
    let period_end: String = batch.get(3)?;
    drop(batch);
    drop(batches);
    if status != "preview" {
        return Err(batch_state_conflict());
    }

    validate_account(&tx, &user.id, request.account_id.as_deref()).await?;
    normalize_batch_records(&tx, &user.id, &id, &source_channel).await?;

    let mappings = load_account_mappings(&tx, &user.id, &source_channel).await?;
    let self_transfer_aliases = load_self_transfer_aliases(&tx, &user.id).await?;
    let mut rows = tx.query("SELECT id,row_index,external_id,direction,amount_cents,occurred_at,occurred_on,occurred_at_precision,currency,counterparty,counterparty_normalized,product,pay_method,source_note,raw_json FROM import_records WHERE batch_id=?1 AND disposition='import' AND transaction_id IS NULL ORDER BY row_index", params![id.clone()]).await?;
    let mut candidates = Vec::new();
    while let Some(row) = rows.next().await? {
        candidates.push(CommitCandidate {
            id: row.get(0)?,
            row_index: row.get(1)?,
            external_id: row.get(2)?,
            direction: row.get(3)?,
            amount_cents: row.get(4)?,
            occurred_at: row.get(5)?,
            occurred_on: row.get(6)?,
            occurred_at_precision: row.get(7)?,
            currency: row.get(8)?,
            counterparty: row.get(9)?,
            counterparty_normalized: row.get(10)?,
            product: row.get(11)?,
            pay_method: row.get(12)?,
            source_note: row.get(13)?,
            raw_json: row.get(14)?,
        });
    }
    drop(rows);

    let mut diagnostics = Vec::new();
    let mut written_transaction_ids = Vec::new();
    let link_label = import_batch_link_label(&source_channel, &period_start, &period_end);
    for candidate in candidates {
        validate_commit_candidate(&candidate)?;
        let pay_method = normalize_pay_method(&candidate.pay_method);
        let mapped_account_id = mappings.get(&pay_method).cloned();
        let resolved_account_id = mapped_account_id
            .clone()
            .or_else(|| request.account_id.clone());
        if candidate.direction == "neutral" && resolved_account_id.is_none() {
            return Err(missing_neutral_mapping(candidate.row_index, &pay_method));
        }
        let is_self_transfer = is_self_transfer(
            &source_channel,
            &candidate.direction,
            &candidate.counterparty_normalized,
            &self_transfer_aliases,
        );
        if is_self_transfer && resolved_account_id.is_none() {
            return commit_validation(
                candidate.row_index,
                Err(ApiError::validation("自转记录必须绑定账单账户")),
            );
        }
        let kind = if candidate.direction == "neutral" || is_self_transfer {
            "transfer"
        } else {
            candidate.direction.as_str()
        };
        let account_id = if kind == "transfer" {
            None
        } else {
            resolved_account_id.clone()
        };
        let (transfer_from_account_id, transfer_to_account_id) = if is_self_transfer {
            match candidate.direction.as_str() {
                "expense" => (resolved_account_id.clone(), None),
                "income" => (None, resolved_account_id.clone()),
                _ => unreachable!("self-transfer recognition only accepts income or expense"),
            }
        } else if kind == "transfer" {
            match neutral_transfer_kind(&source_channel, &candidate) {
                transfer_kind @ (NeutralTransferKind::Withdrawal
                | NeutralTransferKind::Recharge) => {
                    let balance_pay_method = channel_balance_pay_method(&source_channel);
                    let balance_account_id = balance_pay_method
                        .and_then(|method| mappings.get(method))
                        .cloned();
                    let Some(balance_account_id) = balance_account_id else {
                        let expected = balance_pay_method.unwrap_or("渠道余额");
                        let diagnostic = format!(
                            "第 {} 行：{} 缺少渠道余额账户映射（支付方式：{}），该行未导入",
                            candidate.row_index, source_channel, expected
                        );
                        let changed = tx.execute(
                            "UPDATE import_records SET disposition='neutral' WHERE id=?1 AND batch_id=?2 AND disposition='import' AND transaction_id IS NULL",
                            params![candidate.id, id.clone()],
                        )
                        .await?;
                        if changed != 1 {
                            return Err(ApiError::internal(
                                "failed to exclude import with missing balance mapping",
                            ));
                        }
                        diagnostics.push(diagnostic);
                        continue;
                    };
                    let pay_account_id = resolved_account_id
                        .clone()
                        .expect("neutral mapping was validated above");
                    let accounts = match transfer_kind {
                        NeutralTransferKind::Withdrawal => (balance_account_id, pay_account_id),
                        NeutralTransferKind::Recharge => (pay_account_id, balance_account_id),
                        NeutralTransferKind::Other => unreachable!(),
                    };
                    if accounts.0 == accounts.1 {
                        let diagnostic = format!(
                            "第 {} 行：{} 的支付方式账户与渠道余额账户相同，该行未导入以避免自转",
                            candidate.row_index, source_channel
                        );
                        let changed = tx.execute(
                            "UPDATE import_records SET disposition='neutral' WHERE id=?1 AND batch_id=?2 AND disposition='import' AND transaction_id IS NULL",
                            params![candidate.id, id.clone()],
                        )
                        .await?;
                        if changed != 1 {
                            return Err(ApiError::internal(
                                "failed to exclude self-transfer import",
                            ));
                        }
                        diagnostics.push(diagnostic);
                        continue;
                    }
                    (Some(accounts.0), Some(accounts.1))
                }
                NeutralTransferKind::Other => (resolved_account_id, None),
            }
        } else {
            (None, None)
        };
        let payee_name = normalized_text(&candidate.counterparty, 200);
        let payee_key = candidate.counterparty_normalized.clone();
        let description = normalized_text(&candidate.product, 500);
        let note = normalized_note(&candidate);
        commit_validation(candidate.row_index, validate_note(&note))?;
        let transaction_id = Uuid::now_v7().to_string();
        let now = Utc::now().to_rfc3339();
        let changed = insert_transaction_row(
            &tx,
            NewTransactionRow {
                id: transaction_id.clone(),
                user_id: user.id.clone(),
                kind: kind.to_owned(),
                amount_cents: candidate.amount_cents,
                currency: candidate.currency.clone(),
                occurred_on: candidate.occurred_on.clone(),
                occurred_at: Some(candidate.occurred_at.clone()),
                occurred_at_precision: candidate.occurred_at_precision.clone(),
                category: String::new(),
                category_id: None,
                category_source: "none".to_owned(),
                category_rule_id: None,
                payee_name,
                payee_key,
                description,
                account_id,
                transfer_from_account_id,
                transfer_to_account_id,
                note,
                archived_at: None,
                version: 1,
                created_at: now.clone(),
                updated_at: now.clone(),
                source_channel: source_channel.clone(),
                external_id: candidate.external_id.clone(),
                import_batch_id: Some(id.clone()),
                event_id: None,
                pnl_scope: "counted".to_owned(),
                created_by: "plugin:bill-imports".to_owned(),
                on_external_conflict: OnExternalConflict::Ignore,
            },
        )
        .await?;
        if changed == 1 {
            let linked = tx.execute(
                "UPDATE import_records SET disposition='import',transaction_id=?1 WHERE id=?2 AND batch_id=?3 AND disposition='import' AND transaction_id IS NULL",
                params![transaction_id.clone(), candidate.id, id.clone()],
            ).await?;
            if linked != 1 {
                return Err(ApiError::internal("failed to link committed import record"));
            }
            tx.execute(
                "INSERT INTO transaction_links(id,user_id,transaction_id,plugin_id,kind,ref_id,label,created_at) VALUES (?1,?2,?3,'bill-imports','batch',?4,?5,?6)",
                params![Uuid::now_v7().to_string(), user.id.clone(), transaction_id.clone(), id.clone(), link_label.clone(), now],
            )
            .await?;
            written_transaction_ids.push(transaction_id);
        } else if changed == 0 {
            assert_existing_payload_matches(&tx, &user.id, &source_channel, &candidate).await?;
            let marked = tx.execute(
                "UPDATE import_records SET disposition='duplicate' WHERE id=?1 AND batch_id=?2 AND disposition='import' AND transaction_id IS NULL",
                params![candidate.id, id.clone()],
            ).await?;
            if marked != 1 {
                return Err(ApiError::internal("failed to mark import record duplicate"));
            }
        } else {
            return Err(ApiError::internal(
                "targeted import upsert affected unexpected row count",
            ));
        }
    }

    let committed_at = Utc::now().to_rfc3339();
    let changed = tx.execute(
        "UPDATE import_batches SET status='committed',committed_at=?1,updated_at=?1 WHERE id=?2 AND user_id=?3 AND status='preview'",
        params![committed_at.clone(), id.clone(), user.id.clone()],
    ).await?;
    if changed != 1 {
        return Err(batch_state_conflict());
    }
    let mut outcome_counts = tx.query(
        "SELECT count(CASE WHEN disposition='import' AND transaction_id IS NOT NULL THEN 1 END),count(CASE WHEN disposition='duplicate' THEN 1 END) FROM import_records WHERE batch_id=?1",
        params![id.clone()],
    ).await?;
    let outcome_counts = outcome_counts
        .next()
        .await?
        .ok_or_else(|| ApiError::internal("failed to count committed import outcomes"))?;
    let imported_count: i64 = outcome_counts.get(0)?;
    let duplicate_count: i64 = outcome_counts.get(1)?;
    let (summary, _) = aggregate(&tx, &id).await?;
    duplicates::match_committed_batch(&tx, &user.id, &id).await?;
    crate::lifecycle::after_transactions_written(
        &tx,
        &crate::lifecycle::TransactionsWritten {
            user_id: user.id.clone(),
            transaction_ids: written_transaction_ids,
            origin: "import",
        },
    )
    .await?;
    let body = CommitImportResponse {
        id: id.clone(),
        status: "committed".to_owned(),
        imported_count,
        duplicate_count,
        diagnostics,
        committed_at,
        summary,
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

#[utoipa::path(post, path = "/api/v1/imports/{id}/account", params(("id" = String, Path)), request_body = BindImportAccountRequest, responses((status = 200, body = BindImportAccountResponse)), security(("cookieAuth" = [])))]
pub async fn bind_import_account(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(request): Json<BindImportAccountRequest>,
) -> Result<Json<BindImportAccountResponse>, ApiError> {
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    let mut batches = tx
        .query(
            "SELECT status FROM import_batches WHERE id=?1 AND user_id=?2",
            params![id.clone(), user.id.clone()],
        )
        .await?;
    let status: String = batches
        .next()
        .await?
        .ok_or_else(|| ApiError::not_found("找不到该导入批次"))?
        .get(0)?;
    drop(batches);
    if status != "committed" {
        return Err(batch_state_conflict());
    }
    validate_account(&tx, &user.id, Some(&request.account_id)).await?;
    let mut rows = tx
        .query(
            "SELECT id FROM ledger_transactions WHERE user_id=?1 AND import_batch_id=?2 AND account_id IS NULL AND kind<>'transfer' ORDER BY id",
            params![user.id.clone(), id.clone()],
        )
        .await?;
    let mut transaction_ids = Vec::new();
    while let Some(row) = rows.next().await? {
        transaction_ids.push(row.get::<String>(0)?);
    }
    drop(rows);
    let mut updated_count = 0_i64;
    for transaction_id in transaction_ids {
        updated_count += update_transaction_row(
            &tx,
            &user.id,
            &transaction_id,
            TransactionPatch::BindAccountIfUnboundNonTransfer {
                account_id: request.account_id.clone(),
            },
        )
        .await? as i64;
    }
    tx.commit().await?;
    Ok(Json(BindImportAccountResponse {
        id,
        account_id: request.account_id,
        updated_count,
    }))
}

#[utoipa::path(delete, path = "/api/v1/imports/{id}", params(("id" = String, Path)), responses((status = 200, body = DiscardImportResponse), (status = 404, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn discard_import(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let key = idempotency_key(&headers)?;
    let hash = request_hash(&())?;
    let operation = format!("discard_import:{id}");
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) = replay_idempotency(&tx, &user.id, &key, &operation, &hash).await? {
        return Ok(response);
    }

    let mut batches = tx
        .query(
            "SELECT status FROM import_batches WHERE id=?1 AND user_id=?2",
            params![id.clone(), user.id.clone()],
        )
        .await?;
    let status: String = batches
        .next()
        .await?
        .ok_or_else(|| ApiError::not_found("找不到该导入批次"))?
        .get(0)?;
    drop(batches);
    if !matches!(status.as_str(), "preview" | "blocked" | "committed") {
        return Err(batch_state_conflict());
    }

    let mut deleted_count = 0;
    let mut retained_modified_count = 0;
    if status == "committed" {
        let mut rows = tx
            .query(
                "SELECT r.batch_id,r.disposition,r.transaction_id,t.id,t.user_id,t.import_batch_id,t.version,t.archived_at FROM import_records r LEFT JOIN ledger_transactions t ON t.id=r.transaction_id WHERE r.batch_id=?1 AND r.transaction_id IS NOT NULL ORDER BY r.row_index",
                params![id.clone()],
            )
            .await?;
        let mut deletable_ids = Vec::new();
        while let Some(row) = rows.next().await? {
            let record_batch_id: String = row.get(0)?;
            let disposition: String = row.get(1)?;
            let record_transaction_id: String = row.get(2)?;
            let ledger_id: Option<String> = row.get(3)?;
            let ledger_user_id: Option<String> = row.get(4)?;
            let ledger_batch_id: Option<String> = row.get(5)?;
            let version: Option<i64> = row.get(6)?;
            let archived_at: Option<String> = row.get(7)?;
            if record_batch_id != id
                || disposition != "import"
                || ledger_id.as_deref() != Some(record_transaction_id.as_str())
                || ledger_user_id.as_deref() != Some(user.id.as_str())
                || ledger_batch_id.as_deref() != Some(id.as_str())
            {
                return Err(ApiError::internal(
                    "import discard provenance chain is inconsistent",
                ));
            }
            if version == Some(1) && archived_at.is_none() {
                deletable_ids.push(record_transaction_id);
            } else {
                retained_modified_count += 1;
            }
        }
        drop(rows);

        let deleted_event = crate::lifecycle::TransactionsDeleted {
            user_id: user.id.clone(),
            transaction_ids: deletable_ids.clone(),
        };
        let prepared = crate::lifecycle::prepare_transactions_deleted(&tx, &deleted_event).await?;
        for transaction_id in deletable_ids {
            let changed = hard_delete_transaction_row(&tx, &user.id, &transaction_id).await?;
            if changed != 1 {
                return Err(ApiError::internal(
                    "import discard delete affected unexpected row count",
                ));
            }
            deleted_count += 1;
        }
        crate::lifecycle::after_transactions_deleted(&tx, &deleted_event, prepared).await?;
    }

    let now = Utc::now().to_rfc3339();
    let changed = tx
        .execute(
            "UPDATE import_batches SET status='discarded',updated_at=?1 WHERE id=?2 AND user_id=?3 AND status=?4",
            params![now, id.clone(), user.id.clone(), status],
        )
        .await?;
    if changed != 1 {
        return Err(batch_state_conflict());
    }
    let body = DiscardImportResponse {
        id: id.clone(),
        status: "discarded".to_owned(),
        deleted_count,
        retained_modified_count,
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

fn batch_state_conflict() -> ApiError {
    ApiError::conflict(
        "import_batch_state_conflict",
        "导入批次状态已变化，请刷新后重试",
    )
}

fn commit_validation<T>(row_index: i64, result: Result<T, ApiError>) -> Result<T, ApiError> {
    result.map_err(|error| ApiError {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        code: error.code,
        message: format!("第 {row_index} 行：{}", error.message),
        field_errors: Some(serde_json::json!({ "rowIndex": row_index })),
    })
}

fn validate_commit_candidate(candidate: &CommitCandidate) -> Result<(), ApiError> {
    if !matches!(
        candidate.direction.as_str(),
        "income" | "expense" | "neutral"
    ) {
        return commit_validation(
            candidate.row_index,
            Err(ApiError::validation("收支方向无效")),
        );
    }
    if candidate.external_id.trim().is_empty() {
        return commit_validation(
            candidate.row_index,
            Err(ApiError::validation("交易 ID 不能为空")),
        );
    }
    if !(1..=MAX_SAFE_CENTS).contains(&candidate.amount_cents) {
        return commit_validation(
            candidate.row_index,
            Err(ApiError::validation("金额必须大于 0 且在安全范围内")),
        );
    }
    commit_validation(candidate.row_index, validate_amount(candidate.amount_cents))?;
    commit_validation(
        candidate.row_index,
        validate_date(&candidate.occurred_on, "发生日期"),
    )?;
    Ok(())
}

fn normalized_text(value: &str, limit: usize) -> String {
    let replaced: String = value
        .trim()
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    replaced
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(limit)
        .collect()
}

fn normalized_note(candidate: &CommitCandidate) -> String {
    normalized_text(&candidate.source_note, 2_000)
}

fn normalize_pay_method(value: &str) -> String {
    normalized_text(value.split('&').next().unwrap_or_default(), 4_096)
}

async fn load_self_transfer_aliases(
    tx: &libsql::Transaction,
    user_id: &str,
) -> Result<HashSet<String>, ApiError> {
    let mut rows = tx
        .query(
            "SELECT normalized_alias FROM self_transfer_aliases WHERE user_id=?1",
            [user_id],
        )
        .await?;
    let mut aliases = HashSet::new();
    while let Some(row) = rows.next().await? {
        aliases.insert(row.get(0)?);
    }
    Ok(aliases)
}

pub fn is_self_transfer(
    source_channel: &str,
    direction: &str,
    counterparty_normalized: &str,
    aliases: &HashSet<String>,
) -> bool {
    matches!(source_channel, "cmb" | "cmbc")
        && matches!(direction, "income" | "expense")
        && aliases.contains(counterparty_normalized)
}

fn neutral_transfer_kind(source_channel: &str, candidate: &CommitCandidate) -> NeutralTransferKind {
    let category_key = match source_channel {
        "wechat" => "交易类型",
        "alipay" => "交易分类",
        _ => return NeutralTransferKind::Other,
    };
    let Ok(raw) = serde_json::from_str::<serde_json::Value>(&candidate.raw_json) else {
        return NeutralTransferKind::Other;
    };
    let Some(category) = raw.get(category_key).and_then(serde_json::Value::as_str) else {
        return NeutralTransferKind::Other;
    };

    if category.contains("充值") {
        return NeutralTransferKind::Recharge;
    }
    let channel_marks_withdrawal = category.contains("提现");
    let channel_marks_bank_outflow =
        category.contains("转出") || category.contains("到银行卡") || category.contains("至银行卡");
    if channel_marks_withdrawal
        || (channel_marks_bank_outflow && candidate.counterparty.contains("银行"))
    {
        NeutralTransferKind::Withdrawal
    } else {
        NeutralTransferKind::Other
    }
}

fn channel_balance_pay_method(source_channel: &str) -> Option<&'static str> {
    match source_channel {
        "wechat" => Some("零钱"),
        "alipay" => Some("账户余额"),
        _ => None,
    }
}

async fn normalize_batch_records(
    tx: &libsql::Transaction,
    user_id: &str,
    batch_id: &str,
    source_channel: &str,
) -> Result<(), ApiError> {
    let mut rows = tx
        .query(
            "SELECT id,counterparty,transaction_id FROM import_records WHERE batch_id=?1 AND normalization_version<>?2 ORDER BY row_index",
            params![batch_id, NORMALIZATION_VERSION],
        )
        .await?;
    let mut pending = Vec::new();
    while let Some(row) = rows.next().await? {
        pending.push((
            row.get::<String>(0)?,
            row.get::<String>(1)?,
            row.get::<Option<String>>(2)?,
        ));
    }
    drop(rows);

    for (record_id, counterparty, transaction_id) in pending {
        let normalized = normalize_counterparty(source_channel, &counterparty);
        tx.execute(
            "UPDATE import_records SET counterparty_normalized=?1,normalization_version=?2 WHERE id=?3 AND batch_id=?4",
            params![normalized.clone(), NORMALIZATION_VERSION, record_id, batch_id],
        )
        .await?;
        if let Some(transaction_id) = transaction_id {
            // user_id 谓词是纵深防御：provenance 链目前由 commit 路径保证同用户，
            // 但这条 UPDATE 只按主键定位，一旦 import_records.transaction_id 指向
            // 别人的行就会静默改写对方的 payee_key。加上谓词后最坏情况是不更新。
            update_transaction_row(
                tx,
                user_id,
                &transaction_id,
                TransactionPatch::SetPayeeKey {
                    payee_key: normalized,
                },
            )
            .await?;
        }
    }
    Ok(())
}

fn missing_neutral_mapping(row_index: i64, pay_method: &str) -> ApiError {
    let display = if pay_method.is_empty() {
        "（空）"
    } else {
        pay_method
    };
    ApiError {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        code: "import_account_mapping_required",
        message: format!("第 {row_index} 行：中性交易的支付方式 {display} 尚未映射账户"),
        field_errors: Some(serde_json::json!({
            "rowIndex": row_index,
            "payMethod": pay_method,
        })),
    }
}

async fn load_account_mappings(
    conn: &Connection,
    user_id: &str,
    source_channel: &str,
) -> Result<HashMap<String, String>, ApiError> {
    let mut rows = conn
        .query(
            "SELECT m.pay_method,m.account_id FROM import_account_mappings m JOIN ledger_accounts a ON a.id=m.account_id AND a.user_id=m.user_id AND a.archived_at IS NULL WHERE m.user_id=?1 AND m.source_channel=?2 AND m.account_id IS NOT NULL",
            params![user_id, source_channel],
        )
        .await?;
    let mut mappings = HashMap::new();
    while let Some(row) = rows.next().await? {
        mappings.insert(row.get(0)?, row.get(1)?);
    }
    Ok(mappings)
}

async fn assert_existing_payload_matches(
    conn: &Connection,
    user_id: &str,
    source_channel: &str,
    candidate: &CommitCandidate,
) -> Result<(), ApiError> {
    let mut rows = conn.query("SELECT b.source_channel,r.external_id,r.direction,r.amount_cents,r.occurred_on,r.id FROM ledger_transactions t LEFT JOIN import_records r ON r.transaction_id=t.id LEFT JOIN import_batches b ON b.id=r.batch_id WHERE t.user_id=?1 AND t.source_channel=?2 AND t.external_id=?3", params![user_id, source_channel, candidate.external_id.clone()]).await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| ApiError::internal("targeted import conflict row disappeared"))?;
    let provenance: Option<String> = row.get(5)?;
    if provenance.is_none() {
        return Err(ApiError::internal(
            "import ledger row missing staging provenance",
        ));
    }
    let matches = row.get::<String>(0)? == source_channel
        && row.get::<String>(1)? == candidate.external_id
        && row.get::<String>(2)? == candidate.direction
        && row.get::<i64>(3)? == candidate.amount_cents
        && row.get::<String>(4)? == candidate.occurred_on;
    if !matches {
        return Err(ApiError::conflict(
            "external_id_payload_mismatch",
            format!("第 {} 行相同交易 ID 的核心字段不一致", candidate.row_index),
        ));
    }
    Ok(())
}

fn multipart_error(error: impl IntoResponse + std::fmt::Display) -> ApiError {
    let status = error.into_response().status();
    if status == StatusCode::PAYLOAD_TOO_LARGE {
        payload_too_large()
    } else {
        invalid_multipart("multipart 请求无效")
    }
}
fn payload_too_large() -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "payload_too_large",
        "上传文件不能超过 10 MiB",
    )
}
fn invalid_multipart(message: impl Into<String>) -> ApiError {
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_multipart",
        message,
    )
}

fn normalize_file_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base.chars().filter(|c| !c.is_control()).take(255).collect();
    if cleaned.is_empty() {
        "import-file".to_owned()
    } else {
        cleaned
    }
}

fn parse_upload(
    bytes: &[u8],
    requested: Option<&str>,
) -> Result<(SourceChannel, Vec<ParsedRecord>, Option<HeaderIdentity>), ApiError> {
    let result = match requested {
        Some("alipay") => parse_alipay_csv(bytes).map(|r| (SourceChannel::Alipay, r)),
        Some("wechat") => parse_wechat_xlsx(bytes).map(|r| (SourceChannel::Wechat, r)),
        Some("cmb") => parse_cmb_pdf(bytes).map(|r| (SourceChannel::Cmb, r)),
        Some("cmbc") => parse_cmbc_pdf(bytes).map(|r| (SourceChannel::Cmbc, r)),
        None if bytes.starts_with(b"PK\x03\x04") => {
            parse_wechat_xlsx(bytes).map(|r| (SourceChannel::Wechat, r))
        }
        None => parse_alipay_csv(bytes).map(|r| (SourceChannel::Alipay, r)),
        _ => unreachable!(),
    };
    result
        .map(|(channel, records)| {
            let identity = header_identity(channel, bytes);
            (channel, records, identity)
        })
        .map_err(|e| {
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                normalize_parse_code(e.code),
                e.message,
            )
        })
}

fn normalize_parse_code(code: &'static str) -> &'static str {
    match code {
        "duplicate_import_external_id" => "duplicate_external_id_in_file",
        "invalid_import_encoding"
        | "invalid_import_csv"
        | "invalid_import_xlsx"
        | "invalid_import_pdf"
        | "empty_import_file" => "unsupported_import_file",
        "empty_import_external_id" | "invalid_import_datetime" | "import_field_too_long" => {
            "invalid_import_row"
        }
        other => other,
    }
}
fn channel_name(channel: SourceChannel) -> &'static str {
    match channel {
        SourceChannel::Alipay => "alipay",
        SourceChannel::Wechat => "wechat",
        SourceChannel::Cmb => "cmb",
        SourceChannel::Cmbc => "cmbc",
    }
}
fn channel_display_name(channel: &str) -> &str {
    match channel {
        "alipay" => "支付宝",
        "wechat" => "微信支付",
        "cmb" => "招商银行",
        "cmbc" => "民生银行",
        other => other,
    }
}
fn import_batch_link_label(source_channel: &str, period_start: &str, period_end: &str) -> String {
    format!(
        "{} · {} 至 {}",
        channel_display_name(source_channel),
        period_start,
        period_end
    )
}
fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::Income => "income",
        Direction::Expense => "expense",
        Direction::Neutral => "neutral",
    }
}
fn base_disposition(value: BaseDisposition) -> &'static str {
    match value {
        BaseDisposition::Import => "import",
        BaseDisposition::Pending => "pending",
        BaseDisposition::Neutral => "neutral",
        BaseDisposition::Closed => "closed",
        BaseDisposition::ZeroAmount => "zero_amount",
        BaseDisposition::Unknown => "unknown",
    }
}

async fn duplicate_disposition(
    conn: &Connection,
    user_id: &str,
    channel: SourceChannel,
    record: &ParsedRecord,
) -> Result<&'static str, ApiError> {
    let mut rows = conn.query("SELECT b.source_channel,r.external_id,r.direction,r.amount_cents,r.occurred_on,r.id FROM ledger_transactions t LEFT JOIN import_records r ON r.transaction_id=t.id LEFT JOIN import_batches b ON b.id=r.batch_id WHERE t.user_id=?1 AND t.source_channel=?2 AND t.external_id=?3", params![user_id, channel_name(channel), record.external_id.clone()]).await?;
    let Some(row) = rows.next().await? else {
        return Ok(base_disposition(record.disposition));
    };
    let provenance: Option<String> = row.get(5)?;
    if provenance.is_none() {
        return Err(ApiError::internal(
            "import ledger row missing staging provenance",
        ));
    }
    let matches = row.get::<String>(0)? == channel_name(channel)
        && row.get::<String>(1)? == record.external_id
        && row.get::<String>(2)? == direction_name(record.direction)
        && row.get::<i64>(3)? == record.amount_cents
        && row.get::<String>(4)? == record.occurred_on;
    if !matches {
        return Err(ApiError::conflict(
            "external_id_payload_mismatch",
            "相同交易 ID 的核心字段不一致",
        ));
    }
    Ok("duplicate")
}

async fn previous_committed(
    conn: &Connection,
    user_id: &str,
    hash: &str,
) -> Result<Option<(String, String)>, ApiError> {
    let mut rows = conn.query("SELECT id,committed_at FROM import_batches WHERE user_id=?1 AND file_sha256=?2 AND committed_at IS NOT NULL ORDER BY committed_at DESC,id DESC LIMIT 1", params![user_id, hash]).await?;
    Ok(match rows.next().await? {
        Some(row) => Some((row.get(0)?, row.get(1)?)),
        None => None,
    })
}

fn validate_filters(query: &ImportDetailQuery) -> Result<(), ApiError> {
    if let Some(v) = query.disposition.as_deref()
        && !matches!(
            v,
            "import" | "pending" | "neutral" | "closed" | "zero_amount" | "unknown" | "duplicate"
        )
    {
        return Err(ApiError::validation("disposition filter 无效"));
    }
    if let Some(v) = query.direction.as_deref()
        && !matches!(v, "income" | "expense" | "neutral")
    {
        return Err(ApiError::validation("direction filter 无效"));
    }
    Ok(())
}

async fn load_detail(
    conn: &Connection,
    user_id: &str,
    id: &str,
    query: &ImportDetailQuery,
    mut previous: Option<(String, String)>,
    header_identity: Option<&HeaderIdentity>,
) -> Result<ImportDetailResponse, ApiError> {
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(50).clamp(1, 200);
    let mut batches = conn.query("SELECT status,source_channel,parser_version,file_name,period_start,period_end,total_count,committed_at,created_at,file_sha256 FROM import_batches WHERE id=?1 AND user_id=?2", params![id,user_id]).await?;
    let batch = batches
        .next()
        .await?
        .ok_or_else(|| ApiError::not_found("找不到该导入批次"))?;
    if previous.is_none() {
        let file_sha256: String = batch.get(9)?;
        let mut historical = conn.query("SELECT id,committed_at FROM import_batches WHERE user_id=?1 AND file_sha256=?2 AND committed_at IS NOT NULL AND id<>?3 ORDER BY committed_at DESC,id DESC LIMIT 1", params![user_id,file_sha256,id]).await?;
        if let Some(row) = historical.next().await? {
            previous = Some((row.get(0)?, row.get(1)?));
        }
    }
    let status: String = batch.get(0)?;
    let filtered_count: i64 = conn.query("SELECT count(*) FROM import_records WHERE batch_id=?1 AND (?2 IS NULL OR disposition=?2) AND (?3 IS NULL OR direction=?3)", params![id, query.disposition.clone(), query.direction.clone()]).await?.next().await?.unwrap().get(0)?;
    let mut rows = conn.query("SELECT id,row_index,external_id,merchant_order_id,occurred_at,occurred_on,direction,amount_cents,channel_category,counterparty,product,pay_method,channel_status,source_note,disposition,transaction_id FROM import_records WHERE batch_id=?1 AND (?2 IS NULL OR disposition=?2) AND (?3 IS NULL OR direction=?3) ORDER BY row_index ASC LIMIT ?4 OFFSET ?5", params![id, query.disposition.clone(), query.direction.clone(), i64::from(page_size), i64::from(page - 1) * i64::from(page_size)]).await?;
    let mut records = Vec::new();
    while let Some(row) = rows.next().await? {
        let disposition: String = row.get(14)?;
        let transaction_id: Option<String> = row.get(15)?;
        let committed_at: Option<String> = batch.get(7)?;
        let outcome = outcome(
            &status,
            &disposition,
            transaction_id.is_some(),
            committed_at.is_some(),
        );
        records.push(ImportRecordView {
            id: row.get(0)?,
            row_index: row.get(1)?,
            external_id: row.get(2)?,
            merchant_order_id: row.get(3)?,
            occurred_at: row.get(4)?,
            occurred_on: row.get(5)?,
            direction: row.get(6)?,
            amount_cents: row.get(7)?,
            channel_category: row.get(8)?,
            counterparty: row.get(9)?,
            product: row.get(10)?,
            pay_method: row.get(11)?,
            channel_status: row.get(12)?,
            source_note: row.get(13)?,
            disposition,
            transaction_id,
            outcome: outcome.to_owned(),
        });
    }
    let (summary, issues) = aggregate(conn, id).await?;
    let source_channel: String = batch.get(1)?;
    let pay_methods = load_pay_method_summaries(conn, user_id, id, &source_channel).await?;
    let account_id =
        suggested_account(conn, user_id, id, &status, &source_channel, header_identity).await?;
    let (previous_committed_batch_id, previous_committed_at) =
        previous.map_or((None, None), |(a, b)| (Some(a), Some(b)));
    Ok(ImportDetailResponse {
        id: id.to_owned(),
        status,
        channel: batch.get(1)?,
        parser_version: batch.get(2)?,
        file_name: batch.get(3)?,
        period_start: batch.get(4)?,
        period_end: batch.get(5)?,
        total_count: batch.get(6)?,
        committed_at: batch.get(7)?,
        created_at: batch.get(8)?,
        summary,
        account_id,
        pay_methods,
        issues,
        records,
        filtered_count,
        page,
        page_size,
        previous_committed_batch_id,
        previous_committed_at,
    })
}

async fn load_pay_method_summaries(
    conn: &Connection,
    user_id: &str,
    batch_id: &str,
    source_channel: &str,
) -> Result<Vec<ImportPayMethodSummary>, ApiError> {
    let mappings = load_account_mappings(conn, user_id, source_channel).await?;
    let mut rows = conn
        .query(
            "SELECT pay_method,count(*) FROM import_records WHERE batch_id=?1 GROUP BY pay_method ORDER BY pay_method",
            params![batch_id],
        )
        .await?;
    let mut counts = BTreeMap::<String, i64>::new();
    while let Some(row) = rows.next().await? {
        let pay_method = normalize_pay_method(&row.get::<String>(0)?);
        if !pay_method.is_empty() {
            *counts.entry(pay_method).or_default() += row.get::<i64>(1)?;
        }
    }
    Ok(counts
        .into_iter()
        .map(|(pay_method, count)| ImportPayMethodSummary {
            account_id: mappings.get(&pay_method).cloned(),
            pay_method,
            count,
        })
        .collect())
}

async fn aggregate(
    conn: &Connection,
    id: &str,
) -> Result<(ImportSummary, Vec<UnknownIssue>), ApiError> {
    let mut summary = ImportSummary::default();
    let mut rows=conn.query("SELECT disposition,direction,count(*),coalesce(sum(amount_cents),0) FROM import_records WHERE batch_id=?1 GROUP BY disposition,direction",params![id]).await?;
    while let Some(row) = rows.next().await? {
        let d: String = row.get(0)?;
        let dir: String = row.get(1)?;
        let item = ImportSummaryItem {
            count: row.get(2)?,
            amount_cents: row.get(3)?,
        };
        match (d.as_str(), dir.as_str()) {
            ("import", "income") => summary.import_income = item,
            ("import", "expense") => summary.import_expense = item,
            ("pending", _) => add_summary(&mut summary.pending, item),
            ("neutral", _) => add_summary(&mut summary.neutral, item),
            ("closed", _) => add_summary(&mut summary.closed, item),
            ("zero_amount", _) => add_summary(&mut summary.zero_amount, item),
            ("unknown", _) => add_summary(&mut summary.unknown, item),
            ("duplicate", _) => {
                summary.duplicate.count += item.count;
                summary.duplicate.amount_cents += item.amount_cents
            }
            _ => {}
        }
    }
    let mut unknown=conn.query("SELECT row_index,channel_status FROM import_records WHERE batch_id=?1 AND disposition='unknown' ORDER BY row_index",params![id]).await?;
    let mut issues = Vec::new();
    while let Some(r) = unknown.next().await? {
        issues.push(UnknownIssue {
            row_index: r.get(0)?,
            status: r.get(1)?,
        });
    }
    Ok((summary, issues))
}

async fn validate_account(
    conn: &Connection,
    user_id: &str,
    account_id: Option<&str>,
) -> Result<(), ApiError> {
    if let Some(account_id) = account_id {
        let mut accounts = conn
            .query(
                "SELECT id FROM ledger_accounts WHERE id=?1 AND user_id=?2 AND archived_at IS NULL",
                params![account_id, user_id],
            )
            .await?;
        if accounts.next().await?.is_none() {
            return Err(ApiError::validation("账户绑定包含无效或已归档账户"));
        }
    }
    Ok(())
}

async fn suggested_account(
    conn: &Connection,
    user_id: &str,
    batch_id: &str,
    status: &str,
    source_channel: &str,
    header_identity: Option<&HeaderIdentity>,
) -> Result<Option<String>, ApiError> {
    if status == "committed" {
        let mut rows = conn.query("SELECT DISTINCT account_id FROM ledger_transactions WHERE user_id=?1 AND import_batch_id=?2 AND account_id IS NOT NULL LIMIT 2", params![user_id, batch_id]).await?;
        let first = rows
            .next()
            .await?
            .map(|row| row.get::<String>(0))
            .transpose()?;
        return if rows.next().await?.is_none() {
            Ok(first)
        } else {
            Ok(None)
        };
    }

    if let Some(identity) = header_identity {
        let (account_type, field, value) = match identity {
            HeaderIdentity::AlipayEmail(value) => ("alipay_balance", "email", value),
            HeaderIdentity::WechatNickname(value) => ("wechat_balance", "nickname", value),
        };
        let sql = format!(
            "SELECT id FROM ledger_accounts WHERE user_id=?1 AND account_type=?2 AND archived_at IS NULL AND {field}=?3 LIMIT 2"
        );
        let mut rows = conn
            .query(&sql, params![user_id, account_type, value.clone()])
            .await?;
        let first = rows
            .next()
            .await?
            .map(|row| row.get::<String>(0))
            .transpose()?;
        if first.is_some() && rows.next().await?.is_none() {
            return Ok(first);
        }
    }

    let mut rows = conn.query("SELECT t.account_id FROM import_batches b JOIN ledger_transactions t ON t.import_batch_id=b.id AND t.user_id=b.user_id JOIN ledger_accounts a ON a.id=t.account_id AND a.user_id=b.user_id AND a.archived_at IS NULL WHERE b.id=(SELECT recent.id FROM import_batches recent WHERE recent.user_id=?1 AND recent.source_channel=?2 AND recent.status='committed' AND recent.id<>?3 ORDER BY recent.committed_at DESC,recent.id DESC LIMIT 1) LIMIT 1", params![user_id, source_channel, batch_id]).await?;
    if let Some(row) = rows.next().await? {
        return Ok(Some(row.get(0)?));
    }

    let account_type = match source_channel {
        "wechat" => "wechat_balance",
        "alipay" => "alipay_balance",
        _ => return Ok(None),
    };
    let mut rows = conn.query("SELECT id FROM ledger_accounts WHERE user_id=?1 AND account_type=?2 AND archived_at IS NULL LIMIT 2", params![user_id, account_type]).await?;
    let first = rows
        .next()
        .await?
        .map(|row| row.get::<String>(0))
        .transpose()?;
    if first.is_some() && rows.next().await?.is_none() {
        Ok(first)
    } else {
        Ok(None)
    }
}

fn header_identity(channel: SourceChannel, bytes: &[u8]) -> Option<HeaderIdentity> {
    match channel {
        SourceChannel::Alipay => alipay::header_email(bytes).map(HeaderIdentity::AlipayEmail),
        SourceChannel::Wechat => wechat::header_nickname(bytes).map(HeaderIdentity::WechatNickname),
        SourceChannel::Cmb | SourceChannel::Cmbc => None,
    }
}

fn add_summary(target: &mut ImportSummaryItem, item: ImportSummaryItem) {
    target.count += item.count;
    target.amount_cents += item.amount_cents;
}

fn outcome(status: &str, disposition: &str, has_tx: bool, was_committed: bool) -> &'static str {
    match (status, disposition, has_tx, was_committed) {
        (_, "duplicate", _, _) => "duplicate",
        ("preview", "import", _, _) => "will_import",
        ("preview", "neutral", _, _) => "will_import",
        ("committed", "import", true, _) => "imported",
        ("blocked", "unknown", _, _) => "blocked",
        ("discarded", "import", true, _) => "retained_modified",
        ("discarded", "import", false, true) => "removed",
        ("discarded", "import", false, false) => "abandoned",
        (_, "pending" | "neutral" | "closed" | "zero_amount", _, _) => "excluded",
        (_, "unknown", _, _) => "blocked",
        _ => "excluded",
    }
}
