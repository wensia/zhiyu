use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use axum::{
    Json,
    extract::{Query, State, rejection::QueryRejection},
};
use chrono::Utc;
use libsql::{Transaction, TransactionBehavior, params};
use reqwest::{Client, StatusCode, Url, redirect::Policy};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::time::{MissedTickBehavior, interval};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{AppState, auth::AuthUser, config::BillInboxConfig, error::ApiError};

const MAIL_CAPABILITY: &str = "urn:ietf:params:jmap:mail";
const CORE_CAPABILITY: &str = "urn:ietf:params:jmap:core";
const QUERY_PAGE_SIZE: usize = 200;
const GET_BATCH_SIZE: usize = 50;
const MAX_JMAP_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_QUERY_RESTARTS: usize = 3;
const MAX_QUERY_PAGES_PER_SYNC: usize = 100;
const MAX_CHANGE_PAGES_PER_SYNC: usize = 100;
const UNRESOLVED_ACCOUNT_ID: &str = "__jmap_session_unresolved__";

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BillInboxStatus {
    pub address: String,
    pub jmap_account_id: Option<String>,
    pub email_state: Option<String>,
    pub last_attempt_at: Option<String>,
    pub last_success_at: Option<String>,
    pub last_error_code: Option<String>,
    pub last_error_at: Option<String>,
    pub pending_count: u64,
    pub error_count: u64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BillInboxAttachmentView {
    pub name: Option<String>,
    pub media_type: String,
    pub size_bytes: u64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BillInboxMessageView {
    pub id: String,
    pub message_id_header: Option<String>,
    pub from_name: Option<String>,
    pub from_email: Option<String>,
    pub subject: Option<String>,
    pub received_at: String,
    pub size_bytes: u64,
    pub status: String,
    pub error_code: Option<String>,
    pub source_deleted_at: Option<String>,
    pub raw_available: bool,
    pub attachments: Vec<BillInboxAttachmentView>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BillInboxMessageList {
    pub items: Vec<BillInboxMessageView>,
    pub total: u64,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct BillInboxMessageQuery {
    /// 上一页返回的不透明游标。
    pub cursor: Option<String>,
    /// 每页条数，默认 50，最大 100。
    pub limit: Option<u32>,
}

pub fn spawn_scheduler(state: AppState) {
    let Some(config) = state.config.bill_inbox.clone() else {
        return;
    };
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(config.poll_interval_seconds));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            if let Err(error) =
                crate::backup::persist_bill_inbox_backup_owner(&state, &config.owner_email).await
            {
                tracing::error!(error = %error, "bill inbox backup owner persistence failed");
                continue;
            }
            if let Err(error) = sync_once(&state, &config).await {
                tracing::error!(error = %error, "bill inbox sync failed");
            }
        }
    });
}

