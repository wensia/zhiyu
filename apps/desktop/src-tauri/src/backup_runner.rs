use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use tokio::{
    process::Command,
    sync::{Mutex, Notify},
};

use crate::{
    backup_config::{self, BackupConfig},
    backup_state::{self, BackupStatus, SharedBackupStatus},
};

#[async_trait]
trait BackupOperations: Send + Sync {
    async fn create_snapshot(
        &self,
        database_path: &Path,
        repo_path: &Path,
    ) -> Result<(PathBuf, zhiyu_api::backup::Manifest)>;
    async fn commit_snapshot(
        &self,
        repo_path: &Path,
        snapshot: &Path,
        manifest: &zhiyu_api::backup::Manifest,
    ) -> Result<zhiyu_api::backup::Committed>;
    async fn push(&self, config: &BackupConfig) -> Result<()>;
}

struct RealBackupOperations;

#[async_trait]
impl BackupOperations for RealBackupOperations {
    async fn create_snapshot(
        &self,
        database_path: &Path,
        repo_path: &Path,
    ) -> Result<(PathBuf, zhiyu_api::backup::Manifest)> {
        let db = libsql::Builder::new_local(database_path)
            .build()
            .await
            .context("无法打开账本数据库")?;
        zhiyu_api::backup::create_snapshot(
            &db,
            &repo_path.join(zhiyu_api::backup::STAGING_IN_REPO),
            env!("CARGO_PKG_VERSION"),
        )
        .await
    }

    async fn commit_snapshot(
        &self,
        repo_path: &Path,
        snapshot: &Path,
        manifest: &zhiyu_api::backup::Manifest,
    ) -> Result<zhiyu_api::backup::Committed> {
        zhiyu_api::backup::commit_snapshot(repo_path, snapshot, manifest).await
    }

    async fn push(&self, config: &BackupConfig) -> Result<()> {
        zhiyu_api::backup::push(&config.repo_path, &config.remote, &config.branch).await
    }
}

#[derive(Clone)]
pub struct BackupContext {
    pub data_dir: PathBuf,
    pub config_dir: PathBuf,
    pub database_path: PathBuf,
    pub status: SharedBackupStatus,
    run_lock: Arc<Mutex<()>>,
    operations: Arc<dyn BackupOperations>,
    write_generation: Arc<AtomicU64>,
    dirty_notify: Arc<Notify>,
}

impl BackupContext {
    pub fn new(data_dir: PathBuf, config_dir: PathBuf, status: SharedBackupStatus) -> Self {
        Self {
            database_path: data_dir.join("zhiyu.db"),
            data_dir,
            config_dir,
            status,
            run_lock: Arc::new(Mutex::new(())),
            operations: Arc::new(RealBackupOperations),
            write_generation: Arc::new(AtomicU64::new(0)),
            dirty_notify: Arc::new(Notify::new()),
        }
    }
}

impl BackupContext {
    /// 业务写入只调用这个轻量入口，不在请求处理路径执行任何快照或 git 操作。
    pub async fn mark_dirty(&self) {
        self.write_generation.fetch_add(1, Ordering::SeqCst);
        let snapshot = {
            let mut status = self.status.lock().await;
            status.dirty = true;
            status.clone()
        };
        if let Err(error) = backup_state::persist(&self.data_dir, &snapshot).await {
            tracing::warn!(?error, "无法持久化备份 dirty 状态");
        }
        self.dirty_notify.notify_one();
    }
}

/// 执行一次完整备份。重复触发不会排队，持锁者结束后由后续自动触发再尝试。
pub async fn run_once(ctx: &BackupContext) -> Result<()> {
    let Ok(_run_guard) = ctx.run_lock.try_lock() else {
        return Ok(());
    };

    update_status(ctx, |status| {
        status.running = true;
        status.last_error = None;
    })
    .await?;

    let result = run_pipeline(ctx).await;
    match result {
        Ok(()) => {
            update_status(ctx, |status| {
                status.running = false;
                status.last_error = None;
            })
            .await
        }
        Err(error) => {
            let message = readable_error(&error);
            if let Err(persist_error) = update_status(ctx, |status| {
                status.running = false;
                status.last_error = Some(message.clone());
            })
            .await
            {
                return Err(error.context(format!("同时无法保存备份失败状态：{persist_error:#}")));
            }
            Err(error)
        }
    }
}

