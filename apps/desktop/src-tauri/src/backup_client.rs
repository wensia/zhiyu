use std::{
    collections::HashSet,
    fs::OpenOptions,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, SecondsFormat, Utc};
use reqwest::{Client, Response, StatusCode, Url};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, State};
use tokio::{
    io::AsyncWriteExt,
    sync::{Mutex, RwLock},
};
use zhiyu_backup_policy::{Snapshot as RetentionSnapshot, plan_retention};

use crate::config::{
    load_config, resolve_connection, save_connection, validate_server_url, write_private_json,
};

const RETENTION_DAYS: i64 = 30;
const BACKUP_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const SESSION_HANDOFF_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct RemoteSnapshot {
    id: String,
    created_at: String,
    size: u64,
    sha256: String,
    schema_version: i64,
}

#[derive(Debug, Clone)]
struct LocalSnapshot {
    metadata: RemoteSnapshot,
    created_at: DateTime<Utc>,
    database_path: PathBuf,
    metadata_path: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackupStatus {
    last_attempt_at: Option<String>,
    last_success_at: Option<String>,
    last_error: Option<String>,
    #[serde(default)]
    session_handoff_error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HandoffTicketResponse {
    ticket: String,
}

pub(crate) struct HandoffTicket {
    pub(crate) server_url: Url,
    pub(crate) ticket: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    server_url: String,
    has_api_key: bool,
    credential_warning: Option<String>,
    last_pull_at: Option<String>,
    local_snapshot_count: usize,
    last_error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSettingsInput {
    server_url: String,
    api_key: String,
}

#[derive(Clone)]
pub struct BackupClient {
    config_path: PathBuf,
    status_path: PathBuf,
    backup_dir: PathBuf,
    http: Client,
    status: Arc<RwLock<BackupStatus>>,
    cycle_lock: Arc<Mutex<()>>,
}

impl BackupClient {
    pub fn new(config_dir: PathBuf, data_dir: PathBuf) -> Result<Self> {
        let config_path = config_dir.join("backup-client.json");
        let status_path = config_dir.join("backup-status.json");
        let backup_dir = data_dir.join("backups");
        let status = std::fs::read(&status_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(30 * 60))
            .build()
            .context("创建备份 HTTP 客户端失败")?;
        Ok(Self {
            config_path,
            status_path,
            backup_dir,
            http,
            status: Arc::new(RwLock::new(status)),
            cycle_lock: Arc::new(Mutex::new(())),
        })
    }

    pub fn has_connection_config(&self) -> bool {
        resolve_connection(&self.config_path).is_ok()
    }

    pub(crate) async fn create_handoff_ticket(&self) -> Result<HandoffTicket> {
        let connection = resolve_connection(&self.config_path)?;
        let url = endpoint(&connection.server_url, "api/v1/auth/handoff-tickets")?;
        let response = self
            .http
            .post(url)
            .bearer_auth(&connection.api_key)
            .timeout(SESSION_HANDOFF_TIMEOUT)
            .send()
            .await
            .map_err(|error| anyhow!("网络失败：无法用 api-key 获取桌面交接票据：{error}"))?;
        let response = require_success(response, "获取桌面交接票据").await?;
        let body = response
            .json::<HandoffTicketResponse>()
            .await
            .map_err(|error| anyhow!("服务器响应无效：交接票据响应不是预期 JSON：{error}"))?;
        if body.ticket.is_empty() {
            bail!("服务器响应无效：交接票据为空");
        }
        Ok(HandoffTicket {
            server_url: connection.server_url,
            ticket: body.ticket,
        })
    }

    pub(crate) async fn record_session_handoff_error(&self, error: Option<String>) {
        self.update_status(|status| status.session_handoff_error = error)
            .await;
    }

    pub fn spawn_scheduler(&self) {
        let client = self.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                if let Err(error) = client.run_cycle().await {
                    tracing::error!(error = %error, "desktop backup pull failed");
                }
                tokio::time::sleep(BACKUP_INTERVAL).await;
            }
        });
    }

