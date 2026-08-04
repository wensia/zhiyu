import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { render, screen, waitFor, within } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { beforeEach, describe, expect, it, vi } from "vitest"
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom"

import { ApiClientError, api } from "../api/client"
import { AppToastProvider } from "../components/ui"
import { AppShell } from "../App"
import { elapsedCalendarDays } from "./debt-time"
import { DebtDetailPage, DebtFormModal, DebtWorkspace } from "./debt-workspace"

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
    debts: vi.fn(),
    debt: vi.fn(),
    summary: vi.fn(),
    counterparties: vi.fn(),
    ledgerAccounts: vi.fn(),
    createDebt: vi.fn(),
    updateDebt: vi.fn(),
    createDebtAddition: vi.fn(),
    updateDebtAddition: vi.fn(),
    createRepayment: vi.fn(),
    updateRepayment: vi.fn(),
    reverseRepayment: vi.fn(),
    archiveDebt: vi.fn(),
    restoreDebt: vi.fn(),
    deleteDebt: vi.fn(),
    createLedgerAccount: vi.fn(),
    updateLedgerAccount: vi.fn(),
    archiveLedgerAccount: vi.fn(),
    restoreLedgerAccount: vi.fn(),
  },
}))

const ledgerAccount = {
  id: "account-1",
  accountType: "wechat_balance" as const,
  name: "微信支付-测试号",
  nameSource: "custom" as const,
  note: "日常资金账户",
  archived: false,
  version: 1,
  usageCount: 3,
  createdAt: "2026-08-02T00:00:00Z",
  updatedAt: "2026-08-02T00:00:00Z",
}

const accountBrief = { id: ledgerAccount.id, accountType: ledgerAccount.accountType, name: ledgerAccount.name, archived: false }

const debt = {
  id: "debt-1",
  direction: "lend_out",
  counterparty: { id: "person-1", displayName: "阿青" },
  principalCents: 100_000,
  paidCents: 20_000,
  remainingCents: 80_000,
  currency: "CNY",
  occurredOn: "2026-08-02",
  dueOn: "2026-08-09",
  note: "朋友借款",
  account: accountBrief,
  originKind: "cash_movement" as const,
  status: "due_soon" as const,
  archived: false,
  version: 2,
  createdAt: "2026-08-02T00:00:00Z",
  updatedAt: "2026-08-02T00:00:00Z",
  additions: [],
  repayments: [],
}

function LocationProbe() {
  const { pathname } = useLocation()
  return <output data-testid="location">{pathname}</output>
}

function renderWorkspace(initialEntries = ["/app/debts"]) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return render(<QueryClientProvider client={client}><AppToastProvider><MemoryRouter initialEntries={initialEntries}><LocationProbe /><Routes><Route path="/app/debts" element={<DebtWorkspace />} /><Route path="/app/debts/:id" element={<DebtDetailPage />} /></Routes></MemoryRouter></AppToastProvider></QueryClientProvider>)
}

function renderWorkspaceWithShell(initialEntries = ["/app/debts"]) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return render(<QueryClientProvider client={client}><AppToastProvider><MemoryRouter initialEntries={initialEntries}><LocationProbe /><Routes><Route path="/app" element={<AppShell email="test@example.com" />}><Route path="debts" element={<DebtWorkspace />} /><Route path="debts/:id" element={<DebtDetailPage />} /></Route></Routes></MemoryRouter></AppToastProvider></QueryClientProvider>)
}

