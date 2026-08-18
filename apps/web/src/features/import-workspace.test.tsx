import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { render, screen, waitFor, within } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { useState, type ComponentType } from "react"
import { MemoryRouter, Route, Routes } from "react-router-dom"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { api } from "../api/client"
import type { CommitImportResult, ImportDetail } from "../api/types"
import { AppToastProvider } from "../components/ui"
import { TopbarSlotContext, type TopbarSlots } from "../components/topbar-slots"
import { ImportDetailWorkspace, ImportListWorkspace } from "./import-workspace"

vi.mock("../api/client", () => ({
  ApiClientError: class ApiClientError extends Error { code = "request_failed"; status = 400 },
  api: { imports: vi.fn(), importDetail: vi.fn(), uploadImport: vi.fn(), commitImport: vi.fn(), discardImport: vi.fn(), ledgerAccounts: vi.fn(), bindImportAccount: vi.fn(), upsertImportAccountMapping: vi.fn() },
}))

const item = (count: number, amountCents = count * 100) => ({ count, amountCents })
const detail: ImportDetail = {
  id: "batch-fictional", status: "preview", channel: "alipay", parserVersion: 3,
  fileName: "虚构账单.csv", periodStart: "2026-07-01", periodEnd: "2026-07-31", totalCount: 8,
  committedAt: null, createdAt: "2026-08-11T00:00:00Z", filteredCount: 1, page: 1, pageSize: 20,
  previousCommittedBatchId: "old-fictional", previousCommittedAt: "2026-08-10T00:00:00Z", issues: [], accountId: null,
  payMethods: [{ payMethod: "虚构余额", count: 3, accountId: null }, { payMethod: "虚构卡", count: 2, accountId: "account-card" }],
  summary: { importExpense: item(1, 1234), importIncome: item(1, 5000), pending: item(1), neutral: item(1), closed: item(1), zeroAmount: item(1, 0), unknown: item(1), duplicate: item(1) },
  records: [{ id: "record-fictional", rowIndex: 1, externalId: "fictional-order-001", merchantOrderId: "fictional-merchant-001", occurredAt: "2026-07-01T12:00:00+08:00", occurredOn: "2026-07-01", direction: "expense", amountCents: 1234, channelCategory: "餐饮", counterparty: "虚构小馆", product: "虚构午餐", payMethod: "虚构余额", channelStatus: "交易成功", sourceNote: "", disposition: "import", outcome: "will_import", transactionId: null }],
}

function makeCommitImportResult(overrides: Partial<CommitImportResult> = {}): CommitImportResult {
  return {
    id: detail.id,
    status: "committed",
    importedCount: 1,
    duplicateCount: 0,
    committedAt: "2026-08-11T01:00:00Z",
    diagnostics: [],
    summary: detail.summary,
    ...overrides,
  }
}

function Harness({ Workspace }: { Workspace: ComponentType }) {
  const [slots, setSlots] = useState<TopbarSlots>()
  return <TopbarSlotContext.Provider value={setSlots}><div data-testid="topbar">{slots?.actions}</div><Workspace /></TopbarSlotContext.Provider>
}

function renderAt(path: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
  return render(<MemoryRouter initialEntries={[path]}><QueryClientProvider client={client}><AppToastProvider><Routes><Route path="/app/transactions/imports" element={<Harness Workspace={ImportListWorkspace} />} /><Route path="/app/transactions/imports/:id" element={<Harness Workspace={ImportDetailWorkspace} />} /></Routes></AppToastProvider></QueryClientProvider></MemoryRouter>)
}

