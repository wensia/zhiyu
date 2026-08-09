//! 数据库快照：备份链路里与「投递到哪」无关的那一半。
//!
//! 投递由服务端的 scripts/server-backup.sh 接手（restic → 对象存储）。本模块只负责
//! 产出一份**可信**的快照：一致快照、两道校验、内容哈希、先写临时文件。
//! 投递方式换成别的也不影响这里。
//!
//! 为什么是整库快照而不是按表导出文本：恢复目标是「一个满足全部约束、能继续记账
//! 的数据库」，不是「一批人类可读的记录」。逻辑导出要自行处理 NULL/BLOB/排序/外键
//! 装载顺序/视图/迁移版本，任何一处遗漏都可能让恢复出来的账本在语法上完好、在业务
//! 上错乱（悬空的撤销链、丢失的幂等记录）。原生快照恢复时不重新解释任何业务数据。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use libsql::Database;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

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