describe("DebtWorkspace", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.mocked(api.debts).mockResolvedValue({ items: [debt], page: 1, pageSize: 20, total: 1 })
    vi.mocked(api.debt).mockResolvedValue(debt)
    vi.mocked(api.summary).mockResolvedValue({ lendOutRemainingCents: 80_000, borrowInRemainingCents: 0, netCents: 80_000, overdueCount: 0 })
    vi.mocked(api.counterparties).mockResolvedValue([{ id: "person-1", displayName: "阿青", note: "", archived: false, version: 1, lendOutRemainingCents: 80_000, borrowInRemainingCents: 0, netCents: 80_000, activeDebtCount: 1, overdueCount: 0 }])
    vi.mocked(api.ledgerAccounts).mockResolvedValue([ledgerAccount])
  })

  it("renders the financial summary and due-soon row", async () => {
    renderWorkspace()
    expect(await screen.findByRole("button", { name: "阿青" })).toBeInTheDocument()
    const row = screen.getByRole("row", { name: /阿青 借出/ })
    expect(screen.getByRole("columnheader", { name: "债务概况" })).toBeInTheDocument()
    expect(row).toHaveTextContent("本金 ¥1,000.00")
    expect(row).toHaveTextContent("已还 ¥200.00")
    expect(row).toHaveTextContent("剩余 ¥800.00")
    expect(row).toHaveTextContent("2026-08-02")
    expect(row.querySelector(".debt-age")).toHaveTextContent(`${elapsedCalendarDays(debt.occurredOn)} 天`)
    expect(screen.getAllByText("临近到期").length).toBeGreaterThan(1)
    expect(screen.getAllByText("¥800.00").length).toBeGreaterThan(0)
  })

  it("calculates elapsed calendar days without timezone drift", () => {
    expect(elapsedCalendarDays("2026-08-02", "2026-08-03")).toBe(1)
    expect(elapsedCalendarDays("2024-02-28", "2024-03-01")).toBe(2)
    expect(elapsedCalendarDays("2026-08-04", "2026-08-03")).toBe(0)
  })

  it("sends search terms through the list contract", async () => {
    const user = userEvent.setup()
    renderWorkspace()
    await screen.findByRole("button", { name: "阿青" })
    await user.type(screen.getByPlaceholderText("搜索联系人或备注"), "朋友")
    await waitFor(() => expect(api.debts).toHaveBeenLastCalledWith(expect.objectContaining({ query: "朋友" })))
  })

  it("navigates to a dedicated debt URL and returns to the list", async () => {
    const user = userEvent.setup()
    renderWorkspaceWithShell()
    await user.click(await screen.findByRole("row", { name: /阿青 借出/ }))
    const backButton = await screen.findByRole("button", { name: "返回债务列表" })
    expect(backButton.closest(".topbar")).not.toBeNull()
    expect(screen.getByRole("heading", { name: "债务详情" })).toHaveClass("sr-only")
    expect(screen.queryByText("个人往来")).not.toBeInTheDocument()
    expect(screen.getByText("初始借出 ¥1,000.00")).toBeInTheDocument()
    expect(screen.getByText("1 条")).toBeInTheDocument()
    await user.click(screen.getByRole("button", { name: "更多债务操作" }))
    expect(await screen.findByRole("menuitem", { name: "删除债务" })).toBeInTheDocument()
    await user.keyboard("{Escape}")
    expect(screen.getByTestId("location")).toHaveTextContent("/app/debts/debt-1")
    await user.click(backButton)
    expect(await screen.findByRole("heading", { name: "债务管理" })).toBeInTheDocument()
    expect(screen.getByTestId("location")).toHaveTextContent("/app/debts")
  })

  it("keeps the real table structure and pagination mounted while loading", () => {
    vi.mocked(api.debts).mockReturnValue(new Promise(() => undefined))
    const { container } = renderWorkspace()
    expect(screen.getByRole("table")).toBeInTheDocument()
    expect(screen.getByRole("columnheader", { name: "联系人" })).toBeInTheDocument()
    expect(container.querySelectorAll(".skeleton-row")).toHaveLength(6)
    expect(screen.getByText("共 0 笔")).toBeInTheDocument()
  })

  it("uses real filter options and clears from the trigger", async () => {
    const user = userEvent.setup()
    renderWorkspace()
    const direction = screen.getByRole("combobox", { name: "方向" })
    await user.click(direction)
    expect(screen.queryByRole("option", { name: "全部方向" })).not.toBeInTheDocument()
    await user.click(screen.getByRole("option", { name: "借出" }))
    expect(direction).toHaveTextContent("借出")
    await user.click(screen.getByRole("button", { name: "清空方向筛选" }))
    expect(direction).toHaveTextContent("全部方向")
  })

  it("uses the shared Kiln calendar for debt dates", async () => {
    const user = userEvent.setup()
    renderWorkspace()
    await user.click(await screen.findByRole("button", { name: /新增债务/ }))
    await user.click(screen.getByRole("button", { name: "发生日期" }))
    expect(screen.getByRole("dialog", { name: "选择日期" })).toBeInTheDocument()
    expect(screen.queryByDisplayValue("2026-08-02")).not.toBeInTheDocument()
    await user.click(screen.getByRole("gridcell", { name: "2026-08-03" }))
    expect(screen.getByRole("button", { name: "发生日期" })).toHaveTextContent("2026-08-03")
  })

  it("keeps invalid debt input inside the form", async () => {
    const user = userEvent.setup()
    renderWorkspace()
    await user.click(screen.getByRole("button", { name: /新增债务/ }))
    await user.click(screen.getByRole("button", { name: "保存" }))
    expect(await screen.findByText("请输入正确的本金金额")).toBeInTheDocument()
    expect(api.createDebt).not.toHaveBeenCalled()
  })

  it("requires an explicit money account before creating a debt", async () => {
    const user = userEvent.setup()
    vi.mocked(api.createDebt).mockResolvedValue(debt)
    renderWorkspace()

    await user.click(await screen.findByRole("button", { name: /新增债务/ }))
    const dialog = screen.getByRole("dialog", { name: "新增债务" })
    await user.type(within(dialog).getByLabelText("联系人"), "阿青")
    await user.type(within(dialog).getByLabelText("本金（元）"), "1000")
    await user.click(within(dialog).getByRole("button", { name: "保存" }))
    expect(await within(dialog).findByText("请选择收款账户")).toBeInTheDocument()
    expect(api.createDebt).not.toHaveBeenCalled()

    await user.click(within(dialog).getByRole("combobox", { name: "收款账户" }))
    await user.click(screen.getByRole("option", { name: "微信零钱 · 微信支付-测试号" }))
    await user.click(within(dialog).getByRole("button", { name: "保存" }))

    await waitFor(() => expect(api.createDebt).toHaveBeenCalledWith(expect.objectContaining({
      accountId: "account-1",
      counterpartyName: "阿青",
      direction: "borrow_in",
      originKind: "cash_movement",
      principalCents: 100_000,
    })))
  })

  it("creates a cashless debt without choosing an account", async () => {
    const user = userEvent.setup()
    vi.mocked(api.createDebt).mockResolvedValue(debt)
    renderWorkspace()

    await user.click(await screen.findByRole("button", { name: /新增债务/ }))
    const dialog = screen.getByRole("dialog", { name: "新增债务" })
    await user.click(within(dialog).getByRole("button", { name: "赊账·无资金进出" }))
    expect(within(dialog).queryByRole("combobox", { name: "收款账户" })).not.toBeInTheDocument()
    expect(within(dialog).getByText(/不计入任何账户流水/)).toBeInTheDocument()

    await user.type(within(dialog).getByLabelText("联系人"), "代办记账")
    await user.type(within(dialog).getByLabelText("本金（元）"), "1500")
    await user.click(within(dialog).getByRole("button", { name: "保存" }))

    await waitFor(() => expect(api.createDebt).toHaveBeenCalledWith(expect.objectContaining({
      accountId: null,
      counterpartyName: "代办记账",
      direction: "borrow_in",
      originKind: "no_cash_movement",
      principalCents: 150_000,
    })))
  })

  it("shows a cashless debt as a confirmed payable with no account movement", async () => {
    const cashlessDebt = {
      ...debt,
      direction: "borrow_in",
      account: null,
      originKind: "no_cash_movement" as const,
      note: "代办执照+代记账尾款",
    }
    vi.mocked(api.debt).mockResolvedValue(cashlessDebt)
    renderWorkspace(["/app/debts/debt-1"])

    expect(await screen.findByText("确认应付 ¥1,000.00")).toBeInTheDocument()
    expect(screen.queryByText("初始借入 ¥1,000.00")).not.toBeInTheDocument()
    expect(screen.getAllByText("无资金进出").length).toBeGreaterThan(1)
    expect(screen.getAllByText(/资金往来/).length).toBeGreaterThan(1)
    expect(screen.queryByText("历史未指定")).not.toBeInTheDocument()
  })

  it("keeps legacy movements with no structured account readable", async () => {
    const legacyDebt = {
      ...debt,
      account: null,
      additions: [{ id: "legacy-addition", amountCents: 10_000, effectiveOn: "2026-08-04", note: "历史追加", account: null, createdAt: "2026-08-04T09:00:00Z" }],
      repayments: [{ id: "legacy-payment", amountCents: 20_000, effectiveOn: "2026-08-03", note: "历史还款", account: null, kind: "payment", reversed: false, reversesEventId: null, createdAt: "2026-08-03T09:00:00Z" }],
    }
    vi.mocked(api.debt).mockResolvedValue(legacyDebt)
    renderWorkspace(["/app/debts/debt-1"])

    expect(await screen.findByText("历史还款")).toBeInTheDocument()
    expect(screen.getByText("历史追加")).toBeInTheDocument()
    expect(screen.getAllByText("历史未指定")).toHaveLength(4)
  })

  it("derives the initial movement from cumulative principal across additions", async () => {
    vi.mocked(api.debt).mockResolvedValue({
      ...debt,
      direction: "borrow_in",
      principalCents: 6_000_000,
      paidCents: 0,
      remainingCents: 6_000_000,
      occurredOn: "2023-08-02",
      account: null,
      note: "",
      additions: [
        { id: "addition-later", amountCents: 1_000_000, effectiveOn: "2026-09-07", note: "", account: null, createdAt: "2026-08-02T12:43:18Z" },
        { id: "addition-earlier", amountCents: 1_000_000, effectiveOn: "2023-09-11", note: "", account: null, createdAt: "2026-08-02T12:43:34Z" },
      ],
      repayments: [],
    })
    renderWorkspace(["/app/debts/debt-1"])

    const history = await screen.findByRole("region", { name: "债务往来记录" })
    const rows = history.querySelectorAll(".timeline-row")
    expect(within(history).getByText("3 条")).toBeInTheDocument()
    expect(rows).toHaveLength(3)
    expect(rows[0]).toHaveTextContent("追加借入 ¥10,000.00")
    expect(rows[0]).toHaveTextContent("2026-09-07")
    expect(rows[1]).toHaveTextContent("追加借入 ¥10,000.00")
    expect(rows[1]).toHaveTextContent("2023-09-11")
    expect(rows[2]).toHaveTextContent("初始借入 ¥40,000.00")
    expect(rows[2]).toHaveTextContent("2023-08-02")
    expect(rows[2]).toHaveTextContent("收款账户：历史未指定")
    expect(within(history).queryByText("初始借入 ¥60,000.00")).not.toBeInTheDocument()
  })

  it("refreshes current data after an optimistic-lock conflict", async () => {
    const user = userEvent.setup()
    const onSaved = vi.fn().mockResolvedValue(undefined)
    vi.mocked(api.updateDebt).mockRejectedValue(
      new ApiClientError(409, { code: "version_conflict", message: "记录已在其他设备更新，请刷新后重试" }),
    )
    const client = new QueryClient({ defaultOptions: { mutations: { retry: false } } })
    render(
      <QueryClientProvider client={client}>
        <AppToastProvider>
          <DebtFormModal
            counterparties={[{ id: "person-1", displayName: "阿青", note: "", archived: false, version: 1, lendOutRemainingCents: 0, borrowInRemainingCents: 0, netCents: 0, activeDebtCount: 1, overdueCount: 0 }]}
            accounts={[ledgerAccount]}
            debt={debt}
            onOpenChange={() => undefined}
            onSaved={onSaved}
            open
          />
        </AppToastProvider>
      </QueryClientProvider>,
    )
    await user.click(screen.getByRole("button", { name: "保存" }))
    expect(await screen.findByText("记录已在其他设备更新，请刷新后重试")).toBeInTheDocument()
    await waitFor(() => expect(onSaved).toHaveBeenCalled())
  })

  it("binds the selected account when editing a debt", async () => {
    const user = userEvent.setup()
    vi.mocked(api.updateDebt).mockResolvedValue(debt)
    const onSaved = vi.fn().mockResolvedValue(undefined)
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { mutations: { retry: false } } })}>
        <AppToastProvider>
          <DebtFormModal accounts={[ledgerAccount]} counterparties={[{ id: "person-1", displayName: "阿青", note: "", archived: false, version: 1, lendOutRemainingCents: 0, borrowInRemainingCents: 0, netCents: 0, activeDebtCount: 1, overdueCount: 0 }]} debt={debt} onOpenChange={() => undefined} onSaved={onSaved} open />
        </AppToastProvider>
      </QueryClientProvider>,
    )

    const dialog = screen.getByRole("dialog", { name: "编辑债务" })
    expect(within(dialog).getByRole("combobox", { name: "付款账户" })).toHaveTextContent("微信零钱 · 微信支付-测试号")
    await user.click(within(dialog).getByRole("button", { name: "保存" }))
    await waitFor(() => expect(api.updateDebt).toHaveBeenCalledWith("debt-1", expect.objectContaining({ accountId: "account-1" })))
  })

  it("appends in the existing debt direction and merges the activity timeline", async () => {
    const user = userEvent.setup()
    const updatedDebt = {
      ...debt,
      principalCents: 125_000,
      remainingCents: 105_000,
      version: 3,
      additions: [{ id: "addition-1", amountCents: 25_000, effectiveOn: "2026-08-04", note: "又借了一笔", account: accountBrief, createdAt: "2026-08-04T09:00:00Z" }],
      repayments: [{ id: "payment-1", amountCents: 20_000, effectiveOn: "2026-08-03", note: "先还一部分", account: accountBrief, kind: "payment", reversed: false, reversesEventId: null, createdAt: "2026-08-03T09:00:00Z" }],
    }
    vi.mocked(api.debt).mockResolvedValueOnce(debt).mockResolvedValue(updatedDebt)
    vi.mocked(api.createDebtAddition).mockResolvedValue(updatedDebt)
    renderWorkspace()

    await user.click(await screen.findByRole("button", { name: "阿青" }))
    await user.click(await screen.findByRole("button", { name: "登记往来" }))
    const additionDialog = await screen.findByRole("dialog", { name: "登记往来" })
    expect(within(additionDialog).getByRole("combobox", { name: "动作" })).toHaveTextContent("追加借出")
    await user.type(within(additionDialog).getByLabelText("追加借出金额（元）"), "250")
    await user.click(within(additionDialog).getByRole("combobox", { name: "付款账户" }))
    await user.click(screen.getByRole("option", { name: "微信零钱 · 微信支付-测试号" }))
    await user.type(within(additionDialog).getByLabelText("备注"), "又借了一笔")
    await user.click(within(additionDialog).getByRole("button", { name: "确认登记" }))

    await waitFor(() => expect(api.createDebtAddition).toHaveBeenCalledWith("debt-1", {
      amountCents: 25_000,
      effectiveOn: expect.stringMatching(/^\d{4}-\d{2}-\d{2}$/),
      note: "又借了一笔",
      accountId: "account-1",
    }))
    expect(await screen.findByText("往来记录")).toBeInTheDocument()
    expect(await screen.findByText("追加借出 ¥250.00")).toBeInTheDocument()
    expect(screen.getByText("还款 ¥200.00")).toBeInTheDocument()
    expect(screen.getByText("初始借出 ¥1,000.00")).toBeInTheDocument()
    expect(screen.getByText("3 条")).toBeInTheDocument()
    const historyRows = screen.getByRole("region", { name: "债务往来记录" }).querySelectorAll(".timeline-row")
    expect(historyRows).toHaveLength(3)
    expect(historyRows[0]).toHaveTextContent("追加借出 ¥250.00")
    expect(historyRows[1]).toHaveTextContent("还款 ¥200.00")
    expect(historyRows[2]).toHaveTextContent("初始借出 ¥1,000.00")
  })

  it("edits an addition record and keeps an unspecified historical account", async () => {
    const user = userEvent.setup()
    const initialDebt = {
      ...debt,
      additions: [{ id: "addition-1", amountCents: 25_000, effectiveOn: "2026-08-04", note: "旧备注", account: null, createdAt: "2026-08-04T09:00:00Z" }],
    }
    const updatedDebt = {
      ...initialDebt,
      principalCents: 90_000,
      remainingCents: 70_000,
      version: 3,
      additions: [{ ...initialDebt.additions[0], amountCents: 10_000, note: "修正追加" }],
    }
    vi.mocked(api.debt).mockResolvedValue(initialDebt)
    vi.mocked(api.updateDebtAddition).mockResolvedValue(updatedDebt)
    renderWorkspace(["/app/debts/debt-1"])

    await user.click(await screen.findByRole("button", { name: "操作 2026-08-04 追加借出 ¥250.00" }))
    await user.click(await screen.findByRole("menuitem", { name: "编辑记录" }))
    const dialog = await screen.findByRole("dialog", { name: "编辑追加借出" })
    expect(within(dialog).getByLabelText("追加借出金额（元）")).toHaveValue("250")
    expect(within(dialog).getByLabelText("付款账户")).toHaveTextContent("历史未指定")
    await user.clear(within(dialog).getByLabelText("追加借出金额（元）"))
    await user.type(within(dialog).getByLabelText("追加借出金额（元）"), "100")
    await user.clear(within(dialog).getByLabelText("备注"))
    await user.type(within(dialog).getByLabelText("备注"), "修正追加")
    await user.click(within(dialog).getByRole("button", { name: "保存修改" }))

    await waitFor(() => expect(api.updateDebtAddition).toHaveBeenCalledWith("addition-1", {
      version: 2,
      amountCents: 10_000,
      effectiveOn: "2026-08-04",
      note: "修正追加",
      accountId: undefined,
    }))
  })

  it("edits an active repayment record but not reversed history", async () => {
    const user = userEvent.setup()
    const payment = { id: "payment-1", amountCents: 20_000, effectiveOn: "2026-08-03", note: "旧还款", account: accountBrief, kind: "payment", reversed: false, reversesEventId: null, createdAt: "2026-08-03T09:00:00Z" }
    const initialDebt = { ...debt, repayments: [payment] }
    const updatedDebt = { ...initialDebt, paidCents: 25_000, remainingCents: 75_000, version: 3, repayments: [{ ...payment, amountCents: 25_000, note: "修正还款" }] }
    vi.mocked(api.debt).mockResolvedValue(initialDebt)
    vi.mocked(api.updateRepayment).mockResolvedValue(updatedDebt)
    renderWorkspace(["/app/debts/debt-1"])

    await user.click(await screen.findByRole("button", { name: "操作 2026-08-03 还款 ¥200.00" }))
    await user.click(await screen.findByRole("menuitem", { name: "编辑记录" }))
    const dialog = await screen.findByRole("dialog", { name: "编辑还款记录" })
    expect(within(dialog).getByLabelText("还款金额（元）")).toHaveValue("200")
    await user.clear(within(dialog).getByLabelText("还款金额（元）"))
    await user.type(within(dialog).getByLabelText("还款金额（元）"), "250")
    await user.clear(within(dialog).getByLabelText("备注"))
    await user.type(within(dialog).getByLabelText("备注"), "修正还款")
    await user.click(within(dialog).getByRole("button", { name: "保存修改" }))

    await waitFor(() => expect(api.updateRepayment).toHaveBeenCalledWith("payment-1", {
      version: 2,
      amountCents: 25_000,
      effectiveOn: "2026-08-03",
      note: "修正还款",
      accountId: "account-1",
    }))

  })

  it("does not expose edits for archived, reversed, or reversal records", async () => {
    const reversedPayment = { id: "payment-1", amountCents: 20_000, effectiveOn: "2026-08-03", note: "", account: accountBrief, kind: "payment", reversed: true, reversesEventId: null, createdAt: "2026-08-03T09:00:00Z" }
    const reversal = { ...reversedPayment, id: "reversal-1", kind: "reversal", reversed: false, reversesEventId: "payment-1" }
    vi.mocked(api.debt).mockResolvedValue({ ...debt, archived: true, status: "archived", additions: [{ id: "addition-1", amountCents: 1_000, effectiveOn: "2026-08-04", note: "", account: accountBrief, createdAt: "2026-08-04T09:00:00Z" }], repayments: [reversedPayment, reversal] })
    renderWorkspace(["/app/debts/debt-1"])

    await screen.findByText("追加借出 ¥10.00")
    expect(screen.queryByRole("button", { name: /^操作 .*追加借出/ })).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: /^操作 .*还款/ })).not.toBeInTheDocument()
  })

  it("does not offer additions for an archived debt", async () => {
    const user = userEvent.setup()
    vi.mocked(api.debt).mockResolvedValue({ ...debt, archived: true, status: "archived" })
    renderWorkspace()
    await user.click(await screen.findByRole("button", { name: "阿青" }))
    await user.click(await screen.findByRole("button", { name: "更多债务操作" }))
    expect(await screen.findByRole("menuitem", { name: "恢复债务" })).toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "追加借出" })).not.toBeInTheDocument()
  })

  it("keeps principal locked whenever any activity history exists", () => {
    const onSaved = vi.fn().mockResolvedValue(undefined)
    const client = new QueryClient({ defaultOptions: { mutations: { retry: false } } })
    render(
      <QueryClientProvider client={client}>
        <AppToastProvider>
          <DebtFormModal
            counterparties={[{ id: "person-1", displayName: "阿青", note: "", archived: false, version: 1, lendOutRemainingCents: 0, borrowInRemainingCents: 0, netCents: 0, activeDebtCount: 1, overdueCount: 0 }]}
            debt={{ ...debt, paidCents: 0, additions: [{ id: "addition-1", amountCents: 10_000, effectiveOn: "2026-08-03", note: "", account: accountBrief, createdAt: "2026-08-03T00:00:00Z" }] }}
            onOpenChange={() => undefined}
            onSaved={onSaved}
            open
          />
        </AppToastProvider>
      </QueryClientProvider>,
    )
    expect(screen.getByRole("textbox", { name: /本金（元）/ })).toBeDisabled()
    expect(screen.getByText("已有追加或还款记录，本金不可修改")).toBeInTheDocument()
  })

  it("opens the primary action with the keyboard", async () => {
    const user = userEvent.setup()
    renderWorkspace()
    const trigger = await screen.findByRole("button", { name: /新增债务/ })
    trigger.focus()
    await user.keyboard("{Enter}")
    expect(await screen.findByRole("dialog", { name: "新增债务" })).toBeInTheDocument()
  })
})