    async fn run_cycle(&self) -> Result<()> {
        let _guard = self.cycle_lock.lock().await;
        self.update_status(|status| {
            status.last_attempt_at = Some(Utc::now().to_rfc3339());
        })
        .await;

        let result = async {
            self.cleanup_partials().await?;
            let connection = resolve_connection(&self.config_path)?;
            let remote = self
                .fetch_remote_snapshots(&connection.server_url, &connection.api_key)
                .await?;
            let local = self.local_snapshots().await?;
            let local_ids = local
                .iter()
                .map(|snapshot| snapshot.metadata.id.clone())
                .collect::<HashSet<_>>();
            let missing = missing_snapshot_ids(&remote, &local_ids);
            for id in missing {
                let snapshot = remote
                    .iter()
                    .find(|snapshot| snapshot.id == id)
                    .ok_or_else(|| anyhow!("待下载列表包含未知快照 {id}"))?;
                self.download_snapshot(&connection.server_url, &connection.api_key, snapshot)
                    .await?;
            }
            self.apply_retention(Utc::now()).await?;
            Ok::<(), anyhow::Error>(())
        }
        .await;

        match result {
            Ok(()) => {
                self.update_status(|status| {
                    status.last_success_at = Some(Utc::now().to_rfc3339());
                    status.last_error = None;
                })
                .await;
                Ok(())
            }
            Err(error) => {
                let readable = format!("{error:#}");
                self.update_status(|status| status.last_error = Some(readable))
                    .await;
                Err(error)
            }
        }
    }

    async fn update_status(&self, update: impl FnOnce(&mut BackupStatus)) {
        let mut status = self.status.write().await;
        update(&mut status);
        if let Err(error) = write_private_json(&self.status_path, &*status) {
            tracing::error!(error = %error, "persisting desktop backup status failed");
        }
    }

    async fn fetch_remote_snapshots(
        &self,
        base: &Url,
        api_key: &str,
    ) -> Result<Vec<RemoteSnapshot>> {
        let url = endpoint(base, "api/v1/backups")?;
        let response = self
            .http
            .get(url)
            .bearer_auth(api_key)
            .send()
            .await
            .map_err(|error| anyhow!("网络失败：无法获取服务器备份列表：{error}"))?;
        let response = require_success(response, "获取服务器备份列表").await?;
        let body = response
            .bytes()
            .await
            .map_err(|error| anyhow!("网络失败：读取备份列表响应失败：{error}"))?;
        let snapshots: Vec<RemoteSnapshot> = serde_json::from_slice(&body)
            .map_err(|error| anyhow!("服务器响应无效：备份列表不是预期 JSON：{error}"))?;
        validate_remote_snapshots(&snapshots)?;
        Ok(snapshots)
    }

    async fn test_connection(&self, base: &Url, api_key: &str) -> Result<()> {
        self.fetch_remote_snapshots(base, api_key).await.map(|_| ())
    }

