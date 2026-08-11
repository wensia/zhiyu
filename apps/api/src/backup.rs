//! 数据库快照：备份链路里与「投递到哪」无关的那一半。
//!
//! API 进程在这里产出并保留一份**可信**的快照：一致快照、两道校验、内容哈希、
//! 先写临时文件再原子发布。桌面端只下载已发布快照，不接触在线数据库。
//!
//! 为什么是整库快照而不是按表导出文本：恢复目标是「一个满足全部约束、能继续记账
//! 的数据库」，不是「一批人类可读的记录」。逻辑导出要自行处理 NULL/BLOB/排序/外键
//! 装载顺序/视图/迁移版本，任何一处遗漏都可能让恢复出来的账本在语法上完好、在业务
//! 上错乱（悬空的撤销链、丢失的幂等记录）。原生快照恢复时不重新解释任何业务数据。

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration as StdDuration,
};

use anyhow::{Context, Result, anyhow, bail};
use axum::{
    Json,
    body::Body,
    extract::{Path as AxumPath, State},
    http::{StatusCode, header},
    response::Response,
};
use chrono::{DateTime, SecondsFormat, Utc};
use libsql::Database;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;
use tokio_util::io::ReaderStream;
use utoipa::ToSchema;
use uuid::Uuid;
use zhiyu_backup_policy::{Snapshot as RetentionSnapshot, plan_retention};

use crate::{AppState, auth::AuthUser, config::Config, error::ApiError};

pub const BACKUP_RETENTION_DAYS: i64 = 30;
pub const SNAPSHOT_FILE_PREFIX: &str = "zhiyu-";
pub const SNAPSHOT_FILE_SUFFIX: &str = ".db";
pub const MANIFEST_FILE_SUFFIX: &str = ".manifest.json";
const BILL_INBOX_BACKUP_OWNER_FILE: &str = ".bill-inbox-owner.json";

/// 快照旁边那份 manifest 的格式版本。恢复时先看它，不认识就直接拒绝。
pub const BACKUP_FORMAT_VERSION: u32 = 1;

/// 描述一份已经通过全部校验的快照。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub backup_format_version: u32,
    pub created_at_utc: String,
    pub database_sha256: String,
    pub database_size_bytes: u64,
    pub application_version: String,
    /// 完整的有序迁移版本列表，而不是 MAX(version)——否则 `[1,2,4]` 会被误读成
    /// 「已迁移到 4」，恢复时就发现不了中间缺了一个。
    pub schema_migration_versions: Vec<i64>,
    pub snapshot_method: String,
    pub source_journal_mode: String,
    pub integrity_check: String,
    pub foreign_key_violation_count: u64,
}

#[derive(Debug, Clone)]
pub struct ManagedSnapshot {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub database_path: PathBuf,
    pub manifest_path: PathBuf,
    pub manifest: Manifest,
}

#[derive(Debug, Clone, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BackupRuntimeStatus {
    pub running: bool,
    pub last_attempt_at: Option<String>,
    pub last_success_at: Option<String>,
    pub latest_snapshot_id: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct BackupStatusStore(Arc<RwLock<BackupRuntimeStatus>>);

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BackupListItem {
    pub id: String,
    pub created_at: String,
    pub size: u64,
    pub sha256: String,
    pub schema_version: i64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BillInboxBackupOwnerMarker {
    version: u32,
    owner_email: String,
}

impl BackupStatusStore {
    pub async fn get(&self) -> BackupRuntimeStatus {
        self.0.read().await.clone()
    }

    async fn started(&self, now: DateTime<Utc>) {
        let mut status = self.0.write().await;
        status.running = true;
        status.last_attempt_at = Some(now.to_rfc3339());
    }

    async fn succeeded(&self, now: DateTime<Utc>, latest_snapshot_id: Option<String>) {
        let mut status = self.0.write().await;
        status.running = false;
        status.last_success_at = Some(now.to_rfc3339());
        status.latest_snapshot_id = latest_snapshot_id;
        status.last_error = None;
    }

    async fn failed(&self, error: &anyhow::Error) {
        let mut status = self.0.write().await;
        status.running = false;
        status.last_error = Some(format!("{error:#}"));
    }
}

pub fn backup_directory(config: &Config) -> Result<PathBuf> {
    if config.database_url.starts_with("libsql://") || config.database_url.starts_with("https://") {
        bail!("远程 DATABASE_URL 无法生成本地 SQLite 快照");
    }
    let database_path = config
        .database_url
        .strip_prefix("file:")
        .unwrap_or(&config.database_url);
    if database_path == ":memory:" {
        bail!("内存数据库没有可用于备份的数据目录");
    }
    let parent = Path::new(database_path)
        .parent()
        .context("DATABASE_URL 没有数据目录")?;
    Ok(parent.join("backups"))
}

fn has_local_backup_storage(config: &Config) -> bool {
    !(config.database_url.starts_with("libsql://")
        || config.database_url.starts_with("https://")
        || config.database_url.strip_prefix("file:") == Some(":memory:")
        || config.database_url == ":memory:")
}

async fn read_bill_inbox_backup_owner(config: &Config) -> Result<Option<String>> {
    if !has_local_backup_storage(config) {
        return Ok(None);
    }
    let path = backup_directory(config)?.join(BILL_INBOX_BACKUP_OWNER_FILE);
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("无法读取账单备份所有者标记 {}", path.display()));
        }
    };
    let marker: BillInboxBackupOwnerMarker = serde_json::from_slice(&bytes)
        .with_context(|| format!("账单备份所有者标记损坏 {}", path.display()))?;
    if marker.version != 1 {
        bail!("不支持的账单备份所有者标记版本 {}", marker.version);
    }
    let owner_email = crate::domain::validate_email(&marker.owner_email)
        .map_err(|_| anyhow!("账单备份所有者标记中的邮箱无效"))?;
    Ok(Some(owner_email))
}

async fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        tokio::fs::File::open(path).await?.sync_all().await?;
    }
    Ok(())
}

/// 在任何账单原文进入数据库前，把整库备份的唯一所有者写到数据库之外。
///
/// 该标记与快照同目录，因此数据库恢复到旧版本、删除用户或临时关闭同步都不会让
/// 历史 raw BLOB 的下载权限退回为「任意已验证用户」。所有者变更必须人工迁移，
/// 这里遇到不一致时一律失败关闭。
pub(crate) async fn persist_bill_inbox_backup_owner(
    state: &AppState,
    owner_email: &str,
) -> Result<()> {
    // 远程 libSQL/内存数据库没有本地整库快照目录，也就不存在本标记要防护的下载面。
    if !has_local_backup_storage(&state.config) {
        return Ok(());
    }
    let owner_email = crate::domain::validate_email(owner_email)
        .map_err(|_| anyhow!("账单备份所有者邮箱无效"))?;
    if let Some(existing) = read_bill_inbox_backup_owner(&state.config).await? {
        if existing == owner_email {
            return Ok(());
        }
        bail!("账单备份所有者与已持久化标记不一致");
    }

    let backup_dir = backup_directory(&state.config)?;
    tokio::fs::create_dir_all(&backup_dir)
        .await
        .with_context(|| format!("无法创建备份目录 {}", backup_dir.display()))?;
    let destination = backup_dir.join(BILL_INBOX_BACKUP_OWNER_FILE);
    let temporary = backup_dir.join(format!(".bill-inbox-owner-{}.partial", Uuid::now_v7()));
    let marker = serde_json::to_vec_pretty(&BillInboxBackupOwnerMarker {
        version: 1,
        owner_email: owner_email.clone(),
    })?;

    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .await
        .with_context(|| format!("无法创建账单备份所有者标记 {}", temporary.display()))?;
    if let Err(error) = async {
        file.write_all(&marker).await?;
        file.write_all(b"\n").await?;
        file.sync_all().await?;
        drop(file);
        Ok::<_, std::io::Error>(())
    }
    .await
    {
        tokio::fs::remove_file(&temporary).await.ok();
        return Err(error)
            .with_context(|| format!("无法发布账单备份所有者标记 {}", destination.display()));
    }
    match tokio::fs::hard_link(&temporary, &destination).await {
        Ok(()) => {
            tokio::fs::remove_file(&temporary).await?;
            sync_directory(&backup_dir).await?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            tokio::fs::remove_file(&temporary).await.ok();
            match read_bill_inbox_backup_owner(&state.config).await? {
                Some(existing) if existing == owner_email => Ok(()),
                Some(_) => bail!("账单备份所有者与并发发布的标记不一致"),
                None => bail!("账单备份所有者标记并发发布失败"),
            }
        }
        Err(error) => {
            tokio::fs::remove_file(&temporary).await.ok();
            Err(error).with_context(|| {
                format!("无法原子发布账单备份所有者标记 {}", destination.display())
            })
        }
    }
}

async fn persist_current_bill_inbox_backup_owner(state: &AppState) -> Result<()> {
    let mut owners = std::collections::HashSet::new();
    if let Some(config) = state.config.bill_inbox.as_ref() {
        owners.insert(config.owner_email.clone());
    }
    let conn = state
        .connection()
        .await
        .map_err(|error| anyhow!(error.message))?;
    let mut rows = conn
        .query(
            "SELECT DISTINCT u.email FROM users u JOIN (SELECT user_id FROM bill_inbox_sync_state UNION SELECT user_id FROM bill_inbox_messages) inbox_owner ON inbox_owner.user_id = u.id LIMIT 2",
            (),
        )
        .await?;
    while let Some(row) = rows.next().await? {
        owners.insert(row.get::<String>(0)?);
    }
    match owners.len() {
        0 => Ok(()),
        1 => persist_bill_inbox_backup_owner(state, owners.iter().next().unwrap()).await,
        _ => bail!("数据库中存在多个账单备份所有者，拒绝生成整库快照"),
    }
}

pub fn snapshot_id_at(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub fn parse_snapshot_id(id: &str) -> Result<DateTime<Utc>> {
    if id.len() != 20 {
        bail!("备份 ID 格式不正确");
    }
    let parsed = DateTime::parse_from_rfc3339(id)
        .context("备份 ID 不是 RFC3339 时间戳")?
        .with_timezone(&Utc);
    if snapshot_id_at(parsed) != id {
        bail!("备份 ID 不是规范 UTC 时间戳");
    }
    Ok(parsed)
}

pub async fn list_managed_snapshots(backup_dir: &Path) -> Result<Vec<ManagedSnapshot>> {
    tokio::fs::create_dir_all(backup_dir)
        .await
        .with_context(|| format!("无法创建备份目录 {}", backup_dir.display()))?;
    let mut entries = tokio::fs::read_dir(backup_dir)
        .await
        .with_context(|| format!("无法读取备份目录 {}", backup_dir.display()))?;
    let mut snapshots = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        let Some(id) = file_name
            .strip_prefix(SNAPSHOT_FILE_PREFIX)
            .and_then(|value| value.strip_suffix(MANIFEST_FILE_SUFFIX))
        else {
            continue;
        };
        let id_created_at =
            parse_snapshot_id(id).with_context(|| format!("备份清单文件名不合法：{file_name}"))?;
        let manifest_path = entry.path();
        let manifest: Manifest = serde_json::from_slice(
            &tokio::fs::read(&manifest_path)
                .await
                .with_context(|| format!("无法读取备份清单 {}", manifest_path.display()))?,
        )
        .with_context(|| format!("备份清单不是有效 JSON：{}", manifest_path.display()))?;
        let created_at = DateTime::parse_from_rfc3339(&manifest.created_at_utc)
            .context("备份清单 createdAtUtc 不是 RFC3339 时间戳")?
            .with_timezone(&Utc);
        if created_at != id_created_at {
            bail!("备份清单时间与文件名不一致：{file_name}");
        }
        let database_path =
            backup_dir.join(format!("{SNAPSHOT_FILE_PREFIX}{id}{SNAPSHOT_FILE_SUFFIX}"));
        let actual_size = tokio::fs::metadata(&database_path)
            .await
            .with_context(|| format!("备份快照不存在：{}", database_path.display()))?
            .len();
        if actual_size != manifest.database_size_bytes {
            bail!(
                "备份快照长度不匹配：{}（清单 {}，实际 {}）",
                database_path.display(),
                manifest.database_size_bytes,
                actual_size
            );
        }
        snapshots.push(ManagedSnapshot {
            id: id.to_owned(),
            created_at,
            database_path,
            manifest_path,
            manifest,
        });
    }
    snapshots.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.id.cmp(&left.id))
    });
    Ok(snapshots)
}

