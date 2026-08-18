import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { act, render, screen, waitFor, within } from "@testing-library/react"
import { useState, type ComponentType } from "react"
import userEvent from "@testing-library/user-event"
import { MemoryRouter } from "react-router-dom"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { ApiClientError, api } from "../api/client"
import type { Category, Dashboard, DuplicateSuspicion, LedgerTransaction, Plugin } from "../api/types"
import { AppToastProvider } from "../components/ui"
import { TopbarSlotContext, type TopbarSlots } from "../components/topbar-slots"
import { PluginStateProvider } from "../plugins/state"
import { CalendarWorkspace, TransactionListWorkspace, TransactionStatisticsWorkspace } from "./transaction-workspace"
import { debtDraftCounterparty, transactionDebtDirection } from "./transaction-debt"

vi.mock("../api/client", () => ({
  ApiClientError: class ApiClientError extends Error {
    status: number
    code: string
    constructor(status: number, body: { code?: string; message?: string }) {
      super(body.message || "请求失败")
      this.status = status
      this.code = body.code || "request_failed"
    }
  },
  api: {
    transactions: vi.fn(),
    transactionSummary: vi.fn(),
    transactionCategories: vi.fn(),
    categories: vi.fn(),
    createCategory: vi.fn(),
    createCategoryRule: vi.fn(),
    recategorize: vi.fn(),
    ledgerAccounts: vi.fn(),
    createTransaction: vi.fn(),
    updateTransaction: vi.fn(),
    deleteTransaction: vi.fn(),
    restoreTransaction: vi.fn(),
    counterparties: vi.fn(),
    createDebt: vi.fn(),
    duplicateSuspicions: vi.fn(),
    updateDuplicateSuspicion: vi.fn(),
    dashboards: vi.fn(),
    createDashboard: vi.fn(),
    createDefaultDashboard: vi.fn(),
    updateDashboard: vi.fn(),
    deleteDashboard: vi.fn(),
    replaceDashboardWidgets: vi.fn(),
    dashboardWidgetTypes: vi.fn(),
    statisticsAggregate: vi.fn(),
    summary: vi.fn(),
    debts: vi.fn(),
  },
}))