    async fn download_snapshot(
        &self,
        base: &Url,
        api_key: &str,
        snapshot: &RemoteSnapshot,
    ) -> Result<()> {
        tokio::fs::create_dir_all(&self.backup_dir)
            .await
            .with_context(|| format!("无法创建本地备份目录 {}", self.backup_dir.display()))?;
        let database_path = self.backup_dir.join(format!("{}.db", snapshot.id));
        let metadata_path = self.backup_dir.join(format!("{}.json", snapshot.id));
        let partial_database = self.backup_dir.join(format!("{}.db.partial", snapshot.id));
        let partial_metadata = self
            .backup_dir
            .join(format!("{}.json.partial", snapshot.id));

        for stale in [
            &database_path,
            &metadata_path,
            &partial_database,
            &partial_metadata,
        ] {
            if stale.exists() {
                tokio::fs::remove_file(stale)
                    .await
                    .with_context(|| format!("无法清理不完整快照 {}", stale.display()))?;
            }
        }

        let url = endpoint(base, &format!("api/v1/backups/{}", snapshot.id))?;
        let response = self
            .http
            .get(url)
            .bearer_auth(api_key)
            .send()
            .await
            .map_err(|error| anyhow!("网络失败：下载快照 {} 失败：{error}", snapshot.id))?;
        let mut response = require_success(response, &format!("下载快照 {}", snapshot.id)).await?;
        let std_file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&partial_database)
            .with_context(|| format!("无法创建临时快照 {}", partial_database.display()))?;
        let mut file = tokio::fs::File::from_std(std_file);
        let mut digest = Sha256::new();
        let mut actual_size = 0_u64;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| anyhow!("网络失败：读取快照 {} 失败：{error}", snapshot.id))?
        {
            actual_size = actual_size
                .checked_add(chunk.len() as u64)
                .context("快照长度溢出")?;
            if actual_size > snapshot.size {
                bail!(
                    "校验失败：快照 {} 长度超过清单值 {} 字节",
                    snapshot.id,
                    snapshot.size
                );
            }
            digest.update(&chunk);
            file.write_all(&chunk).await?;
        }
        if actual_size != snapshot.size {
            bail!(
                "校验失败：快照 {} 长度不匹配（清单 {}，实际 {}）",
                snapshot.id,
                snapshot.size,
                actual_size
            );
        }
        let actual_sha256 = format!("{:x}", digest.finalize());
        if actual_sha256 != snapshot.sha256.to_ascii_lowercase() {
            bail!(
                "校验失败：快照 {} SHA-256 不匹配（清单 {}，实际 {}）",
                snapshot.id,
                snapshot.sha256,
                actual_sha256
            );
        }
        file.sync_all().await?;
        drop(file);

        let metadata_bytes = serde_json::to_vec_pretty(snapshot)?;
        let std_metadata = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&partial_metadata)?;
        let mut metadata_file = tokio::fs::File::from_std(std_metadata);
        metadata_file.write_all(&metadata_bytes).await?;
        metadata_file.sync_all().await?;
        drop(metadata_file);

        tokio::fs::rename(&partial_database, &database_path)
            .await
            .with_context(|| format!("无法原子发布快照 {}", snapshot.id))?;
        tokio::fs::rename(&partial_metadata, &metadata_path)
            .await
            .with_context(|| format!("无法原子发布快照元信息 {}", snapshot.id))?;
        sync_directory(&self.backup_dir).await?;
        Ok(())
    }

    async fn cleanup_partials(&self) -> Result<()> {
        tokio::fs::create_dir_all(&self.backup_dir).await?;
        let mut entries = tokio::fs::read_dir(&self.backup_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.ends_with(".partial"))
            {
                tokio::fs::remove_file(entry.path())
                    .await
                    .with_context(|| format!("无法清理残留临时文件 {}", entry.path().display()))?;
            }
        }
        Ok(())
    }

    async fn local_snapshots(&self) -> Result<Vec<LocalSnapshot>> {
        tokio::fs::create_dir_all(&self.backup_dir).await?;
        let mut entries = tokio::fs::read_dir(&self.backup_dir).await?;
        let mut snapshots = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            let Some(id) = file_name.strip_suffix(".json") else {
                continue;
            };
            let metadata_path = entry.path();
            let metadata: RemoteSnapshot = serde_json::from_slice(
                &tokio::fs::read(&metadata_path)
                    .await
                    .with_context(|| format!("无法读取快照元信息 {}", metadata_path.display()))?,
            )
            .with_context(|| format!("快照元信息不是有效 JSON：{}", metadata_path.display()))?;
            if metadata.id != id {
                bail!("快照元信息 ID 与文件名不一致：{file_name}");
            }
            let created_at = parse_snapshot_id(&metadata.id)?;
            let database_path = self.backup_dir.join(format!("{}.db", metadata.id));
            if !database_path.exists() {
                continue;
            }
            let actual_size = tokio::fs::metadata(&database_path).await?.len();
            if actual_size != metadata.size {
                bail!(
                    "校验失败：本地快照 {} 长度不匹配（元信息 {}，实际 {}）",
                    metadata.id,
                    metadata.size,
                    actual_size
                );
            }
            snapshots.push(LocalSnapshot {
                metadata,
                created_at,
                database_path,
                metadata_path,
            });
        }
        Ok(snapshots)
    }

    async fn apply_retention(&self, now: DateTime<Utc>) -> Result<()> {
        let snapshots = self.local_snapshots().await?;
        let expired_ids = retention_deletions(&snapshots, now);
        for id in expired_ids {
            let snapshot = snapshots
                .iter()
                .find(|snapshot| snapshot.metadata.id == id)
                .ok_or_else(|| anyhow!("保留策略返回了未知快照 {id}"))?;
            tokio::fs::remove_file(&snapshot.database_path)
                .await
                .with_context(|| {
                    format!("无法删除过期快照 {}", snapshot.database_path.display())
                })?;
            tokio::fs::remove_file(&snapshot.metadata_path)
                .await
                .with_context(|| {
                    format!("无法删除过期元信息 {}", snapshot.metadata_path.display())
                })?;
        }
        sync_directory(&self.backup_dir).await?;
        Ok(())
    }

    async fn settings_view(&self) -> SettingsView {
        let resolved = resolve_connection(&self.config_path);
        let (server_url, has_api_key, credential_warning) = match resolved {
            Ok(connection) => (
                connection
                    .server_url
                    .as_str()
                    .trim_end_matches('/')
                    .to_owned(),
                true,
                connection.credential_warning,
            ),
            Err(_) => {
                let config = load_config(&self.config_path).ok();
                (
                    config
                        .as_ref()
                        .map(|value| value.server_url.clone())
                        .unwrap_or_default(),
                    false,
                    config.and_then(|value| value.keychain_warning),
                )
            }
        };
        let status = self.status.read().await.clone();
        let local_snapshot_count = self.local_snapshots().await.map_or(0, |items| items.len());
        SettingsView {
            server_url,
            has_api_key,
            credential_warning,
            last_pull_at: status.last_success_at,
            local_snapshot_count,
            last_error: status.session_handoff_error.or(status.last_error),
        }
    }

    async fn save_settings(
        &self,
        input: SaveSettingsInput,
        app: &AppHandle,
    ) -> Result<SettingsView> {
        let server_url = validate_server_url(&input.server_url)?;
        let api_key = if input.api_key.is_empty() {
            resolve_connection(&self.config_path)
                .context("api-key 留空时无法读取现有凭证，请重新输入")?
                .api_key
        } else {
            input.api_key
        };
        self.test_connection(&server_url, &api_key).await?;
        let warning = save_connection(&self.config_path, &server_url, &api_key)?;
        let client = self.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = client.run_cycle().await {
                tracing::error!(error = %error, "backup pull after settings save failed");
            }
        });
        crate::handoff_main_window_session(app, self).await;
        let mut view = self.settings_view().await;
        view.credential_warning = warning;
        view.has_api_key = true;
        Ok(view)
    }
}

