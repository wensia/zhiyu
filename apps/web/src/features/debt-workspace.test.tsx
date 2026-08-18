import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { render, screen, waitFor, within } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { beforeEach, describe, expect, it, vi } from "vitest"
import { type ReactNode, useState } from "react"
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom"

import { ApiClientError, api } from "../api/client"
import { AppToastProvider } from "../components/ui"
import { TopbarSlotContext, type TopbarSlots } from "../components/topbar-slots"
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
    plugins: vi.fn(),
    debts: vi.fn(),
    debt: vi.fn(),
    transactionLinkCandidates: vi.fn(),
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
  openingBalanceCents: 0,
  balanceCents: 0,
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
  transactionAutoCreated: false,
  additions: [],
  repayments: [],
}

function LocationProbe() {
  const { pathname } = useLocation()
  return <output data-testid="location">{pathname}</output>
}

// 页面名和主操作都住在顶栏插槽里（kiln：Title Authority），裸渲染工作区就看不到它们。
// 这个替身按真实外壳的语义把插槽摆出来：标题是 h1，动作原样渲染。
function TopbarHarness({ children }: { children: ReactNode }) {
  const [slots, setSlots] = useState<TopbarSlots>()
  return <TopbarSlotContext.Provider value={setSlots}>
    {slots?.title ? <h1 className="topbar-title">{slots.title}</h1> : null}
    <div data-testid="topbar-actions">{slots?.actions}</div>
    {children}
  </TopbarSlotContext.Provider>
}

function renderWorkspace(initialEntries = ["/app/debts"]) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return render(<QueryClientProvider client={client}><AppToastProvider><MemoryRouter initialEntries={initialEntries}><LocationProbe /><TopbarHarness><Routes><Route path="/app/debts" element={<DebtWorkspace />} /><Route path="/app/debts/:id" element={<DebtDetailPage />} /></Routes></TopbarHarness></MemoryRouter></AppToastProvider></QueryClientProvider>)
}

function renderWorkspaceWithShell(initialEntries = ["/app/debts"]) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return render(<QueryClientProvider client={client}><AppToastProvider><MemoryRouter initialEntries={initialEntries}><LocationProbe /><Routes><Route path="/app" element={<AppShell />}><Route path="debts" element={<DebtWorkspace />} /><Route path="debts/:id" element={<DebtDetailPage />} /></Route></Routes></MemoryRouter></AppToastProvider></QueryClientProvider>)
}