async fn run_pipeline(ctx: &BackupContext) -> Result<()> {
    let started_generation = ctx.write_generation.load(Ordering::SeqCst);
    let config = backup_config::load(&ctx.config_dir)?
        .context("备份尚未配置；请先打开“备份设置…”完成配置")?;
    let (snapshot, manifest) = ctx
        .operations
        .create_snapshot(&ctx.database_path, &config.repo_path)
        .await?;
    let snapshot_at = manifest.created_at_utc.clone();
    update_status(ctx, move |status| {
        status.last_snapshot_at = Some(snapshot_at);
    })
    .await?;

    let committed = ctx
        .operations
        .commit_snapshot(&config.repo_path, &snapshot, &manifest)
        .await?;
    let commit_id = match committed {
        zhiyu_api::backup::Committed::New(id) => Some(id),
        zhiyu_api::backup::Committed::Unchanged => git_head(&config.repo_path).await?,
    };
    let unpushed =
        backup_state::count_unpushed(&config.repo_path, &config.remote, &config.branch).await?;
    update_status(ctx, move |status| {
        status.last_commit_at = Some(Utc::now().to_rfc3339());
        status.last_commit_id = commit_id;
        status.unpushed_commits = unpushed;
    })
    .await?;

    ctx.operations.push(&config).await?;
    let unpushed =
        backup_state::count_unpushed(&config.repo_path, &config.remote, &config.branch).await?;
    let no_writes_during_backup = ctx.write_generation.load(Ordering::SeqCst) == started_generation;
    update_status(ctx, move |status| {
        status.last_remote_confirm_at = Some(Utc::now().to_rfc3339());
        status.unpushed_commits = unpushed;
        // 备份执行期间若又发生写入，新变更不在本次快照里，必须继续保持 dirty。
        if no_writes_during_backup {
            status.dirty = false;
        }
    })
    .await?;
    Ok(())
}

pub fn start_automation(ctx: BackupContext, debounce_delay: Duration, watchdog_delay: Duration) {
    let debounce_context = ctx.clone();
    tauri::async_runtime::spawn(async move {
        debounce_loop(debounce_context, debounce_delay).await;
    });
    let watchdog_context = ctx.clone();
    tauri::async_runtime::spawn(async move {
        watchdog_loop(watchdog_context, watchdog_delay).await;
    });
    tauri::async_runtime::spawn(async move {
        if let Err(error) = initialize_pending_push(&ctx).await {
            tracing::warn!(?error, "启动时检查待推送提交失败");
        }
    });
}

async fn debounce_loop(ctx: BackupContext, delay: Duration) {
    loop {
        ctx.dirty_notify.notified().await;
        loop {
            tokio::select! {
                () = tokio::time::sleep(delay) => break,
                () = ctx.dirty_notify.notified() => continue,
            }
        }
        if auto_backup_enabled(&ctx)
            && ctx.status.lock().await.dirty
            && let Err(error) = run_once(&ctx).await
        {
            tracing::warn!(?error, "写后防抖备份失败，保留 dirty 等待后续触发");
        }
    }
}

async fn watchdog_loop(ctx: BackupContext, delay: Duration) {
    loop {
        tokio::time::sleep(delay).await;
        if auto_backup_enabled(&ctx)
            && ctx.status.lock().await.dirty
            && let Err(error) = run_once(&ctx).await
        {
            tracing::warn!(?error, "备份看门狗补偿执行失败");
        }
    }
}

fn auto_backup_enabled(ctx: &BackupContext) -> bool {
    backup_config::load(&ctx.config_dir)
        .ok()
        .flatten()
        .is_some_and(|config| config.auto_backup)
}

async fn initialize_pending_push(ctx: &BackupContext) -> Result<()> {
    let Some(config) = backup_config::load(&ctx.config_dir)? else {
        return Ok(());
    };
    let unpushed =
        backup_state::count_unpushed(&config.repo_path, &config.remote, &config.branch).await?;
    update_status(ctx, |status| status.unpushed_commits = unpushed).await?;
    if unpushed > 0 {
        retry_pending_push(ctx, &config).await?;
    }
    Ok(())
}

