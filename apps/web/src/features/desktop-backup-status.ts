type GitHubCapabilityState = "missing" | "unauthenticated" | "ready" | "error"
type RepositoryBindingState = "unconfigured" | "invalid" | "restoreRequired" | "ready"

export type DesktopBackupConfig = {
  githubCapability: {
    state: GitHubCapabilityState
    account?: string
    message: string
  }
  repositoryBinding: {
    state: RepositoryBindingState
    nameWithOwner?: string
    message: string
  }
  syncEnabled: boolean
}

export type DesktopBackupStatus = {
  lastSnapshotAt?: string
  lastCommitAt?: string
  lastRemoteConfirmAt?: string
  lastCommitId?: string
  unpushedCommits: number
  lastError?: string
  running: boolean
  dirty: boolean
}

export type BackupSyncView = {
  label: string
  detail: string
  tone: "neutral" | "success" | "warning" | "error"
  canSync: boolean
}

function formatRemoteConfirmation(value?: string) {
  if (!value) return "尚无远端确认记录"
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return "已有远端确认记录"
  return `远端确认于 ${date.toLocaleString("zh-CN", { hour12: false })}`
}

export function getBackupSyncView(
  config?: DesktopBackupConfig,
  status?: DesktopBackupStatus,
  requestFailed = false,
): BackupSyncView {
  if (requestFailed) {
    return { label: "状态不可用", detail: "无法读取桌面备份状态，请打开备份设置检查", tone: "error", canSync: false }
  }
  if (!config || !status) {
    return { label: "正在检查 Git", detail: "正在读取 GitHub 与仓库状态", tone: "neutral", canSync: false }
  }

  const repository = config.repositoryBinding.nameWithOwner
  if (!config.syncEnabled) {
    const state = config.githubCapability.state
    if (state === "missing") return { label: "未安装 GitHub CLI", detail: config.githubCapability.message, tone: "warning", canSync: false }
    if (state === "unauthenticated") return { label: "GitHub 未登录", detail: config.githubCapability.message, tone: "warning", canSync: false }
    if (state === "error") return { label: "GitHub 检测失败", detail: config.githubCapability.message, tone: "error", canSync: false }
    if (config.repositoryBinding.state === "restoreRequired") {
      return { label: "等待恢复账本", detail: config.repositoryBinding.message, tone: "warning", canSync: false }
    }
    return {
      label: config.repositoryBinding.state === "invalid" ? "备份配置无效" : "备份尚未配置",
      detail: config.repositoryBinding.message,
      tone: config.repositoryBinding.state === "invalid" ? "error" : "neutral",
      canSync: false,
    }
  }

  if (status.running) return { label: "正在同步", detail: repository || "正在生成并推送账本快照", tone: "warning", canSync: false }
  if (status.lastError) return { label: "同步失败", detail: status.lastError, tone: "error", canSync: true }
  if (status.dirty || status.unpushedCommits > 0) {
    const pending = status.unpushedCommits > 0 ? `${status.unpushedCommits} 个本地提交待推送` : "账本有尚未同步的变更"
    return { label: "待同步", detail: repository ? `${repository} · ${pending}` : pending, tone: "warning", canSync: true }
  }
  if (status.lastRemoteConfirmAt) {
    return { label: "Git 已同步", detail: repository ? `${repository} · ${formatRemoteConfirmation(status.lastRemoteConfirmAt)}` : formatRemoteConfirmation(status.lastRemoteConfirmAt), tone: "success", canSync: true }
  }
  return { label: "尚未同步", detail: repository || "可以生成首个账本快照", tone: "neutral", canSync: true }
}
