import { useMutation, useQuery } from "@tanstack/react-query"
import { RefreshCwIcon, SettingsIcon } from "lucide-react"

import { Button, useToast } from "../components/ui"
import { getBackupSyncView, type DesktopBackupConfig, type DesktopBackupStatus } from "./desktop-backup-status"

async function desktopBackupRequest<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, {
    ...init,
    headers: {
      Accept: "application/json",
      ...(init?.body ? { "Content-Type": "application/json" } : {}),
      ...init?.headers,
    },
  })
  const body = await response.json().catch(() => ({})) as T & { error?: string }
  if (!response.ok) throw new Error(body.error || `请求失败（${response.status}）`)
  return body
}

export function DesktopBackupFooter() {
  const toast = useToast()
  const configQuery = useQuery({
    queryKey: ["desktop-backup", "config"],
    queryFn: () => desktopBackupRequest<DesktopBackupConfig>("/desktop/backup/api/config"),
    retry: false,
    staleTime: 15_000,
    refetchInterval: 60_000,
  })
  const statusQuery = useQuery({
    queryKey: ["desktop-backup", "status"],
    queryFn: () => desktopBackupRequest<DesktopBackupStatus>("/desktop/backup/api/status"),
    retry: false,
    refetchInterval: (query) => query.state.data?.running ? 1_000 : 5_000,
  })
  const sync = useMutation({
    mutationFn: () => desktopBackupRequest<{ started: boolean }>("/desktop/backup/api/run", { method: "POST" }),
    onSuccess: () => {
      void statusQuery.refetch()
      toast({ title: "已开始同步", description: "知余正在生成快照并推送到 GitHub。", type: "success" })
    },
    onError: (error) => toast({ title: "无法同步", description: error.message, type: "error" }),
  })
  const openSettings = useMutation({
    mutationFn: () => desktopBackupRequest<{ opened: boolean }>("/desktop/backup/api/open-settings", { method: "POST" }),
    onError: (error) => toast({ title: "无法打开备份设置", description: error.message, type: "error" }),
  })
  const view = getBackupSyncView(configQuery.data, statusQuery.data, configQuery.isError || statusQuery.isError)
  const syncing = sync.isPending || statusQuery.data?.running === true

  return (
    <div className="backup-footer">
      <div className="backup-sync-state" data-tone={view.tone} title={view.detail}>
        <span aria-hidden="true" className="backup-sync-dot" />
        <span className="backup-sync-copy">
          <strong>{view.label}</strong>
          <small>{configQuery.data?.repositoryBinding.nameWithOwner || "GitHub 私有备份"}</small>
        </span>
      </div>
      <div className="backup-footer-actions">
        <Button
          aria-label={syncing ? "正在同步 Git 备份" : `立即同步 Git 备份：${view.label}`}
          className="backup-footer-action backup-sync-action"
          disabled={!view.canSync || syncing}
          onClick={() => sync.mutate()}
          size="icon-sm"
          title={`${view.label}。${view.detail}`}
          variant="ghost"
        >
          <RefreshCwIcon aria-hidden="true" className={syncing ? "backup-sync-spinning" : undefined} />
          <span aria-hidden="true" className="backup-sync-badge" data-tone={view.tone} />
        </Button>
        <Button
          aria-label="打开备份设置"
          className="backup-footer-action"
          disabled={openSettings.isPending}
          onClick={() => openSettings.mutate()}
          size="icon-sm"
          title="备份设置"
          variant="ghost"
        ><SettingsIcon aria-hidden="true" /></Button>
      </div>
    </div>
  )
}