async fn sync_file(path: &Path) -> Result<()> {
    tokio::fs::OpenOptions::new()
        .read(true)
        .open(path)
        .await?
        .sync_all()
        .await?;
    Ok(())
}

pub async fn create_managed_snapshot(
    db: &Database,
    backup_dir: &Path,
    now: DateTime<Utc>,
) -> Result<ManagedSnapshot> {
    tokio::fs::create_dir_all(backup_dir).await?;
    let id = snapshot_id_at(now);
    let database_path =
        backup_dir.join(format!("{SNAPSHOT_FILE_PREFIX}{id}{SNAPSHOT_FILE_SUFFIX}"));
    let manifest_path =
        backup_dir.join(format!("{SNAPSHOT_FILE_PREFIX}{id}{MANIFEST_FILE_SUFFIX}"));
    if database_path.exists() || manifest_path.exists() {
        bail!("同一秒的备份已经存在：{id}");
    }

    let staging_dir = backup_dir.join(".staging");
    let (staged_database, mut manifest) =
        create_snapshot(db, &staging_dir, env!("CARGO_PKG_VERSION")).await?;
    manifest.created_at_utc = id.clone();
    sync_file(&staged_database).await?;

    let staged_manifest = staging_dir.join(format!("manifest-{}.partial", Uuid::now_v7()));
    tokio::fs::write(&staged_manifest, serde_json::to_vec_pretty(&manifest)?).await?;
    sync_file(&staged_manifest).await?;
    tokio::fs::rename(&staged_database, &database_path)
        .await
        .context("无法原子发布备份快照")?;
    if let Err(error) = tokio::fs::rename(&staged_manifest, &manifest_path).await {
        tokio::fs::remove_file(&database_path).await.ok();
        return Err(error).context("无法原子发布备份清单");
    }

    Ok(ManagedSnapshot {
        id,
        created_at: now,
        database_path,
        manifest_path,
        manifest,
    })
}

async fn apply_retention(backup_dir: &Path, now: DateTime<Utc>) -> Result<Vec<ManagedSnapshot>> {
    let snapshots = list_managed_snapshots(backup_dir).await?;
    let candidates = snapshots
        .iter()
        .map(|snapshot| RetentionSnapshot {
            id: snapshot.id.clone(),
            created_at: snapshot.created_at,
        })
        .collect::<Vec<_>>();
    let plan = plan_retention(&candidates, now, BACKUP_RETENTION_DAYS);
    for expired in plan.delete {
        let snapshot = snapshots
            .iter()
            .find(|snapshot| snapshot.id == expired.id)
            .ok_or_else(|| anyhow!("保留策略返回了未知快照 {}", expired.id))?;
        tokio::fs::remove_file(&snapshot.manifest_path)
            .await
            .with_context(|| format!("无法删除过期清单 {}", snapshot.manifest_path.display()))?;
        tokio::fs::remove_file(&snapshot.database_path)
            .await
            .with_context(|| format!("无法删除过期快照 {}", snapshot.database_path.display()))?;
    }
    list_managed_snapshots(backup_dir).await
}

pub async fn run_backup_cycle(state: &AppState, now: DateTime<Utc>) -> Result<()> {
    state.backup_status.started(now).await;
    let result = async {
        persist_current_bill_inbox_backup_owner(state).await?;
        let backup_dir = backup_directory(&state.config)?;
        let existing = list_managed_snapshots(&backup_dir).await?;
        if !existing
            .iter()
            .any(|snapshot| snapshot.created_at.date_naive() == now.date_naive())
        {
            create_managed_snapshot(&state.db, &backup_dir, now).await?;
        }
        let retained = apply_retention(&backup_dir, now).await?;
        Ok::<_, anyhow::Error>(retained.first().map(|snapshot| snapshot.id.clone()))
    }
    .await;

    match result {
        Ok(latest_snapshot_id) => {
            state.backup_status.succeeded(now, latest_snapshot_id).await;
            Ok(())
        }
        Err(error) => {
            tracing::error!(error = %error, "scheduled database backup failed");
            state.backup_status.failed(&error).await;
            Err(error)
        }
    }
}

pub fn spawn_backup_scheduler(state: AppState) {
    tokio::spawn(async move {
        loop {
            let _ = run_backup_cycle(&state, Utc::now()).await;
            tokio::time::sleep(StdDuration::from_secs(60 * 60)).await;
        }
    });
}