async fn retry_pending_push(ctx: &BackupContext, config: &BackupConfig) -> Result<()> {
    let Ok(_run_guard) = ctx.run_lock.try_lock() else {
        return Ok(());
    };
    update_status(ctx, |status| {
        status.running = true;
        status.last_error = None;
    })
    .await?;
    let result = ctx.operations.push(config).await;
    match result {
        Ok(()) => {
            let unpushed =
                backup_state::count_unpushed(&config.repo_path, &config.remote, &config.branch)
                    .await?;
            update_status(ctx, |status| {
                status.running = false;
                status.last_remote_confirm_at = Some(Utc::now().to_rfc3339());
                status.unpushed_commits = unpushed;
            })
            .await
        }
        Err(error) => {
            let message = readable_error(&error);
            update_status(ctx, |status| {
                status.running = false;
                status.last_error = Some(message);
            })
            .await?;
            Err(error)
        }
    }
}

async fn update_status(ctx: &BackupContext, update: impl FnOnce(&mut BackupStatus)) -> Result<()> {
    let mut status = ctx.status.lock().await;
    update(&mut status);
    backup_state::persist(&ctx.data_dir, &status).await
}

async fn git_head(repo: &Path) -> Result<Option<String>> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo)
        .output()
        .await
        .context("无法读取本地备份提交")?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8(output.stdout)?.trim().to_owned()))
}

