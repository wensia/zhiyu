import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { useState } from "react"
import { MemoryRouter } from "react-router-dom"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { api } from "../../api/client"
import type { Dashboard, Plugin } from "../../api/types"
import { TopbarSlotContext, type TopbarSlots } from "../../components/topbar-slots"
import { AppToastProvider } from "../../components/ui"
import { PluginStateProvider } from "../../plugins/state"
import { StatisticsDashboardWorkspace } from "./statistics-dashboard-workspace"

vi.mock("../../api/client", () => ({
  ApiClientError: class ApiClientError extends Error { status = 500; code = "request_failed" },
  api: {
    dashboards: vi.fn(),
    createDashboard: vi.fn(),
    createDefaultDashboard: vi.fn(),
    updateDashboard: vi.fn(),
    deleteDashboard: vi.fn(),
    replaceDashboardWidgets: vi.fn(),
    dashboardWidgetTypes: vi.fn(),
    transactionSummary: vi.fn(),
    statisticsAggregate: vi.fn(),
    ledgerAccounts: vi.fn(),
    summary: vi.fn(),
    debts: vi.fn(),
  },
}))

const definitions = {
  core: [
    { id: "income-expense-trend", name: "收支趋势", description: "按日展示月度收入与支出趋势。", defaultW: 8, defaultH: 4, minW: 4, minH: 3 },
    { id: "category-share", name: "分类占比", description: "按分类展示收入与支出占比。", defaultW: 4, defaultH: 4, minW: 3, minH: 3 },
    { id: "account-balances", name: "账户余额", description: "展示当前账户余额。", defaultW: 4, defaultH: 3, minW: 3, minH: 2 },
    { id: "month-compare", name: "月度对比", description: "对比近六个月收入与支出。", defaultW: 8, defaultH: 3, minW: 4, minH: 2 },
  ],
  plugins: [{ pluginId: "debts", enabled: true, widgets: [{ id: "overview", name: "谁欠我多少", description: "汇总当前债务余额。", defaultW: 4, defaultH: 3, minW: 3, minH: 2 }] }],
}

const baseDashboard: Dashboard = {
  id: "dashboard-1",
  name: "月度",
  position: 0,
  widgets: [{ id: "widget-1", widgetType: "core:income-expense-trend", pluginId: null, x: 0, y: 0, w: 8, h: 4, config: {} }],
}

const enabledPlugins: Plugin[] = [{ id: "debts", name: "债务", description: "", enabled: true, ownsTransactions: true, routePrefixes: ["/app/debts"] }]
const disabledPlugins: Plugin[] = [{ ...enabledPlugins[0], enabled: false }]

function Harness({ plugins = enabledPlugins }: { plugins?: Plugin[] }) {
  const [slots, setSlots] = useState<TopbarSlots>()
  return (
    <TopbarSlotContext.Provider value={setSlots}>
      <PluginStateProvider value={{ plugins, isLoading: false }}>
        <div data-testid="topbar-actions">{slots?.actions}</div>
        <StatisticsDashboardWorkspace />
      </PluginStateProvider>
    </TopbarSlotContext.Provider>
  )
}

function renderWorkspace(plugins?: Plugin[]) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
  return render(
    <MemoryRouter>
      <QueryClientProvider client={client}>
        <AppToastProvider><Harness plugins={plugins} /></AppToastProvider>
      </QueryClientProvider>
    </MemoryRouter>,
  )
}