#[tauri::command]
pub async fn get_backup_settings(client: State<'_, BackupClient>) -> Result<SettingsView, String> {
    Ok(client.settings_view().await)
}

#[tauri::command]
pub async fn save_backup_settings(
    input: SaveSettingsInput,
    client: State<'_, BackupClient>,
    app: AppHandle,
) -> Result<SettingsView, String> {
    client
        .save_settings(input, &app)
        .await
        .map_err(|error| format!("{error:#}"))
}

fn endpoint(base: &Url, relative: &str) -> Result<Url> {
    base.join(relative)
        .with_context(|| format!("无法构造服务器接口地址：{relative}"))
}

async fn require_success(response: Response, operation: &str) -> Result<Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    let detail = body.chars().take(500).collect::<String>();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        bail!("认证失败：{operation}返回 HTTP {status}：{detail}");
    }
    bail!("服务器请求失败：{operation}返回 HTTP {status}：{detail}")
}

fn parse_snapshot_id(id: &str) -> Result<DateTime<Utc>> {
    if id.len() != 20 {
        bail!("备份 ID 格式不正确：{id}");
    }
    let parsed = DateTime::parse_from_rfc3339(id)
        .with_context(|| format!("备份 ID 不是 RFC3339 时间戳：{id}"))?
        .with_timezone(&Utc);
    if parsed.to_rfc3339_opts(SecondsFormat::Secs, true) != id {
        bail!("备份 ID 不是秒精度 UTC 时间戳：{id}");
    }
    Ok(parsed)
}

