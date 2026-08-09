use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::{process::Command, sync::Mutex};

const STATE_FILE: &str = "backup-state.json";

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BackupStatus {
    /// 最近一次成功生成可校验快照的时间。
    pub last_snapshot_at: Option<String>,
    /// 最近一次确认本地提交存在的时间。
    pub last_commit_at: Option<String>,
    /// 最近一次确认远端分支与本地一致的时间；只有这一态表示异地备份完成。
    pub last_remote_confirm_at: Option<String>,
    pub last_commit_id: Option<String>,
    pub unpushed_commits: u32,
    pub last_error: Option<String>,
    pub running: bool,
    pub dirty: bool,
}

pub type SharedBackupStatus = Arc<Mutex<BackupStatus>>;

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(STATE_FILE)
}

pub fn load(data_dir: &Path) -> Result<BackupStatus> {
    let state_path = path(data_dir);
    if !state_path.exists() {
        return Ok(BackupStatus::default());
    }
    serde_json::from_slice(&std::fs::read(&state_path).context("无法读取备份状态")?)
        .context("backup-state.json 格式错误")
}

pub async fn persist(data_dir: &Path, status: &BackupStatus) -> Result<()> {
    tokio::fs::create_dir_all(data_dir)
        .await
        .context("无法创建应用数据目录")?;
    let state_path = path(data_dir);
    let temporary_path = data_dir.join(format!(".{STATE_FILE}.tmp"));
    tokio::fs::write(&temporary_path, serde_json::to_vec_pretty(status)?)
        .await
        .context("无法写入备份状态临时文件")?;
    tokio::fs::rename(&temporary_path, &state_path)
        .await
        .context("无法原子更新备份状态")?;
    Ok(())
}

/// 计算尚未到达远端分支的本地提交数。远端分支尚不存在时，全部本地提交都算待推送。
pub async fn count_unpushed(repo: &Path, remote: &str, branch: &str) -> Result<u32> {
    let remote_range = format!("{remote}/{branch}..HEAD");
    if let Ok(count) = rev_list_count(repo, &remote_range).await {
        return Ok(count);
    }
    rev_list_count(repo, "HEAD").await.or_else(|error| {
        // 新初始化且尚无提交的仓库没有 HEAD，此时没有待推送提交。
        if !repo.join(".git").is_dir() {
            Err(error)
        } else {
            Ok(0)
        }
    })
}

async fn rev_list_count(repo: &Path, range: &str) -> Result<u32> {
    let output = Command::new("git")
        .args(["rev-list", "--count", range])
        .current_dir(repo)
        .output()
        .await
        .context("无法执行 git rev-list")?;
    if !output.status.success() {
        anyhow::bail!(
            "git rev-list 失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout)?
        .trim()
        .parse()
        .context("git rev-list 返回了无效计数")
}

#[cfg(test)]
mod tests {
    use std::process::Command as StdCommand;

    use super::*;

    fn git(directory: &Path, arguments: &[&str]) {
        let output = StdCommand::new("git")
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
    }

    #[tokio::test]
    async fn three_states_survive_persistence_round_trip() {
        let root = tempfile::tempdir().unwrap();
        let expected = BackupStatus {
            last_snapshot_at: Some("2026-08-09T01:00:00Z".into()),
            last_commit_at: Some("2026-08-09T01:00:01Z".into()),
            last_remote_confirm_at: Some("2026-08-09T01:00:02Z".into()),
            last_commit_id: Some("abc123".into()),
            unpushed_commits: 0,
            last_error: None,
            running: false,
            dirty: false,
        };
        persist(root.path(), &expected).await.unwrap();
        assert_eq!(load(root.path()).unwrap(), expected);
    }

    #[tokio::test]
    async fn counts_commits_missing_from_remote() {
        let root = tempfile::tempdir().unwrap();
        let bare = root.path().join("remote.git");
        let repo = root.path().join("repo");
        std::fs::create_dir(&bare).unwrap();
        git(&bare, &["init", "--bare", "--initial-branch", "main"]);
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "--initial-branch", "main"]);
        git(&repo, &["config", "user.name", "Backup Test"]);
        git(&repo, &["config", "user.email", "backup@example.invalid"]);
        git(&repo, &["remote", "add", "origin", bare.to_str().unwrap()]);
        std::fs::write(repo.join("ledger"), "one").unwrap();
        git(&repo, &["add", "ledger"]);
        git(&repo, &["commit", "-m", "one"]);

        assert_eq!(count_unpushed(&repo, "origin", "main").await.unwrap(), 1);
        git(&repo, &["push", "-u", "origin", "main"]);
        assert_eq!(count_unpushed(&repo, "origin", "main").await.unwrap(), 0);

        std::fs::write(repo.join("ledger"), "two").unwrap();
        git(&repo, &["commit", "-am", "two"]);
        assert_eq!(count_unpushed(&repo, "origin", "main").await.unwrap(), 1);
    }
}
