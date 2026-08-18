import { QueryClient, QueryClientProvider, useQuery } from "@tanstack/react-query"
import { render, screen, waitFor, within } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { MemoryRouter } from "react-router-dom"
import type { ReactNode } from "react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { api } from "../api/client"
import type { Plugin } from "../api/types"
import { AppToastProvider } from "../components/ui"
import { PluginStateProvider } from "../plugins/state"
import { PluginRoute, PluginSettingsPage } from "./plugin-settings"

vi.mock("../api/client", () => ({
  api: { plugins: vi.fn(), updatePlugin: vi.fn() },
}))

const disabledDebt: Plugin = {
  id: "debts",
  name: "债务",
  description: "记录借入、借出及还款进度。",
  enabled: false,
  ownsTransactions: true,
  routePrefixes: ["/api/v1/debts"],
}
const enabledImport: Plugin = {
  id: "bill-imports",
  name: "账单导入",
  description: "从受支持的账单来源导入流水。",
  enabled: true,
  ownsTransactions: false,
  routePrefixes: ["/api/v1/imports"],
}
const enabledAuto: Plugin = {
  id: "auto-categorize",
  name: "自动分类",
  description: "按规则为流水自动匹配分类。",
  enabled: true,
  ownsTransactions: false,
  routePrefixes: ["/api/v1/category-rules"],
}

function PluginHarness({ children, initial }: { children: ReactNode; initial: Plugin[] }) {
  const query = useQuery({ queryKey: ["plugins"], queryFn: api.plugins, initialData: initial, staleTime: Infinity })
  return <PluginStateProvider value={{ plugins: query.data, isLoading: query.isLoading }}>{children}</PluginStateProvider>
}

function renderWithPlugins(children: ReactNode, initial: Plugin[]) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
  return render(
    <MemoryRouter>
      <QueryClientProvider client={client}>
        <AppToastProvider>
          <PluginHarness initial={initial}>{children}</PluginHarness>
        </AppToastProvider>
      </QueryClientProvider>
    </MemoryRouter>,
  )
}

beforeEach(() => {
  vi.clearAllMocks()
})

describe("PluginRoute", () => {
  it("shows a disabled placeholder and reopens after self-check", async () => {
    const enabledDebt = { ...disabledDebt, enabled: true }
    vi.mocked(api.updatePlugin).mockResolvedValue({ ...enabledDebt, reconciled: 2 })
    vi.mocked(api.plugins).mockResolvedValue([enabledDebt, enabledImport, enabledAuto])
    const user = userEvent.setup()
    renderWithPlugins(
      <PluginRoute pluginId="debts"><div>债务正文</div></PluginRoute>,
      [disabledDebt, enabledImport, enabledAuto],
    )

    expect(screen.getByRole("heading", { name: "债务插件已关闭" })).toBeInTheDocument()
    expect(screen.queryByText("债务正文")).not.toBeInTheDocument()
    await user.click(screen.getByRole("button", { name: "开启插件" }))
    await waitFor(() => expect(api.updatePlugin).toHaveBeenCalledWith("debts", true))
    expect(await screen.findByText("债务正文")).toBeInTheDocument()
    expect(await screen.findByText("自检修复了 2 项。")).toBeInTheDocument()
  })
})

describe("PluginSettingsPage", () => {
  it("confirms before closing and preserves the ownership warning", async () => {
    const plugins = [{ ...disabledDebt, enabled: true }, enabledImport, enabledAuto]
    const closedImport = { ...enabledImport, enabled: false }
    vi.mocked(api.updatePlugin).mockResolvedValue({ ...closedImport, reconciled: 0 })
    vi.mocked(api.plugins).mockResolvedValue([plugins[0], closedImport, enabledAuto])
    const user = userEvent.setup()
    renderWithPlugins(<PluginSettingsPage />, plugins)

    expect(screen.getByText("它创建的流水只能在插件里删除")).toBeInTheDocument()
    await user.click(screen.getByRole("switch", { name: "启用账单导入" }))
    const dialog = await screen.findByRole("alertdialog")
    expect(dialog).toHaveTextContent("关闭后页面隐藏、数据保留，重新开启会先自检")
    expect(api.updatePlugin).not.toHaveBeenCalled()
    await user.click(within(dialog).getByRole("button", { name: "确认关闭" }))
    await waitFor(() => expect(api.updatePlugin).toHaveBeenCalledWith("bill-imports", false))
    await waitFor(() => expect(screen.getByRole("switch", { name: "启用账单导入" })).not.toBeChecked())
  })
})
