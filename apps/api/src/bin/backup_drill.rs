use std::{path::Path, process::Command};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use zhiyu_api::backup::{self, MANIFEST_IN_REPO, Manifest, SNAPSHOT_IN_REPO, STAGING_IN_REPO};

fn git(cwd: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git").arg("-C").arg(cwd).args(args).output()?;
    if !output.status.success() {
        bail!(
            "git {} 失败：{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

async fn file_sha256(path: &Path) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(tokio::fs::read(path).await?)
    ))
}

#[tokio::main]
async fn main() -> Result<()> {
    let root = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .context("用法：backup_drill <全新临时目录>")?;
    if root.exists() && root.read_dir()?.next().is_some() {
        bail!("演练目录必须为空：{}", root.display());
    }
    std::fs::create_dir_all(&root)?;
    let live = root.join("source/zhiyu.db");
    std::fs::create_dir_all(live.parent().expect("live 一定有父目录"))?;
    let db = libsql::Builder::new_local(&live).build().await?;
    zhiyu_api::db::migrate(&db).await?;
    let conn = db.connect()?;
    conn.execute(
        "INSERT INTO users(id, email, password_hash, timezone, created_at, updated_at)
         VALUES ('drill-user', 'drill@example.com', 'hash', 'Asia/Shanghai', 'now', 'now')",
        (),
    )
    .await?;
    drop(conn);

    let repo = root.join("backup-work");
    let remote = root.join("remote.git");
    std::fs::create_dir_all(&repo)?;
    git(&repo, &["init", "--quiet", "--initial-branch", "main"])?;
    git(&repo, &["config", "user.email", "drill@example.com"])?;
    git(&repo, &["config", "user.name", "backup drill"])?;
    git(
        &root,
        &[
            "init",
            "--quiet",
            "--bare",
            "--initial-branch",
            "main",
            remote.to_str().context("临时路径不是 UTF-8")?,
        ],
    )?;
    git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    )?;
    let (snapshot, manifest) =
        backup::create_snapshot(&db, &repo.join(STAGING_IN_REPO), "backup-drill").await?;
    let expected_database_sha256 = manifest.database_sha256.clone();
    backup::commit_snapshot(&repo, &snapshot, &manifest).await?;
    backup::push(&repo, "origin", "main").await?;
    println!("PASS 备份已提交并由本地裸远端确认：{expected_database_sha256}");
    drop(db);

    std::fs::remove_file(&live)?;
    let rescue = root.join("rescue-clone");
    git(
        &root,
        &[
            "clone",
            "--quiet",
            remote.to_str().unwrap(),
            rescue.to_str().unwrap(),
        ],
    )?;
    let report = backup::restore(&rescue, &live, &root.join("quarantine")).await?;
    let restored = libsql::Builder::new_local(&live).build().await?;
    let conn = restored.connect()?;
    let mut rows = conn
        .query("SELECT email FROM users WHERE id = 'drill-user'", ())
        .await?;
    let email: String = rows.next().await?.context("恢复后样本用户丢失")?.get(0)?;
    if email != "drill@example.com" || file_sha256(&live).await? != expected_database_sha256 {
        bail!("恢复后的样本数据或 database_sha256 不一致");
    }
    drop(rows);
    drop(conn);
    drop(restored);
    println!(
        "PASS 原库销毁后从独立 clone 恢复，database_sha256 一致；旧库隔离到 {}",
        report.quarantined_to.display()
    );

    let production_before = tokio::fs::read(&live).await?;
    let manifest_path = rescue.join(MANIFEST_IN_REPO);
    let original_manifest = tokio::fs::read(&manifest_path).await?;
    let mut newer: Manifest = serde_json::from_slice(&original_manifest)?;
    newer.schema_migration_versions.push(999);
    tokio::fs::write(&manifest_path, serde_json::to_vec_pretty(&newer)?).await?;
    let newer_error = backup::restore(&rescue, &live, &root.join("newer-quarantine"))
        .await
        .expect_err("新版本备份必须被拒绝");
    if tokio::fs::read(&live).await? != production_before {
        bail!("新版本备份拒绝后生产库发生变化");
    }
    println!("PASS 新版本备份被拒绝且生产库未变：{newer_error}");

    let mut bad_hash: Manifest = serde_json::from_slice(&original_manifest)?;
    bad_hash.database_sha256 = "00".repeat(32);
    tokio::fs::write(&manifest_path, serde_json::to_vec_pretty(&bad_hash)?).await?;
    let hash_error = backup::restore(&rescue, &live, &root.join("hash-quarantine"))
        .await
        .expect_err("哈希错误必须被拒绝");
    if tokio::fs::read(&live).await? != production_before {
        bail!("哈希错误拒绝后生产库发生变化");
    }
    println!("PASS 哈希不符被拒绝且生产库未变：{hash_error}");

    // 保证演练读取的权威快照确实来自远端 clone，而不是源机器的旧文件。
    if !rescue.join(SNAPSHOT_IN_REPO).exists() {
        bail!("独立 clone 中没有备份快照");
    }
    println!("BACKUP DRILL PASSED");
    Ok(())
}