describe("import workspace", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.mocked(api.imports).mockResolvedValue({ items: [], page: 1, pageSize: 20, total: 0 })
    vi.mocked(api.importDetail).mockResolvedValue(detail)
    vi.mocked(api.ledgerAccounts).mockResolvedValue([])
  })

  it("uploads the chosen file through the FormData action and opens its detail", async () => {
    vi.mocked(api.uploadImport).mockResolvedValue(detail)
    const user = userEvent.setup()
    renderAt("/app/transactions/imports")
    await user.click(await screen.findByRole("button", { name: "选择文件" }))
    const file = new File(["fictional fixture"], "虚构账单.csv", { type: "text/csv" })
    await user.upload(screen.getByLabelText("账单文件"), file)
    await user.click(screen.getByRole("button", { name: "上传并预览" }))
    await waitFor(() => expect(api.uploadImport).toHaveBeenCalledWith({ file }, expect.any(Object)))
    expect(await screen.findByText("虚构账单.csv")).toBeInTheDocument()
  })

  it("renders required copy, history hash, responsive records, and preview actions", async () => {
    renderAt("/app/transactions/imports/batch-fictional")
    expect(await screen.findByText("源账单尚未完成，本批次暂不计入收支；完成后重新导出上传，新批次重新判断。")).toBeInTheDocument()
    expect(screen.getByText(/账户余额会包含导入流水，仅供参考/)).toBeInTheDocument()
    expect(screen.getByText(/历史同 hash/)).toBeInTheDocument()
    expect(screen.getAllByText("虚构小馆")).toHaveLength(2)
    expect(screen.getByTestId("topbar")).toHaveTextContent("确认入账")
    expect(screen.getByTestId("topbar")).toHaveTextContent("放弃")
  })

  it("preselects account attribution and sends the current choices on commit", async () => {
    vi.mocked(api.importDetail).mockResolvedValue({ ...detail, accountId: "account-card", payMethods: detail.payMethods.map((item) => ({ ...item, accountId: "account-card" })) })
    vi.mocked(api.ledgerAccounts).mockResolvedValue([{ id: "account-card", name: "招商银行 4444", archived: false }] as never)
    vi.mocked(api.commitImport).mockResolvedValue(makeCommitImportResult())
    const user = userEvent.setup()
    renderAt("/app/transactions/imports/batch-fictional")
    expect(await screen.findByRole("region", { name: "账户" })).toHaveTextContent("本批账单绑定到")
    expect(screen.getByRole("combobox", { name: "本批账单绑定账户" })).toHaveTextContent("招商银行 4444")
    await user.click(screen.getByRole("button", { name: "确认入账" }))
    await user.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "确认入账" }))
    await waitFor(() => expect(api.commitImport).toHaveBeenCalledWith("batch-fictional", { accountId: "account-card" }, expect.any(Object)))
  })

  it("renders payment methods, marks unbound values, and saves a mapping", async () => {
    vi.mocked(api.ledgerAccounts).mockResolvedValue([{ id: "account-card", name: "招商银行 4444", archived: false }] as never)
    vi.mocked(api.upsertImportAccountMapping).mockResolvedValue({ sourceChannel: "alipay", payMethod: "虚构余额", accountId: "account-card" })
    const user = userEvent.setup()
    renderAt("/app/transactions/imports/batch-fictional")
    const mappings = await screen.findByRole("region", { name: "支付方式映射" })
    expect(mappings).toHaveTextContent("虚构余额")
    expect(mappings).toHaveTextContent("3 行")
    expect(mappings).toHaveTextContent("1 项未绑定")
    expect(within(mappings).getAllByText("未绑定").length).toBeGreaterThan(0)
    await user.click(within(mappings).getByRole("combobox", { name: "支付方式 虚构余额 绑定账户" }))
    await user.click(screen.getByRole("option", { name: /招商银行 4444/ }))
    await waitFor(() => expect(api.upsertImportAccountMapping).toHaveBeenCalledWith({ sourceChannel: "alipay", payMethod: "虚构余额", accountId: "account-card" }, expect.any(Object)))
  })

  it("keeps the commit diagnostic visible when a payment method mapping is missing", async () => {
    vi.mocked(api.importDetail).mockResolvedValue({ ...detail, payMethods: detail.payMethods.map((item) => ({ ...item, accountId: "account-card" })) })
    vi.mocked(api.commitImport).mockRejectedValue(new Error("支付方式“虚构余额”缺少账户映射"))
    const user = userEvent.setup()
    renderAt("/app/transactions/imports/batch-fictional")
    await user.click(await screen.findByRole("button", { name: "确认入账" }))
    await user.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "确认入账" }))
    expect(await screen.findByRole("alert")).toHaveTextContent("确认入账失败：支付方式“虚构余额”缺少账户映射")
  })

  it("filters from summary cards, syncs tabs, clears direction, and toggles back to all", async () => {
    const user = userEvent.setup()
    renderAt("/app/transactions/imports/batch-fictional")
    const expenseCard = await screen.findByRole("button", { name: "按导入支出筛选汇总记录" })
    expect(expenseCard).toHaveAttribute("aria-pressed", "false")

    await user.click(expenseCard)
    await waitFor(() => expect(api.importDetail).toHaveBeenLastCalledWith("batch-fictional", expect.objectContaining({ disposition: "import", direction: "expense" })))
    expect(await screen.findByRole("button", { name: "按导入支出筛选汇总记录" })).toHaveAttribute("aria-pressed", "true")
    expect(screen.getByRole("tab", { name: "待入账" })).toHaveAttribute("data-state", "active")

    await user.click(screen.getByRole("tab", { name: "未完成" }))
    await waitFor(() => expect(api.importDetail).toHaveBeenLastCalledWith("batch-fictional", expect.objectContaining({ disposition: "pending", direction: undefined })))
    expect(await screen.findByRole("button", { name: "按导入支出筛选汇总记录" })).toHaveAttribute("aria-pressed", "false")

    const pendingCard = screen.getByRole("button", { name: "按未完成筛选汇总记录" })
    await user.click(pendingCard)
    expect(screen.getByRole("tab", { name: "全部" })).toHaveAttribute("data-state", "active")
    expect(pendingCard).toHaveAttribute("aria-pressed", "false")
  })

  it("keeps zero-count summary cards operable", async () => {
    vi.mocked(api.importDetail).mockResolvedValue({ ...detail, summary: { ...detail.summary, zeroAmount: item(0, 0) }, records: [], filteredCount: 0 })
    const user = userEvent.setup()
    renderAt("/app/transactions/imports/batch-fictional")
    const zeroCard = await screen.findByRole("button", { name: "按零金额筛选汇总记录" })
    expect(zeroCard).toBeEnabled()
    await user.click(zeroCard)
    await waitFor(() => expect(api.importDetail).toHaveBeenLastCalledWith("batch-fictional", expect.objectContaining({ disposition: "zero_amount", direction: undefined })))
    expect(await screen.findByRole("button", { name: "按零金额筛选汇总记录" })).toHaveAttribute("aria-pressed", "true")
  })

  it("blocks commit for blocked batches and exposes only discard", async () => {
    vi.mocked(api.importDetail).mockResolvedValue({ ...detail, status: "blocked" })
    renderAt("/app/transactions/imports/batch-fictional")
    expect((await screen.findAllByText(/本批次已阻止确认/)).length).toBeGreaterThan(0)
    expect(screen.getByTestId("topbar")).toHaveTextContent("放弃")
    expect(screen.getByTestId("topbar")).not.toHaveTextContent("确认入账")
  })

  it.each([["committed", "撤销导入"], ["discarded", ""]])("controls actions for %s status", async (status, label) => {
    vi.mocked(api.importDetail).mockResolvedValue({ ...detail, status })
    renderAt("/app/transactions/imports/batch-fictional")
    await screen.findByText("虚构账单.csv")
    if (label) {
      expect(screen.getByTestId("topbar")).toHaveTextContent(label)
    } else {
      expect(screen.getByTestId("topbar")).toBeEmptyDOMElement()
    }
  })

  it("reports retained edited or archived rows after undo without toast undo action", async () => {
    vi.mocked(api.importDetail).mockResolvedValue({ ...detail, status: "committed" })
    vi.mocked(api.discardImport).mockResolvedValue({ id: detail.id, status: "discarded", deletedCount: 2, retainedModifiedCount: 3 })
    const user = userEvent.setup()
    renderAt("/app/transactions/imports/batch-fictional")
    await user.click(await screen.findByRole("button", { name: "撤销导入" }))
    await user.click(screen.getByRole("button", { name: "确认撤销" }))
    expect(await screen.findByText("仍保留 3 条用户已编辑或归档的交易。")).toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "撤销" })).not.toBeInTheDocument()
  })
})