describe("DebtWorkspace", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.mocked(api.plugins).mockResolvedValue([
      { id: "debts", name: "债务", description: "记录借入、借出及还款进度。", enabled: true, ownsTransactions: true, routePrefixes: ["/api/v1/debts"] },
      { id: "bill-imports", name: "账单导入", description: "从受支持的账单来源导入流水。", enabled: true, ownsTransactions: false, routePrefixes: ["/api/v1/imports"] },
      { id: "auto-categorize", name: "自动分类", description: "按规则为流水自动匹配分类。", enabled: true, ownsTransactions: false, routePrefixes: ["/api/v1/category-rules"] },
    ])
    vi.mocked(api.debts).mockResolvedValue({ items: [debt], page: 1, pageSize: 20, total: 1 })
    vi.mocked(api.debt).mockResolvedValue(debt)
    vi.mocked(api.summary).mockResolvedValue({ lendOutRemainingCents: 80_000, borrowInRemainingCents: 0, netCents: 80_000, overdueCount: 0 })
    vi.mocked(api.counterparties).mockResolvedValue([{ id: "person-1", displayName: "阿青", note: "", archived: false, version: 1, lendOutRemainingCents: 80_000, borrowInRemainingCents: 0, netCents: 80_000, activeDebtCount: 1, overdueCount: 0 }])
    vi.mocked(api.ledgerAccounts).mockResolvedValue([ledgerAccount])
    vi.mocked(api.transactionLinkCandidates).mockResolvedValue([])
  })

  it("renders the financial summary and due-soon row", async () => {
    renderWorkspace()
    expect(screen.getByRole("heading", { name: "债务" })).toBeInTheDocument()
    expect(screen.queryByText("个人往来")).not.toBeInTheDocument()
    expect(screen.queryByRole("heading", { name: "债务管理" })).not.toBeInTheDocument()
    expect(screen.getByTestId("topbar-actions")).toHaveTextContent("新增债务")
    expect(screen.getByRole("button", { name: "新增债务" }).closest(".debt-commandbar")).toBeNull()
    // 命令栏只放控件：汇总条搬到它下面自成一行，留在里面会把这排控件的高度撑乱。
    expect(screen.getByRole("region", { name: "债务汇总" }).closest(".debt-commandbar")).toBeNull()
    expect(screen.getByRole("region", { name: "债务汇总" })).toHaveClass("summary-strip")
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

  it("归档后离开详情页回到列表，不把「找不到该笔债务」摆给用户看", async () => {
    const user = userEvent.setup()
    vi.mocked(api.archiveDebt).mockResolvedValue(undefined as never)
    renderWorkspaceWithShell(["/app/debts/debt-1"])
    await screen.findByRole("heading", { name: "阿青" })

    await user.click(await screen.findByRole("button", { name: "更多债务操作" }))
    await user.click(await screen.findByRole("menuitem", { name: "归档债务" }))
    const confirmDialog = await screen.findByRole("alertdialog")
    await user.click(within(confirmDialog).getByRole("button", { name: "确认归档" }))

    await waitFor(() => expect(api.archiveDebt).toHaveBeenCalled())
    // 详情页对一笔刚归档的债务已经无事可做，留在这里只会展示一个空壳或报错
    await waitFor(() => expect(screen.getByTestId("location")).toHaveTextContent("/app/debts"))
    expect(screen.queryByText("找不到该笔债务")).not.toBeInTheDocument()
  })

  it("删除后同样回到列表", async () => {
    const user = userEvent.setup()
    vi.mocked(api.deleteDebt).mockResolvedValue(undefined as never)
    renderWorkspaceWithShell(["/app/debts/debt-1"])
    await screen.findByRole("heading", { name: "阿青" })

    await user.click(await screen.findByRole("button", { name: "更多债务操作" }))
    await user.click(await screen.findByRole("menuitem", { name: "删除债务" }))
    const confirmDialog = await screen.findByRole("alertdialog")
    await user.click(within(confirmDialog).getByRole("button", { name: "确认删除" }))

    await waitFor(() => expect(api.deleteDebt).toHaveBeenCalled())
    await waitFor(() => expect(screen.getByTestId("location")).toHaveTextContent("/app/debts"))
    expect(screen.queryByText("找不到该笔债务")).not.toBeInTheDocument()
  })

  it("navigates to a dedicated debt URL and returns to the list", async () => {
    const user = userEvent.setup()
    renderWorkspaceWithShell()
    await user.click(await screen.findByRole("row", { name: /阿青 借出/ }))
    const backButton = await screen.findByRole("button", { name: "返回债务列表" })
    expect(backButton.closest(".topbar")).not.toBeNull()
    // 详情路由的标题是这条记录本身：概览卡里的联系人名就是 h1，不再是一句 sr-only 的
    // 「债务详情」——那句话在七条债务上念出来是同一个词。
    expect(screen.getByRole("heading", { level: 1, name: "阿青" })).toHaveClass("detail-contact-name")
    expect(screen.queryByRole("heading", { name: "债务详情" })).not.toBeInTheDocument()
    expect(screen.queryByText("个人往来")).not.toBeInTheDocument()
    expect(screen.getByText("初始借出 ¥1,000.00")).toBeInTheDocument()
    expect(screen.getByText("1 条")).toBeInTheDocument()
    await user.click(screen.getByRole("button", { name: "更多债务操作" }))
    expect(await screen.findByRole("menuitem", { name: "删除债务" })).toBeInTheDocument()
    await user.keyboard("{Escape}")
    expect(screen.getByTestId("location")).toHaveTextContent("/app/debts/debt-1")
    await user.click(backButton)
    expect(await screen.findByRole("heading", { name: "债务" })).toBeInTheDocument()
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
    await user.click(await screen.findByRole("button", { name: "新增债务" }))
    await user.click(screen.getByRole("button", { name: "发生日期" }))
    expect(screen.getByRole("dialog", { name: "选择日期" })).toBeInTheDocument()
    expect(screen.queryByDisplayValue("2026-08-02")).not.toBeInTheDocument()
    await user.click(screen.getByRole("gridcell", { name: "2026-08-03" }))
    expect(screen.getByRole("button", { name: "发生日期" })).toHaveTextContent("2026-08-03")
  })

  it("keeps invalid debt input inside the form", async () => {
    const user = userEvent.setup()
    renderWorkspace()
    await user.click(screen.getByRole("button", { name: "新增债务" }))
    await user.click(screen.getByRole("button", { name: "保存" }))
    expect(await screen.findByText("请输入正确的本金金额")).toBeInTheDocument()
    expect(api.createDebt).not.toHaveBeenCalled()
  })

  it("requires an explicit money account before creating a debt", async () => {
    const user = userEvent.setup()
    vi.mocked(api.createDebt).mockResolvedValue(debt)
    renderWorkspace()

    await user.click(await screen.findByRole("button", { name: "新增债务" }))
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
    }), expect.objectContaining({ idempotencyKey: expect.any(String) })))
  })

  it("creates a cashless debt without choosing an account", async () => {
    const user = userEvent.setup()
    vi.mocked(api.createDebt).mockResolvedValue(debt)
    renderWorkspace()

    await user.click(await screen.findByRole("button", { name: "新增债务" }))
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
    }), expect.objectContaining({ idempotencyKey: expect.any(String) })))
  })

  it("shows a cashless debt as a confirmed payable with no account movement", async () => {
    const user = userEvent.setup()
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
    await user.click(screen.getByRole("button", { name: "操作 2026-08-02 确认应付 ¥1,000.00" }))
    expect(await screen.findByRole("menuitem", { name: "编辑记录" })).toBeInTheDocument()
    expect(screen.queryByRole("menuitem", { name: /关联流水|管理流水/ })).not.toBeInTheDocument()
  })

  it("exposes edit and transaction linking actions on the initial cash movement", async () => {
    const user = userEvent.setup()
    renderWorkspace(["/app/debts/debt-1"])

    await user.click(await screen.findByRole("button", { name: "操作 2026-08-02 初始借出 ¥1,000.00" }))
    expect(await screen.findByRole("menuitem", { name: "编辑记录" })).toBeInTheDocument()
    expect(screen.getByRole("menuitem", { name: "关联流水" })).toBeInTheDocument()
    await user.click(screen.getByRole("menuitem", { name: "编辑记录" }))
    expect(await screen.findByRole("dialog", { name: "编辑债务" })).toBeInTheDocument()
  })

  it("links an existing transaction and can switch back to the automatic transaction", async () => {
    const user = userEvent.setup()
    const candidate = { id: "transaction-1", kind: "expense" as const, amountCents: 100_000, occurredOn: "2026-08-02", note: "借给阿青", account: accountBrief }
    vi.mocked(api.transactionLinkCandidates).mockResolvedValue([candidate])
    vi.mocked(api.updateDebt).mockResolvedValue(debt)
    const firstRender = renderWorkspace(["/app/debts/debt-1"])

    await user.click(await screen.findByRole("button", { name: "操作 2026-08-02 初始借出 ¥1,000.00" }))
    await user.click(screen.getByRole("menuitem", { name: "关联流水" }))
    const linkDialog = await screen.findByRole("dialog", { name: "关联流水" })
    await user.click(within(linkDialog).getByRole("button", { name: "从流水选取" }))
    await user.click((await screen.findByText("借给阿青")).closest("button")!)
    await user.click(within(linkDialog).getByRole("button", { name: "保存" }))

    await waitFor(() => expect(api.transactionLinkCandidates).toHaveBeenCalledWith("debt-1", { amountCents: 100_000 }))
    await waitFor(() => expect(api.updateDebt).toHaveBeenCalledWith("debt-1", { version: 2, accountId: "account-1", originKind: "cash_movement", counterpartyId: "person-1", principalCents: 100_000, occurredOn: "2026-08-02", dueOn: "2026-08-09", note: "朋友借款", transactionId: "transaction-1" }, expect.objectContaining({ idempotencyKey: expect.any(String) })))

    firstRender.unmount()
    vi.mocked(api.debt).mockResolvedValue({ ...debt, transactionId: "transaction-1" })
    renderWorkspace(["/app/debts/debt-1"])
    await user.click(await screen.findByRole("button", { name: "操作 2026-08-02 初始借出 ¥1,000.00" }))
    await user.click(screen.getByRole("menuitem", { name: "管理流水" }))
    const unlinkDialog = await screen.findByRole("dialog", { name: "管理流水" })
    await user.click(within(unlinkDialog).getByRole("button", { name: "使用自动流水" }))
    await user.click(within(unlinkDialog).getByRole("button", { name: "保存" }))
    await waitFor(() => expect(api.updateDebt).toHaveBeenLastCalledWith("debt-1", { version: 2, accountId: "account-1", originKind: "cash_movement", counterpartyId: "person-1", principalCents: 100_000, occurredOn: "2026-08-02", dueOn: "2026-08-09", note: "朋友借款", transactionId: null }, expect.objectContaining({ idempotencyKey: expect.any(String) })))
  })

  it("keeps legacy movements with no structured account readable", async () => {
    const legacyDebt = {
      ...debt,
      account: null,
      additions: [{ id: "legacy-addition", amountCents: 10_000, effectiveOn: "2026-08-04", note: "历史追加", account: null, createdAt: "2026-08-04T09:00:00Z", transactionAutoCreated: false }],
      repayments: [{ id: "legacy-payment", amountCents: 20_000, effectiveOn: "2026-08-03", note: "历史还款", account: null, kind: "payment", reversed: false, reversesEventId: null, createdAt: "2026-08-03T09:00:00Z", transactionAutoCreated: false }],
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
        { id: "addition-later", amountCents: 1_000_000, effectiveOn: "2026-09-07", note: "", account: null, createdAt: "2026-08-02T12:43:18Z", transactionAutoCreated: false },
        { id: "addition-earlier", amountCents: 1_000_000, effectiveOn: "2023-09-11", note: "", account: null, createdAt: "2026-08-02T12:43:34Z", transactionAutoCreated: false },
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
    await waitFor(() => expect(api.updateDebt).toHaveBeenCalledWith("debt-1", expect.objectContaining({ accountId: "account-1" }), expect.objectContaining({ idempotencyKey: expect.any(String) })))
  })

  it("appends in the existing debt direction and merges the activity timeline", async () => {
    const user = userEvent.setup()
    const updatedDebt = {
      ...debt,
      principalCents: 125_000,
      remainingCents: 105_000,
      version: 3,
      additions: [{ id: "addition-1", amountCents: 25_000, effectiveOn: "2026-08-04", note: "又借了一笔", account: accountBrief, createdAt: "2026-08-04T09:00:00Z", transactionAutoCreated: false }],
      repayments: [{ id: "payment-1", amountCents: 20_000, effectiveOn: "2026-08-03", note: "先还一部分", account: accountBrief, kind: "payment", reversed: false, reversesEventId: null, createdAt: "2026-08-03T09:00:00Z", transactionAutoCreated: false }],
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
    }, expect.objectContaining({ idempotencyKey: expect.any(String) })))
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
      additions: [{ id: "addition-1", amountCents: 25_000, effectiveOn: "2026-08-04", note: "旧备注", account: null, createdAt: "2026-08-04T09:00:00Z", transactionId: "auto-addition-1", transactionAutoCreated: true }],
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
    }, expect.objectContaining({ idempotencyKey: expect.any(String) })))
  })

  it("edits an active repayment record but not reversed history", async () => {
    const user = userEvent.setup()
    const payment = { id: "payment-1", amountCents: 20_000, effectiveOn: "2026-08-03", note: "旧还款", account: accountBrief, kind: "payment", reversed: false, reversesEventId: null, createdAt: "2026-08-03T09:00:00Z", transactionId: "auto-payment-1", transactionAutoCreated: true }
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
    }, expect.objectContaining({ idempotencyKey: expect.any(String) })))

  })

  it("does not expose edits for archived, reversed, or reversal records", async () => {
    const reversedPayment = { id: "payment-1", amountCents: 20_000, effectiveOn: "2026-08-03", note: "", account: accountBrief, kind: "payment", reversed: true, reversesEventId: null, createdAt: "2026-08-03T09:00:00Z", transactionAutoCreated: false }
    const reversal = { ...reversedPayment, id: "reversal-1", kind: "reversal", reversed: false, reversesEventId: "payment-1" }
    vi.mocked(api.debt).mockResolvedValue({ ...debt, archived: true, status: "archived", additions: [{ id: "addition-1", amountCents: 1_000, effectiveOn: "2026-08-04", note: "", account: accountBrief, createdAt: "2026-08-04T09:00:00Z", transactionAutoCreated: false }], repayments: [reversedPayment, reversal] })
    renderWorkspace(["/app/debts/debt-1"])

    await screen.findByText("追加借出 ¥10.00")
    expect(screen.queryByRole("button", { name: /^操作 .*初始借出/ })).not.toBeInTheDocument()
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
            debt={{ ...debt, paidCents: 0, additions: [{ id: "addition-1", amountCents: 10_000, effectiveOn: "2026-08-03", note: "", account: accountBrief, createdAt: "2026-08-03T00:00:00Z", transactionAutoCreated: false }] }}
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
    const trigger = await screen.findByRole("button", { name: "新增债务" })
    trigger.focus()
    await user.keyboard("{Enter}")
    expect(await screen.findByRole("dialog", { name: "新增债务" })).toBeInTheDocument()
  })
})
