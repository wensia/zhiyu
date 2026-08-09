import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { render, screen, waitFor } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { AppToastProvider } from "../components/ui"
import {
  DesktopBackupFooter,
} from "./desktop-backup-footer"
import {
  getBackupSyncView,
  type DesktopBackupConfig,
  type DesktopBackupStatus,
} from "./desktop-backup-status"

const readyConfig: DesktopBackupConfig = {
  githubCapability: { state: "ready", account: "wensia", message: "GitHub CLI 已就绪" },
  repositoryBinding: { state: "ready", nameWithOwner: "wensia/zhiyu-backup", message: "仓库已绑定" },
  syncEnabled: true,
}

const cleanStatus: DesktopBackupStatus = {
  unpushedCommits: 0,
  running: false,
  dirty: false,
  lastRemoteConfirmAt: "2026-08-09T08:00:00Z",
}

function jsonResponse(body: unknown, status = 200) {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
  } as Response
}

function renderFooter() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
  return render(
    <QueryClientProvider client={client}>
      <AppToastProvider><DesktopBackupFooter /></AppToastProvider>
    </QueryClientProvider>,
  )
}

describe("desktop backup sidebar footer", () => {
  beforeEach(() => vi.useRealTimers())
  afterEach(() => vi.unstubAllGlobals())

  it("derives readable Git states without relying on color", () => {
    expect(getBackupSyncView({
      ...readyConfig,
      githubCapability: { state: "missing", message: "请安装 gh" },
      syncEnabled: false,
    }, cleanStatus)).toMatchObject({ label: "未安装 GitHub CLI", tone: "warning", canSync: false })

    expect(getBackupSyncView(readyConfig, { ...cleanStatus, lastRemoteConfirmAt: undefined, dirty: true, unpushedCommits: 2 }))
      .toMatchObject({ label: "待同步", tone: "warning", canSync: true })
    expect(getBackupSyncView(readyConfig, { ...cleanStatus, lastError: "push 被拒绝" }))
      .toMatchObject({ label: "同步失败", tone: "error", canSync: true })
    expect(getBackupSyncView(readyConfig, cleanStatus))
      .toMatchObject({ label: "Git 已同步", tone: "success", canSync: true })
  })

  it("shows repository status and calls the desktop sync and settings actions", async () => {
    const status = { ...cleanStatus, lastRemoteConfirmAt: undefined, dirty: true, unpushedCommits: 1 }
    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const path = input.toString()
      if (path.endsWith("/config")) return jsonResponse(readyConfig)
      if (path.endsWith("/status")) return jsonResponse(status)
      if (path.endsWith("/run") && init?.method === "POST") return jsonResponse({ started: true }, 202)
      if (path.endsWith("/open-settings") && init?.method === "POST") return jsonResponse({ opened: true })
      return jsonResponse({ error: "unexpected request" }, 404)
    })
    vi.stubGlobal("fetch", fetchMock)
    const user = userEvent.setup()
    renderFooter()

    expect(await screen.findByText("待同步")).toBeInTheDocument()
    expect(screen.getByText("wensia/zhiyu-backup")).toBeInTheDocument()
    await user.click(screen.getByRole("button", { name: /立即同步 Git 备份/ }))
    await waitFor(() => expect(fetchMock).toHaveBeenCalledWith(
      "/desktop/backup/api/run",
      expect.objectContaining({ method: "POST" }),
    ))
    await user.click(screen.getByRole("button", { name: "打开备份设置" }))
    await waitFor(() => expect(fetchMock).toHaveBeenCalledWith(
      "/desktop/backup/api/open-settings",
      expect.objectContaining({ method: "POST" }),
    ))
  })
})
