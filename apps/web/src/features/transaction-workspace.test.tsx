import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { render, screen, waitFor, within } from "@testing-library/react"
import { useState, type ComponentType } from "react"
import userEvent from "@testing-library/user-event"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { ApiClientError, api } from "../api/client"
import { AppToastProvider } from "../components/ui"
import { TopbarSlotContext, type TopbarSlots } from "../components/topbar-slots"
import { CalendarWorkspace, TransactionListWorkspace, TransactionStatisticsWorkspace } from "./transaction-workspace"

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
    ledgerAccounts: vi.fn(),
    createTransaction: vi.fn(),
    updateTransaction: vi.fn(),
    deleteTransaction: vi.fn(),
    restoreTransaction: vi.fn(),
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

const incomeItem = {
  id: "tx-1",
  kind: "income" as const,
  amountCents: 500000,
  occurredOn: day1,
  category: "工资",
  account: null,
  note: "八月工资",
  archived: false,
  version: 1,
  createdAt: "2026-08-02T00:00:00Z",
  updatedAt: "2026-08-02T00:00:00Z",
}
const expenseItem = {
  id: "tx-2",
  kind: "expense" as const,
  amountCents: 1234,
  occurredOn: day2,
  category: "餐饮",
  account: accountBrief,
  note: "午饭",
  archived: false,
  version: 3,
  createdAt: "2026-08-02T00:00:00Z",
  updatedAt: "2026-08-02T00:00:00Z",
}

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

function TopbarHarness({ Workspace }: { Workspace: ComponentType }) {
  const [slots, setSlots] = useState<TopbarSlots>()
  return <TopbarSlotContext.Provider value={setSlots}><div data-testid="topbar-title">{slots?.title}</div><div data-testid="topbar-actions">{slots?.actions}</div><Workspace /></TopbarSlotContext.Provider>
}

function renderWorkspace(Workspace: ComponentType = CalendarWorkspace) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
  const invalidateSpy = vi.spyOn(client, "invalidateQueries")
  render(<QueryClientProvider client={client}><AppToastProvider><TopbarHarness Workspace={Workspace} /></AppToastProvider></QueryClientProvider>)
  return { invalidateSpy }
}

describe("TransactionWorkspace", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.mocked(api.transactionSummary).mockImplementation((month: string) => Promise.resolve({ ...summary, month }))
    vi.mocked(api.transactions).mockImplementation((params: Record<string, string | number | undefined>) => Promise.resolve(
      params.pageSize === 200
        ? { items: [incomeItem, expenseItem], page: 1, pageSize: 200, total: 2 }
        : { items: [incomeItem, expenseItem], page: Number(params.page || 1), pageSize: 20, total: 22 },
    ))
    vi.mocked(api.transactionCategories).mockResolvedValue(["交通", "工资", "餐饮"])
    vi.mocked(api.ledgerAccounts).mockResolvedValue([ledgerAccount])
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
    vi.mocked(api.deleteTransaction).mockResolvedValue(undefined)
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
    vi.mocked(api.deleteTransaction).mockResolvedValue(undefined)
    vi.mocked(api.restoreTransaction).mockResolvedValue(expenseItem)
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "确认删除" }))
    await user.click(await screen.findByRole("button", { name: "撤销" }))
    // Deleting bumps the version from 3 to 4, so the restore must target 4.
    await waitFor(() => expect(api.restoreTransaction).toHaveBeenCalledWith("tx-2", 4, expect.objectContaining({ idempotencyKey: expect.any(String) })))
    expect(await screen.findByText("已撤销删除")).toBeInTheDocument()
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

  it("shows one coordinated empty statistics state", async () => {
    vi.mocked(api.transactionSummary).mockResolvedValue({ ...summary, days: [], byCategory: [], incomeCents: 0, expenseCents: 0, netCents: 0, transactionCount: 0 })
    renderWorkspace(TransactionStatisticsWorkspace)
    expect(await screen.findByText(/本月暂无收支数据/)).toBeInTheDocument()
    expect(screen.queryByText(/本月暂无支出分类数据/)).not.toBeInTheDocument()
  })

  it("places statistics title and month navigation in the topbar with a compact summary", async () => {
    const user = userEvent.setup()
    renderWorkspace(TransactionStatisticsWorkspace)
    await screen.findByLabelText("本月收支汇总")
    expect(screen.getByTestId("topbar-title")).toHaveTextContent("统计")
    expect(screen.getByTestId("topbar-actions")).toHaveTextContent("今天")
    expect(document.querySelectorAll(".statistics-summary-item")).toHaveLength(3)
    expect(screen.getByText("共 3 笔")).toBeInTheDocument()
    await user.click(screen.getByRole("button", { name: "下一月" }))
    await waitFor(() => expect(api.transactionSummary).toHaveBeenCalledWith(nextMonthKey))
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

  it("renders the statistics category share with 未分类 and excludes income-only categories", async () => {
    renderWorkspace(TransactionStatisticsWorkspace)
    const share = await screen.findByLabelText("分类占比")
    expect(await within(share).findByText("餐饮")).toBeInTheDocument()
    expect(within(share).getByText("¥1,500.00 · 79%")).toBeInTheDocument()
    expect(within(share).getByText("未分类")).toBeInTheDocument()
    expect(within(share).queryByText("工资")).not.toBeInTheDocument()
  })
})