#[utoipa::path(
    get,
    path = "/api/v1/backups",
    responses((status = 200, body = [BackupListItem]), (status = 401, body = crate::error::ErrorBody), (status = 403, body = crate::error::ErrorBody)),
    security(("cookieAuth" = []), ("bearerAuth" = []))
)]
pub async fn list_backups(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<BackupListItem>>, ApiError> {
    require_backup_owner(&state, &user).await?;
    let backup_dir = backup_directory(&state.config).map_err(ApiError::internal)?;
    let snapshots = list_managed_snapshots(&backup_dir)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(
        snapshots
            .into_iter()
            .map(|snapshot| BackupListItem {
                id: snapshot.id,
                created_at: snapshot.manifest.created_at_utc,
                size: snapshot.manifest.database_size_bytes,
                sha256: snapshot.manifest.database_sha256,
                schema_version: snapshot
                    .manifest
                    .schema_migration_versions
                    .last()
                    .copied()
                    .unwrap_or_default(),
            })
            .collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/api/v1/backups/status",
    responses((status = 200, body = BackupRuntimeStatus), (status = 401, body = crate::error::ErrorBody), (status = 403, body = crate::error::ErrorBody)),
    security(("cookieAuth" = []), ("bearerAuth" = []))
)]
pub async fn backup_status(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<BackupRuntimeStatus>, ApiError> {
    require_backup_owner(&state, &user).await?;
    Ok(Json(state.backup_status.get().await))
}

#[utoipa::path(
    get,
    path = "/api/v1/backups/{id}",
    params(("id" = String, Path)),
    responses(
        (status = 200, content_type = "application/octet-stream"),
        (status = 400, body = crate::error::ErrorBody),
        (status = 401, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody)
    ),
    security(("cookieAuth" = []), ("bearerAuth" = []))
)]
pub async fn download_backup(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    user: AuthUser,
) -> Result<Response, ApiError> {
    require_backup_owner(&state, &user).await?;
    parse_snapshot_id(&id)
        .map_err(|_| ApiError::bad_request("invalid_backup_id", "备份 ID 格式不正确"))?;
    let backup_dir = backup_directory(&state.config).map_err(ApiError::internal)?;
    let snapshots = list_managed_snapshots(&backup_dir)
        .await
        .map_err(ApiError::internal)?;
    let snapshot = snapshots
        .into_iter()
        .find(|snapshot| snapshot.id == id)
        .ok_or_else(|| ApiError::not_found("备份不存在"))?;
    check_against_manifest(&snapshot.database_path, &snapshot.manifest)
        .await
        .map_err(ApiError::internal)?;
    let file = tokio::fs::File::open(&snapshot.database_path)
        .await
        .map_err(ApiError::internal)?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(
            header::CONTENT_LENGTH,
            snapshot.manifest.database_size_bytes,
        )
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(ReaderStream::new(file)))
        .map_err(ApiError::internal)
}

async fn require_backup_owner(state: &AppState, user: &AuthUser) -> Result<(), ApiError> {
    if let Some(config) = state.config.bill_inbox.as_ref() {
        if user.email != config.owner_email {
            return Err(ApiError::forbidden("无权访问包含账单原文的数据库备份"));
        }
        persist_bill_inbox_backup_owner(state, &config.owner_email)
            .await
            .map_err(ApiError::internal)?;
        return Ok(());
    }

    if let Some(owner_email) = read_bill_inbox_backup_owner(&state.config)
        .await
        .map_err(ApiError::internal)?
    {
        if user.email == owner_email {
            return Ok(());
        }
        return Err(ApiError::forbidden("无权访问包含账单原文的数据库备份"));
    }

    // 同步被临时停用时，历史 raw BLOB 和已发布快照仍然存在，不能因为配置缺失
    // 就把整库下载权限重新放给任意已验证用户。sync state 在首次成功连接后即持久化，
    // 与 messages 做并集可覆盖空邮箱、已删邮件和已有原文三种状态。
    let conn = state.connection().await?;
    let mut rows = conn
        .query(
            "SELECT DISTINCT u.email FROM users u JOIN (SELECT user_id FROM bill_inbox_sync_state UNION SELECT user_id FROM bill_inbox_messages) owners ON owners.user_id = u.id LIMIT 2",
            (),
        )
        .await?;
    let mut owners = Vec::new();
    while let Some(row) = rows.next().await? {
        owners.push(row.get::<String>(0)?);
    }
    if owners.is_empty() {
        return Ok(());
    }
    if owners.len() == 1 {
        persist_bill_inbox_backup_owner(state, &owners[0])
            .await
            .map_err(ApiError::internal)?;
        if owners[0] == user.email {
            return Ok(());
        }
    }
    Err(ApiError::forbidden("无权访问包含账单原文的数据库备份"))
}

/// 生成一份经过校验的快照，返回快照文件路径与其 manifest。
///
/// 快照落在 `staging_dir` 下一个唯一的临时文件里，**不会**碰任何既有文件：
/// `VACUUM INTO` 要求目标不存在，且中途被杀会留下不完整的产物——所以绝不能让它
/// 直接写向正在被当作「上一份可用备份」的路径。调用方校验通过后再自行原子落位。
pub async fn create_snapshot(
    db: &Database,
    staging_dir: &Path,
    application_version: &str,
) -> Result<(PathBuf, Manifest)> {
    tokio::fs::create_dir_all(staging_dir)
        .await
        .with_context(|| format!("无法创建暂存目录 {}", staging_dir.display()))?;
    let snapshot_path = staging_dir.join(format!("snapshot-{}.db", Uuid::now_v7()));

    // 用一条全新连接，避免复用可能仍持有旧读事务的业务连接。
    let source = db.connect().context("打开备份连接失败")?;
    let journal_mode = scalar_text(&source, "PRAGMA journal_mode").await?;
    let target = snapshot_path
        .to_str()
        .context("暂存路径包含非 UTF-8 字符")?;
    source
        .execute("VACUUM main INTO ?1", [target])
        .await
        .context("生成快照失败（VACUUM INTO）")?;
    drop(source);

    let manifest = verify_snapshot(
        &snapshot_path,
        application_version,
        &journal_mode,
        "vacuum_into",
    )
    .await?;
    Ok((snapshot_path, manifest))
}

