use std::{
    env,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use uuid::Uuid;

use crate::backup_config::{self, BackupConfig};

const INSTALL_COMMAND: &str = "brew install gh";
const LOGIN_COMMAND: &str =
    "gh auth login --hostname github.com --git-protocol https --web --clipboard";
const REPOSITORY_QUERY: &str = r#"query($endCursor: String) {
  viewer {
    repositories(first: 100, after: $endCursor, privacy: PRIVATE,
      affiliations: [OWNER, COLLABORATOR, ORGANIZATION_MEMBER],
      orderBy: {field: UPDATED_AT, direction: DESC}) {
      nodes { nameWithOwner isEmpty isArchived viewerPermission defaultBranchRef { name } }
      pageInfo { hasNextPage endCursor }
    }
  }
}"#;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GitHubCapabilityState {
    Missing,
    Unauthenticated,
    Ready,
    Error,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubCapability {
    pub state: GitHubCapabilityState,
    pub version: Option<String>,
    pub account: Option<String>,
    pub message: String,
    pub install_command: Option<&'static str>,
    pub login_command: Option<&'static str>,
}

impl GitHubCapability {
    pub fn ready(&self) -> bool {
        self.state == GitHubCapabilityState::Ready
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryCandidate {
    pub name_with_owner: String,
    pub is_empty: bool,
    pub default_branch: String,
    pub viewer_permission: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RepositoryBindingState {
    Unconfigured,
    Invalid,
    RestoreRequired,
    Ready,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryBinding {
    pub state: RepositoryBindingState,
    pub name_with_owner: Option<String>,
    pub repo_path: Option<PathBuf>,
    pub message: String,
}

impl RepositoryBinding {
    pub fn ready(&self) -> bool {
        self.state == RepositoryBindingState::Ready
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupState {
    pub github_capability: GitHubCapability,
    pub repository_binding: RepositoryBinding,
    pub sync_enabled: bool,
}

#[derive(Clone)]
pub struct GitHubService {
    executor: Arc<dyn CommandExecutor>,
    gh_override: Option<PathBuf>,
    capability_override: Option<GitHubCapability>,
}

impl Default for GitHubService {
    fn default() -> Self {
        Self {
            executor: Arc::new(RealCommandExecutor),
            gh_override: None,
            capability_override: None,
        }
    }
}

#[derive(Debug)]
struct CommandOutput {
    success: bool,
    stdout: String,
    stderr: String,
}

#[async_trait]
trait CommandExecutor: Send + Sync {
    async fn run(
        &self,
        executable: &Path,
        args: &[String],
        cwd: Option<&Path>,
    ) -> Result<CommandOutput>;
}

struct RealCommandExecutor;

#[async_trait]
impl CommandExecutor for RealCommandExecutor {
    async fn run(
        &self,
        executable: &Path,
        args: &[String],
        cwd: Option<&Path>,
    ) -> Result<CommandOutput> {
        let mut command = Command::new(executable);
        command.args(args);
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        let output = command
            .output()
            .await
            .with_context(|| format!("无法执行 {}", executable.display()))?;
        Ok(CommandOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryMetadata {
    name_with_owner: String,
    is_private: bool,
    is_archived: bool,
    is_empty: bool,
    viewer_permission: String,
    default_branch_ref: Option<DefaultBranch>,
}

#[derive(Debug, Deserialize)]
struct DefaultBranch {
    name: String,
}

#[derive(Debug, Deserialize)]
struct RepositoryPage {
    data: RepositoryData,
}

#[derive(Debug, Deserialize)]
struct RepositoryData {
    viewer: RepositoryViewer,
}

#[derive(Debug, Deserialize)]
struct RepositoryViewer {
    repositories: RepositoryConnection,
}

#[derive(Debug, Deserialize)]
struct RepositoryConnection {
    nodes: Vec<GraphqlRepository>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphqlRepository {
    name_with_owner: String,
    is_empty: bool,
    is_archived: bool,
    viewer_permission: String,
    default_branch_ref: Option<DefaultBranch>,
}

#[derive(Clone, Copy)]
enum RepositoryContents {
    Empty,
    ValidBackup,
}

impl GitHubService {
    pub async fn capability(&self) -> GitHubCapability {
        if let Some(capability) = &self.capability_override {
            return capability.clone();
        }
        let Some(gh) = self.gh_executable() else {
            return GitHubCapability {
                state: GitHubCapabilityState::Missing,
                version: None,
                account: None,
                message: "尚未安装 GitHub CLI，安装并登录后才能启用云同步。".into(),
                install_command: Some(INSTALL_COMMAND),
                login_command: None,
            };
        };

        let version_output = match self.run(&gh, &["--version"], None).await {
            Ok(output) if output.success => output,
            Ok(output) => {
                return capability_error(
                    None,
                    format!("GitHub CLI 无法运行：{}", safe_detail(&output.stderr)),
                );
            }
            Err(error) => return capability_error(None, format!("GitHub CLI 无法运行：{error}")),
        };
        let version = version_output
            .stdout
            .lines()
            .next()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned);

        let auth = self
            .run(
                &gh,
                &["auth", "status", "--active", "--hostname", "github.com"],
                None,
            )
            .await;
        if !matches!(auth, Ok(ref output) if output.success) {
            return GitHubCapability {
                state: GitHubCapabilityState::Unauthenticated,
                version,
                account: None,
                message: "GitHub CLI 已安装，但尚未登录 GitHub.com。".into(),
                install_command: None,
                login_command: Some(LOGIN_COMMAND),
            };
        }

        match self
            .run(&gh, &["api", "user", "--jq", ".login"], None)
            .await
        {
            Ok(output) if output.success && !output.stdout.trim().is_empty() => GitHubCapability {
                state: GitHubCapabilityState::Ready,
                version,
                account: Some(output.stdout.trim().to_owned()),
                message: format!("已登录 GitHub：{}", output.stdout.trim()),
                install_command: None,
                login_command: None,
            },
            Ok(output) => capability_error(
                version,
                format!("无法读取 GitHub 登录账号：{}", safe_detail(&output.stderr)),
            ),
            Err(error) => capability_error(version, format!("无法读取 GitHub 登录账号：{error}")),
        }
    }

    pub async fn setup_state(&self, config_dir: &Path) -> SetupState {
        let capability = self.capability().await;
        let mut binding = binding_from_config(config_dir);
        if self.capability_override.is_none()
            && capability.ready()
            && binding.state == RepositoryBindingState::Ready
            && let Some(name_with_owner) = binding.name_with_owner.as_deref()
            && let Some(gh) = self.gh_executable()
            && let Err(error) = self.repository_metadata(&gh, name_with_owner).await
        {
            binding.state = RepositoryBindingState::Invalid;
            binding.message = format!(
                "已绑定仓库当前不可用于同步：{error:#}。请检查仓库隐私性、归档状态和写权限。"
            );
        }
        let sync_enabled = capability.ready() && binding.ready();
        SetupState {
            github_capability: capability,
            repository_binding: binding,
            sync_enabled,
        }
    }

    pub async fn require_sync_ready(&self, config_dir: &Path) -> Result<BackupConfig> {
        let state = self.setup_state(config_dir).await;
        if !state.github_capability.ready() {
            bail!(state.github_capability.message);
        }
        if !state.repository_binding.ready() {
            bail!(state.repository_binding.message);
        }
        backup_config::load(config_dir)?.context("备份仓库尚未配置")
    }

    pub async fn list_repositories(&self) -> Result<Vec<RepositoryCandidate>> {
        let (gh, _) = self.require_gh().await?;
        let output = self
            .run(
                &gh,
                &[
                    "api",
                    "graphql",
                    "--paginate",
                    "--slurp",
                    "-f",
                    &format!("query={REPOSITORY_QUERY}"),
                ],
                None,
            )
            .await?;
        if !output.success {
            bail!("读取 GitHub 私有仓库失败：{}", safe_detail(&output.stderr));
        }
        let pages: Vec<RepositoryPage> =
            serde_json::from_str(&output.stdout).context("GitHub 返回了无法识别的仓库列表")?;
        let mut repositories = pages
            .into_iter()
            .flat_map(|page| page.data.viewer.repositories.nodes)
            .filter(|repo| !repo.is_archived && has_write_permission(&repo.viewer_permission))
            .map(|repo| RepositoryCandidate {
                name_with_owner: repo.name_with_owner,
                is_empty: repo.is_empty,
                default_branch: repo
                    .default_branch_ref
                    .map(|branch| branch.name.trim().to_owned())
                    .filter(|branch| !branch.is_empty())
                    .unwrap_or_else(|| "main".into()),
                viewer_permission: repo.viewer_permission,
            })
            .collect::<Vec<_>>();
        repositories.sort_by(|left, right| left.name_with_owner.cmp(&right.name_with_owner));
        Ok(repositories)
    }

    pub async fn create_repository(
        &self,
        data_dir: &Path,
        config_dir: &Path,
        name: &str,
    ) -> Result<SetupState> {
        validate_repository_name(name)?;
        let (gh, capability) = self.require_gh().await?;
        let account = capability.account.context("GitHub 登录账号不可用")?;
        let name_with_owner = format!("{account}/{name}");
        let create = self
            .run(
                &gh,
                &["repo", "create", &name_with_owner, "--private"],
                None,
            )
            .await?;
        if !create.success {
            bail!("创建 GitHub 私有仓库失败：{}", safe_detail(&create.stderr));
        }

        let bind_result = async {
            let metadata = self.repository_metadata(&gh, &name_with_owner).await?;
            self.clone_and_bind(data_dir, config_dir, &gh, metadata, false)
                .await
        }
        .await;
        if let Err(error) = bind_result {
            bail!(
                "GitHub 私有仓库 {name_with_owner} 已创建，但本地绑定失败：{error:#}。请在“选择已有私有仓库”中重试；应用没有删除远端仓库。"
            );
        }
        Ok(self.setup_state(config_dir).await)
    }

    pub async fn bind_repository(
        &self,
        data_dir: &Path,
        config_dir: &Path,
        name_with_owner: &str,
    ) -> Result<SetupState> {
        validate_name_with_owner(name_with_owner)?;
        let (gh, _) = self.require_gh().await?;
        let metadata = self.repository_metadata(&gh, name_with_owner).await?;
        self.clone_and_bind(data_dir, config_dir, &gh, metadata, true)
            .await?;
        Ok(self.setup_state(config_dir).await)
    }

    async fn require_gh(&self) -> Result<(PathBuf, GitHubCapability)> {
        let capability = self.capability().await;
        if !capability.ready() {
            bail!(capability.message.clone());
        }
        let gh = self
            .gh_executable()
            .context("GitHub CLI 状态发生变化，请重新检测")?;
        Ok((gh, capability))
    }

    async fn repository_metadata(
        &self,
        gh: &Path,
        name_with_owner: &str,
    ) -> Result<RepositoryMetadata> {
        let output = self
            .run(
                gh,
                &[
                    "repo",
                    "view",
                    name_with_owner,
                    "--json",
                    "nameWithOwner,isPrivate,isArchived,isEmpty,viewerPermission,defaultBranchRef",
                ],
                None,
            )
            .await?;
        if !output.success {
            bail!("无法读取仓库信息：{}", safe_detail(&output.stderr));
        }
        let metadata: RepositoryMetadata =
            serde_json::from_str(&output.stdout).context("GitHub 返回了无法识别的仓库信息")?;
        if !metadata.is_private {
            bail!("仓库不是私有仓库；为避免账本泄露，知余拒绝绑定");
        }
        if metadata.is_archived {
            bail!("仓库已归档，无法写入备份");
        }
        if !has_write_permission(&metadata.viewer_permission) {
            bail!("当前 GitHub 账号没有该仓库的写权限");
        }
        Ok(metadata)
    }

    async fn clone_and_bind(
        &self,
        data_dir: &Path,
        config_dir: &Path,
        gh: &Path,
        metadata: RepositoryMetadata,
        validate_contents: bool,
    ) -> Result<()> {
        let managed_root = data_dir.join("backups");
        tokio::fs::create_dir_all(&managed_root)
            .await
            .context("无法创建应用托管的备份目录")?;
        let directory_name = metadata.name_with_owner.replace('/', "--");
        let destination = managed_root.join(&directory_name);
        if destination.exists() {
            bail!(
                "本地托管目录 {} 已存在；为避免覆盖，应用没有继续操作",
                destination.display()
            );
        }
        let staging = managed_root.join(format!(".setup-{}", Uuid::now_v7()));
        let staging_text = staging.to_string_lossy().into_owned();
        let clone = self
            .run(
                gh,
                &["repo", "clone", &metadata.name_with_owner, &staging_text],
                None,
            )
            .await?;
        if !clone.success {
            remove_setup_directory(&staging).await;
            bail!("克隆仓库失败：{}", safe_detail(&clone.stderr));
        }

        let result = async {
            self.git(&staging, &["config", "user.name", "知余备份"])
                .await?;
            self.git(&staging, &["config", "user.email", "local@zhiyu.desktop"])
                .await?;
            let branch = self
                .safe_default_branch(&staging, metadata.default_branch_ref.as_ref())
                .await?;
            if metadata.is_empty {
                self.git(
                    &staging,
                    &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")],
                )
                .await?;
            }
            let contents = if validate_contents {
                inspect_repository_contents(&staging).await?
            } else {
                RepositoryContents::Empty
            };
            if metadata.is_empty && !matches!(contents, RepositoryContents::Empty) {
                bail!("GitHub 报告空仓库，但克隆后检测到已有提交，请重新选择仓库");
            }
            if !metadata.is_empty && !matches!(contents, RepositoryContents::ValidBackup) {
                bail!("非空仓库不是可验证的知余备份，已拒绝绑定");
            }
            tokio::fs::rename(&staging, &destination)
                .await
                .context("无法把已校验仓库移入托管目录")?;

            let requires_restore = matches!(contents, RepositoryContents::ValidBackup);
            let config = BackupConfig {
                repo_path: destination.clone(),
                remote: "origin".into(),
                branch,
                auto_backup: !requires_restore,
                github_repository: Some(metadata.name_with_owner.clone()),
                requires_restore,
            };
            if let Err(error) = backup_config::save(config_dir, &config) {
                remove_setup_directory(&destination).await;
                return Err(error).context("仓库已克隆，但无法保存备份配置");
            }
            Ok(())
        }
        .await;
        if result.is_err() {
            remove_setup_directory(&staging).await;
        }
        result
    }

    async fn git(&self, cwd: &Path, args: &[&str]) -> Result<()> {
        let output = self
            .run_executable(Path::new("git"), args, Some(cwd))
            .await?;
        if !output.success {
            bail!(
                "git {} 失败：{}",
                args.join(" "),
                safe_detail(&output.stderr)
            );
        }
        Ok(())
    }

    async fn safe_default_branch(
        &self,
        cwd: &Path,
        default_branch: Option<&DefaultBranch>,
    ) -> Result<String> {
        let candidate = default_branch
            .map(|branch| branch.name.trim())
            .filter(|branch| !branch.is_empty())
            .unwrap_or("main");
        if candidate == "main" {
            return Ok("main".into());
        }
        let output = self
            .run_executable(
                Path::new("git"),
                &["check-ref-format", "--branch", candidate],
                Some(cwd),
            )
            .await?;
        if output.success {
            Ok(candidate.to_owned())
        } else {
            Ok("main".into())
        }
    }

    async fn run(
        &self,
        executable: &Path,
        args: &[&str],
        cwd: Option<&Path>,
    ) -> Result<CommandOutput> {
        self.run_executable(executable, args, cwd).await
    }

    async fn run_executable(
        &self,
        executable: &Path,
        args: &[&str],
        cwd: Option<&Path>,
    ) -> Result<CommandOutput> {
        let owned = args
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>();
        self.executor.run(executable, &owned, cwd).await
    }

    fn gh_executable(&self) -> Option<PathBuf> {
        self.gh_override.clone().or_else(locate_gh)
    }

    #[cfg(test)]
    pub fn test_ready() -> Self {
        Self {
            executor: Arc::new(RealCommandExecutor),
            gh_override: Some(PathBuf::from("/fake/gh")),
            capability_override: Some(GitHubCapability {
                state: GitHubCapabilityState::Ready,
                version: Some("test".into()),
                account: Some("test".into()),
                message: "ready".into(),
                install_command: None,
                login_command: None,
            }),
        }
    }
}

fn binding_from_config(config_dir: &Path) -> RepositoryBinding {
    let config = match backup_config::read_for_display(config_dir) {
        Ok(config) => config,
        Err(error) => {
            return RepositoryBinding {
                state: RepositoryBindingState::Invalid,
                name_with_owner: None,
                repo_path: None,
                message: format!("无法读取备份配置：{error:#}"),
            };
        }
    };
    if backup_config::is_unconfigured_template(&config) {
        return RepositoryBinding {
            state: RepositoryBindingState::Unconfigured,
            name_with_owner: None,
            repo_path: None,
            message: "尚未绑定 GitHub 私有仓库。请创建新仓库或选择已有私有仓库。".into(),
        };
    }
    let validation = backup_config::validate(&config);
    if !validation.valid {
        return RepositoryBinding {
            state: RepositoryBindingState::Invalid,
            name_with_owner: None,
            repo_path: Some(config.repo_path),
            message: format!("当前备份配置不可用：{}", validation.message),
        };
    }
    let name_with_owner = config.github_repository.clone().or_else(|| {
        git_remote_url(&config.repo_path, &config.remote)
            .ok()
            .and_then(|url| parse_github_remote(&url))
    });
    let Some(name_with_owner) = name_with_owner else {
        return RepositoryBinding {
            state: RepositoryBindingState::Invalid,
            name_with_owner: None,
            repo_path: Some(config.repo_path),
            message: "本地 Git 仓库有效，但 origin 不是可识别的 GitHub.com 仓库；请重新绑定。"
                .into(),
        };
    };
    if config.requires_restore {
        RepositoryBinding {
            state: RepositoryBindingState::RestoreRequired,
            name_with_owner: Some(name_with_owner),
            repo_path: Some(config.repo_path),
            message: "这个仓库已有知余备份。请先从该备份恢复并重启，再启用同步。".into(),
        }
    } else {
        RepositoryBinding {
            state: RepositoryBindingState::Ready,
            name_with_owner: Some(name_with_owner),
            repo_path: Some(config.repo_path),
            message: "GitHub 私有仓库已绑定，可以同步。".into(),
        }
    }
}

async fn inspect_repository_contents(repo: &Path) -> Result<RepositoryContents> {
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo)
        .output()
        .await
        .context("无法检查仓库提交")?;
    if !head.status.success() {
        return Ok(RepositoryContents::Empty);
    }
    let manifest_path = repo.join(zhiyu_api::backup::MANIFEST_IN_REPO);
    let snapshot_path = repo.join(zhiyu_api::backup::SNAPSHOT_IN_REPO);
    let manifest: zhiyu_api::backup::Manifest = serde_json::from_slice(
        &tokio::fs::read(&manifest_path)
            .await
            .context("非空仓库缺少知余备份清单")?,
    )
    .context("知余备份清单格式错误")?;
    zhiyu_api::backup::check_against_manifest(&snapshot_path, &manifest).await?;
    zhiyu_api::backup::verify_snapshot(
        &snapshot_path,
        &manifest.application_version,
        &manifest.source_journal_mode,
        &manifest.snapshot_method,
    )
    .await?;
    Ok(RepositoryContents::ValidBackup)
}

fn locate_gh() -> Option<PathBuf> {
    let path = env::var_os("PATH");
    locate_gh_in(
        path.as_deref(),
        &["/opt/homebrew/bin/gh", "/usr/local/bin/gh"],
    )
}

fn locate_gh_in(path: Option<&std::ffi::OsStr>, fallbacks: &[&str]) -> Option<PathBuf> {
    path.into_iter()
        .flat_map(env::split_paths)
        .map(|directory| directory.join("gh"))
        .chain(fallbacks.iter().map(PathBuf::from))
        .find(|candidate| candidate.is_file())
}

fn parse_github_remote(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches(".git");
    let path = trimmed
        .strip_prefix("https://github.com/")
        .or_else(|| trimmed.strip_prefix("http://github.com/"))
        .or_else(|| trimmed.strip_prefix("git@github.com:"))
        .or_else(|| trimmed.strip_prefix("ssh://git@github.com/"))?;
    validate_name_with_owner(path).ok()?;
    Some(path.to_owned())
}

fn git_remote_url(repo: &Path, remote: &str) -> Result<String> {
    let output = std::process::Command::new("git")
        .args(["remote", "get-url", remote])
        .current_dir(repo)
        .output()
        .context("无法读取 Git remote")?;
    if !output.status.success() {
        bail!("无法读取 Git remote");
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn validate_repository_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 100
        || name.contains('/')
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".-_".contains(character))
    {
        bail!("仓库名只能包含字母、数字、点、短横线或下划线，且不能超过 100 个字符");
    }
    Ok(())
}

fn validate_name_with_owner(value: &str) -> Result<()> {
    let mut parts = value.split('/');
    let owner = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    if parts.next().is_some() || owner.is_empty() {
        bail!("仓库标识必须是 owner/name");
    }
    validate_repository_name(owner)?;
    validate_repository_name(name)
}

fn has_write_permission(permission: &str) -> bool {
    matches!(permission, "WRITE" | "MAINTAIN" | "ADMIN")
}

fn capability_error(version: Option<String>, message: String) -> GitHubCapability {
    GitHubCapability {
        state: GitHubCapabilityState::Error,
        version,
        account: None,
        message,
        install_command: None,
        login_command: Some(LOGIN_COMMAND),
    }
}

fn safe_detail(value: &str) -> String {
    let text = value.trim();
    if text.is_empty() {
        return "未返回详细原因".into();
    }
    text.split_whitespace()
        .map(|word| {
            if word.starts_with("gho_")
                || word.starts_with("ghp_")
                || word.starts_with("ghu_")
                || word.starts_with("ghs_")
                || word.starts_with("ghr_")
                || word.starts_with("github_pat_")
            {
                "[已隐藏凭据]"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(600)
        .collect()
}

async fn remove_setup_directory(path: &Path) {
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(".setup-") || name.contains("--"))
    {
        tokio::fs::remove_dir_all(path).await.ok();
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, ffi::OsString, sync::Mutex};

    use super::*;

    struct FakeExecutor {
        outputs: Mutex<VecDeque<CommandOutput>>,
    }

    #[async_trait]
    impl CommandExecutor for FakeExecutor {
        async fn run(
            &self,
            _executable: &Path,
            _args: &[String],
            _cwd: Option<&Path>,
        ) -> Result<CommandOutput> {
            Ok(self.outputs.lock().unwrap().pop_front().unwrap())
        }
    }

    fn service(outputs: Vec<CommandOutput>) -> GitHubService {
        GitHubService {
            executor: Arc::new(FakeExecutor {
                outputs: Mutex::new(outputs.into()),
            }),
            gh_override: Some(PathBuf::from("/fake/gh")),
            capability_override: None,
        }
    }

    fn output(success: bool, stdout: &str, stderr: &str) -> CommandOutput {
        CommandOutput {
            success,
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    #[test]
    fn finds_homebrew_gh_when_gui_path_does_not_include_it() {
        let root = tempfile::tempdir().unwrap();
        let gh = root.path().join("gh");
        std::fs::write(&gh, b"fake").unwrap();
        assert_eq!(
            locate_gh_in(Some(&OsString::from("/missing")), &[gh.to_str().unwrap()]),
            Some(gh)
        );
    }

    #[tokio::test]
    async fn distinguishes_unauthenticated_and_ready() {
        let unauthenticated = service(vec![
            output(true, "gh version 2.92.0\n", ""),
            output(false, "", "not logged in"),
        ]);
        assert_eq!(
            unauthenticated.capability().await.state,
            GitHubCapabilityState::Unauthenticated
        );

        let ready = service(vec![
            output(true, "gh version 2.92.0\n", ""),
            output(true, "", ""),
            output(true, "wensia\n", ""),
        ]);
        let capability = ready.capability().await;
        assert_eq!(capability.state, GitHubCapabilityState::Ready);
        assert_eq!(capability.account.as_deref(), Some("wensia"));
    }

    #[test]
    fn redacts_tokens_from_errors() {
        assert_eq!(
            safe_detail("request failed gho_secret github_pat_other"),
            "request failed [已隐藏凭据] [已隐藏凭据]"
        );
    }

    #[test]
    fn parses_https_and_ssh_github_remotes() {
        assert_eq!(
            parse_github_remote("https://github.com/acme/ledger.git").as_deref(),
            Some("acme/ledger")
        );
        assert_eq!(
            parse_github_remote("git@github.com:acme/ledger.git").as_deref(),
            Some("acme/ledger")
        );
        assert!(parse_github_remote("https://example.com/acme/ledger.git").is_none());
    }

    #[test]
    fn template_config_is_an_actionable_unconfigured_state() {
        let root = tempfile::tempdir().unwrap();
        let binding = binding_from_config(root.path());
        assert_eq!(binding.state, RepositoryBindingState::Unconfigured);
        assert!(binding.message.contains("创建新仓库或选择已有私有仓库"));
        assert!(!binding.message.contains("/请填写"));
    }

    #[tokio::test]
    async fn rejects_public_or_read_only_repository_metadata() {
        let public = service(vec![output(
            true,
            r#"{"nameWithOwner":"acme/public","isPrivate":false,"isArchived":false,"isEmpty":true,"viewerPermission":"ADMIN","defaultBranchRef":null}"#,
            "",
        )]);
        let error = public
            .repository_metadata(Path::new("/fake/gh"), "acme/public")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("不是私有仓库"));

        let read_only = service(vec![output(
            true,
            r#"{"nameWithOwner":"acme/readonly","isPrivate":true,"isArchived":false,"isEmpty":false,"viewerPermission":"READ","defaultBranchRef":{"name":"main"}}"#,
            "",
        )]);
        let error = read_only
            .repository_metadata(Path::new("/fake/gh"), "acme/readonly")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("没有该仓库的写权限"));
    }

    #[tokio::test]
    async fn reports_remote_creation_when_local_binding_fails() {
        let service = service(vec![
            output(true, "gh version 2.92.0\n", ""),
            output(true, "", ""),
            output(true, "wensia\n", ""),
            output(true, "", ""),
            output(false, "", "temporary API failure"),
        ]);
        let root = tempfile::tempdir().unwrap();
        let error = service
            .create_repository(
                &root.path().join("data"),
                &root.path().join("config"),
                "zhiyu-backup",
            )
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("已创建，但本地绑定失败"));
        assert!(message.contains("应用没有删除远端仓库"));
        assert!(!backup_config::path(&root.path().join("config")).exists());
    }

    #[tokio::test]
    async fn empty_or_invalid_default_branch_falls_back_to_main() {
        let no_commands = service(vec![]);
        let empty = DefaultBranch { name: "  ".into() };
        assert_eq!(
            no_commands
                .safe_default_branch(Path::new("/tmp"), Some(&empty))
                .await
                .unwrap(),
            "main"
        );

        let invalid = service(vec![output(false, "", "invalid branch")]);
        let bad = DefaultBranch {
            name: "bad..branch".into(),
        };
        assert_eq!(
            invalid
                .safe_default_branch(Path::new("/tmp"), Some(&bad))
                .await
                .unwrap(),
            "main"
        );
    }

    #[tokio::test]
    async fn recognizes_only_a_verified_zhiyu_backup_in_non_empty_repo() {
        let root = tempfile::tempdir().unwrap();
        let ordinary = root.path().join("ordinary");
        std::fs::create_dir(&ordinary).unwrap();
        git_for_test(&ordinary, &["init", "--initial-branch", "main"]);
        git_for_test(&ordinary, &["config", "user.name", "test"]);
        git_for_test(&ordinary, &["config", "user.email", "test@example.invalid"]);
        std::fs::write(ordinary.join("README.md"), "not a backup").unwrap();
        git_for_test(&ordinary, &["add", "README.md"]);
        git_for_test(&ordinary, &["commit", "-m", "ordinary"]);
        assert!(inspect_repository_contents(&ordinary).await.is_err());

        let verified = root.path().join("verified");
        let db_path = root.path().join("source.db");
        std::fs::create_dir(&verified).unwrap();
        git_for_test(&verified, &["init", "--initial-branch", "main"]);
        git_for_test(&verified, &["config", "user.name", "test"]);
        git_for_test(&verified, &["config", "user.email", "test@example.invalid"]);
        let db = libsql::Builder::new_local(&db_path).build().await.unwrap();
        zhiyu_api::db::migrate(&db).await.unwrap();
        let (snapshot, manifest) = zhiyu_api::backup::create_snapshot(
            &db,
            &verified.join(zhiyu_api::backup::STAGING_IN_REPO),
            "test",
        )
        .await
        .unwrap();
        zhiyu_api::backup::commit_snapshot(&verified, &snapshot, &manifest)
            .await
            .unwrap();
        assert!(matches!(
            inspect_repository_contents(&verified).await.unwrap(),
            RepositoryContents::ValidBackup
        ));
    }

    fn git_for_test(directory: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(directory)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