#[utoipa::path(
    get,
    path = "/api/v1/bill-inbox/status",
    responses((status = 200, body = BillInboxStatus), (status = 401, body = crate::error::ErrorBody), (status = 403, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody)),
    security(("cookieAuth" = []), ("bearerAuth" = []))
)]
pub async fn status(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<BillInboxStatus>, ApiError> {
    let config = owner_config(&state, &user)?;
    let conn = state.connection().await?;
    let mut rows = conn
        .query(
            "SELECT jmap_account_id, email_state, last_attempt_at, last_success_at, last_error_code, last_error_at FROM bill_inbox_sync_state WHERE user_id = ?1 ORDER BY COALESCE(last_attempt_at, '') DESC LIMIT 1",
            [user.id.clone()],
        )
        .await?;
    let sync = rows.next().await?;
    let (
        jmap_account_id,
        email_state,
        last_attempt_at,
        last_success_at,
        last_error_code,
        last_error_at,
    ) = if let Some(row) = sync {
        (
            match row.get::<String>(0)? {
                value if value == UNRESOLVED_ACCOUNT_ID => None,
                value => Some(value),
            },
            row.get::<Option<String>>(1)?,
            row.get::<Option<String>>(2)?,
            row.get::<Option<String>>(3)?,
            row.get::<Option<String>>(4)?,
            row.get::<Option<String>>(5)?,
        )
    } else {
        (None, None, None, None, None, None)
    };
    drop(rows);
    let mut counts = conn
        .query(
            "SELECT SUM(CASE WHEN status = 'pending' THEN 1 ELSE 0 END), SUM(CASE WHEN status = 'error' THEN 1 ELSE 0 END) FROM bill_inbox_messages WHERE user_id = ?1",
            [user.id],
        )
        .await?;
    let count_row = counts.next().await?;
    let pending_count = count_row
        .as_ref()
        .and_then(|row| row.get::<Option<i64>>(0).ok().flatten())
        .unwrap_or(0) as u64;
    let error_count = count_row
        .as_ref()
        .and_then(|row| row.get::<Option<i64>>(1).ok().flatten())
        .unwrap_or(0) as u64;
    Ok(Json(BillInboxStatus {
        address: config.address.clone(),
        jmap_account_id,
        email_state,
        last_attempt_at,
        last_success_at,
        last_error_code,
        last_error_at,
        pending_count,
        error_count,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/bill-inbox/messages",
    params(BillInboxMessageQuery),
    responses((status = 200, body = BillInboxMessageList), (status = 400, body = crate::error::ErrorBody), (status = 401, body = crate::error::ErrorBody), (status = 403, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody)),
    security(("cookieAuth" = []), ("bearerAuth" = []))
)]
pub async fn list_messages(
    State(state): State<AppState>,
    user: AuthUser,
    query: Result<Query<BillInboxMessageQuery>, QueryRejection>,
) -> Result<Json<BillInboxMessageList>, ApiError> {
    let Query(query) =
        query.map_err(|_| ApiError::bad_request("invalid_query", "账单邮箱分页参数格式不正确"))?;
    owner_config(&state, &user)?;
    let conn = state.connection().await?;
    let limit = query.limit.unwrap_or(50).clamp(1, 100) as usize;
    let (boundary_received_at, boundary_id) = if let Some(cursor) = query.cursor {
        let mut cursor_rows = conn
            .query(
                "SELECT received_at, id FROM bill_inbox_messages WHERE user_id = ?1 AND id = ?2",
                params![user.id.clone(), cursor],
            )
            .await?;
        let Some(row) = cursor_rows.next().await? else {
            return Err(ApiError::bad_request(
                "invalid_cursor",
                "账单邮箱分页游标无效",
            ));
        };
        let boundary = (row.get::<String>(0)?, row.get::<String>(1)?);
        drop(cursor_rows);
        (Some(boundary.0), Some(boundary.1))
    } else {
        (None, None)
    };
    let mut total_rows = conn
        .query(
            "SELECT COUNT(*) FROM bill_inbox_messages WHERE user_id = ?1",
            [user.id.clone()],
        )
        .await?;
    let total = total_rows
        .next()
        .await?
        .map(|row| row.get::<i64>(0))
        .transpose()?
        .unwrap_or(0) as u64;
    drop(total_rows);

    let mut rows = if let (Some(received_at), Some(id)) = (boundary_received_at, boundary_id) {
        conn.query(
            "WITH page AS (SELECT id, message_id_header, from_name, from_email, subject, received_at, size_bytes, status, error_code, source_deleted_at, raw_blob_id, raw_content_blob_id, raw_content FROM bill_inbox_messages WHERE user_id = ?1 AND (received_at, id) < (?2, ?3) ORDER BY received_at DESC, id DESC LIMIT ?4) SELECT p.id, p.message_id_header, p.from_name, p.from_email, p.subject, p.received_at, p.size_bytes, p.status, p.error_code, p.source_deleted_at, p.raw_content IS NOT NULL AND p.raw_content_blob_id = p.raw_blob_id, a.name, a.media_type, a.size_bytes FROM page p LEFT JOIN bill_inbox_attachments a ON a.message_id = p.id ORDER BY p.received_at DESC, p.id DESC, a.ordinal ASC",
            params![user.id.clone(), received_at, id, (limit + 1) as i64],
        )
        .await?
    } else {
        conn.query(
            "WITH page AS (SELECT id, message_id_header, from_name, from_email, subject, received_at, size_bytes, status, error_code, source_deleted_at, raw_blob_id, raw_content_blob_id, raw_content FROM bill_inbox_messages WHERE user_id = ?1 ORDER BY received_at DESC, id DESC LIMIT ?2) SELECT p.id, p.message_id_header, p.from_name, p.from_email, p.subject, p.received_at, p.size_bytes, p.status, p.error_code, p.source_deleted_at, p.raw_content IS NOT NULL AND p.raw_content_blob_id = p.raw_blob_id, a.name, a.media_type, a.size_bytes FROM page p LEFT JOIN bill_inbox_attachments a ON a.message_id = p.id ORDER BY p.received_at DESC, p.id DESC, a.ordinal ASC",
            params![user.id, (limit + 1) as i64],
        )
        .await?
    };
    let mut items = Vec::<BillInboxMessageView>::new();
    while let Some(row) = rows.next().await? {
        let id: String = row.get(0)?;
        if items.last().map(|item| item.id.as_str()) != Some(id.as_str()) {
            items.push(BillInboxMessageView {
                id,
                message_id_header: row.get(1)?,
                from_name: row.get(2)?,
                from_email: row.get(3)?,
                subject: row.get(4)?,
                received_at: row.get(5)?,
                size_bytes: row.get::<i64>(6)?.max(0) as u64,
                status: row.get(7)?,
                error_code: row.get(8)?,
                source_deleted_at: row.get(9)?,
                raw_available: row.get::<i64>(10)? != 0,
                attachments: Vec::new(),
            });
        }
        let media_type = row.get::<Option<String>>(12)?;
        if let (Some(item), Some(media_type)) = (items.last_mut(), media_type) {
            item.attachments.push(BillInboxAttachmentView {
                name: row.get(11)?,
                media_type,
                size_bytes: row.get::<Option<i64>>(13)?.unwrap_or(0).max(0) as u64,
            });
        }
    }
    let has_more = items.len() > limit;
    items.truncate(limit);
    let next_cursor = has_more
        .then(|| items.last().map(|item| item.id.clone()))
        .flatten();
    Ok(Json(BillInboxMessageList {
        items,
        total,
        next_cursor,
    }))
}

fn owner_config<'a>(state: &'a AppState, user: &AuthUser) -> Result<&'a BillInboxConfig, ApiError> {
    let config = state
        .config
        .bill_inbox
        .as_ref()
        .ok_or_else(|| ApiError::not_found("账单邮箱未启用"))?;
    if user.email != config.owner_email {
        return Err(ApiError::forbidden("无权访问账单邮箱"));
    }
    Ok(config)
}

#[derive(Debug, Clone)]
struct RemoteAttachment {
    part_id: Option<String>,
    blob_id: String,
    name: Option<String>,
    media_type: String,
    size_bytes: i64,
}

#[derive(Debug, Clone)]
struct RemoteMessage {
    id: String,
    blob_id: String,
    message_id_header: Option<String>,
    from_name: Option<String>,
    from_email: Option<String>,
    subject: Option<String>,
    received_at: String,
    size_bytes: i64,
    attachments: Vec<RemoteAttachment>,
}

#[derive(Debug, Clone)]
struct QueryPage {
    ids: Vec<String>,
    query_state: String,
    total: usize,
}

#[derive(Debug, Clone)]
struct GetPage {
    messages: Vec<RemoteMessage>,
    not_found: Vec<String>,
}

#[derive(Debug, Clone)]
struct ChangesPage {
    created: Vec<String>,
    updated: Vec<String>,
    destroyed: Vec<String>,
    new_state: String,
    has_more_changes: bool,
}

#[derive(Debug, Clone)]
enum ChangesResult {
    Page(ChangesPage),
    CannotCalculate,
}

#[derive(Debug)]
enum RawDownload {
    Found(Vec<u8>),
    NotFound,
    TooLarge,
}

#[async_trait]
trait BillInboxSource: Send + Sync {
    fn account_id(&self) -> &str;
    async fn initial_state(&self) -> Result<String>;
    async fn query_page(&self, position: usize) -> Result<QueryPage>;
    async fn get_messages(&self, ids: &[String]) -> Result<GetPage>;
    async fn changes(&self, since_state: &str) -> Result<ChangesResult>;
    async fn download_raw(&self, blob_id: &str, max_bytes: usize) -> Result<RawDownload>;
}

async fn sync_once(state: &AppState, config: &BillInboxConfig) -> Result<()> {
    let owner_id = resolve_owner_id(state, &config.owner_email).await?;
    record_attempt(state, &owner_id, UNRESOLVED_ACCOUNT_ID).await?;
    let source = match JmapBillInboxSource::connect(config).await {
        Ok(source) => source,
        Err(error) => {
            record_failure(
                state,
                &owner_id,
                UNRESOLVED_ACCOUNT_ID,
                "session_connect_failed",
            )
            .await?;
            return Err(error);
        }
    };
    record_attempt(state, &owner_id, source.account_id()).await?;
    delete_sync_state(state, &owner_id, UNRESOLVED_ACCOUNT_ID).await?;
    let result = sync_from_source(state, config, &owner_id, &source).await;
    match result {
        Ok(()) => {
            record_success(state, &owner_id, source.account_id()).await?;
            Ok(())
        }
        Err(error) => {
            if let Err(record_error) =
                record_failure(state, &owner_id, source.account_id(), "sync_failed").await
            {
                tracing::error!(error = %record_error, "failed to record bill inbox sync error");
            }
            Err(error)
        }
    }
}

async fn resolve_owner_id(state: &AppState, email: &str) -> Result<String> {
    let conn = state
        .connection()
        .await
        .map_err(|error| anyhow!(error.message))?;
    let mut rows = conn
        .query(
            "SELECT id FROM users WHERE email = ?1 AND email_verified_at IS NOT NULL LIMIT 1",
            [email],
        )
        .await?;
    rows.next()
        .await?
        .map(|row| row.get::<String>(0))
        .transpose()?
        .ok_or_else(|| anyhow!("configured bill inbox owner is missing or unverified"))
}

async fn sync_from_source(
    state: &AppState,
    config: &BillInboxConfig,
    owner_id: &str,
    source: &dyn BillInboxSource,
) -> Result<()> {
    let mut cursor = load_cursor(state, owner_id, source.account_id()).await?;
    for _ in 0..2 {
        if let Some(current) = cursor.as_deref() {
            match apply_changes(state, config, owner_id, source, current).await? {
                ApplyChanges::Complete => return Ok(()),
                ApplyChanges::Reset => {
                    clear_cursor(state, owner_id, source.account_id()).await?;
                    cursor = None;
                    continue;
                }
            }
        } else {
            let initial_state = source.initial_state().await?;
            full_sync(state, config, owner_id, source).await?;
            match apply_changes(state, config, owner_id, source, &initial_state).await? {
                ApplyChanges::Complete => return Ok(()),
                ApplyChanges::Reset => {
                    clear_cursor(state, owner_id, source.account_id()).await?;
                    cursor = None;
                    continue;
                }
            }
        }
    }
    bail!("JMAP changes could not be calculated after a full resync")
}

async fn full_sync(
    state: &AppState,
    config: &BillInboxConfig,
    owner_id: &str,
    source: &dyn BillInboxSource,
) -> Result<()> {
    for _ in 0..MAX_QUERY_RESTARTS {
        let mut position = 0;
        let mut query_state: Option<String> = None;
        let mut expected_total: Option<usize> = None;
        let mut snapshot_ids = HashSet::new();
        let mut page_count = 0;
        loop {
            if page_count >= MAX_QUERY_PAGES_PER_SYNC {
                bail!("JMAP Email/query exceeded the per-sync page limit")
            }
            page_count += 1;
            let page = source.query_page(position).await?;
            if let Some(expected) = query_state.as_deref() {
                if expected != page.query_state {
                    break;
                }
            } else {
                query_state = Some(page.query_state.clone());
            }
            if let Some(expected) = expected_total {
                if expected != page.total {
                    break;
                }
            } else {
                expected_total = Some(page.total);
            }
            if page.ids.is_empty() && position < page.total {
                bail!("JMAP Email/query returned an empty page before total was reached")
            }
            if position.saturating_add(page.ids.len()) > page.total {
                bail!("JMAP Email/query returned more ids than its declared total")
            }
            let mut page_ids = HashSet::new();
            for id in &page.ids {
                if !page_ids.insert(id.clone()) || snapshot_ids.contains(id) {
                    bail!("JMAP Email/query returned a duplicate id")
                }
            }
            stage_ids(state, config, owner_id, source, &page.ids).await?;
            snapshot_ids.extend(page.ids.iter().cloned());
            position += page.ids.len();
            if position >= page.total {
                reconcile_full_snapshot(state, owner_id, source.account_id(), &snapshot_ids)
                    .await?;
                return Ok(());
            }
        }
    }
    bail!("JMAP Email/query state changed repeatedly during full sync")
}

async fn reconcile_full_snapshot(
    state: &AppState,
    owner_id: &str,
    account_id: &str,
    snapshot_ids: &HashSet<String>,
) -> Result<()> {
    let conn = state
        .connection()
        .await
        .map_err(|error| anyhow!(error.message))?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    let mut rows = tx
        .query(
            "SELECT jmap_email_id FROM bill_inbox_messages WHERE user_id = ?1 AND jmap_account_id = ?2 AND source_deleted_at IS NULL",
            params![owner_id, account_id],
        )
        .await?;
    let mut missing = Vec::new();
    while let Some(row) = rows.next().await? {
        let id: String = row.get(0)?;
        if !snapshot_ids.contains(&id) {
            missing.push(id);
        }
    }
    drop(rows);
    let now = Utc::now().to_rfc3339();
    for id in missing {
        tx.execute(
            "UPDATE bill_inbox_messages SET source_deleted_at = ?1, updated_at = ?1 WHERE user_id = ?2 AND jmap_account_id = ?3 AND jmap_email_id = ?4 AND source_deleted_at IS NULL",
            params![now.clone(), owner_id, account_id, id],
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum ApplyChanges {
    Complete,
    Reset,
}

async fn apply_changes(
    state: &AppState,
    config: &BillInboxConfig,
    owner_id: &str,
    source: &dyn BillInboxSource,
    initial_state: &str,
) -> Result<ApplyChanges> {
    let mut since = initial_state.to_owned();
    for _ in 0..MAX_CHANGE_PAGES_PER_SYNC {
        let page = match source.changes(&since).await? {
            ChangesResult::CannotCalculate => return Ok(ApplyChanges::Reset),
            ChangesResult::Page(page) => page,
        };
        if page.has_more_changes && page.new_state == since {
            bail!("JMAP Email/changes did not advance its state")
        }
        let mut changed = page.created;
        changed.extend(page.updated);
        let mut seen = HashSet::new();
        changed.retain(|id| seen.insert(id.clone()));
        stage_ids(state, config, owner_id, source, &changed).await?;
        persist_destroyed_and_cursor(
            state,
            owner_id,
            source.account_id(),
            &page.destroyed,
            &page.new_state,
        )
        .await?;
        since = page.new_state;
        if !page.has_more_changes {
            return Ok(ApplyChanges::Complete);
        }
    }
    bail!("JMAP Email/changes exceeded the per-sync page limit")
}

async fn stage_ids(
    state: &AppState,
    config: &BillInboxConfig,
    owner_id: &str,
    source: &dyn BillInboxSource,
    ids: &[String],
) -> Result<()> {
    for batch in ids.chunks(GET_BATCH_SIZE) {
        let page = source.get_messages(batch).await?;
        validate_get_page(batch, &page)?;
        let staged_raw_blobs =
            load_staged_raw_blobs(state, owner_id, source.account_id(), batch).await?;
        for id in page.not_found {
            mark_deleted(state, owner_id, source.account_id(), &id).await?;
        }
        for message in page.messages {
            let raw_already_staged =
                staged_raw_blobs.contains(&(message.id.clone(), message.blob_id.clone()));
            let raw = if raw_already_staged
                || message.size_bytes < 0
                || message.size_bytes as usize > config.max_message_bytes
            {
                None
            } else {
                match source
                    .download_raw(&message.blob_id, config.max_message_bytes)
                    .await?
                {
                    RawDownload::Found(raw) => Some(raw),
                    RawDownload::TooLarge => None,
                    RawDownload::NotFound => {
                        let recheck = source
                            .get_messages(std::slice::from_ref(&message.id))
                            .await?;
                        validate_get_page(std::slice::from_ref(&message.id), &recheck)?;
                        if recheck.not_found == [message.id.clone()] {
                            persist_deleted_metadata(
                                state,
                                owner_id,
                                source.account_id(),
                                &config.address,
                                &message,
                            )
                            .await?;
                            continue;
                        }
                        bail!("raw message disappeared while metadata still exists")
                    }
                }
            };
            persist_message(
                state,
                owner_id,
                source.account_id(),
                &config.address,
                &message,
                raw,
            )
            .await?;
        }
    }
    Ok(())
}

fn validate_get_page(requested_ids: &[String], page: &GetPage) -> Result<()> {
    let requested = requested_ids.iter().cloned().collect::<HashSet<_>>();
    if requested.len() != requested_ids.len() {
        bail!("JMAP Email/get request contained duplicate ids")
    }
    let mut accounted_for = HashSet::new();
    for message in &page.messages {
        if !requested.contains(&message.id) || !accounted_for.insert(message.id.clone()) {
            bail!("JMAP Email/get returned an unexpected or duplicate message id")
        }
    }
    for id in &page.not_found {
        if !requested.contains(id) || !accounted_for.insert(id.clone()) {
            bail!("JMAP Email/get returned an unexpected or duplicate notFound id")
        }
    }
    if accounted_for != requested {
        bail!("JMAP Email/get omitted requested ids")
    }
    Ok(())
}

async fn load_staged_raw_blobs(
    state: &AppState,
    owner_id: &str,
    account_id: &str,
    email_ids: &[String],
) -> Result<HashSet<(String, String)>> {
    if email_ids.is_empty() {
        return Ok(HashSet::new());
    }
    let conn = state
        .connection()
        .await
        .map_err(|error| anyhow!(error.message))?;
    let placeholders = std::iter::repeat_n("?", email_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT jmap_email_id, raw_content_blob_id FROM bill_inbox_messages WHERE user_id = ? AND jmap_account_id = ? AND jmap_email_id IN ({placeholders}) AND raw_content IS NOT NULL AND raw_content_blob_id IS NOT NULL"
    );
    let values = std::iter::once(owner_id.to_owned())
        .chain(std::iter::once(account_id.to_owned()))
        .chain(email_ids.iter().cloned());
    let mut rows = conn.query(&sql, libsql::params_from_iter(values)).await?;
    let mut staged = HashSet::new();
    while let Some(row) = rows.next().await? {
        staged.insert((row.get::<String>(0)?, row.get::<String>(1)?));
    }
    Ok(staged)
}

async fn load_cursor(state: &AppState, owner_id: &str, account_id: &str) -> Result<Option<String>> {
    let conn = state
        .connection()
        .await
        .map_err(|error| anyhow!(error.message))?;
    let mut rows = conn
        .query(
            "SELECT email_state FROM bill_inbox_sync_state WHERE user_id = ?1 AND jmap_account_id = ?2",
            params![owner_id, account_id],
        )
        .await?;
    Ok(rows
        .next()
        .await?
        .map(|row| row.get::<Option<String>>(0))
        .transpose()?
        .flatten())
}

async fn record_attempt(state: &AppState, owner_id: &str, account_id: &str) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let conn = state
        .connection()
        .await
        .map_err(|error| anyhow!(error.message))?;
    conn.execute(
        "INSERT INTO bill_inbox_sync_state(user_id, jmap_account_id, last_attempt_at) VALUES (?1, ?2, ?3) ON CONFLICT(user_id, jmap_account_id) DO UPDATE SET last_attempt_at = excluded.last_attempt_at",
        params![owner_id, account_id, now],
    )
    .await?;
    Ok(())
}

async fn record_success(state: &AppState, owner_id: &str, account_id: &str) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let conn = state
        .connection()
        .await
        .map_err(|error| anyhow!(error.message))?;
    conn.execute(
        "UPDATE bill_inbox_sync_state SET last_success_at = ?1, last_error_code = NULL, last_error_at = NULL WHERE user_id = ?2 AND jmap_account_id = ?3",
        params![now, owner_id, account_id],
    )
    .await?;
    Ok(())
}

async fn record_failure(
    state: &AppState,
    owner_id: &str,
    account_id: &str,
    error_code: &str,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let conn = state
        .connection()
        .await
        .map_err(|error| anyhow!(error.message))?;
    conn.execute(
        "UPDATE bill_inbox_sync_state SET last_error_code = ?1, last_error_at = ?2 WHERE user_id = ?3 AND jmap_account_id = ?4",
        params![error_code, now, owner_id, account_id],
    )
    .await?;
    Ok(())
}

async fn clear_cursor(state: &AppState, owner_id: &str, account_id: &str) -> Result<()> {
    let conn = state
        .connection()
        .await
        .map_err(|error| anyhow!(error.message))?;
    conn.execute(
        "UPDATE bill_inbox_sync_state SET email_state = NULL WHERE user_id = ?1 AND jmap_account_id = ?2",
        params![owner_id, account_id],
    )
    .await?;
    Ok(())
}

async fn delete_sync_state(state: &AppState, owner_id: &str, account_id: &str) -> Result<()> {
    let conn = state
        .connection()
        .await
        .map_err(|error| anyhow!(error.message))?;
    conn.execute(
        "DELETE FROM bill_inbox_sync_state WHERE user_id = ?1 AND jmap_account_id = ?2",
        params![owner_id, account_id],
    )
    .await?;
    Ok(())
}

async fn persist_destroyed_and_cursor(
    state: &AppState,
    owner_id: &str,
    account_id: &str,
    destroyed: &[String],
    new_state: &str,
) -> Result<()> {
    let conn = state
        .connection()
        .await
        .map_err(|error| anyhow!(error.message))?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    let now = Utc::now().to_rfc3339();
    for id in destroyed {
        tx.execute(
            "UPDATE bill_inbox_messages SET source_deleted_at = COALESCE(source_deleted_at, ?1), updated_at = ?1 WHERE user_id = ?2 AND jmap_account_id = ?3 AND jmap_email_id = ?4",
            params![now.clone(), owner_id, account_id, id.clone()],
        )
        .await?;
    }
    tx.execute(
        "UPDATE bill_inbox_sync_state SET email_state = ?1 WHERE user_id = ?2 AND jmap_account_id = ?3",
        params![new_state, owner_id, account_id],
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn mark_deleted(
    state: &AppState,
    owner_id: &str,
    account_id: &str,
    email_id: &str,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let conn = state
        .connection()
        .await
        .map_err(|error| anyhow!(error.message))?;
    conn.execute(
        "UPDATE bill_inbox_messages SET source_deleted_at = COALESCE(source_deleted_at, ?1), updated_at = ?1 WHERE user_id = ?2 AND jmap_account_id = ?3 AND jmap_email_id = ?4",
        params![now, owner_id, account_id, email_id],
    )
    .await?;
    Ok(())
}

async fn persist_message(
    state: &AppState,
    owner_id: &str,
    account_id: &str,
    configured_address: &str,
    message: &RemoteMessage,
    raw: Option<Vec<u8>>,
) -> Result<()> {
    let (raw_sha256, status, error_code) = if let Some(raw) = raw.as_ref() {
        (Some(format!("{:x}", Sha256::digest(raw))), "pending", None)
    } else {
        (None, "error", Some("message_too_large"))
    };
    persist_message_with_status(
        state,
        owner_id,
        account_id,
        configured_address,
        message,
        raw_sha256,
        raw,
        status,
        error_code,
        None,
    )
    .await
}

async fn persist_deleted_metadata(
    state: &AppState,
    owner_id: &str,
    account_id: &str,
    configured_address: &str,
    message: &RemoteMessage,
) -> Result<()> {
    persist_message_with_status(
        state,
        owner_id,
        account_id,
        configured_address,
        message,
        None,
        None,
        "error",
        Some("source_deleted_during_download"),
        Some(Utc::now().to_rfc3339()),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn persist_message_with_status(
    state: &AppState,
    owner_id: &str,
    account_id: &str,
    configured_address: &str,
    message: &RemoteMessage,
    raw_sha256: Option<String>,
    raw: Option<Vec<u8>>,
    status: &str,
    error_code: Option<&str>,
    source_deleted_at: Option<String>,
) -> Result<()> {
    let conn = state
        .connection()
        .await
        .map_err(|error| anyhow!(error.message))?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    persist_message_tx(
        &tx,
        owner_id,
        account_id,
        configured_address,
        message,
        raw_sha256,
        raw,
        status,
        error_code,
        source_deleted_at,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn persist_message_tx(
    tx: &Transaction,
    owner_id: &str,
    account_id: &str,
    configured_address: &str,
    message: &RemoteMessage,
    raw_sha256: Option<String>,
    raw: Option<Vec<u8>>,
    status: &str,
    error_code: Option<&str>,
    source_deleted_at: Option<String>,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let (existing_id, content_changed) = {
        let mut rows = tx
            .query(
                "SELECT id, raw_blob_id FROM bill_inbox_messages WHERE user_id = ?1 AND jmap_account_id = ?2 AND jmap_email_id = ?3",
                params![owner_id, account_id, message.id.clone()],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            let id = row.get::<String>(0)?;
            let old_blob_id = row.get::<String>(1)?;
            (Some(id), old_blob_id != message.blob_id)
        } else {
            (None, false)
        }
    };
    let id = existing_id.unwrap_or_else(|| Uuid::now_v7().to_string());
    let raw_content_blob_id = raw.as_ref().map(|_| message.blob_id.clone());
    tx.execute(
        "INSERT INTO bill_inbox_messages(id, user_id, jmap_account_id, jmap_email_id, configured_address, raw_blob_id, message_id_header, from_name, from_email, subject, received_at, size_bytes, raw_sha256, raw_content, raw_content_blob_id, status, error_code, source_deleted_at, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?19) ON CONFLICT(user_id, jmap_account_id, jmap_email_id) DO UPDATE SET configured_address = excluded.configured_address, raw_blob_id = excluded.raw_blob_id, message_id_header = excluded.message_id_header, from_name = excluded.from_name, from_email = excluded.from_email, subject = excluded.subject, received_at = excluded.received_at, size_bytes = excluded.size_bytes, raw_sha256 = CASE WHEN excluded.raw_content IS NULL AND bill_inbox_messages.raw_content IS NOT NULL THEN bill_inbox_messages.raw_sha256 ELSE excluded.raw_sha256 END, raw_content = CASE WHEN excluded.raw_content IS NULL AND bill_inbox_messages.raw_content IS NOT NULL THEN bill_inbox_messages.raw_content ELSE excluded.raw_content END, raw_content_blob_id = CASE WHEN excluded.raw_content IS NULL AND bill_inbox_messages.raw_content IS NOT NULL THEN bill_inbox_messages.raw_content_blob_id ELSE excluded.raw_content_blob_id END, status = CASE WHEN ?20 != 0 AND excluded.raw_content IS NULL THEN excluded.status WHEN excluded.raw_content IS NULL AND bill_inbox_messages.raw_content IS NOT NULL THEN bill_inbox_messages.status WHEN ?20 != 0 THEN excluded.status WHEN excluded.source_deleted_at IS NOT NULL THEN excluded.status WHEN bill_inbox_messages.status IN ('processed', 'ignored') THEN bill_inbox_messages.status WHEN bill_inbox_messages.status = 'error' AND excluded.status = 'pending' THEN 'pending' ELSE excluded.status END, error_code = CASE WHEN ?20 != 0 AND excluded.raw_content IS NULL THEN excluded.error_code WHEN excluded.raw_content IS NULL AND bill_inbox_messages.raw_content IS NOT NULL THEN bill_inbox_messages.error_code WHEN ?20 != 0 THEN excluded.error_code WHEN excluded.source_deleted_at IS NOT NULL THEN excluded.error_code WHEN bill_inbox_messages.status IN ('processed', 'ignored') THEN bill_inbox_messages.error_code WHEN bill_inbox_messages.status = 'error' AND excluded.status = 'pending' THEN NULL ELSE excluded.error_code END, source_deleted_at = CASE WHEN excluded.source_deleted_at IS NOT NULL THEN COALESCE(bill_inbox_messages.source_deleted_at, excluded.source_deleted_at) ELSE NULL END, updated_at = excluded.updated_at",
        params![
            id.clone(),
            owner_id,
            account_id,
            message.id.clone(),
            configured_address,
            message.blob_id.clone(),
            message.message_id_header.clone(),
            message.from_name.clone(),
            message.from_email.clone(),
            message.subject.clone(),
            message.received_at.clone(),
            message.size_bytes.max(0),
            raw_sha256,
            raw,
            raw_content_blob_id,
            status,
            error_code,
            source_deleted_at,
            now,
            i64::from(content_changed),
        ],
    )
    .await?;
    tx.execute(
        "DELETE FROM bill_inbox_attachments WHERE message_id = ?1",
        [id.clone()],
    )
    .await?;
    for (ordinal, attachment) in message.attachments.iter().enumerate() {
        tx.execute(
            "INSERT INTO bill_inbox_attachments(id, message_id, ordinal, part_id, blob_id, name, media_type, size_bytes) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                Uuid::now_v7().to_string(),
                id.clone(),
                ordinal as i64,
                attachment.part_id.clone(),
                attachment.blob_id.clone(),
                attachment.name.clone(),
                attachment.media_type.clone(),
                attachment.size_bytes.max(0),
            ],
        )
        .await?;
    }
    Ok(())
}

#[derive(Clone)]
struct JmapBillInboxSource {
    client: Client,
    username: String,
    password: String,
    api_url: Url,
    download_url: String,
    account_id: String,
}

impl JmapBillInboxSource {
    async fn connect(config: &BillInboxConfig) -> Result<Self> {
        let session_url = Url::parse(&config.session_url).context("invalid JMAP session URL")?;
        let client = Client::builder()
            .redirect(Policy::none())
            .timeout(Duration::from_secs(30))
            .build()?;
        let response = client
            .get(session_url.clone())
            .basic_auth(&config.username, Some(&config.password))
            .send()
            .await
            .context("JMAP session request failed")?;
        let bytes = read_limited(response, MAX_JMAP_RESPONSE_BYTES).await?;
        let session: JmapSession =
            serde_json::from_slice(&bytes).context("invalid JMAP session")?;
        let api_url = Url::parse(&session.api_url).context("invalid JMAP apiUrl")?;
        ensure_same_origin(&session_url, &api_url)?;
        let download_probe = replace_download_template(&session.download_url, "account", "blob");
        let download_url = Url::parse(&download_probe).context("invalid JMAP downloadUrl")?;
        ensure_same_origin(&session_url, &download_url)?;
        let account_id = session
            .primary_accounts
            .get(MAIL_CAPABILITY)
            .cloned()
            .ok_or_else(|| anyhow!("JMAP session has no primary mail account"))?;
        Ok(Self {
            client,
            username: config.username.clone(),
            password: config.password.clone(),
            api_url,
            download_url: session.download_url,
            account_id,
        })
    }

    async fn call<T: DeserializeOwned>(&self, method: &str, arguments: Value) -> Result<T> {
        let body = json!({
            "using": [CORE_CAPABILITY, MAIL_CAPABILITY],
            "methodCalls": [[method, arguments, "bill-inbox"]],
        });
        let response = self
            .client
            .post(self.api_url.clone())
            .basic_auth(&self.username, Some(&self.password))
            .json(&body)
            .send()
            .await
            .context("JMAP API request failed")?;
        let bytes = read_limited(response, MAX_JMAP_RESPONSE_BYTES).await?;
        let envelope: MethodEnvelope =
            serde_json::from_slice(&bytes).context("invalid JMAP method response")?;
        let response = envelope
            .method_responses
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("JMAP response had no methodResponses"))?;
        if response.0 == "error" {
            let kind = response
                .1
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            bail!("JMAP method returned error type {kind}")
        }
        if response.0 != method {
            bail!("JMAP returned an unexpected method response")
        }
        serde_json::from_value(response.1).context("invalid JMAP method arguments")
    }

    async fn call_changes(&self, since_state: &str) -> Result<ChangesResult> {
        let body = json!({
            "using": [CORE_CAPABILITY, MAIL_CAPABILITY],
            "methodCalls": [["Email/changes", {
                "accountId": self.account_id,
                "sinceState": since_state,
                "maxChanges": 500,
            }, "bill-inbox"]],
        });
        let response = self
            .client
            .post(self.api_url.clone())
            .basic_auth(&self.username, Some(&self.password))
            .json(&body)
            .send()
            .await
            .context("JMAP Email/changes request failed")?;
        let bytes = read_limited(response, MAX_JMAP_RESPONSE_BYTES).await?;
        let envelope: MethodEnvelope = serde_json::from_slice(&bytes)?;
        let response = envelope
            .method_responses
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("JMAP response had no methodResponses"))?;
        if response.0 == "error"
            && response.1.get("type").and_then(Value::as_str) == Some("cannotCalculateChanges")
        {
            return Ok(ChangesResult::CannotCalculate);
        }
        if response.0 != "Email/changes" {
            bail!("JMAP returned an unexpected Email/changes response")
        }
        let value: EmailChangesResult = serde_json::from_value(response.1)?;
        Ok(ChangesResult::Page(ChangesPage {
            created: value.created,
            updated: value.updated,
            destroyed: value.destroyed,
            new_state: value.new_state,
            has_more_changes: value.has_more_changes,
        }))
    }
}

#[async_trait]
impl BillInboxSource for JmapBillInboxSource {
    fn account_id(&self) -> &str {
        &self.account_id
    }

    async fn initial_state(&self) -> Result<String> {
        let result: EmailGetResult = self
            .call(
                "Email/get",
                json!({
                    "accountId": self.account_id,
                    "ids": [],
                    "properties": ["id"],
                }),
            )
            .await?;
        Ok(result.state)
    }

    async fn query_page(&self, position: usize) -> Result<QueryPage> {
        let result: EmailQueryResult = self
            .call(
                "Email/query",
                json!({
                    "accountId": self.account_id,
                    "filter": null,
                    "sort": [{"property": "receivedAt", "isAscending": true}],
                    "position": position,
                    "limit": QUERY_PAGE_SIZE,
                    "calculateTotal": true,
                }),
            )
            .await?;
        if result.position != position {
            bail!("JMAP Email/query returned an unexpected position")
        }
        Ok(QueryPage {
            ids: result.ids,
            query_state: result.query_state,
            total: result.total,
        })
    }

    async fn get_messages(&self, ids: &[String]) -> Result<GetPage> {
        if ids.is_empty() {
            return Ok(GetPage {
                messages: Vec::new(),
                not_found: Vec::new(),
            });
        }
        let result: EmailGetResult = self
            .call(
                "Email/get",
                json!({
                    "accountId": self.account_id,
                    "ids": ids,
                    "properties": [
                        "id", "blobId", "messageId", "from", "subject", "receivedAt",
                        "size", "attachments"
                    ],
                }),
            )
            .await?;
        let messages = result
            .list
            .into_iter()
            .map(|email| {
                let from = email.from.and_then(|mut values| values.drain(..).next());
                RemoteMessage {
                    id: email.id,
                    blob_id: email.blob_id,
                    message_id_header: email.message_id.and_then(|mut ids| ids.drain(..).next()),
                    from_name: from.as_ref().and_then(|value| value.name.clone()),
                    from_email: from.and_then(|value| value.email),
                    subject: email.subject,
                    received_at: email.received_at,
                    size_bytes: email.size,
                    attachments: email
                        .attachments
                        .unwrap_or_default()
                        .into_iter()
                        .map(|attachment| RemoteAttachment {
                            part_id: attachment.part_id,
                            blob_id: attachment.blob_id,
                            name: attachment.name,
                            media_type: attachment.media_type,
                            size_bytes: attachment.size,
                        })
                        .collect(),
                }
            })
            .collect();
        Ok(GetPage {
            messages,
            not_found: result.not_found.unwrap_or_default(),
        })
    }

    async fn changes(&self, since_state: &str) -> Result<ChangesResult> {
        self.call_changes(since_state).await
    }

    async fn download_raw(&self, blob_id: &str, max_bytes: usize) -> Result<RawDownload> {
        let url = replace_download_template(&self.download_url, &self.account_id, blob_id);
        let url = Url::parse(&url).context("invalid expanded JMAP download URL")?;
        ensure_same_origin(&self.api_url, &url)?;
        let response = self
            .client
            .get(url)
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .await
            .context("JMAP raw message download failed")?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(RawDownload::NotFound);
        }
        read_raw_limited(response, max_bytes).await
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JmapSession {
    api_url: String,
    download_url: String,
    primary_accounts: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MethodEnvelope {
    method_responses: Vec<(String, Value, String)>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EmailQueryResult {
    query_state: String,
    position: usize,
    ids: Vec<String>,
    total: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EmailGetResult {
    state: String,
    #[serde(default)]
    list: Vec<JmapEmail>,
    not_found: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JmapEmail {
    id: String,
    blob_id: String,
    message_id: Option<Vec<String>>,
    from: Option<Vec<JmapAddress>>,
    subject: Option<String>,
    received_at: String,
    size: i64,
    attachments: Option<Vec<JmapAttachment>>,
}

#[derive(Debug, Deserialize)]
struct JmapAddress {
    name: Option<String>,
    email: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JmapAttachment {
    part_id: Option<String>,
    blob_id: String,
    name: Option<String>,
    #[serde(rename = "type")]
    media_type: String,
    size: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EmailChangesResult {
    created: Vec<String>,
    updated: Vec<String>,
    destroyed: Vec<String>,
    new_state: String,
    has_more_changes: bool,
}

async fn read_limited(response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    let status = response.status();
    if !status.is_success() {
        bail!("remote mail service returned HTTP {status}")
    }
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        bail!("remote mail response exceeded the configured size limit")
    }
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > limit {
            bail!("remote mail response exceeded the configured size limit")
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn read_raw_limited(mut response: reqwest::Response, limit: usize) -> Result<RawDownload> {
    let status = response.status();
    if !status.is_success() {
        bail!("remote mail service returned HTTP {status}")
    }
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Ok(RawDownload::TooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Ok(RawDownload::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(RawDownload::Found(bytes))
}

fn ensure_same_origin(expected: &Url, actual: &Url) -> Result<()> {
    if expected.scheme() != actual.scheme()
        || expected.host_str() != actual.host_str()
        || expected.port_or_known_default() != actual.port_or_known_default()
    {
        bail!("JMAP advertised a cross-origin endpoint")
    }
    Ok(())
}

fn replace_download_template(template: &str, account_id: &str, blob_id: &str) -> String {
    template
        .replace("{accountId}", &percent_encode(account_id))
        .replace("{blobId}", &percent_encode(blob_id))
        .replace("{name}", "message.eml")
        .replace("{type}", "message%2Frfc822")
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            use std::fmt::Write;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{HashMap, HashSet, VecDeque},
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use super::*;
    use crate::{
        backup::BackupStatusStore, config::Config, db, email::DevFileEmailSender,
        rate_limit::RateLimiter,
    };

    struct FakeSource {
        account_id: String,
        initial_state: String,
        queries: Mutex<VecDeque<QueryPage>>,
        messages: HashMap<String, RemoteMessage>,
        missing: HashSet<String>,
        omitted: HashSet<String>,
        omitted_after_metadata: HashSet<String>,
        vanishing_after_metadata: HashSet<String>,
        get_counts: Mutex<HashMap<String, usize>>,
        changes: HashMap<String, ChangesResult>,
        raw: HashMap<String, Vec<u8>>,
        failing_blobs: HashSet<String>,
        download_calls: AtomicUsize,
    }

    impl FakeSource {
        fn new(messages: Vec<RemoteMessage>) -> Self {
            Self {
                account_id: "account-1".into(),
                initial_state: "S0".into(),
                queries: Mutex::new(VecDeque::new()),
                messages: messages
                    .into_iter()
                    .map(|message| (message.id.clone(), message))
                    .collect(),
                missing: HashSet::new(),
                omitted: HashSet::new(),
                omitted_after_metadata: HashSet::new(),
                vanishing_after_metadata: HashSet::new(),
                get_counts: Mutex::new(HashMap::new()),
                changes: HashMap::new(),
                raw: HashMap::new(),
                failing_blobs: HashSet::new(),
                download_calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl BillInboxSource for FakeSource {
        fn account_id(&self) -> &str {
            &self.account_id
        }

        async fn initial_state(&self) -> Result<String> {
            Ok(self.initial_state.clone())
        }

        async fn query_page(&self, _position: usize) -> Result<QueryPage> {
            self.queries
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| anyhow!("unexpected query call"))
        }

        async fn get_messages(&self, ids: &[String]) -> Result<GetPage> {
            let mut counts = self.get_counts.lock().unwrap();
            let mut messages = Vec::new();
            let mut not_found = Vec::new();
            for id in ids {
                let count = counts.entry(id.clone()).or_default();
                if self.omitted.contains(id)
                    || (self.omitted_after_metadata.contains(id) && *count > 0)
                {
                    *count += 1;
                    continue;
                }
                let vanished = self.vanishing_after_metadata.contains(id) && *count > 0;
                if !vanished && !self.missing.contains(id) {
                    if let Some(message) = self.messages.get(id) {
                        messages.push(message.clone());
                    } else {
                        not_found.push(id.clone());
                    }
                } else {
                    not_found.push(id.clone());
                }
                *count += 1;
            }
            Ok(GetPage {
                messages,
                not_found,
            })
        }

        async fn changes(&self, since_state: &str) -> Result<ChangesResult> {
            self.changes
                .get(since_state)
                .cloned()
                .ok_or_else(|| anyhow!("unexpected changes state {since_state}"))
        }

        async fn download_raw(&self, blob_id: &str, _max_bytes: usize) -> Result<RawDownload> {
            self.download_calls.fetch_add(1, Ordering::SeqCst);
            if self.failing_blobs.contains(blob_id) {
                bail!("simulated download failure")
            }
            Ok(self
                .raw
                .get(blob_id)
                .cloned()
                .map(RawDownload::Found)
                .unwrap_or(RawDownload::NotFound))
        }
    }

    fn message(id: &str, size_bytes: i64) -> RemoteMessage {
        RemoteMessage {
            id: id.into(),
            blob_id: format!("blob-{id}"),
            message_id_header: Some(format!("<{id}@example.com>")),
            from_name: Some("账单服务".into()),
            from_email: Some("bills@example.com".into()),
            subject: Some(format!("账单 {id}")),
            received_at: "2026-08-11T00:00:00Z".into(),
            size_bytes,
            attachments: vec![RemoteAttachment {
                part_id: Some("1".into()),
                blob_id: format!("attachment-{id}"),
                name: Some("statement.pdf".into()),
                media_type: "application/pdf".into(),
                size_bytes: 123,
            }],
        }
    }

    async fn test_state() -> (AppState, BillInboxConfig, String, tempfile::TempDir) {
        let root = tempfile::tempdir().unwrap();
        let bill_inbox = BillInboxConfig {
            session_url: "https://mail.example.com/jmap/session".into(),
            username: "inbox@example.com".into(),
            password: "secret".into(),
            address: "zhiyu-bills@example.com".into(),
            owner_email: "owner@example.com".into(),
            poll_interval_seconds: 300,
            max_message_bytes: 1024,
        };
        let config = Config {
            app_env: "test".into(),
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            public_base_url: "http://test.local".into(),
            database_url: format!("file:{}", root.path().join("test.db").display()),
            turso_auth_token: None,
            dev_mail_dir: root.path().join("mail"),
            web_dist_dir: root.path().join("web"),
            bill_inbox: Some(bill_inbox.clone()),
        };
        let database = db::connect(&config).await.unwrap();
        let owner_id = "owner-1".to_owned();
        let conn = database.connect().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .await
            .unwrap();
        conn.execute(
            "INSERT INTO users(id, email, password_hash, timezone, email_verified_at, created_at, updated_at) VALUES (?1, ?2, 'hash', 'Asia/Shanghai', '2026-08-11T00:00:00Z', '2026-08-11T00:00:00Z', '2026-08-11T00:00:00Z')",
            params![owner_id.clone(), bill_inbox.owner_email.clone()],
        )
        .await
        .unwrap();
        let state = AppState {
            db: Arc::new(database),
            config: Arc::new(config),
            email: Arc::new(DevFileEmailSender::new(root.path().join("mail"))),
            rate_limiter: RateLimiter::default(),
            backup_status: BackupStatusStore::default(),
        };
        (state, bill_inbox, owner_id, root)
    }

    async fn cursor(state: &AppState, owner_id: &str) -> Option<String> {
        load_cursor(state, owner_id, "account-1").await.unwrap()
    }

    #[tokio::test]
    async fn initial_sync_catches_changes_pages_and_replay_is_idempotent() {
        let (state, config, owner_id, _root) = test_state().await;
        let m1 = message("m1", 100);
        let m2 = message("m2", 100);
        let mut source = FakeSource::new(vec![m1, m2]);
        source.queries.get_mut().unwrap().push_back(QueryPage {
            ids: vec!["m1".into()],
            query_state: "Q1".into(),
            total: 1,
        });
        source.raw.insert("blob-m1".into(), b"raw one".to_vec());
        source.raw.insert("blob-m2".into(), b"raw two".to_vec());
        source.changes.insert(
            "S0".into(),
            ChangesResult::Page(ChangesPage {
                created: vec!["m2".into()],
                updated: vec![],
                destroyed: vec![],
                new_state: "S1".into(),
                has_more_changes: true,
            }),
        );
        source.changes.insert(
            "S1".into(),
            ChangesResult::Page(ChangesPage {
                created: vec![],
                updated: vec!["m1".into()],
                destroyed: vec![],
                new_state: "S2".into(),
                has_more_changes: false,
            }),
        );
        source.changes.insert(
            "S2".into(),
            ChangesResult::Page(ChangesPage {
                created: vec![],
                updated: vec![],
                destroyed: vec![],
                new_state: "S2".into(),
                has_more_changes: false,
            }),
        );
        record_attempt(&state, &owner_id, source.account_id())
            .await
            .unwrap();

        sync_from_source(&state, &config, &owner_id, &source)
            .await
            .unwrap();
        sync_from_source(&state, &config, &owner_id, &source)
            .await
            .unwrap();

        assert_eq!(cursor(&state, &owner_id).await.as_deref(), Some("S2"));
        let conn = state.connection().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT COUNT(*), COUNT(DISTINCT jmap_email_id) FROM bill_inbox_messages WHERE user_id = ?1",
                [owner_id],
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<i64>(0).unwrap(), 2);
        assert_eq!(row.get::<i64>(1).unwrap(), 2);
    }

    #[tokio::test]
    async fn transient_failure_does_not_advance_cursor() {
        let (state, config, owner_id, _root) = test_state().await;
        let mut source = FakeSource::new(vec![message("m1", 100)]);
        source.failing_blobs.insert("blob-m1".into());
        source.changes.insert(
            "S1".into(),
            ChangesResult::Page(ChangesPage {
                created: vec!["m1".into()],
                updated: vec![],
                destroyed: vec![],
                new_state: "S2".into(),
                has_more_changes: false,
            }),
        );
        record_attempt(&state, &owner_id, source.account_id())
            .await
            .unwrap();
        persist_destroyed_and_cursor(&state, &owner_id, source.account_id(), &[], "S1")
            .await
            .unwrap();

        assert!(
            sync_from_source(&state, &config, &owner_id, &source)
                .await
                .is_err()
        );
        assert_eq!(cursor(&state, &owner_id).await.as_deref(), Some("S1"));
    }

    #[tokio::test]
    async fn oversize_message_is_terminal_and_destroyed_source_keeps_raw_evidence() {
        let (state, config, owner_id, _root) = test_state().await;
        let old = message("old", 100);
        persist_message(
            &state,
            &owner_id,
            "account-1",
            &config.address,
            &old,
            Some(b"old raw".to_vec()),
        )
        .await
        .unwrap();
        let mut source = FakeSource::new(vec![message("big", 2048)]);
        source.changes.insert(
            "S1".into(),
            ChangesResult::Page(ChangesPage {
                created: vec!["big".into()],
                updated: vec![],
                destroyed: vec!["old".into()],
                new_state: "S2".into(),
                has_more_changes: false,
            }),
        );
        record_attempt(&state, &owner_id, source.account_id())
            .await
            .unwrap();
        persist_destroyed_and_cursor(&state, &owner_id, source.account_id(), &[], "S1")
            .await
            .unwrap();

        sync_from_source(&state, &config, &owner_id, &source)
            .await
            .unwrap();

        assert_eq!(cursor(&state, &owner_id).await.as_deref(), Some("S2"));
        assert_eq!(source.download_calls.load(Ordering::SeqCst), 0);
        let conn = state.connection().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT status, error_code, raw_content IS NULL FROM bill_inbox_messages WHERE jmap_email_id = 'big'",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "error");
        assert_eq!(row.get::<String>(1).unwrap(), "message_too_large");
        assert_eq!(row.get::<i64>(2).unwrap(), 1);
        let mut rows = conn
            .query(
                "SELECT raw_content IS NOT NULL, source_deleted_at IS NOT NULL FROM bill_inbox_messages WHERE jmap_email_id = 'old'",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<i64>(0).unwrap(), 1);
        assert_eq!(row.get::<i64>(1).unwrap(), 1);
    }

    #[tokio::test]
    async fn full_sync_restarts_when_query_state_changes() {
        let (state, config, owner_id, _root) = test_state().await;
        let mut source = FakeSource::new(vec![message("m1", 100), message("m2", 100)]);
        source.raw.insert("blob-m1".into(), b"raw one".to_vec());
        source.raw.insert("blob-m2".into(), b"raw two".to_vec());
        source.queries.get_mut().unwrap().extend([
            QueryPage {
                ids: vec!["m1".into()],
                query_state: "Q1".into(),
                total: 2,
            },
            QueryPage {
                ids: vec!["m2".into()],
                query_state: "Q2".into(),
                total: 2,
            },
            QueryPage {
                ids: vec!["m1".into(), "m2".into()],
                query_state: "Q3".into(),
                total: 2,
            },
        ]);

        full_sync(&state, &config, &owner_id, &source)
            .await
            .unwrap();

        let conn = state.connection().await.unwrap();
        let mut rows = conn
            .query("SELECT COUNT(*) FROM bill_inbox_messages", ())
            .await
            .unwrap();
        assert_eq!(
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            2
        );
    }

    #[tokio::test]
    async fn duplicate_query_id_across_pages_fails_before_snapshot_reconciliation() {
        let (state, config, owner_id, _root) = test_state().await;
        persist_message(
            &state,
            &owner_id,
            "account-1",
            &config.address,
            &message("keep", 100),
            Some(b"keep raw".to_vec()),
        )
        .await
        .unwrap();
        let mut source = FakeSource::new(vec![message("m1", 100)]);
        source.raw.insert("blob-m1".into(), b"raw one".to_vec());
        source.queries.get_mut().unwrap().extend([
            QueryPage {
                ids: vec!["m1".into()],
                query_state: "Q1".into(),
                total: 2,
            },
            QueryPage {
                ids: vec!["m1".into()],
                query_state: "Q1".into(),
                total: 2,
            },
        ]);

        assert!(
            full_sync(&state, &config, &owner_id, &source)
                .await
                .is_err()
        );
        let conn = state.connection().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT source_deleted_at FROM bill_inbox_messages WHERE jmap_email_id = 'keep'",
                (),
            )
            .await
            .unwrap();
        assert!(
            rows.next()
                .await
                .unwrap()
                .unwrap()
                .get::<Option<String>>(0)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn full_query_page_limit_accepts_exact_boundary_and_fails_closed_beyond_it() {
        let (state, config, owner_id, _root) = test_state().await;

        let mut exact = FakeSource::new(vec![]);
        let exact_ids = (0..MAX_QUERY_PAGES_PER_SYNC)
            .map(|index| format!("exact-{index}"))
            .collect::<Vec<_>>();
        exact.missing.extend(exact_ids.iter().cloned());
        exact
            .queries
            .get_mut()
            .unwrap()
            .extend(exact_ids.iter().map(|id| QueryPage {
                ids: vec![id.clone()],
                query_state: "Q-exact".into(),
                total: MAX_QUERY_PAGES_PER_SYNC,
            }));
        full_sync(&state, &config, &owner_id, &exact).await.unwrap();

        persist_message(
            &state,
            &owner_id,
            "account-1",
            &config.address,
            &message("keep", 100),
            Some(b"keep raw".to_vec()),
        )
        .await
        .unwrap();
        let mut overflow = FakeSource::new(vec![]);
        let overflow_ids = (0..MAX_QUERY_PAGES_PER_SYNC)
            .map(|index| format!("overflow-{index}"))
            .collect::<Vec<_>>();
        overflow.missing.extend(overflow_ids.iter().cloned());
        overflow
            .queries
            .get_mut()
            .unwrap()
            .extend(overflow_ids.iter().map(|id| QueryPage {
                ids: vec![id.clone()],
                query_state: "Q-overflow".into(),
                total: MAX_QUERY_PAGES_PER_SYNC + 1,
            }));
        assert!(
            full_sync(&state, &config, &owner_id, &overflow)
                .await
                .is_err()
        );
        let conn = state.connection().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT source_deleted_at FROM bill_inbox_messages WHERE jmap_email_id = 'keep'",
                (),
            )
            .await
            .unwrap();
        assert!(
            rows.next()
                .await
                .unwrap()
                .unwrap()
                .get::<Option<String>>(0)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn full_resync_marks_records_missing_from_the_stable_snapshot_deleted() {
        let (state, config, owner_id, _root) = test_state().await;
        persist_message(
            &state,
            &owner_id,
            "account-1",
            &config.address,
            &message("stale", 100),
            Some(b"stale raw".to_vec()),
        )
        .await
        .unwrap();
        let mut source = FakeSource::new(vec![]);
        source.queries.get_mut().unwrap().push_back(QueryPage {
            ids: vec![],
            query_state: "Q1".into(),
            total: 0,
        });
        source.changes.insert(
            "S0".into(),
            ChangesResult::Page(ChangesPage {
                created: vec![],
                updated: vec![],
                destroyed: vec![],
                new_state: "S1".into(),
                has_more_changes: false,
            }),
        );
        record_attempt(&state, &owner_id, source.account_id())
            .await
            .unwrap();

        sync_from_source(&state, &config, &owner_id, &source)
            .await
            .unwrap();

        let conn = state.connection().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT raw_content IS NOT NULL, source_deleted_at IS NOT NULL FROM bill_inbox_messages WHERE jmap_email_id = 'stale'",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<i64>(0).unwrap(), 1);
        assert_eq!(row.get::<i64>(1).unwrap(), 1);
    }

    #[tokio::test]
    async fn contradictory_empty_query_page_fails_closed_without_reconciling() {
        let (state, config, owner_id, _root) = test_state().await;
        persist_message(
            &state,
            &owner_id,
            "account-1",
            &config.address,
            &message("keep", 100),
            Some(b"keep raw".to_vec()),
        )
        .await
        .unwrap();
        let mut source = FakeSource::new(vec![]);
        source.queries.get_mut().unwrap().push_back(QueryPage {
            ids: vec![],
            query_state: "Q1".into(),
            total: 1,
        });

        assert!(
            full_sync(&state, &config, &owner_id, &source)
                .await
                .is_err()
        );
        let conn = state.connection().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT source_deleted_at IS NULL FROM bill_inbox_messages WHERE jmap_email_id = 'keep'",
                (),
            )
            .await
            .unwrap();
        assert_eq!(
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn raw_download_delete_race_creates_tombstone_and_preserves_existing_evidence() {
        let (state, config, owner_id, _root) = test_state().await;
        let m1 = message("m1", 100);
        let mut source = FakeSource::new(vec![m1.clone()]);
        source.vanishing_after_metadata.insert("m1".into());
        source.changes.insert(
            "S1".into(),
            ChangesResult::Page(ChangesPage {
                created: vec!["m1".into()],
                updated: vec![],
                destroyed: vec![],
                new_state: "S2".into(),
                has_more_changes: false,
            }),
        );
        record_attempt(&state, &owner_id, source.account_id())
            .await
            .unwrap();
        persist_destroyed_and_cursor(&state, &owner_id, source.account_id(), &[], "S1")
            .await
            .unwrap();

        sync_from_source(&state, &config, &owner_id, &source)
            .await
            .unwrap();
        let conn = state.connection().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT status, error_code, raw_content IS NULL, source_deleted_at IS NOT NULL FROM bill_inbox_messages WHERE jmap_email_id = 'm1'",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "error");
        assert_eq!(
            row.get::<String>(1).unwrap(),
            "source_deleted_during_download"
        );
        assert_eq!(row.get::<i64>(2).unwrap(), 1);
        assert_eq!(row.get::<i64>(3).unwrap(), 1);
        drop(row);
        drop(rows);
        drop(conn);

        let conn = state.connection().await.unwrap();
        conn.execute(
            "UPDATE bill_inbox_messages SET source_deleted_at = '2026-08-11T00:00:00Z' WHERE jmap_email_id = 'm1'",
            (),
        )
        .await
        .unwrap();
        drop(conn);
        persist_deleted_metadata(&state, &owner_id, "account-1", &config.address, &m1)
            .await
            .unwrap();
        let conn = state.connection().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT source_deleted_at FROM bill_inbox_messages WHERE jmap_email_id = 'm1'",
                (),
            )
            .await
            .unwrap();
        assert_eq!(
            rows.next()
                .await
                .unwrap()
                .unwrap()
                .get::<String>(0)
                .unwrap(),
            "2026-08-11T00:00:00Z"
        );
        drop(rows);
        drop(conn);

        let m2 = message("m2", 100);
        persist_message(
            &state,
            &owner_id,
            "account-1",
            &config.address,
            &m2,
            Some(b"preserve this raw".to_vec()),
        )
        .await
        .unwrap();
        let mut changed_m2 = m2;
        changed_m2.blob_id = "blob-m2-v2".into();
        let mut source = FakeSource::new(vec![changed_m2]);
        source.vanishing_after_metadata.insert("m2".into());
        source.changes.insert(
            "S2".into(),
            ChangesResult::Page(ChangesPage {
                created: vec![],
                updated: vec!["m2".into()],
                destroyed: vec![],
                new_state: "S3".into(),
                has_more_changes: false,
            }),
        );
        sync_from_source(&state, &config, &owner_id, &source)
            .await
            .unwrap();
        let conn = state.connection().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT raw_content, raw_sha256 IS NOT NULL, source_deleted_at IS NOT NULL FROM bill_inbox_messages WHERE jmap_email_id = 'm2'",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<Vec<u8>>(0).unwrap(), b"preserve this raw");
        assert_eq!(row.get::<i64>(1).unwrap(), 1);
        assert_eq!(row.get::<i64>(2).unwrap(), 1);
    }

    #[tokio::test]
    async fn mailbox_metadata_update_preserves_processed_status() {
        let (state, config, owner_id, _root) = test_state().await;
        let m1 = message("m1", 100);
        persist_message(
            &state,
            &owner_id,
            "account-1",
            &config.address,
            &m1,
            Some(b"old raw".to_vec()),
        )
        .await
        .unwrap();
        let conn = state.connection().await.unwrap();
        conn.execute(
            "UPDATE bill_inbox_messages SET status = 'processed' WHERE jmap_email_id = 'm1'",
            (),
        )
        .await
        .unwrap();
        drop(conn);
        let mut source = FakeSource::new(vec![m1]);
        source.raw.insert("blob-m1".into(), b"new raw".to_vec());
        source.changes.insert(
            "S1".into(),
            ChangesResult::Page(ChangesPage {
                created: vec![],
                updated: vec!["m1".into()],
                destroyed: vec![],
                new_state: "S2".into(),
                has_more_changes: false,
            }),
        );
        record_attempt(&state, &owner_id, source.account_id())
            .await
            .unwrap();
        persist_destroyed_and_cursor(&state, &owner_id, source.account_id(), &[], "S1")
            .await
            .unwrap();

        sync_from_source(&state, &config, &owner_id, &source)
            .await
            .unwrap();

        let conn = state.connection().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT status, raw_content FROM bill_inbox_messages WHERE jmap_email_id = 'm1'",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "processed");
        assert_eq!(row.get::<Vec<u8>>(1).unwrap(), b"old raw");
        assert_eq!(source.download_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn changed_raw_blob_resets_processed_message_to_pending() {
        let (state, config, owner_id, _root) = test_state().await;
        let old = message("m1", 100);
        persist_message(
            &state,
            &owner_id,
            "account-1",
            &config.address,
            &old,
            Some(b"old raw".to_vec()),
        )
        .await
        .unwrap();
        let conn = state.connection().await.unwrap();
        conn.execute(
            "UPDATE bill_inbox_messages SET status = 'processed' WHERE jmap_email_id = 'm1'",
            (),
        )
        .await
        .unwrap();
        drop(conn);

        let mut changed = old;
        changed.blob_id = "blob-m1-v2".into();
        let mut source = FakeSource::new(vec![changed]);
        source
            .raw
            .insert("blob-m1-v2".into(), b"replacement raw".to_vec());
        source.changes.insert(
            "S1".into(),
            ChangesResult::Page(ChangesPage {
                created: vec![],
                updated: vec!["m1".into()],
                destroyed: vec![],
                new_state: "S2".into(),
                has_more_changes: false,
            }),
        );
        record_attempt(&state, &owner_id, source.account_id())
            .await
            .unwrap();
        persist_destroyed_and_cursor(&state, &owner_id, source.account_id(), &[], "S1")
            .await
            .unwrap();

        sync_from_source(&state, &config, &owner_id, &source)
            .await
            .unwrap();
        let conn = state.connection().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT status, error_code, raw_content FROM bill_inbox_messages WHERE jmap_email_id = 'm1'",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "pending");
        assert!(row.get::<Option<String>>(1).unwrap().is_none());
        assert_eq!(row.get::<Vec<u8>>(2).unwrap(), b"replacement raw");
        assert_eq!(source.download_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn changed_oversize_blob_keeps_old_evidence_without_mislabeling_it() {
        let (state, config, owner_id, _root) = test_state().await;
        let old = message("m1", 100);
        persist_message(
            &state,
            &owner_id,
            "account-1",
            &config.address,
            &old,
            Some(b"old evidence".to_vec()),
        )
        .await
        .unwrap();
        let mut changed = old;
        changed.blob_id = "blob-m1-v2".into();
        changed.size_bytes = config.max_message_bytes as i64 + 1;
        let mut source = FakeSource::new(vec![changed]);
        source.changes.insert(
            "S1".into(),
            ChangesResult::Page(ChangesPage {
                created: vec![],
                updated: vec!["m1".into()],
                destroyed: vec![],
                new_state: "S2".into(),
                has_more_changes: false,
            }),
        );
        record_attempt(&state, &owner_id, source.account_id())
            .await
            .unwrap();
        persist_destroyed_and_cursor(&state, &owner_id, source.account_id(), &[], "S1")
            .await
            .unwrap();

        sync_from_source(&state, &config, &owner_id, &source)
            .await
            .unwrap();
        let conn = state.connection().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT raw_blob_id, raw_content_blob_id, raw_content, status, error_code FROM bill_inbox_messages WHERE jmap_email_id = 'm1'",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "blob-m1-v2");
        assert_eq!(row.get::<String>(1).unwrap(), "blob-m1");
        assert_eq!(row.get::<Vec<u8>>(2).unwrap(), b"old evidence");
        assert_eq!(row.get::<String>(3).unwrap(), "error");
        assert_eq!(row.get::<String>(4).unwrap(), "message_too_large");

        let owner = AuthUser {
            id: owner_id,
            email: config.owner_email,
            timezone: "Asia/Shanghai".into(),
            session_hash: None,
        };
        let list = list_messages(
            State(state),
            owner,
            Ok(Query(BillInboxMessageQuery::default())),
        )
        .await
        .unwrap()
        .0;
        assert!(!list.items[0].raw_available);
    }

    #[tokio::test]
    async fn lowering_message_limit_never_erases_previously_staged_raw() {
        let (state, mut config, owner_id, _root) = test_state().await;
        let m1 = message("m1", 100);
        persist_message(
            &state,
            &owner_id,
            "account-1",
            &config.address,
            &m1,
            Some(b"original evidence".to_vec()),
        )
        .await
        .unwrap();
        config.max_message_bytes = 10;
        let mut source = FakeSource::new(vec![m1]);
        source.changes.insert(
            "S1".into(),
            ChangesResult::Page(ChangesPage {
                created: vec![],
                updated: vec!["m1".into()],
                destroyed: vec![],
                new_state: "S2".into(),
                has_more_changes: false,
            }),
        );
        record_attempt(&state, &owner_id, source.account_id())
            .await
            .unwrap();
        persist_destroyed_and_cursor(&state, &owner_id, source.account_id(), &[], "S1")
            .await
            .unwrap();

        sync_from_source(&state, &config, &owner_id, &source)
            .await
            .unwrap();

        let conn = state.connection().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT raw_content, raw_sha256 IS NOT NULL, status, error_code FROM bill_inbox_messages WHERE jmap_email_id = 'm1'",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<Vec<u8>>(0).unwrap(), b"original evidence");
        assert_eq!(row.get::<i64>(1).unwrap(), 1);
        assert_eq!(row.get::<String>(2).unwrap(), "pending");
        assert!(row.get::<Option<String>>(3).unwrap().is_none());
    }

    #[tokio::test]
    async fn non_advancing_changes_page_fails_without_moving_cursor() {
        let (state, config, owner_id, _root) = test_state().await;
        let mut source = FakeSource::new(vec![]);
        source.changes.insert(
            "S1".into(),
            ChangesResult::Page(ChangesPage {
                created: vec![],
                updated: vec![],
                destroyed: vec![],
                new_state: "S1".into(),
                has_more_changes: true,
            }),
        );
        record_attempt(&state, &owner_id, source.account_id())
            .await
            .unwrap();
        persist_destroyed_and_cursor(&state, &owner_id, source.account_id(), &[], "S1")
            .await
            .unwrap();

        assert!(
            sync_from_source(&state, &config, &owner_id, &source)
                .await
                .is_err()
        );
        assert_eq!(cursor(&state, &owner_id).await.as_deref(), Some("S1"));
    }

    #[tokio::test]
    async fn incomplete_email_get_response_does_not_advance_cursor() {
        let (state, config, owner_id, _root) = test_state().await;
        let mut source = FakeSource::new(vec![message("m1", 100)]);
        source.omitted.insert("m1".into());
        source.changes.insert(
            "S1".into(),
            ChangesResult::Page(ChangesPage {
                created: vec!["m1".into()],
                updated: vec![],
                destroyed: vec![],
                new_state: "S2".into(),
                has_more_changes: false,
            }),
        );
        record_attempt(&state, &owner_id, source.account_id())
            .await
            .unwrap();
        persist_destroyed_and_cursor(&state, &owner_id, source.account_id(), &[], "S1")
            .await
            .unwrap();

        assert!(
            sync_from_source(&state, &config, &owner_id, &source)
                .await
                .is_err()
        );
        assert_eq!(cursor(&state, &owner_id).await.as_deref(), Some("S1"));
    }

    #[tokio::test]
    async fn incomplete_raw_not_found_recheck_does_not_create_tombstone_or_advance() {
        let (state, config, owner_id, _root) = test_state().await;
        let mut source = FakeSource::new(vec![message("m1", 100)]);
        source.omitted_after_metadata.insert("m1".into());
        source.changes.insert(
            "S1".into(),
            ChangesResult::Page(ChangesPage {
                created: vec!["m1".into()],
                updated: vec![],
                destroyed: vec![],
                new_state: "S2".into(),
                has_more_changes: false,
            }),
        );
        record_attempt(&state, &owner_id, source.account_id())
            .await
            .unwrap();
        persist_destroyed_and_cursor(&state, &owner_id, source.account_id(), &[], "S1")
            .await
            .unwrap();

        assert!(
            sync_from_source(&state, &config, &owner_id, &source)
                .await
                .is_err()
        );
        assert_eq!(cursor(&state, &owner_id).await.as_deref(), Some("S1"));
        let conn = state.connection().await.unwrap();
        let mut rows = conn
            .query(
                "SELECT COUNT(*) FROM bill_inbox_messages WHERE jmap_email_id = 'm1'",
                (),
            )
            .await
            .unwrap();
        assert_eq!(
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn api_views_are_owner_only_and_never_expose_raw_content() {
        let (state, config, owner_id, _root) = test_state().await;
        persist_message(
            &state,
            &owner_id,
            "account-1",
            &config.address,
            &message("m1", 100),
            Some(b"private raw message bytes".to_vec()),
        )
        .await
        .unwrap();
        record_attempt(&state, &owner_id, "account-1")
            .await
            .unwrap();
        let owner = AuthUser {
            id: owner_id.clone(),
            email: config.owner_email.clone(),
            timezone: "Asia/Shanghai".into(),
            session_hash: None,
        };

        let status_view = status(State(state.clone()), owner.clone()).await.unwrap().0;
        assert_eq!(status_view.pending_count, 1);
        let list = list_messages(
            State(state.clone()),
            owner.clone(),
            Ok(Query(BillInboxMessageQuery::default())),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(list.items.len(), 1);
        assert!(list.items[0].raw_available);
        let serialized = serde_json::to_string(&list).unwrap();
        assert!(!serialized.contains("private raw message bytes"));
        assert!(!serialized.contains("rawContent"));

        let other = AuthUser {
            id: "other".into(),
            email: "other@example.com".into(),
            timezone: "Asia/Shanghai".into(),
            session_hash: None,
        };
        assert_eq!(
            status(State(state.clone()), other)
                .await
                .unwrap_err()
                .status,
            axum::http::StatusCode::FORBIDDEN
        );

        let mut disabled_config = (*state.config).clone();
        disabled_config.bill_inbox = None;
        let mut disabled_state = state;
        disabled_state.config = Arc::new(disabled_config);
        assert_eq!(
            status(State(disabled_state), owner)
                .await
                .unwrap_err()
                .status,
            axum::http::StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn message_api_cursor_reaches_every_row_after_the_first_hundred() {
        let (state, config, owner_id, _root) = test_state().await;
        for index in 0..105 {
            let remote = message(&format!("m{index:03}"), 100);
            persist_message(
                &state,
                &owner_id,
                "account-1",
                &config.address,
                &remote,
                Some(format!("raw-{index}").into_bytes()),
            )
            .await
            .unwrap();
        }
        let owner = AuthUser {
            id: owner_id,
            email: config.owner_email,
            timezone: "Asia/Shanghai".into(),
            session_hash: None,
        };
        let mut cursor = None;
        let mut ids = HashSet::new();
        loop {
            let page = list_messages(
                State(state.clone()),
                owner.clone(),
                Ok(Query(BillInboxMessageQuery {
                    cursor,
                    limit: Some(40),
                })),
            )
            .await
            .unwrap()
            .0;
            assert_eq!(page.total, 105);
            for item in page.items {
                assert!(ids.insert(item.id));
            }
            let Some(next) = page.next_cursor else {
                break;
            };
            cursor = Some(next);
        }
        assert_eq!(ids.len(), 105);
    }

    #[test]
    fn endpoint_origin_and_template_expansion_are_strict() {
        let session = Url::parse("https://mail.example.com/jmap/session").unwrap();
        let same = Url::parse("https://mail.example.com/jmap/").unwrap();
        let other = Url::parse("https://evil.example/jmap/").unwrap();
        assert!(ensure_same_origin(&session, &same).is_ok());
        assert!(ensure_same_origin(&session, &other).is_err());
        assert_eq!(
            replace_download_template(
                "https://mail.example.com/d/{accountId}/{blobId}/{name}?accept={type}",
                "a/b",
                "x y",
            ),
            "https://mail.example.com/d/a%2Fb/x%20y/message.eml?accept=message%2Frfc822"
        );
    }
}