/// 校验一份快照文件本身——注意是校验产出物，不是校验在线源库。
///
/// `integrity_check` 明确不检查外键，`foreign_key_check` 也不检查页面结构，两者
/// 缺一不可。
pub async fn verify_snapshot(
    snapshot_path: &Path,
    application_version: &str,
    source_journal_mode: &str,
    snapshot_method: &str,
) -> Result<Manifest> {
    let snapshot = libsql::Builder::new_local(snapshot_path)
        .build()
        .await
        .context("快照无法作为数据库打开")?;
    let conn = snapshot.connect()?;

    let integrity = scalar_text(&conn, "PRAGMA integrity_check").await?;
    if integrity != "ok" {
        bail!("快照未通过完整性检查：{integrity}");
    }

    let mut rows = conn.query("PRAGMA foreign_key_check", ()).await?;
    let mut violations = 0_u64;
    while rows.next().await?.is_some() {
        violations += 1;
    }
    if violations > 0 {
        bail!("快照存在 {violations} 处外键违规");
    }

    let mut rows = conn
        .query("SELECT version FROM schema_migrations ORDER BY version", ())
        .await
        .context("快照里读不到 schema_migrations，可能不是本应用的数据库")?;
    let mut versions = Vec::new();
    while let Some(row) = rows.next().await? {
        versions.push(row.get::<i64>(0)?);
    }
    if versions.is_empty() {
        bail!("快照没有任何迁移记录");
    }
    drop(rows);
    drop(conn);

    let bytes = tokio::fs::read(snapshot_path).await?;
    Ok(Manifest {
        backup_format_version: BACKUP_FORMAT_VERSION,
        created_at_utc: Utc::now().to_rfc3339(),
        database_sha256: format!("{:x}", Sha256::digest(&bytes)),
        database_size_bytes: bytes.len() as u64,
        application_version: application_version.to_owned(),
        schema_migration_versions: versions,
        snapshot_method: snapshot_method.to_owned(),
        source_journal_mode: source_journal_mode.to_owned(),
        integrity_check: integrity,
        foreign_key_violation_count: 0,
    })
}

/// 恢复前的把关：文件内容必须和 manifest 记录的哈希逐字节一致，否则拒绝。
pub async fn check_against_manifest(snapshot_path: &Path, manifest: &Manifest) -> Result<()> {
    if manifest.backup_format_version != BACKUP_FORMAT_VERSION {
        bail!(
            "备份格式版本不匹配：manifest 是 {}，当前程序只认 {}",
            manifest.backup_format_version,
            BACKUP_FORMAT_VERSION
        );
    }
    let bytes = tokio::fs::read(snapshot_path).await?;
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if actual != manifest.database_sha256 {
        bail!(
            "快照哈希与 manifest 不一致：期望 {}，实际 {actual}",
            manifest.database_sha256
        );
    }
    Ok(())
}

/// 一次离线恢复的结果。
#[derive(Debug)]
pub struct RestoreReport {
    pub source_manifest: Manifest,
    pub quarantined_to: PathBuf,
    pub migrated: bool,
}

/// 从备份仓库恢复数据库。
///
/// 这是离线操作：调用前所有数据库连接都必须已经关闭，由调用方保证。函数会先在与
/// 生产库同一文件系统的暂存副本上完成全部校验和迁移，之后才隔离生产库并原子落位。
pub async fn restore(repo: &Path, live_db: &Path, quarantine_root: &Path) -> Result<RestoreReport> {
    let manifest_path = repo.join(MANIFEST_IN_REPO);
    let snapshot_path = repo.join(SNAPSHOT_IN_REPO);
    let manifest_bytes = tokio::fs::read(&manifest_path)
        .await
        .with_context(|| format!("无法读取备份清单 {}", manifest_path.display()))?;
    let source_manifest: Manifest =
        serde_json::from_slice(&manifest_bytes).context("备份清单不是有效的 JSON")?;
    if source_manifest.backup_format_version != BACKUP_FORMAT_VERSION {
        bail!(
            "备份格式版本不匹配：manifest 是 {}，当前程序只认 {}",
            source_manifest.backup_format_version,
            BACKUP_FORMAT_VERSION
        );
    }
    check_against_manifest(&snapshot_path, &source_manifest).await?;

    let live_parent = live_db.parent().context("生产数据库路径没有父目录")?;
    tokio::fs::create_dir_all(live_parent).await?;
    let staging = live_parent.join(format!(".restore-{}.db", Uuid::now_v7()));
    tokio::fs::copy(&snapshot_path, &staging)
        .await
        .context("无法把备份复制到恢复暂存区")?;

    let result = restore_from_staging(&staging, &source_manifest, live_db, quarantine_root).await;
    if result.is_err() {
        tokio::fs::remove_file(&staging).await.ok();
    }
    result
}

async fn restore_from_staging(
    staging: &Path,
    source_manifest: &Manifest,
    live_db: &Path,
    quarantine_root: &Path,
) -> Result<RestoreReport> {
    verify_snapshot(
        staging,
        &source_manifest.application_version,
        &source_manifest.source_journal_mode,
        &source_manifest.snapshot_method,
    )
    .await?;

    let known = crate::db::known_migration_versions();
    let backup_versions = &source_manifest.schema_migration_versions;
    let migrated = if backup_versions == &known {
        false
    } else if known.starts_with(backup_versions) {
        let db = libsql::Builder::new_local(staging)
            .build()
            .await
            .context("无法打开暂存副本执行迁移")?;
        crate::db::migrate(&db).await.context("迁移旧备份失败")?;
        drop(db);
        let verified = verify_snapshot(
            staging,
            env!("CARGO_PKG_VERSION"),
            &source_manifest.source_journal_mode,
            &source_manifest.snapshot_method,
        )
        .await?;
        if verified.schema_migration_versions != known {
            bail!("旧备份迁移后版本集合仍不完整");
        }
        true
    } else {
        bail!(
            "备份包含当前程序不认识的迁移版本 {:?}，请先升级应用再恢复",
            backup_versions
                .iter()
                .filter(|version| !known.contains(version))
                .collect::<Vec<_>>()
        );
    };

    let quarantine = quarantine_root.join(Utc::now().to_rfc3339());
    tokio::fs::create_dir_all(&quarantine)
        .await
        .context("无法创建旧库隔离目录")?;
    quarantine_database_group(live_db, &quarantine).await?;
    tokio::fs::rename(staging, live_db)
        .await
        .context("无法把已验证的暂存副本原子替换为生产库")?;

    Ok(RestoreReport {
        source_manifest: source_manifest.clone(),
        quarantined_to: quarantine,
        migrated,
    })
}