describe("StatisticsDashboardWorkspace", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 1280 })
    vi.mocked(api.dashboards).mockResolvedValue([baseDashboard])
    vi.mocked(api.dashboardWidgetTypes).mockResolvedValue(definitions)
    vi.mocked(api.transactionSummary).mockResolvedValue({ month: "2026-08", days: [], byCategory: [], incomeCents: 0, expenseCents: 0, netCents: 0, transactionCount: 0 })
    vi.mocked(api.statisticsAggregate).mockResolvedValue([])
    vi.mocked(api.ledgerAccounts).mockResolvedValue([])
    vi.mocked(api.summary).mockResolvedValue({ lendOutRemainingCents: 0, borrowInRemainingCents: 0, netCents: 0, overdueCount: 0 })
    vi.mocked(api.debts).mockResolvedValue({ items: [], page: 1, pageSize: 100, total: 0 })
    vi.mocked(api.replaceDashboardWidgets).mockImplementation(async (id, widgets) => ({ ...baseDashboard, id, widgets: widgets.map((widget, index) => ({ ...widget, id: widget.id ?? `saved-${index}`, pluginId: widget.pluginId ?? null, config: widget.config ?? {} })) }))
  })

  it("hides every editing control in read-only mode and adds and deletes widgets in edit mode", async () => {
    const user = userEvent.setup()
    renderWorkspace()
    expect(await screen.findByText("收支趋势")).toBeInTheDocument()
    expect(screen.queryByRole("button", { name: /操作组件/ })).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: /添加组件/ })).not.toBeInTheDocument()

    await user.click(screen.getByRole("button", { name: "编辑" }))
    expect(screen.getByRole("button", { name: "添加组件" })).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "操作组件 收支趋势" })).toBeInTheDocument()
    await user.click(screen.getByRole("button", { name: "添加组件" }))
    const sheet = await screen.findByRole("dialog", { name: "添加组件" })
    await user.click(within(sheet).getByRole("button", { name: /分类占比/ }))
    await waitFor(() => expect(api.replaceDashboardWidgets).toHaveBeenCalled(), { timeout: 1500 })
    await user.click(within(sheet).getByRole("button", { name: "关闭" }))
    expect(await screen.findByRole("heading", { name: "分类占比" })).toBeInTheDocument()

    await user.click(screen.getByRole("button", { name: "操作组件 分类占比" }))
    await user.click(screen.getByRole("menuitem", { name: "删除" }))
    await waitFor(() => expect(api.replaceDashboardWidgets).toHaveBeenCalledTimes(2), { timeout: 1500 })
  })

  it("does not save an unchanged layout when the dashboard mounts", async () => {
    renderWorkspace()
    expect(await screen.findByText("收支趋势")).toBeInTheDocument()
    await new Promise((resolve) => window.setTimeout(resolve, 600))
    expect(api.replaceDashboardWidgets).not.toHaveBeenCalled()
  })

  it("does not save the six-column projection when mounted at 900px", async () => {
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 900 })
    renderWorkspace()
    expect(await screen.findByText("请在桌面宽度下编辑布局")).toBeInTheDocument()
    await new Promise((resolve) => window.setTimeout(resolve, 600))
    expect(api.replaceDashboardWidgets).not.toHaveBeenCalled()
  })

  it("flushes a pending widget save immediately on unmount", async () => {
    const user = userEvent.setup()
    const view = renderWorkspace()
    expect(await screen.findByText("收支趋势")).toBeInTheDocument()
    await user.click(screen.getByRole("button", { name: "编辑" }))
    await user.click(screen.getByRole("button", { name: "添加组件" }))
    const sheet = await screen.findByRole("dialog", { name: "添加组件" })
    vi.useFakeTimers()
    try {
      fireEvent.click(within(sheet).getByRole("button", { name: /分类占比/ }))
      expect(api.replaceDashboardWidgets).not.toHaveBeenCalled()

      view.unmount()

      expect(api.replaceDashboardWidgets).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it("keeps a closed plugin widget as a placeholder", async () => {
    vi.mocked(api.dashboards).mockResolvedValue([{ ...baseDashboard, widgets: [{ id: "debt-widget", widgetType: "plugin:debts:overview", pluginId: "debts", x: 0, y: 0, w: 4, h: 3, config: {} }] }])
    vi.mocked(api.dashboardWidgetTypes).mockResolvedValue({ ...definitions, plugins: [{ ...definitions.plugins[0], enabled: false }] })
    renderWorkspace(disabledPlugins)
    expect(await screen.findByText("〈债务〉已关闭")).toBeInTheDocument()
    expect(screen.getByRole("link", { name: "去开启" })).toHaveAttribute("href", "/app/settings/plugins")
  })

  it("offers both first-use actions", async () => {
    const user = userEvent.setup()
    vi.mocked(api.dashboards).mockResolvedValue([])
    vi.mocked(api.createDefaultDashboard).mockResolvedValue(baseDashboard)
    renderWorkspace()
    expect(await screen.findByText("还没有统计面板")).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "从空白开始" })).toBeInTheDocument()
    await user.click(screen.getByRole("button", { name: "使用默认布局" }))
    await waitFor(() => expect(api.createDefaultDashboard).toHaveBeenCalledWith(expect.objectContaining({ idempotencyKey: expect.any(String) })))
  })

  it("creates an empty first page from the secondary first-use action", async () => {
    const user = userEvent.setup()
    vi.mocked(api.dashboards).mockResolvedValue([])
    vi.mocked(api.createDashboard).mockResolvedValue({ ...baseDashboard, name: "面板 1", widgets: [] })
    renderWorkspace()
    await user.click(await screen.findByRole("button", { name: "从空白开始" }))
    await waitFor(() => expect(api.createDashboard).toHaveBeenCalledWith({ name: "面板 1" }, expect.objectContaining({ idempotencyKey: expect.any(String) })))
    expect(await screen.findByText("当前面板还没有组件")).toBeInTheDocument()
  })

  it("rolls an optimistic widget change back when the debounced save fails", async () => {
    const user = userEvent.setup()
    vi.mocked(api.replaceDashboardWidgets).mockRejectedValue(new Error("网络不可用"))
    renderWorkspace()
    await screen.findByText("收支趋势")
    await user.click(screen.getByRole("button", { name: "编辑" }))
    await user.click(screen.getByRole("button", { name: "添加组件" }))
    const sheet = await screen.findByRole("dialog", { name: "添加组件" })
    await user.click(within(sheet).getByRole("button", { name: /分类占比/ }))
    await user.click(within(sheet).getByRole("button", { name: "关闭" }))
    expect(await screen.findByRole("heading", { name: "分类占比" })).toBeInTheDocument()
    expect(await screen.findByText("布局未保存，重试", {}, { timeout: 1500 })).toBeInTheDocument()
    await waitFor(() => expect(screen.queryByRole("heading", { name: "分类占比" })).not.toBeInTheDocument())
  })

  it("creates, renames and deletes pages while disabling deletion of the last page", async () => {
    const user = userEvent.setup()
    const second = { ...baseDashboard, id: "dashboard-2", name: "面板 2", position: 1, widgets: [] }
    vi.mocked(api.createDashboard).mockResolvedValue(second)
    vi.mocked(api.updateDashboard).mockResolvedValue({ ...second, name: "年度" })
    vi.mocked(api.deleteDashboard).mockResolvedValue(undefined)
    renderWorkspace()
    await screen.findByText("收支趋势")
    await user.click(screen.getByRole("button", { name: "编辑" }))
    await user.click(screen.getByRole("button", { name: "操作面板 月度" }))
    expect(screen.getByRole("menuitem", { name: "删除面板" })).toHaveAttribute("data-disabled")
    await user.keyboard("{Escape}")

    await user.click(screen.getByRole("button", { name: "新建面板" }))
    const createDialog = await screen.findByRole("dialog", { name: "新建面板" })
    await user.click(within(createDialog).getByRole("button", { name: "创建" }))
    expect(await screen.findByRole("tab", { name: "面板 2" })).toBeInTheDocument()
    await user.click(screen.getByRole("button", { name: "操作面板 面板 2" }))
    await user.click(screen.getByRole("menuitem", { name: "重命名" }))
    const renameDialog = await screen.findByRole("dialog", { name: "重命名面板" })
    const input = within(renameDialog).getByLabelText("名称")
    await user.clear(input)
    await user.type(input, "年度")
    await user.click(within(renameDialog).getByRole("button", { name: "确认" }))
    expect(await screen.findByRole("tab", { name: "年度" })).toBeInTheDocument()

    await user.click(screen.getByRole("button", { name: "操作面板 年度" }))
    await user.click(screen.getByRole("menuitem", { name: "删除面板" }))
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "删除面板" }))
    await waitFor(() => expect(api.deleteDashboard).toHaveBeenCalled())
  })

  it("forces read-only mode below the desktop breakpoint", async () => {
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 900 })
    renderWorkspace()
    expect(await screen.findByText("请在桌面宽度下编辑布局")).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "编辑" })).toBeDisabled()
    expect(screen.queryByRole("button", { name: /操作组件/ })).not.toBeInTheDocument()
  })
})