fn validate_remote_snapshots(snapshots: &[RemoteSnapshot]) -> Result<()> {
    let mut ids = HashSet::new();
    for snapshot in snapshots {
        let created_at = parse_snapshot_id(&snapshot.id)?;
        let declared = DateTime::parse_from_rfc3339(&snapshot.created_at)
            .with_context(|| format!("快照 {} 的 createdAt 无效", snapshot.id))?
            .with_timezone(&Utc);
        if created_at != declared {
            bail!("快照 {} 的 ID 与 createdAt 不一致", snapshot.id);
        }
        if snapshot.sha256.len() != 64
            || !snapshot.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            bail!("快照 {} 的 SHA-256 格式无效", snapshot.id);
        }
        if !ids.insert(&snapshot.id) {
            bail!("服务器备份列表包含重复 ID：{}", snapshot.id);
        }
    }
    Ok(())
}

fn missing_snapshot_ids(remote: &[RemoteSnapshot], local_ids: &HashSet<String>) -> Vec<String> {
    remote
        .iter()
        .filter(|snapshot| !local_ids.contains(&snapshot.id))
        .map(|snapshot| snapshot.id.clone())
        .collect()
}

fn retention_deletions(snapshots: &[LocalSnapshot], now: DateTime<Utc>) -> Vec<String> {
    let candidates = snapshots
        .iter()
        .map(|snapshot| RetentionSnapshot {
            id: snapshot.metadata.id.clone(),
            created_at: snapshot.created_at,
        })
        .collect::<Vec<_>>();
    plan_retention(&candidates, now, RETENTION_DAYS)
        .delete
        .into_iter()
        .map(|snapshot| snapshot.id)
        .collect()
}

async fn sync_directory(path: &Path) -> Result<()> {
    let path = path.to_owned();
    let sync_path = path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        std::fs::File::open(&sync_path)?.sync_all()?;
        Ok::<(), std::io::Error>(())
    })
    .await
    .context("等待目录同步任务失败")?
    .with_context(|| format!("同步备份目录 {} 失败", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(id: &str) -> RemoteSnapshot {
        RemoteSnapshot {
            id: id.to_owned(),
            created_at: id.to_owned(),
            size: 42,
            sha256: "a".repeat(64),
            schema_version: 1,
        }
    }

    fn local(id: &str) -> LocalSnapshot {
        LocalSnapshot {
            metadata: remote(id),
            created_at: parse_snapshot_id(id).unwrap(),
            database_path: PathBuf::from(format!("{id}.db")),
            metadata_path: PathBuf::from(format!("{id}.json")),
        }
    }

    #[test]
    fn computes_every_server_snapshot_missing_locally() {
        let remote = vec![
            remote("2026-08-08T02:00:00Z"),
            remote("2026-08-09T02:00:00Z"),
            remote("2026-08-10T02:00:00Z"),
        ];
        let local_ids = HashSet::from(["2026-08-09T02:00:00Z".to_owned()]);

        assert_eq!(
            missing_snapshot_ids(&remote, &local_ids),
            vec![
                "2026-08-08T02:00:00Z".to_owned(),
                "2026-08-10T02:00:00Z".to_owned()
            ]
        );
    }

    #[test]
    fn retention_delegates_to_shared_policy_and_keeps_latest() {
        let snapshots = vec![
            local("2026-01-01T00:00:00Z"),
            local("2026-02-01T00:00:00Z"),
            local("2026-08-01T00:00:00Z"),
        ];
        let now = "2026-12-10T00:00:00Z".parse().unwrap();

        assert_eq!(
            retention_deletions(&snapshots, now),
            vec![
                "2026-01-01T00:00:00Z".to_owned(),
                "2026-02-01T00:00:00Z".to_owned()
            ]
        );
    }
}