async fn quarantine_database_group(live_db: &Path, quarantine: &Path) -> Result<()> {
    let file_name = live_db.file_name().context("生产数据库路径没有文件名")?;
    let base = file_name.to_string_lossy();
    let candidates = [
        live_db.to_path_buf(),
        live_db.with_file_name(format!("{base}-journal")),
        live_db.with_file_name(format!("{base}-wal")),
        live_db.with_file_name(format!("{base}-shm")),
    ];
    let mut moved = Vec::new();
    for source in candidates.iter().filter(|path| path.exists()) {
        let destination = quarantine.join(source.file_name().expect("候选路径一定有文件名"));
        if let Err(error) = tokio::fs::rename(source, &destination).await {
            for (old, new) in moved.into_iter().rev() {
                tokio::fs::rename(new, old).await.ok();
            }
            return Err(error).context("无法整组隔离生产数据库及 SQLite sidecar");
        }
        moved.push((source.clone(), destination));
    }
    Ok(())
}

/// 备份目录里快照和清单的固定位置。
pub const SNAPSHOT_IN_REPO: &str = "data/ledger.sqlite3";
pub const MANIFEST_IN_REPO: &str = "data/manifest.json";
/// 暂存目录放在仓库内，保证 rename 不跨文件系统——跨文件系统的 rename 不是原子的。
pub const STAGING_IN_REPO: &str = ".staging";