fn readable_error(error: &anyhow::Error) -> String {
    let detail = format!("{error:#}");
    let lowercase = detail.to_ascii_lowercase();
    if lowercase.contains("sqlite_busy") || lowercase.contains("database is locked") {
        format!("数据库正忙，本次备份已跳过，将在下次触发时重试：{detail}")
    } else {
        format!("备份失败：{detail}")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use anyhow::bail;
    use zhiyu_api::backup::{BACKUP_FORMAT_VERSION, Manifest};

    use super::*;

    struct FakeOperations {
        starts: AtomicUsize,
        fail: bool,
    }

    #[async_trait]
    impl BackupOperations for FakeOperations {
        async fn create_snapshot(
            &self,
            _database_path: &Path,
            repo_path: &Path,
        ) -> Result<(PathBuf, Manifest)> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
            if self.fail {
                bail!("模拟快照失败");
            }
            Ok((
                repo_path.join("snapshot.db"),
                Manifest {
                    backup_format_version: BACKUP_FORMAT_VERSION,
                    created_at_utc: "2026-08-09T01:00:00Z".into(),
                    database_sha256: "test".into(),
                    database_size_bytes: 1,
                    application_version: "test".into(),
                    schema_migration_versions: vec![1],
                    snapshot_method: "test".into(),
                    source_journal_mode: "delete".into(),
                    integrity_check: "ok".into(),
                    foreign_key_violation_count: 0,
                },
            ))
        }

        async fn commit_snapshot(
            &self,
            _repo_path: &Path,
            _snapshot: &Path,
            _manifest: &Manifest,
        ) -> Result<zhiyu_api::backup::Committed> {
            Ok(zhiyu_api::backup::Committed::Unchanged)
        }

        async fn push(&self, _config: &BackupConfig) -> Result<()> {
            Ok(())
        }
    }

    fn test_context(root: &Path, operations: Arc<FakeOperations>) -> BackupContext {
        let data_dir = root.join("data");
        let config_dir = root.join("config");
        let repo = root.join("repo");
        let remote = root.join("remote.git");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&remote).unwrap();
        for (directory, arguments) in [
            (
                remote.as_path(),
                vec!["init", "--bare", "--initial-branch", "main"],
            ),
            (repo.as_path(), vec!["init", "--initial-branch", "main"]),
        ] {
            let output = std::process::Command::new("git")
                .args(arguments)
                .current_dir(directory)
                .output()
                .unwrap();
            assert!(output.status.success());
        }
        let output = std::process::Command::new("git")
            .args(["remote", "add", "origin", remote.to_str().unwrap()])
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(output.status.success());
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            backup_config::path(&config_dir),
            serde_json::to_vec(&BackupConfig {
                repo_path: repo,
                remote: "origin".into(),
                branch: "main".into(),
                auto_backup: true,
            })
            .unwrap(),
        )
        .unwrap();
        BackupContext {
            database_path: data_dir.join("zhiyu.db"),
            data_dir,
            config_dir,
            status: Arc::new(Mutex::new(BackupStatus::default())),
            run_lock: Arc::new(Mutex::new(())),
            operations,
            write_generation: Arc::new(AtomicU64::new(0)),
            dirty_notify: Arc::new(Notify::new()),
        }
    }

    #[tokio::test]
    async fn concurrent_calls_only_start_one_backup() {
        let root = tempfile::tempdir().unwrap();
        let operations = Arc::new(FakeOperations {
            starts: AtomicUsize::new(0),
            fail: true,
        });
        let ctx = test_context(root.path(), operations.clone());
        let (first, second) = tokio::join!(run_once(&ctx), run_once(&ctx));
        assert!(first.is_err() || second.is_err());
        assert_eq!(operations.starts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failure_records_error_and_resets_running() {
        let root = tempfile::tempdir().unwrap();
        let operations = Arc::new(FakeOperations {
            starts: AtomicUsize::new(0),
            fail: true,
        });
        let ctx = test_context(root.path(), operations);
        assert!(run_once(&ctx).await.is_err());
        let status = ctx.status.lock().await.clone();
        assert!(!status.running);
        assert!(status.last_error.unwrap().contains("模拟快照失败"));
    }

    #[tokio::test]
    async fn repeated_dirty_marks_trigger_one_debounced_run() {
        let root = tempfile::tempdir().unwrap();
        let operations = Arc::new(FakeOperations {
            starts: AtomicUsize::new(0),
            fail: true,
        });
        let ctx = test_context(root.path(), operations.clone());
        let task = tokio::spawn(debounce_loop(ctx.clone(), Duration::from_millis(15)));
        ctx.mark_dirty().await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        ctx.mark_dirty().await;
        tokio::time::sleep(Duration::from_millis(5)).await;
        ctx.mark_dirty().await;
        tokio::time::sleep(Duration::from_millis(80)).await;
        task.abort();
        assert_eq!(operations.starts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn debounced_real_backup_reaches_local_bare_remote() {
        let root = tempfile::tempdir().unwrap();
        let data_dir = root.path().join("data");
        let config_dir = root.path().join("config");
        let repo = root.path().join("repo");
        let remote = root.path().join("remote.git");
        for directory in [&data_dir, &config_dir, &repo, &remote] {
            std::fs::create_dir_all(directory).unwrap();
        }
        let run_git = |directory: &Path, arguments: &[&str]| {
            let output = std::process::Command::new("git")
                .args(arguments)
                .current_dir(directory)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {:?} 失败：{}",
                arguments,
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run_git(&remote, &["init", "--bare", "--initial-branch", "main"]);
        run_git(&repo, &["init", "--initial-branch", "main"]);
        run_git(&repo, &["config", "user.name", "Backup Test"]);
        run_git(&repo, &["config", "user.email", "backup@example.invalid"]);
        run_git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        std::fs::write(
            backup_config::path(&config_dir),
            serde_json::to_vec(&BackupConfig {
                repo_path: repo.clone(),
                remote: "origin".into(),
                branch: "main".into(),
                auto_backup: true,
            })
            .unwrap(),
        )
        .unwrap();

        let database_path = data_dir.join("zhiyu.db");
        let database = libsql::Builder::new_local(&database_path)
            .build()
            .await
            .unwrap();
        zhiyu_api::db::migrate(&database).await.unwrap();
        drop(database);
        let status = Arc::new(Mutex::new(BackupStatus::default()));
        let context = BackupContext::new(data_dir, config_dir, status.clone());
        let task = tokio::spawn(debounce_loop(context.clone(), Duration::from_millis(10)));

        context.mark_dirty().await;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if status.lock().await.last_remote_confirm_at.is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        task.abort();

        let final_status = status.lock().await.clone();
        assert!(final_status.last_snapshot_at.is_some());
        assert!(final_status.last_commit_at.is_some());
        assert!(final_status.last_remote_confirm_at.is_some());
        assert_eq!(final_status.unpushed_commits, 0);
        assert!(!final_status.dirty);
        run_git(&repo, &["fetch", "origin", "main"]);
        let local = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo)
            .output()
            .unwrap();
        let remote_head = std::process::Command::new("git")
            .args(["rev-parse", "origin/main"])
            .current_dir(&repo)
            .output()
            .unwrap();
        assert_eq!(local.stdout, remote_head.stdout);
    }
}