const now = new Date()
const monthKey = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}`
const today = `${monthKey}-${String(now.getDate()).padStart(2, "0")}`
const nextMonth = new Date(now.getFullYear(), now.getMonth() + 1, 1)
const nextMonthKey = `${nextMonth.getFullYear()}-${String(nextMonth.getMonth() + 1).padStart(2, "0")}`
const day1 = `${monthKey}-01`
const day2 = `${monthKey}-02`
const otherDay = now.getDate() === 10 ? 11 : 10
const otherDate = `${monthKey}-${String(otherDay).padStart(2, "0")}`

const accountBrief = { id: "account-1", accountType: "wechat_balance" as const, name: "微信支付-测试号", archived: false }
const ledgerAccount = {
  id: "account-1",
  accountType: "wechat_balance" as const,
  name: "微信支付-测试号",
  nameSource: "custom" as const,
  note: "",
  archived: false,
  version: 1,
  usageCount: 2,
  openingBalanceCents: 0,
  balanceCents: 4800,
  createdAt: "2026-08-02T00:00:00Z",
  updatedAt: "2026-08-02T00:00:00Z",
}

const expenseCategory: Category = {
  id: "category-food",
  parentId: null,
  name: "餐饮",
  kind: "expense",
  sortOrder: 0,
  archived: false,
  version: 1,
  children: [],
}

const incomeCategory: Category = {
  ...expenseCategory,
  id: "category-salary",
  name: "工资",
  kind: "income",
}

function makeTransaction(overrides: Partial<LedgerTransaction> = {}): LedgerTransaction {
  return {
    id: "tx-default",
    kind: "expense",
    amountCents: 0,
    currency: "CNY",
    occurredOn: day2,
    occurredAt: null,
    occurredAtPrecision: "second",
    category: "",
    payeeName: "",
    payeeKey: "",
    description: "",
    account: null,
    note: "",
    archived: false,
    version: 1,
    createdAt: "2026-08-02T00:00:00Z",
    updatedAt: "2026-08-02T00:00:00Z",
    pnlScope: "counted",
    createdBy: "user",
    links: [],
    ...overrides,
  }
}

function makeDuplicateSuspicion(overrides: Partial<DuplicateSuspicion> = {}): DuplicateSuspicion {
  return {
    id: "duplicate-default",
    clusterKey: "cluster-default",
    score: 0.9,
    matchRule: "same_amount",
    reason: "同日同金额",
    status: "open",
    createdAt: "2026-08-12T00:00:00Z",
    updatedAt: "2026-08-12T00:00:00Z",
    transactionA: { id: "tx-a", kind: "expense", amountCents: 1234, currency: "CNY", occurredOn: day2, occurredAt: null, occurredAtPrecision: "day", sourceChannel: "alipay", accountId: null },
    transactionB: { id: "tx-b", kind: "expense", amountCents: 1234, currency: "CNY", occurredOn: day2, occurredAt: null, occurredAtPrecision: "day", sourceChannel: "wechat", accountId: null },
    ...overrides,
  }
}

const incomeItem = makeTransaction({
  id: "tx-1",
  kind: "income",
  amountCents: 500000,
  occurredOn: day1,
  category: "工资",
  note: "八月工资",
})
const expenseItem = makeTransaction({
  id: "tx-2",
  kind: "expense",
  amountCents: 1234,
  occurredOn: day2,
  category: "餐饮",
  account: accountBrief,
  note: "午饭",
  version: 3,
})

const summary = {
  month: monthKey,
  days: [
    { date: day1, incomeCents: 500000, expenseCents: 0 },
    { date: day2, incomeCents: 0, expenseCents: 1234 },
  ],
  byCategory: [
    { category: "餐饮", incomeCents: 0, expenseCents: 150000, count: 2 },
    { category: "", incomeCents: 0, expenseCents: 40000, count: 1 },
    { category: "工资", incomeCents: 500000, expenseCents: 0, count: 1 },
  ],
  incomeCents: 500000,
  expenseCents: 190000,
  netCents: 310000,
  transactionCount: 3,
}

const statisticsDashboard: Dashboard = {
  id: "dashboard-monthly",
  name: "月度",
  position: 0,
  widgets: [
    { id: "trend", widgetType: "core:income-expense-trend", pluginId: null, x: 0, y: 0, w: 8, h: 4, config: {} },
    { id: "category", widgetType: "core:category-share", pluginId: null, x: 8, y: 0, w: 4, h: 4, config: { kind: "expense" } },
    { id: "balances", widgetType: "core:account-balances", pluginId: null, x: 0, y: 4, w: 4, h: 3, config: { unarchivedOnly: true } },
    { id: "compare", widgetType: "core:month-compare", pluginId: null, x: 4, y: 4, w: 8, h: 3, config: {} },
  ],
}

const statisticsWidgetTypes = {
  core: [
    { id: "income-expense-trend", name: "收支趋势", description: "按日展示月度收入与支出趋势。", defaultW: 8, defaultH: 4, minW: 4, minH: 3 },
    { id: "category-share", name: "分类占比", description: "按分类展示收入与支出占比。", defaultW: 4, defaultH: 4, minW: 3, minH: 3 },
    { id: "account-balances", name: "账户余额", description: "展示当前账户余额。", defaultW: 4, defaultH: 3, minW: 3, minH: 2 },
    { id: "month-compare", name: "月度对比", description: "对比近六个月收入与支出。", defaultW: 8, defaultH: 3, minW: 4, minH: 2 },
  ],
  plugins: [],
}

/** jsdom 不排版，日历量不到格子高度。装一个可手动喂高度的 ResizeObserver，
 * 用它驱动「按格子高度算行数和行高」这段逻辑。 */
function stubResizeObserver() {
  const callbacks: ResizeObserverCallback[] = []
  const targets: Element[] = []
  class StubResizeObserver {
    constructor(private readonly callback: ResizeObserverCallback) { callbacks.push(callback) }
    observe(target: Element) { targets.push(target) }
    unobserve() {}
    disconnect() {}
  }
  vi.stubGlobal("ResizeObserver", StubResizeObserver)
  return {
    resize(height: number) {
      act(() => {
        for (const [index, callback] of callbacks.entries()) {
          callback([{ target: targets[index], contentRect: { height } } as ResizeObserverEntry], {} as ResizeObserver)
        }
      })
    },
  }
}

function TopbarHarness({ Workspace }: { Workspace: ComponentType }) {
  const [slots, setSlots] = useState<TopbarSlots>()
  return <TopbarSlotContext.Provider value={setSlots}><div data-testid="topbar-title">{slots?.title}</div><div data-testid="topbar-actions">{slots?.actions}</div><Workspace /></TopbarSlotContext.Provider>
}

function renderWorkspace(Workspace: ComponentType = CalendarWorkspace, plugins?: Plugin[]) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
  const invalidateSpy = vi.spyOn(client, "invalidateQueries")
  render(<MemoryRouter><QueryClientProvider client={client}><AppToastProvider><PluginStateProvider value={{ plugins, isLoading: false }}><TopbarHarness Workspace={Workspace} /></PluginStateProvider></AppToastProvider></QueryClientProvider></MemoryRouter>)
  return { invalidateSpy }
}

describe("TransactionWorkspace", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.unstubAllGlobals()
    vi.mocked(api.transactionSummary).mockImplementation((month: string) => Promise.resolve({ ...summary, month }))
    vi.mocked(api.transactions).mockImplementation((params: Record<string, string | number | undefined>) => Promise.resolve(
      params.pageSize === 200
        ? { items: [incomeItem, expenseItem], page: 1, pageSize: 200, total: 2 }
        : { items: [incomeItem, expenseItem], page: Number(params.page || 1), pageSize: 20, total: 22 },
    ))
    vi.mocked(api.transactionCategories).mockResolvedValue(["交通", "工资", "餐饮"])
    vi.mocked(api.categories).mockResolvedValue([expenseCategory, incomeCategory])
    vi.mocked(api.ledgerAccounts).mockResolvedValue([ledgerAccount])
    vi.mocked(api.counterparties).mockResolvedValue([])
    vi.mocked(api.duplicateSuspicions).mockResolvedValue({ clusters: [], items: [], page: 1, pageSize: 200, total: 0 })
    vi.mocked(api.dashboards).mockResolvedValue([statisticsDashboard])
    vi.mocked(api.dashboardWidgetTypes).mockResolvedValue(statisticsWidgetTypes)
    vi.mocked(api.statisticsAggregate).mockImplementation((params) => {
      if (params.groupBy === "day") return Promise.resolve(summary.days.map((day) => ({ key: day.date, label: day.date, incomeCents: day.incomeCents, expenseCents: day.expenseCents, count: 1 })))
      if (params.groupBy === "category") return Promise.resolve(summary.byCategory.map((item) => ({ key: item.category, label: item.category, incomeCents: item.incomeCents, expenseCents: item.expenseCents, count: item.count })))
      if (params.groupBy === "month") return Promise.resolve([{ key: monthKey, label: monthKey, incomeCents: summary.incomeCents, expenseCents: summary.expenseCents, count: summary.transactionCount }])
      return Promise.resolve([])
    })
  })

  it("shows the secondary import route entry beside bookkeeping", async () => {
    renderWorkspace(TransactionListWorkspace)
    expect(await screen.findByTestId("topbar-actions")).toHaveTextContent("导入账单")
    expect(screen.getByTestId("topbar-actions")).toHaveTextContent("记一笔")
  })

  it("keeps calendar content to the 42-cell Monday-first calendar", async () => {
    renderWorkspace()
    const calendar = await screen.findByLabelText("记账日历")
    expect(screen.queryByText("本月收入")).not.toBeInTheDocument()
    expect(screen.queryByLabelText("每日趋势")).not.toBeInTheDocument()
    expect(screen.queryByLabelText("分类占比")).not.toBeInTheDocument()
    expect(screen.queryByRole("tab")).not.toBeInTheDocument()
    expect(screen.getByTestId("topbar-actions")).toHaveTextContent("今天")
    expect(screen.getByRole("button", { name: "上一月" })).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "下一月" })).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "记一笔" })).toBeInTheDocument()
    const weekdays = calendar.querySelectorAll(".tx-calendar-weekdays span")
    expect(weekdays[0]).toHaveTextContent("一")
    expect(weekdays[6]).toHaveTextContent("日")
    expect(calendar.querySelectorAll(".tx-day")).toHaveLength(42)
    const todayButton = calendar.querySelector<HTMLButtonElement>('[aria-current="date"]')
    expect(todayButton).toBeInTheDocument()
    expect(todayButton).toHaveAttribute("aria-current", "date")
    const todayMarker = todayButton!.querySelector(".tx-day-today")
    expect(todayMarker).toHaveTextContent(String(now.getDate()))
    expect(todayMarker).toHaveAttribute("data-center-content")
    expect(todayMarker?.firstElementChild).toHaveAttribute("data-center-ink")
    expect(calendar.querySelector(".tx-amount-net-positive, .tx-amount-net-negative")).not.toBeInTheDocument()

  })

  it("lists each transaction under the day header with income and expense told apart", async () => {
    renderWorkspace()
    const incomeCell = await screen.findByRole("button", { name: new RegExp(`^${day1}，.*共 1 笔$`) })
    const head = incomeCell.querySelector(".tx-day-head")
    expect(head).toHaveTextContent("+¥5.0k")
    expect(head?.querySelector(".tx-day-number")).toHaveTextContent("1")
    const incomeEntry = incomeCell.querySelector(".tx-day-entry")
    expect(incomeEntry).toHaveClass("tx-day-entry-income")
    expect(incomeEntry).toHaveTextContent("工资")
    expect(incomeEntry).toHaveTextContent("+¥5.0k")

    const expenseEntry = screen.getByRole("button", { name: new RegExp(`^${day2}，`) }).querySelector(".tx-day-entry")
    expect(expenseEntry).toHaveClass("tx-day-entry-expense")
    expect(expenseEntry).toHaveTextContent("餐饮")
    expect(expenseEntry).toHaveTextContent("-¥12")
  })

  it("uses payeeName as the primary label and keeps structured details secondary", async () => {
    const imported = { ...expenseItem, id: "tx-imported", category: "商户消费", payeeName: "见福便利超市", description: "二维码收款", note: "收款方备注", occurredOn: day2 }
    vi.mocked(api.transactions).mockResolvedValue({ items: [imported], page: 1, pageSize: 200, total: 1 })
    const user = userEvent.setup()
    renderWorkspace()
    const cell = await screen.findByRole("button", { name: new RegExp(`^${day2}，.*共 1 笔$`) })
    const entry = cell.querySelector(".tx-day-entry")
    expect(entry).toHaveTextContent("见福便利超市")
    expect(entry).not.toHaveTextContent("商户消费")

    if (day2 !== today) await user.click(cell)
    const row = (await screen.findByLabelText("当日明细")).querySelector(".tx-row")!
    expect(row.querySelector("strong")).toHaveTextContent("见福便利超市")
    expect(row.querySelector(".tx-row-copy > span:last-child")).toHaveTextContent("二维码收款 · 收款方备注 · 微信零钱 · 微信支付-测试号 · 商户消费")
  })

  it("fills the cell with the largest amounts, not the earliest rows", async () => {
    const noise = Array.from({ length: 5 }, (_, index) => ({ ...expenseItem, id: `tx-fee-${index}`, payeeName: `分账手续费${index}`, amountCents: 8, occurredOn: day2 }))
    const big = { ...expenseItem, id: "tx-big", payeeName: "京东", description: "订单编号1", amountCents: 28371, occurredOn: day2 }
    vi.mocked(api.transactions).mockResolvedValue({ items: [...noise, big], page: 1, pageSize: 200, total: 6 })
    renderWorkspace()
    const cell = await screen.findByRole("button", { name: new RegExp(`^${day2}，.*共 6 笔$`) })
    expect(cell.querySelectorAll(".tx-day-entry")[0]).toHaveTextContent("京东")
    expect(cell).toHaveTextContent("还有 3 笔")
  })

  it("caps the cell list and counts the rest", async () => {
    const many = Array.from({ length: 6 }, (_, index) => ({ ...expenseItem, id: `tx-many-${index}`, occurredOn: day2 }))
    vi.mocked(api.transactions).mockResolvedValue({ items: many, page: 1, pageSize: 200, total: many.length })
    renderWorkspace()
    const cell = await screen.findByRole("button", { name: new RegExp(`^${day2}，.*共 6 笔$`) })
    expect(cell.querySelectorAll(".tx-day-entry")).toHaveLength(3)
    expect(cell).toHaveTextContent("还有 3 笔")
  })

  it("refits row count and row height to the measured cell height", async () => {
    const many = Array.from({ length: 6 }, (_, index) => ({ ...expenseItem, id: `tx-many-${index}`, occurredOn: day2 }))
    vi.mocked(api.transactions).mockResolvedValue({ items: many, page: 1, pageSize: 200, total: many.length })
    const observers = stubResizeObserver()
    renderWorkspace()
    const cell = await screen.findByRole("button", { name: new RegExp(`^${day2}，.*共 6 笔$`) })
    const grid = screen.getByLabelText("记账日历").querySelector<HTMLElement>(".tx-calendar-grid")!

    // 100px 放得下 5 行：4 笔加一行「还有 2 笔」，行高摊满可用高度。
    observers.resize(100)
    expect(cell.querySelectorAll(".tx-day-entry")).toHaveLength(4)
    expect(cell).toHaveTextContent("还有 2 笔")
    expect(grid.style.getPropertyValue("--tx-entry-height")).toBe("18.4px")

    // 矮格子少列几笔，行高跟着变，「还有 N 笔」始终占得下一整行。
    observers.resize(40)
    expect(cell.querySelectorAll(".tx-day-entry")).toHaveLength(1)
    expect(cell).toHaveTextContent("还有 5 笔")
    expect(grid.style.getPropertyValue("--tx-entry-height")).toBe("19px")

    // 一行都放不下时清单整体让位，只留日期与当日汇总。
    observers.resize(10)
    expect(cell.querySelectorAll(".tx-day-entry")).toHaveLength(0)
    expect(cell.querySelector(".tx-day-more")).not.toBeInTheDocument()
  })

  it("pages through the whole month instead of stopping at the server page cap", async () => {
    // 没有备注的手工账退回分类，正好当第二页的标记。
    const later = { ...expenseItem, id: "tx-page-2", category: "打车", note: "", amountCents: 2000, occurredOn: day2 }
    vi.mocked(api.transactions).mockImplementation((params: Record<string, string | number | undefined>) => Promise.resolve(
      params.pageSize === 200
        ? { items: Number(params.page) === 1 ? [incomeItem] : [later], page: Number(params.page), pageSize: 200, total: 201 }
        : { items: [], page: 1, pageSize: 20, total: 0 },
    ))
    renderWorkspace()
    await screen.findByLabelText("记账日历")
    await waitFor(() => expect(screen.getByRole("button", { name: new RegExp(`^${day2}，`) })).toHaveTextContent("打车"))
    expect(api.transactions).toHaveBeenCalledWith(expect.objectContaining({ month: monthKey, page: 2, pageSize: 200 }))
  })

  it("opens the day sheet on one click and creates from an empty day", async () => {
    const user = userEvent.setup()
    renderWorkspace()
    const cell = await screen.findByRole("button", { name: new RegExp(`^${otherDate}，`) })
    await user.click(cell)
    expect(await screen.findByRole("dialog")).toBeInTheDocument()
    expect(screen.getByText("当日暂无记录")).toBeInTheDocument()
    await user.click(screen.getByRole("button", { name: "记一笔" }))
    const dialog = await screen.findByRole("dialog", { name: "记一笔" })
    expect(screen.queryByText("当日暂无记录")).not.toBeInTheDocument()
    expect(within(dialog).getByText(otherDate)).toBeInTheDocument()
  })

  it("creates a transaction with yuan-to-cents conversion and invalidates account balances", async () => {
    const user = userEvent.setup()
    const { invalidateSpy } = renderWorkspace()
    vi.mocked(api.createTransaction).mockResolvedValue(incomeItem)
    await user.click(await screen.findByRole("button", { name: "记一笔" }))
    const dialog = await screen.findByRole("dialog", { name: "记一笔" })
    await user.type(within(dialog).getByLabelText("金额（元）"), "12.34")
    await user.click(within(dialog).getByLabelText("分类"))
    await user.type(within(dialog).getByLabelText("分类"), "零食")
    await user.click(await within(dialog).findByRole("option", { name: /新建"零食"/ }))
    await user.click(within(dialog).getByRole("button", { name: "保存" }))

    await waitFor(() => expect(api.createTransaction).toHaveBeenCalledWith({
      kind: "expense",
      amountCents: 1234,
      occurredOn: today,
      category: "零食",
      accountId: null,
      transferFromAccountId: null,
      transferToAccountId: null,
      note: "",
    }, expect.objectContaining({ idempotencyKey: expect.any(String) })))
    await waitFor(() => expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: ["ledger-accounts"] }))
    expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: ["transaction-summary"] })
    expect(await screen.findByText("已记一笔")).toBeInTheDocument()
  })

  it("edits a day-detail entry with its optimistic-lock version", async () => {
    const user = userEvent.setup()
    renderWorkspace()
    if (day2 !== today) await user.click(await screen.findByRole("button", { name: new RegExp(`^${day2}，`) }))
    await user.click(await screen.findByRole("button", { name: "操作 餐饮 ¥12.34" }))
    await user.click(screen.getByRole("menuitem", { name: "编辑" }))
    const dialog = await screen.findByRole("dialog", { name: "编辑记账" })
    const amount = within(dialog).getByLabelText("金额（元）")
    expect(amount).toHaveValue("12.34")
    vi.mocked(api.updateTransaction).mockResolvedValue(expenseItem)
    await user.clear(amount)
    await user.type(amount, "20")
    await user.click(within(dialog).getByRole("button", { name: "保存" }))

    await waitFor(() => expect(api.updateTransaction).toHaveBeenCalledWith("tx-2", {
      version: 3,
      kind: "expense",
      amountCents: 2000,
      occurredOn: day2,
      category: "餐饮",
      accountId: "account-1",
      transferFromAccountId: null,
      transferToAccountId: null,
      note: "午饭",
    }, expect.objectContaining({ idempotencyKey: expect.any(String) })))
    expect(await screen.findByText("记账已更新")).toBeInTheDocument()
  })

  it("deletes an entry through the confirm dialog", async () => {
    const user = userEvent.setup()
    renderWorkspace()
    if (day2 !== today) await user.click(await screen.findByRole("button", { name: new RegExp(`^${day2}，`) }))
    await user.click(await screen.findByRole("button", { name: "操作 餐饮 ¥12.34" }))
    await user.click(screen.getByRole("menuitem", { name: "删除" }))
    const dialog = await screen.findByRole("alertdialog")
    expect(within(dialog).getByText(/删除后不再计入统计与账户余额/)).toBeInTheDocument()
    vi.mocked(api.deleteTransaction).mockResolvedValue({ ...expenseItem, archived: true, version: 4 })
    await user.click(within(dialog).getByRole("button", { name: "确认删除" }))
    await waitFor(() => expect(api.deleteTransaction).toHaveBeenCalledWith("tx-2", 3, expect.objectContaining({ idempotencyKey: expect.any(String) })))
    expect(await screen.findByText("记账已删除")).toBeInTheDocument()
  })

  it("undoes a delete from the toast action", async () => {
    const user = userEvent.setup()
    renderWorkspace()
    if (day2 !== today) await user.click(await screen.findByRole("button", { name: new RegExp(`^${day2}，`) }))
    await user.click(await screen.findByRole("button", { name: "操作 餐饮 ¥12.34" }))
    await user.click(screen.getByRole("menuitem", { name: "删除" }))
    vi.mocked(api.deleteTransaction).mockResolvedValue({ ...expenseItem, archived: true, version: 4 })
    vi.mocked(api.restoreTransaction).mockResolvedValue(expenseItem)
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "确认删除" }))
    await user.click(await screen.findByRole("button", { name: "撤销" }))
    // Deleting bumps the version from 3 to 4, so the restore must target 4.
    await waitFor(() => expect(api.restoreTransaction).toHaveBeenCalledWith("tx-2", 4, expect.objectContaining({ idempotencyKey: expect.any(String) })))
    expect(await screen.findByText("已撤销删除")).toBeInTheDocument()
  })

  it("hides delete for plugin-owned transactions and explains where to delete", async () => {
    const linked = makeTransaction({
      ...expenseItem,
      createdBy: "plugin:debts",
      links: [{ pluginId: "debts", kind: "repayment", refId: "debt-1", label: "测试往来方" }],
    })
    vi.mocked(api.transactions).mockResolvedValue({ items: [linked], page: 1, pageSize: 200, total: 1 })
    const user = userEvent.setup()
    renderWorkspace(TransactionListWorkspace)
    const categoryCells = await screen.findAllByText("餐饮")
    const row = categoryCells.map((cell) => cell.closest("tr")).find(Boolean)
    expect(row).toBeTruthy()
    await user.click(within(row as HTMLTableRowElement).getByRole("button", { name: "操作 餐饮 ¥12.34" }))
    expect(screen.queryByRole("menuitem", { name: "删除" })).not.toBeInTheDocument()
    await user.keyboard("{Escape}")
    await user.click(row as HTMLTableRowElement)
    const dialog = await screen.findByRole("dialog", { name: "流水详情" })
    expect(within(dialog).queryByRole("button", { name: "删除" })).not.toBeInTheDocument()
    expect(within(dialog).getByRole("status")).toHaveTextContent("这笔由债务创建，请在债务里删除对应记录")
  })

  it("surfaces a version conflict inside the form modal", async () => {
    const user = userEvent.setup()
    renderWorkspace()
    vi.mocked(api.createTransaction).mockRejectedValue(new ApiClientError(409, { code: "version_conflict", message: "记录已在其他设备更新，请刷新后重试" }))
    await user.click(await screen.findByRole("button", { name: "记一笔" }))
    const dialog = await screen.findByRole("dialog", { name: "记一笔" })
    await user.type(within(dialog).getByLabelText("金额（元）"), "5")
    await user.click(within(dialog).getByRole("button", { name: "保存" }))
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("记录已在其他设备更新，请刷新后重试")
  })

  it("refetches the summary when navigating months and returns with 今天", async () => {
    const user = userEvent.setup()
    renderWorkspace()
    await screen.findByLabelText("记账日历")
    await user.click(screen.getByRole("button", { name: "下一月" }))
    await waitFor(() => expect(api.transactionSummary).toHaveBeenCalledWith(nextMonthKey))
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
    await user.click(screen.getByRole("button", { name: "今天" }))
    await waitFor(() => expect(api.transactionSummary).toHaveBeenCalledWith(monthKey))
  })

  it("keeps the dashboard structure when the selected month has no data", async () => {
    vi.mocked(api.transactionSummary).mockResolvedValue({ ...summary, days: [], byCategory: [], incomeCents: 0, expenseCents: 0, netCents: 0, transactionCount: 0 })
    vi.mocked(api.statisticsAggregate).mockResolvedValue([])
    renderWorkspace(TransactionStatisticsWorkspace)
    const strip = await screen.findByLabelText("本月收支汇总")
    await waitFor(() => expect(strip).toHaveTextContent("本月笔数0 笔"))
    expect((await screen.findAllByText("本月暂无数据")).length).toBeGreaterThanOrEqual(3)
  })

  it("places dashboard tabs and month navigation in the topbar with a four-value summary", async () => {
    const user = userEvent.setup()
    renderWorkspace(TransactionStatisticsWorkspace)
    await screen.findByLabelText("本月收支汇总")
    expect(screen.getByTestId("topbar-title")).toHaveTextContent("统计")
    expect(screen.getByTestId("topbar-actions")).toHaveTextContent("月度")
    expect(screen.getByTestId("topbar-actions")).toHaveTextContent("编辑")
    expect(document.querySelectorAll(".summary-strip-item")).toHaveLength(4)
    expect(await screen.findByText("3 笔")).toBeInTheDocument()
    await user.click(screen.getByRole("button", { name: "下一月" }))
    await waitFor(() => expect(api.transactionSummary).toHaveBeenCalledWith(nextMonthKey))
    await waitFor(() => expect(api.statisticsAggregate).toHaveBeenCalledWith(expect.objectContaining({ from: `${nextMonthKey}-01` })))
  })

  it("renders the independent list route and paginates through the server", async () => {
    const user = userEvent.setup()
    renderWorkspace(TransactionListWorkspace)
    expect(screen.getByTestId("topbar-title")).toHaveTextContent("流水")
    expect(screen.getByRole("button", { name: "记一笔" })).toBeInTheDocument()
    expect(await screen.findByText("共 22 笔")).toBeInTheDocument()
    expect(api.transactions).toHaveBeenCalledWith({ month: monthKey, kind: "", category: "", accountId: "", page: 1, pageSize: 20 })
    expect(screen.getAllByText("+¥5,000.00").length).toBeGreaterThan(0)
    await user.click(screen.getByRole("button", { name: "第 2 页" }))
    await waitFor(() => expect(api.transactions).toHaveBeenCalledWith({ month: monthKey, kind: "", category: "", accountId: "", page: 2, pageSize: 20 }))
  })

  it("marks transactions excluded from income and expense statistics", async () => {
    const excluded = makeTransaction({ id: "tx-excluded", pnlScope: "excluded", links: [{ pluginId: "debts", kind: "principal", refId: "debt-1", label: "测试往来方" }] })
    vi.mocked(api.transactions).mockResolvedValue({ items: [excluded], page: 1, pageSize: 20, total: 1 })
    renderWorkspace(TransactionListWorkspace)
    expect((await screen.findAllByText("不计入收支")).length).toBeGreaterThan(0)
    expect(screen.getAllByText("债务往来").length).toBeGreaterThan(0)
    expect(screen.getAllByRole("link", { name: "债务往来" }).every((link) => link.getAttribute("href") === "/app/debts/debt-1")).toBe(true)
  })

  it("links bill import badges back to their batch", async () => {
    const imported = makeTransaction({ id: "tx-imported", links: [{ pluginId: "bill-imports", kind: "batch", refId: "batch-1", label: "支付宝 · 2026-07-01 至 2026-07-31" }] })
    vi.mocked(api.transactions).mockResolvedValue({ items: [imported], page: 1, pageSize: 20, total: 1 })
    renderWorkspace(TransactionListWorkspace)
    expect((await screen.findAllByText("账单导入")).length).toBeGreaterThan(0)
    expect(screen.getAllByRole("link", { name: "账单导入" }).every((link) => link.getAttribute("href") === "/app/transactions/imports/batch-1")).toBe(true)
  })

  it("keeps a disabled plugin link label visible but gray and non-clickable", async () => {
    const linked = makeTransaction({ id: "tx-disabled-link", links: [{ pluginId: "debts", kind: "repayment", refId: "debt-1", label: "测试往来方" }] })
    vi.mocked(api.transactions).mockResolvedValue({ items: [linked], page: 1, pageSize: 20, total: 1 })
    renderWorkspace(TransactionListWorkspace, [{
      id: "debts",
      name: "债务",
      description: "记录借入、借出及还款进度。",
      enabled: false,
      ownsTransactions: true,
      routePrefixes: ["/api/v1/debts"],
    }])
    const badges = await screen.findAllByText("债务往来")
    expect(badges.length).toBeGreaterThan(0)
    expect(badges.every((badge) => badge.tagName === "SPAN")).toBe(true)
    expect(badges.every((badge) => badge.classList.contains("debt-link-badge-disabled"))).toBe(true)
    expect(badges.every((badge) => badge.getAttribute("title") === "插件已关闭")).toBe(true)
    expect(screen.queryByRole("link", { name: "债务往来" })).not.toBeInTheDocument()
  })

  it("debounces transaction search, resets pagination, and clears the query", async () => {
    const user = userEvent.setup()
    renderWorkspace(TransactionListWorkspace)
    await screen.findByText("共 22 笔")
    await user.click(screen.getByRole("button", { name: "第 2 页" }))
    await waitFor(() => expect(api.transactions).toHaveBeenLastCalledWith({ month: monthKey, kind: "", category: "", accountId: "", page: 2, pageSize: 20 }))

    await user.type(screen.getByRole("searchbox", { name: "搜索流水" }), "  Merchant Alpha  ")
    expect(api.transactions).not.toHaveBeenCalledWith(expect.objectContaining({ q: "Merchant Alpha" }))
    await waitFor(() => expect(api.transactions).toHaveBeenLastCalledWith({ month: monthKey, kind: "", category: "", accountId: "", q: "Merchant Alpha", page: 1, pageSize: 20 }))

    await user.click(screen.getByRole("button", { name: "清除搜索" }))
    expect(screen.getByRole("searchbox", { name: "搜索流水" })).toHaveValue("")
    await waitFor(() => expect(api.transactions).toHaveBeenLastCalledWith({ month: monthKey, kind: "", category: "", accountId: "", page: 1, pageSize: 20 }))
    expect(screen.queryByRole("button", { name: "清除搜索" })).not.toBeInTheDocument()
  })

  it("opens categorization from 未分类, patches one transaction, and updates the row in place", async () => {
    const unclassified = makeTransaction({
      id: "tx-unclassified",
      amountCents: 2800,
      payeeName: "华尔街见闻1364473102",
      payeeKey: "华尔街见闻",
      category: "",
    })
    vi.mocked(api.transactions).mockImplementation((params) => Promise.resolve(
      params.pageSize === 1
        ? { items: [], page: 1, pageSize: 1, total: 3 }
        : { items: [unclassified], page: 1, pageSize: Number(params.pageSize || 20), total: 1 },
    ))
    vi.mocked(api.updateTransaction).mockResolvedValue({ ...unclassified, categoryId: expenseCategory.id, categorySource: "user", version: 2 })
    const user = userEvent.setup()
    renderWorkspace(TransactionListWorkspace)

    const entries = await screen.findAllByRole("button", { name: "归类 华尔街见闻1364473102" })
    expect(entries[0]).toHaveTextContent("未分类")
    await user.click(entries[0])
    const dialog = await screen.findByRole("dialog", { name: "归类流水" })
    expect(dialog).toHaveTextContent("华尔街见闻1364473102")
    await user.click(within(dialog).getByRole("combobox", { name: "分类" }))
    await user.click(within(dialog).getByRole("option", { name: "餐饮" }))
    await user.click(within(dialog).getByRole("button", { name: "确认归类" }))

    await waitFor(() => expect(api.updateTransaction).toHaveBeenCalledWith("tx-unclassified", expect.objectContaining({
      version: 1,
      categoryId: "category-food",
    }), expect.any(Object)))
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "归类流水" })).not.toBeInTheDocument())
    expect(screen.getAllByRole("button", { name: "归类 华尔街见闻1364473102" })[0]).toHaveTextContent("餐饮")
  })

  it("creates a payeeKey exact rule before recategorizing and updates matching rows in place", async () => {
    const unclassified = makeTransaction({
      id: "tx-rule",
      amountCents: 3600,
      payeeName: "华尔街见闻1364473102",
      payeeKey: "华尔街见闻",
      category: "",
    })
    vi.mocked(api.transactions).mockImplementation((params) => Promise.resolve(
      params.pageSize === 1
        ? { items: [], page: 1, pageSize: 1, total: 4 }
        : { items: [unclassified], page: 1, pageSize: Number(params.pageSize || 20), total: 1 },
    ))
    vi.mocked(api.createCategoryRule).mockResolvedValue({
      id: "rule-payee",
      priority: 100,
      enabled: true,
      sourceChannel: "",
      categoryId: expenseCategory.id,
      note: "",
      conditions: [{ id: "condition-payee", matchField: "payee_key", matchKind: "exact", matchValue: "华尔街见闻" }],
      warnings: [],
    })
    vi.mocked(api.recategorize).mockResolvedValue({ eligible: 4, matched: 4, changed: 4 })
    const user = userEvent.setup()
    renderWorkspace(TransactionListWorkspace)

    await user.click((await screen.findAllByRole("button", { name: "归类 华尔街见闻1364473102" }))[0])
    const dialog = await screen.findByRole("dialog", { name: "归类流水" })
    await user.click(within(dialog).getByRole("combobox", { name: "分类" }))
    await user.click(within(dialog).getByRole("option", { name: "餐饮" }))
    await user.click(within(dialog).getByRole("button", { name: "以后这个商户都归它" }))
    expect(await within(dialog).findByText(/当前筛选下预计影响 4 笔/)).toBeInTheDocument()
    await user.click(within(dialog).getByRole("button", { name: "确认归类" }))

    await waitFor(() => expect(api.createCategoryRule).toHaveBeenCalledWith({
      priority: 100,
      enabled: true,
      sourceChannel: "",
      categoryId: "category-food",
      note: "",
      conditions: [{ matchField: "payee_key", matchKind: "exact", matchValue: "华尔街见闻" }],
    }, expect.any(Object)))
    expect(vi.mocked(api.createCategoryRule).mock.invocationCallOrder[0]).toBeLessThan(vi.mocked(api.recategorize).mock.invocationCallOrder[0])
    expect(api.recategorize).toHaveBeenCalledWith(expect.any(Object))
    await waitFor(() => expect(screen.getAllByRole("button", { name: "归类 华尔街见闻1364473102" })[0]).toHaveTextContent("餐饮"))
  })

  it("creates a new category and immediately assigns it", async () => {
    const unclassified = makeTransaction({ id: "tx-new-category", payeeName: "虚构商店", payeeKey: "虚构商店" })
    const newCategory = { ...expenseCategory, id: "category-daily", name: "日用品" }
    vi.mocked(api.transactions).mockResolvedValue({ items: [unclassified], page: 1, pageSize: 20, total: 1 })
    vi.mocked(api.createCategory).mockResolvedValue(newCategory)
    vi.mocked(api.updateTransaction).mockResolvedValue({ ...unclassified, categoryId: newCategory.id, categorySource: "user", version: 2 })
    const user = userEvent.setup()
    renderWorkspace(TransactionListWorkspace)

    await user.click((await screen.findAllByRole("button", { name: "归类 虚构商店" }))[0])
    const dialog = await screen.findByRole("dialog", { name: "归类流水" })
    await user.type(within(dialog).getByRole("combobox", { name: "分类" }), "日用品")
    await user.click(within(dialog).getByRole("option", { name: "新建\"日用品\"" }))
    await user.click(within(dialog).getByRole("button", { name: "确认归类" }))

    await waitFor(() => expect(api.createCategory).toHaveBeenCalledWith({ parentId: null, name: "日用品", kind: "expense", sortOrder: 0 }, expect.any(Object)))
    expect(vi.mocked(api.createCategory).mock.invocationCallOrder[0]).toBeLessThan(vi.mocked(api.updateTransaction).mock.invocationCallOrder[0])
    await waitFor(() => expect(screen.getAllByRole("button", { name: "归类 虚构商店" })[0]).toHaveTextContent("日用品"))
  })

  it("renders transfer flow with neutral amounts and exposes the transfer filter", async () => {
    const transfer = {
      ...expenseItem,
      id: "tx-transfer",
      kind: "transfer" as const,
      amountCents: 8800,
      category: "账户互转",
      payeeName: "余额归集",
      note: "",
      account: null,
      transferFromAccount: { ...accountBrief, name: "微信零钱" },
      transferToAccount: { ...accountBrief, id: "account-card", accountType: "bank_card" as const, name: "招商银行 4444" },
    }
    vi.mocked(api.transactions).mockResolvedValue({ items: [transfer], page: 1, pageSize: 20, total: 1 })
    const user = userEvent.setup()
    renderWorkspace(TransactionListWorkspace)
    const flowCells = await screen.findAllByText((_, element) => element?.tagName === "TD" && Boolean(element.textContent?.includes("微信零钱") && element.textContent.includes("→") && element.textContent.includes("招商银行 4444")))
    expect(flowCells.length).toBeGreaterThan(0)
    for (const amount of screen.getAllByText("¥88.00")) {
      expect(amount).toHaveClass("tx-amount-transfer")
      expect(amount).not.toHaveClass("tx-amount-income", "tx-amount-expense")
    }
    await user.click(screen.getByRole("combobox", { name: "收支类型" }))
    expect(screen.getByRole("option", { name: "转账" })).toBeInTheDocument()
  })

  it("edits a transfer entry without dropping its transfer accounts", async () => {
    const cardAccount = { ...ledgerAccount, id: "account-card", name: "招商银行 4444", accountType: "bank_card" as const }
    const transfer = makeTransaction({
      id: "tx-transfer",
      kind: "transfer",
      amountCents: 8800,
      occurredOn: day2,
      category: "账户互转",
      payeeName: "余额归集",
      note: "归集",
      account: null,
      transferFromAccount: accountBrief,
      transferToAccount: { id: cardAccount.id, accountType: cardAccount.accountType, name: cardAccount.name, archived: false },
      version: 5,
    })
    vi.mocked(api.ledgerAccounts).mockResolvedValue([ledgerAccount, cardAccount])
    vi.mocked(api.transactions).mockImplementation(() => Promise.resolve({ items: [transfer], page: 1, pageSize: 200, total: 1 }))
    vi.mocked(api.updateTransaction).mockResolvedValue(transfer)
    const user = userEvent.setup()
    renderWorkspace()
    if (day2 !== today) await user.click(await screen.findByRole("button", { name: new RegExp(`^${day2}，`) }))
    await user.click(await screen.findByRole("button", { name: "操作 余额归集 ¥88.00" }))
    await user.click(screen.getByRole("menuitem", { name: "编辑" }))
    const dialog = await screen.findByRole("dialog", { name: "编辑记账" })

    // 转账必须给出转出/转入两个选择器，而不是单一账户
    expect(within(dialog).getByRole("combobox", { name: "转出账户" })).toBeInTheDocument()
    expect(within(dialog).getByRole("combobox", { name: "转入账户" })).toBeInTheDocument()
    expect(within(dialog).queryByRole("combobox", { name: "账户" })).not.toBeInTheDocument()

    const note = within(dialog).getByLabelText("备注（可选）")
    await user.clear(note)
    await user.type(note, "月末归集")
    await user.click(within(dialog).getByRole("button", { name: "保存" }))

    // 回归点：提交必须带上两个 transfer 账户，否则后端 validate_transaction_accounts 会 400
    await waitFor(() => expect(api.updateTransaction).toHaveBeenCalledWith("tx-transfer", {
      version: 5,
      kind: "transfer",
      amountCents: 8800,
      occurredOn: day2,
      category: "账户互转",
      accountId: null,
      transferFromAccountId: "account-1",
      transferToAccountId: "account-card",
      note: "月末归集",
    }, expect.objectContaining({ idempotencyKey: expect.any(String) })))
  })

  it("shows an open duplicate badge and resolves it from the review modal", async () => {
    const suspicion = makeDuplicateSuspicion({
      id: "duplicate-fictional",
      clusterKey: "cluster-fictional",
      score: 0.91,
      reason: "同日同金额，来源渠道不同",
      transactionA: { id: expenseItem.id, kind: "expense", amountCents: 1234, currency: "CNY", occurredOn: day2, occurredAt: null, occurredAtPrecision: "day", sourceChannel: "alipay", accountId: null },
      transactionB: { id: "tx-other", kind: "expense", amountCents: 1234, currency: "CNY", occurredOn: day2, occurredAt: null, occurredAtPrecision: "day", sourceChannel: "wechat", accountId: null },
    })
    vi.mocked(api.duplicateSuspicions).mockResolvedValue({ clusters: [{ clusterKey: suspicion.clusterKey, items: [suspicion] }], items: [suspicion], page: 1, pageSize: 200, total: 1 })
    vi.mocked(api.updateDuplicateSuspicion).mockResolvedValue({ ...suspicion, status: "confirmed" })
    const user = userEvent.setup()
    renderWorkspace(TransactionListWorkspace)
    const badges = await screen.findAllByRole("button", { name: "疑似重复" })
    await user.click(badges[0])
    const dialog = await screen.findByRole("dialog", { name: "处理疑似重复" })
    expect(dialog).toHaveTextContent("同日同金额，来源渠道不同")
    expect(dialog).toHaveTextContent("支付宝")
    expect(dialog).toHaveTextContent("微信支付")
    expect(within(dialog).getByRole("button", { name: "忽略提示" })).toBeInTheDocument()
    await user.click(within(dialog).getByRole("button", { name: "确认重复" }))
    await waitFor(() => expect(api.updateDuplicateSuspicion).toHaveBeenCalledWith("duplicate-fictional", { status: "confirmed" }, expect.any(Object)))
  })

  it("shows create debt only for unlinked transactions in both list renderings", async () => {
    const user = userEvent.setup()
    const linked = { ...incomeItem, links: [{ pluginId: "debts", kind: "principal", refId: "debt-1", label: "阿青" }] }
    vi.mocked(api.transactions).mockResolvedValue({ items: [expenseItem, linked], page: 1, pageSize: 20, total: 2 })
    renderWorkspace(TransactionListWorkspace)
    const unlinkedMenus = await screen.findAllByRole("button", { name: "操作 餐饮 ¥12.34" })
    for (const menu of unlinkedMenus) {
      await user.click(menu)
      expect(screen.getByRole("menuitem", { name: "创建债务" })).toBeInTheDocument()
      await user.keyboard("{Escape}")
    }
    const linkedMenus = screen.getAllByRole("button", { name: "操作 工资 ¥5,000.00" })
    for (const menu of linkedMenus) {
      await user.click(menu)
      expect(screen.queryByRole("menuitem", { name: "创建债务" })).not.toBeInTheDocument()
      await user.keyboard("{Escape}")
    }
  })

  it("maps transaction direction and chooses the longest counterparty substring", () => {
    expect(transactionDebtDirection("expense")).toBe("lend_out")
    expect(transactionDebtDirection("income")).toBe("borrow_in")
    const counterparties = [
      { id: "short", displayName: "阿青", archived: false },
      { id: "long", displayName: "阿青工作室", archived: false },
    ] as never
    expect(debtDraftCounterparty({ note: "项目款", payeeName: "阿青工作室", description: "", category: "" }, counterparties)).toEqual({ counterpartyId: "long", counterpartyName: "" })
  })

  it("uses the structured payee when creating a debt draft", () => {
    expect(debtDraftCounterparty({ note: "午餐", payeeName: "虚构商户（杭州店）", description: "", category: "餐饮" }, [])).toEqual({ counterpartyId: "", counterpartyName: "虚构商户（杭州店）" })
  })

  it("renders the statistics category share with 未分类 and excludes income-only categories", async () => {
    renderWorkspace(TransactionStatisticsWorkspace)
    const share = await screen.findByLabelText("分类占比")
    expect(await within(share).findByText("餐饮")).toBeInTheDocument()
    expect(within(share).getByText("¥1,500.00 · 79%")).toBeInTheDocument()
    expect(within(share).getByText("未分类")).toBeInTheDocument()
    expect(within(share).queryByText("工资")).not.toBeInTheDocument()
  })
})

describe("流水详情", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.mocked(api.transactionSummary).mockImplementation((month: string) => Promise.resolve({ ...summary, month }))
    vi.mocked(api.transactions).mockResolvedValue({ items: [expenseItem], page: 1, pageSize: 20, total: 1 })
    vi.mocked(api.transactionCategories).mockResolvedValue(["餐饮"])
    vi.mocked(api.categories).mockResolvedValue([expenseCategory, incomeCategory])
    vi.mocked(api.ledgerAccounts).mockResolvedValue([ledgerAccount])
    vi.mocked(api.counterparties).mockResolvedValue([])
    vi.mocked(api.duplicateSuspicions).mockResolvedValue({ clusters: [], items: [], page: 1, pageSize: 200, total: 0 })
  })

  it("点击流水行打开的是同一张表单的只读态，不是另一套 UI", async () => {
    const user = userEvent.setup()
    renderWorkspace(TransactionListWorkspace)
    await user.click(await screen.findByText("午饭"))

    const dialog = await screen.findByRole("dialog", { name: "流水详情" })
    // 用的是表单本体：同样的字段、同样的标签，只是禁用
    expect(within(dialog).getByLabelText("金额（元）")).toBeDisabled()
    expect(within(dialog).getByLabelText("备注（可选）")).toBeDisabled()
    // 表单控件装不下、也不该让用户改的字段，作为末尾的只读补充
    expect(within(dialog).getByText("分类来源")).toBeInTheDocument()
    expect(within(dialog).getByText(/第 3 版/)).toBeInTheDocument()
    // 只读态不给保存，给的是删除与编辑
    expect(within(dialog).getByRole("button", { name: "编辑" })).toBeInTheDocument()
    expect(within(dialog).queryByRole("button", { name: "保存" })).not.toBeInTheDocument()
  })

  it("shows the traced rule name as the category source", async () => {
    vi.mocked(api.transactions).mockResolvedValue({
      items: [makeTransaction({
        ...expenseItem,
        categorySource: "rule",
        categoryRuleId: "rule-breakfast",
        categoryRuleName: "早餐规则",
      })],
      page: 1,
      pageSize: 20,
      total: 1,
    })
    const user = userEvent.setup()
    renderWorkspace(TransactionListWorkspace)
    await user.click(await screen.findByText("午饭"))
    const dialog = await screen.findByRole("dialog", { name: "流水详情" })
    expect(within(dialog).getByText("规则：早餐规则")).toBeInTheDocument()
  })

  it("从详情点编辑就地解锁同一张表单，不再开第二个弹窗", async () => {
    const user = userEvent.setup()
    renderWorkspace(TransactionListWorkspace)
    await user.click(await screen.findByText("午饭"))
    const dialog = await screen.findByRole("dialog", { name: "流水详情" })
    await user.click(within(dialog).getByRole("button", { name: "编辑" }))

    // 标题变了，但还是同一个 dialog——字段没有被重新挂载成另一套
    const editing = await screen.findByRole("dialog", { name: "编辑记账" })
    expect(within(editing).getByLabelText("金额（元）")).toBeEnabled()
    expect(within(editing).getByLabelText("金额（元）")).toHaveValue("12.34")
    expect(within(editing).getByRole("button", { name: "保存" })).toBeInTheDocument()
    // 只读态的元数据在编辑态依然可见
    expect(within(editing).getByText("分类来源")).toBeInTheDocument()
  })

  it("点击行内按钮不应顺带打开详情", async () => {
    const user = userEvent.setup()
    renderWorkspace(TransactionListWorkspace)
    // 「归类」按钮有自己的弹窗，它不该被详情抢走
    await user.click((await screen.findAllByRole("button", { name: /^归类 / }))[0])

    expect(await screen.findByRole("dialog", { name: "归类流水" })).toBeInTheDocument()
    expect(screen.queryByRole("dialog", { name: "流水详情" })).not.toBeInTheDocument()
  })

  it("拖选文字后松手不算点击——单元格允许复制订单号", async () => {
    const user = userEvent.setup()
    renderWorkspace(TransactionListWorkspace)
    const cell = await screen.findByText("午饭")
    // 模拟「已经选中了一段文字」的状态
    vi.spyOn(window, "getSelection").mockReturnValue({ toString: () => "订单编号3586429" } as unknown as Selection)
    await user.click(cell)

    expect(screen.queryByRole("dialog", { name: "流水详情" })).not.toBeInTheDocument()
    vi.mocked(window.getSelection).mockRestore()
  })
})