async fn scalar_text(conn: &libsql::Connection, sql: &str) -> Result<String> {
    let mut rows = conn.query(sql, ()).await?;
    let row = rows
        .next()
        .await?
        .with_context(|| format!("{sql} 没有返回任何行"))?;
    Ok(row.get::<String>(0)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{email::DevFileEmailSender, rate_limit::RateLimiter};

    async fn application_db(path: &Path) -> Database {
        let db = libsql::Builder::new_local(path).build().await.unwrap();
        crate::db::migrate(&db).await.unwrap();
        db.connect()
            .unwrap()
            .execute(
                "INSERT INTO users(id, email, password_hash, timezone, created_at, updated_at)
                 VALUES ('u1', 'u1@example.com', 'hash', 'Asia/Shanghai', 'now', 'now')",
                (),
            )
            .await
            .unwrap();
        db
    }

    async fn write_backup(repo: &Path, db: &Database) -> Manifest {
        tokio::fs::create_dir_all(repo.join("data")).await.unwrap();
        let (snapshot, manifest) = create_snapshot(db, &repo.join("staging"), "test")
            .await
            .unwrap();
        tokio::fs::rename(snapshot, repo.join(SNAPSHOT_IN_REPO))
            .await
            .unwrap();
        tokio::fs::write(
            repo.join(MANIFEST_IN_REPO),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .await
        .unwrap();
        manifest
    }

    async fn user_count(path: &Path) -> i64 {
        let db = libsql::Builder::new_local(path).build().await.unwrap();
        let conn = db.connect().unwrap();
        let mut rows = conn.query("SELECT COUNT(*) FROM users", ()).await.unwrap();
        rows.next().await.unwrap().unwrap().get(0).unwrap()
    }

    async fn seeded_db(dir: &Path) -> Database {
        let db = libsql::Builder::new_local(dir.join("live.db"))
            .build()
            .await
            .unwrap();
        let conn = db.connect().unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);
             INSERT INTO schema_migrations VALUES (1, 'now'), (2, 'now');
             CREATE TABLE debts(id TEXT PRIMARY KEY, cents INTEGER NOT NULL CHECK(cents > 0));
             CREATE VIEW balances AS SELECT id, cents FROM debts;
             INSERT INTO debts VALUES('a', 12345);",
        )
        .await
        .unwrap();
        db
    }

    async fn test_state(root: &Path) -> AppState {
        let config = Config {
            app_env: "test".into(),
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            public_base_url: "http://test.local".into(),
            database_url: format!("file:{}", root.join("live.db").display()),
            turso_auth_token: None,
            dev_mail_dir: root.join("mail"),
            web_dist_dir: root.join("web"),
            bill_inbox: None,
        };
        let db = crate::db::connect(&config).await.unwrap();
        AppState {
            db: Arc::new(db),
            email: Arc::new(DevFileEmailSender::new(config.dev_mail_dir.clone())),
            config: Arc::new(config),
            rate_limiter: RateLimiter::default(),
            backup_status: BackupStatusStore::default(),
        }
    }

    #[tokio::test]
    async fn bill_inbox_raw_backups_are_restricted_to_the_configured_owner() {
        let root = tempfile::tempdir().unwrap();
        let mut state = test_state(root.path()).await;
        let mut config = (*state.config).clone();
        config.bill_inbox = Some(crate::config::BillInboxConfig {
            session_url: "https://mail.example.com/jmap/session".into(),
            username: "bills@example.com".into(),
            password: "secret".into(),
            address: "bills@example.com".into(),
            owner_email: "owner@example.com".into(),
            poll_interval_seconds: 300,
            max_message_bytes: 1024,
        });
        state.config = Arc::new(config);
        let owner = AuthUser {
            id: "owner".into(),
            email: "owner@example.com".into(),
            timezone: "Asia/Shanghai".into(),
            session_hash: None,
        };
        let other = AuthUser {
            id: "other".into(),
            email: "other@example.com".into(),
            timezone: "Asia/Shanghai".into(),
            session_hash: None,
        };

        assert!(require_backup_owner(&state, &owner).await.is_ok());
        assert_eq!(
            require_backup_owner(&state, &other)
                .await
                .unwrap_err()
                .status,
            StatusCode::FORBIDDEN
        );

        let conn = state.connection().await.unwrap();
        conn.execute(
            "INSERT INTO users(id, email, password_hash, timezone, email_verified_at, created_at, updated_at) VALUES ('owner', 'owner@example.com', 'hash', 'Asia/Shanghai', 'now', 'now', 'now')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO bill_inbox_sync_state(user_id, jmap_account_id, email_state) VALUES ('owner', 'account-1', 'state-1')",
            (),
        )
        .await
        .unwrap();
        drop(conn);
        let mut disabled_config = (*state.config).clone();
        disabled_config.bill_inbox = None;
        state.config = Arc::new(disabled_config);
        assert!(require_backup_owner(&state, &owner).await.is_ok());
        assert_eq!(
            require_backup_owner(&state, &other)
                .await
                .unwrap_err()
                .status,
            StatusCode::FORBIDDEN
        );

        // 即使后来恢复到没有 BillInbox 行的旧库，数据库之外的 marker 仍必须保护
        // 已经发布、可能含 raw 邮件原文的历史快照。
        let conn = state.connection().await.unwrap();
        conn.execute("DELETE FROM bill_inbox_sync_state", ())
            .await
            .unwrap();
        conn.execute("DELETE FROM users WHERE id = 'owner'", ())
            .await
            .unwrap();
        drop(conn);
        assert!(require_backup_owner(&state, &owner).await.is_ok());
        assert_eq!(
            require_backup_owner(&state, &other)
                .await
                .unwrap_err()
                .status,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn backup_owner_marker_is_no_clobber_under_concurrent_first_writes() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(root.path()).await;

        let (first, second) = tokio::join!(
            persist_bill_inbox_backup_owner(&state, "first@example.com"),
            persist_bill_inbox_backup_owner(&state, "second@example.com")
        );
        assert_ne!(first.is_ok(), second.is_ok());
        let stored = read_bill_inbox_backup_owner(&state.config)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            stored.as_str(),
            "first@example.com" | "second@example.com"
        ));
    }

    #[tokio::test]
    async fn backup_cycle_persists_owner_from_live_inbox_before_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(root.path()).await;
        let conn = state.connection().await.unwrap();
        conn.execute(
            "INSERT INTO users(id, email, password_hash, timezone, email_verified_at, created_at, updated_at) VALUES ('owner', 'owner@example.com', 'hash', 'Asia/Shanghai', 'now', 'now', 'now')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO bill_inbox_sync_state(user_id, jmap_account_id, email_state) VALUES ('owner', 'account-1', 'state-1')",
            (),
        )
        .await
        .unwrap();
        drop(conn);

        run_backup_cycle(&state, Utc::now()).await.unwrap();
        assert_eq!(
            read_bill_inbox_backup_owner(&state.config)
                .await
                .unwrap()
                .as_deref(),
            Some("owner@example.com")
        );
    }

    #[tokio::test]
    async fn snapshot_is_verified_and_hashed() {
        let dir = tempfile::tempdir().unwrap();
        let db = seeded_db(dir.path()).await;

        let (path, manifest) = create_snapshot(&db, &dir.path().join("staging"), "test")
            .await
            .unwrap();

        assert!(path.exists());
        assert_eq!(manifest.integrity_check, "ok");
        assert_eq!(manifest.foreign_key_violation_count, 0);
        assert_eq!(manifest.schema_migration_versions, vec![1, 2]);
        assert_eq!(manifest.snapshot_method, "vacuum_into");
        assert_eq!(manifest.database_size_bytes, path.metadata().unwrap().len());
        check_against_manifest(&path, &manifest).await.unwrap();
    }

    #[tokio::test]
    async fn snapshot_preserves_views_and_constraints() {
        let dir = tempfile::tempdir().unwrap();
        let db = seeded_db(dir.path()).await;
        let (path, _) = create_snapshot(&db, &dir.path().join("staging"), "test")
            .await
            .unwrap();

        let restored = libsql::Builder::new_local(&path).build().await.unwrap();
        let conn = restored.connect().unwrap();

        // 视图跟着快照一起走——按表导出文本最容易漏掉的就是这类非表对象。
        let mut rows = conn
            .query("SELECT cents FROM balances WHERE id = 'a'", ())
            .await
            .unwrap();
        let cents: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
        assert_eq!(cents, 12345);

        // CHECK 约束也必须还在，否则恢复出来的库能写进非法数据。
        let violation = conn.execute("INSERT INTO debts VALUES('b', -1)", ()).await;
        assert!(violation.is_err(), "CHECK 约束在快照中丢失了");
    }

    #[tokio::test]
    async fn each_snapshot_gets_a_unique_path() {
        let dir = tempfile::tempdir().unwrap();
        let db = seeded_db(dir.path()).await;
        let staging = dir.path().join("staging");

        let (first, _) = create_snapshot(&db, &staging, "test").await.unwrap();
        let (second, _) = create_snapshot(&db, &staging, "test").await.unwrap();

        assert_ne!(first, second, "两次快照撞到了同一路径");
        assert!(first.exists() && second.exists());
    }

    #[tokio::test]
    async fn tampered_snapshot_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let db = seeded_db(dir.path()).await;
        let (path, manifest) = create_snapshot(&db, &dir.path().join("staging"), "test")
            .await
            .unwrap();

        let mut bytes = tokio::fs::read(&path).await.unwrap();
        let tail = bytes.len() - 1;
        bytes[tail] ^= 0xff;
        tokio::fs::write(&path, &bytes).await.unwrap();

        let error = check_against_manifest(&path, &manifest).await.unwrap_err();
        assert!(error.to_string().contains("哈希与 manifest 不一致"));
    }

    #[tokio::test]
    async fn startup_cycle_backfills_once_and_publishes_verified_pair() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(root.path()).await;
        let now = "2026-08-10T02:03:04Z".parse().unwrap();

        run_backup_cycle(&state, now).await.unwrap();
        run_backup_cycle(&state, "2026-08-10T20:00:00Z".parse().unwrap())
            .await
            .unwrap();

        let backup_dir = backup_directory(&state.config).unwrap();
        let snapshots = list_managed_snapshots(&backup_dir).await.unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].id, "2026-08-10T02:03:04Z");
        check_against_manifest(&snapshots[0].database_path, &snapshots[0].manifest)
            .await
            .unwrap();
        let status = state.backup_status.get().await;
        assert!(!status.running);
        assert_eq!(
            status.latest_snapshot_id.as_deref(),
            Some("2026-08-10T02:03:04Z")
        );
        assert!(status.last_error.is_none());
    }

    #[tokio::test]
    async fn filesystem_retention_never_deletes_the_only_successful_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let db = seeded_db(root.path()).await;
        let backup_dir = root.path().join("backups");
        create_managed_snapshot(&db, &backup_dir, "2026-01-01T00:00:00Z".parse().unwrap())
            .await
            .unwrap();

        let retained = apply_retention(&backup_dir, "2026-08-10T00:00:00Z".parse().unwrap())
            .await
            .unwrap();

        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].id, "2026-01-01T00:00:00Z");
    }

    #[tokio::test]
    async fn restore_returns_data_to_backup_point_and_quarantines_journal() {
        let root = tempfile::tempdir().unwrap();
        let live = root.path().join("live.db");
        let repo = root.path().join("repo");
        let db = application_db(&live).await;
        let manifest = write_backup(&repo, &db).await;
        db.connect()
            .unwrap()
            .execute(
                "INSERT INTO users(id, email, password_hash, timezone, created_at, updated_at)
                 VALUES ('u2', 'u2@example.com', 'hash', 'Asia/Shanghai', 'now', 'now')",
                (),
            )
            .await
            .unwrap();
        drop(db);
        tokio::fs::write(root.path().join("live.db-journal"), b"old journal")
            .await
            .unwrap();

        let report = restore(&repo, &live, &root.path().join("quarantine"))
            .await
            .unwrap();

        assert_eq!(user_count(&live).await, 1);
        assert_eq!(
            report.source_manifest.database_sha256,
            manifest.database_sha256
        );
        assert!(!report.migrated);
        assert!(report.quarantined_to.join("live.db").exists());
        assert!(report.quarantined_to.join("live.db-journal").exists());
        assert!(!root.path().join("live.db-journal").exists());
    }

    #[tokio::test]
    async fn old_backup_is_migrated_and_verified() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("old.db");
        let db = libsql::Builder::new_local(&old).build().await.unwrap();
        let conn = db.connect().unwrap();
        for (version, sql) in [
            (1, include_str!("../migrations/0001_initial.sql")),
            (2, include_str!("../migrations/0002_debt_additions.sql")),
            (3, include_str!("../migrations/0003_ledger_accounts.sql")),
            (
                4,
                include_str!("../migrations/0004_ledger_account_types.sql"),
            ),
            (
                5,
                include_str!("../migrations/0005_ledger_account_details.sql"),
            ),
            (
                6,
                include_str!("../migrations/0006_ledger_account_name_source.sql"),
            ),
            (
                7,
                include_str!("../migrations/0007_ledger_account_card_number.sql"),
            ),
            (8, include_str!("../migrations/0008_debt_origin_kind.sql")),
        ] {
            conn.execute_batch(sql).await.unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO schema_migrations(version, applied_at) VALUES (?1, 'now')",
                [version],
            )
            .await
            .unwrap();
        }
        drop(conn);
        let repo = root.path().join("repo");
        write_backup(&repo, &db).await;
        drop(db);
        let live = root.path().join("live.db");
        tokio::fs::write(&live, b"old production").await.unwrap();

        let report = restore(&repo, &live, &root.path().join("quarantine"))
            .await
            .unwrap();
        assert!(report.migrated);
        let verified = verify_snapshot(&live, "test", "delete", "restore")
            .await
            .unwrap();
        assert_eq!(
            verified.schema_migration_versions,
            crate::db::known_migration_versions()
        );
    }

    #[tokio::test]
    async fn newer_backup_is_rejected_without_touching_production() {
        let root = tempfile::tempdir().unwrap();
        let live = root.path().join("live.db");
        let repo = root.path().join("repo");
        let db = application_db(&live).await;
        let mut manifest = write_backup(&repo, &db).await;
        drop(db);
        manifest.schema_migration_versions.push(999);
        tokio::fs::write(
            repo.join(MANIFEST_IN_REPO),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .await
        .unwrap();
        let before = tokio::fs::read(&live).await.unwrap();

        let error = restore(&repo, &live, &root.path().join("quarantine"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("升级应用"));
        assert_eq!(tokio::fs::read(&live).await.unwrap(), before);
    }

    #[tokio::test]
    async fn hash_mismatch_is_rejected_without_touching_production() {
        let root = tempfile::tempdir().unwrap();
        let live = root.path().join("live.db");
        let repo = root.path().join("repo");
        let db = application_db(&live).await;
        let mut manifest = write_backup(&repo, &db).await;
        drop(db);
        manifest.database_sha256 = "00".repeat(32);
        tokio::fs::write(
            repo.join(MANIFEST_IN_REPO),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .await
        .unwrap();
        let before = tokio::fs::read(&live).await.unwrap();

        let error = restore(&repo, &live, &root.path().join("quarantine"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("哈希与 manifest 不一致"));
        assert_eq!(tokio::fs::read(&live).await.unwrap(), before);
    }
}
