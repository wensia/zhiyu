use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const CONFIG_FILE: &str = "backup.json";
const TEMPLATE_REPO_PATH: &str = "/请填写/ledger-backup";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupConfig {
    pub repo_path: PathBuf,
    pub remote: String,
    pub branch: String,
    #[serde(default = "default_auto_backup")]
    pub auto_backup: bool,
    /// GitHub `owner/name`。旧配置没有该字段时会从 origin URL 尝试识别。
    #[serde(default)]
    pub github_repository: Option<String>,
    /// 已绑定远端备份但尚未完成离线恢复时，禁止本机继续向远端写入。
    #[serde(default)]
    pub requires_restore: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigValidation {
    pub directory_exists: bool,
    pub git_repository: bool,
    pub remote_exists: bool,
    pub valid: bool,
    pub message: String,
}

fn default_auto_backup() -> bool {
    true
}

pub fn path(config_dir: &Path) -> PathBuf {
    config_dir.join(CONFIG_FILE)
}

fn template() -> BackupConfig {
    BackupConfig {
        repo_path: PathBuf::from(TEMPLATE_REPO_PATH),
        remote: "origin".into(),
        branch: "main".into(),
        auto_backup: true,
        github_repository: None,
        requires_restore: false,
    }
}

pub fn is_unconfigured_template(config: &BackupConfig) -> bool {
    config.repo_path == Path::new(TEMPLATE_REPO_PATH) && config.github_repository.is_none()
}

/// 读取备份配置；第一次启动会写入模板，并用 `None` 表示尚未配置。
pub fn load(config_dir: &Path) -> Result<Option<BackupConfig>> {
    std::fs::create_dir_all(config_dir).context("无法创建应用配置目录")?;
    let config_path = path(config_dir);
    if !config_path.exists() {
        std::fs::write(&config_path, serde_json::to_vec_pretty(&template())?)
            .context("无法写入备份配置模板")?;
        return Ok(None);
    }

    let config = read_existing(config_dir)?;
    let validation = validate(&config);
    if !validation.valid {
        bail!(validation.message);
    }
    Ok(Some(config))
}

/// 读取供设置页回显的原始配置；即使仓库尚未建好也返回用户已填写的值。
pub fn read_for_display(config_dir: &Path) -> Result<BackupConfig> {
    std::fs::create_dir_all(config_dir).context("无法创建应用配置目录")?;
    if !path(config_dir).exists() {
        std::fs::write(path(config_dir), serde_json::to_vec_pretty(&template())?)
            .context("无法写入备份配置模板")?;
    }
    read_existing(config_dir)
}

fn read_existing(config_dir: &Path) -> Result<BackupConfig> {
    serde_json::from_slice(&std::fs::read(path(config_dir)).context("无法读取 backup.json")?)
        .context("backup.json 格式错误")
}

pub fn validate(config: &BackupConfig) -> ConfigValidation {
    let directory_exists = config.repo_path.is_dir();
    let git_repository = directory_exists
        && Command::new("git")
            .args(["rev-parse", "--is-inside-work-tree"])
            .current_dir(&config.repo_path)
            .output()
            .is_ok_and(|output| {
                output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "true"
            });
    let remote_exists = git_repository
        && !config.remote.trim().is_empty()
        && Command::new("git")
            .args(["remote", "get-url", config.remote.trim()])
            .current_dir(&config.repo_path)
            .output()
            .is_ok_and(|output| output.status.success());
    let branch_valid = !config.branch.trim().is_empty();
    let valid = directory_exists && git_repository && remote_exists && branch_valid;
    let message = if config.repo_path.as_os_str().is_empty() {
        "备份仓库路径为空；请先选择一个目录".into()
    } else if !directory_exists {
        format!("目录 {} 不存在，请先创建该目录", config.repo_path.display())
    } else if !git_repository {
        "该目录不是 git 仓库，请先执行 `git init --initial-branch main` 并配好 remote".into()
    } else if config.remote.trim().is_empty() {
        "remote 不能为空，请填写已经配置的远端名称（通常是 origin）".into()
    } else if !remote_exists {
        format!(
            "找不到 remote `{}`，请先执行 `git remote add {} <仓库地址>`",
            config.remote.trim(),
            config.remote.trim()
        )
    } else if !branch_valid {
        "branch 不能为空，请填写要推送的分支名（通常是 main）".into()
    } else {
        "配置有效，可以执行备份".into()
    };
    ConfigValidation {
        directory_exists,
        git_repository,
        remote_exists,
        valid,
        message,
    }
}

pub fn save(config_dir: &Path, config: &BackupConfig) -> Result<ConfigValidation> {
    let validation = validate(config);
    if !validation.valid {
        bail!(validation.message.clone());
    }
    std::fs::create_dir_all(config_dir).context("无法创建应用配置目录")?;
    let temporary_path = config_dir.join(".backup.json.tmp");
    std::fs::write(&temporary_path, serde_json::to_vec_pretty(config)?)
        .context("无法写入备份配置临时文件")?;
    std::fs::rename(&temporary_path, path(config_dir)).context("无法原子更新备份配置")?;
    Ok(validation)
}

pub fn mark_restore_completed(config_dir: &Path) -> Result<()> {
    let mut config = read_existing(config_dir)?;
    config.requires_restore = false;
    save(config_dir, &config)?;
    Ok(())
}

pub fn set_auto_backup(config_dir: &Path, enabled: bool) -> Result<BackupConfig> {
    let mut config = read_existing(config_dir)?;
    if enabled && config.requires_restore {
        bail!("必须先从已绑定的备份恢复，才能启用自动同步");
    }
    config.auto_backup = enabled;
    save(config_dir, &config)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_config_writes_template_and_returns_unconfigured() {
        let root = tempfile::tempdir().unwrap();
        assert!(load(root.path()).unwrap().is_none());
        assert!(path(root.path()).exists());
        assert!(is_unconfigured_template(
            &read_for_display(root.path()).unwrap()
        ));
    }

    #[test]
    fn non_git_repo_has_actionable_error() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        std::fs::write(
            path(root.path()),
            serde_json::to_vec(&BackupConfig {
                repo_path: repo,
                remote: "origin".into(),
                branch: "main".into(),
                auto_backup: true,
                github_repository: None,
                requires_restore: false,
            })
            .unwrap(),
        )
        .unwrap();
        let error = load(root.path()).unwrap_err();
        assert!(error.to_string().contains("git init"));
    }

    #[test]
    fn invalid_save_does_not_overwrite_existing_config() {
        let root = tempfile::tempdir().unwrap();
        let existing = br#"{"repoPath":"/old","remote":"origin","branch":"main"}"#;
        std::fs::write(path(root.path()), existing).unwrap();
        let invalid_repo = root.path().join("not-git");
        std::fs::create_dir(&invalid_repo).unwrap();
        let result = save(
            root.path(),
            &BackupConfig {
                repo_path: invalid_repo,
                remote: "origin".into(),
                branch: "main".into(),
                auto_backup: true,
                github_repository: None,
                requires_restore: false,
            },
        );
        assert!(result.is_err());
        assert_eq!(std::fs::read(path(root.path())).unwrap(), existing);
    }
}
