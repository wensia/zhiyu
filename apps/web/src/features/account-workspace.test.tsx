import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { render, screen, waitFor, within } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { beforeEach, describe, expect, it, vi } from "vitest"
import { type ReactNode, useState } from "react"

import { api } from "../api/client"
import { AppToastProvider } from "../components/ui"
import { TopbarSlotContext, type TopbarSlots } from "../components/topbar-slots"
import { AccountWorkspace } from "./account-workspace"
import { OTHER_BANK_VALUE } from "./ledger-account"

vi.mock("../api/client", () => ({
  ApiClientError: class ApiClientError extends Error {
    status: number
    constructor(status: number, body: { message?: string }) {
      super(body.message || "请求失败")
      this.status = status
    }
  },
  api: {
    ledgerAccounts: vi.fn(),
    createLedgerAccount: vi.fn(),
    updateLedgerAccount: vi.fn(),
    archiveLedgerAccount: vi.fn(),
    restoreLedgerAccount: vi.fn(),
  },
}))

const accounts = [
  {
    id: "account-wechat",
    accountType: "wechat_balance" as const,
    name: "日常零钱",
    nameSource: "custom" as const,
    note: "买菜",
    bankName: null,
    branchName: null,
    cardNumber: null,
    nickname: "小余",
    phone: "13800138000",
    email: null,
    archived: false,
    version: 1,
    usageCount: 3,
    openingBalanceCents: 0,
    balanceCents: 0,
    createdAt: "2026-08-02T00:00:00Z",
    updatedAt: "2026-08-02T00:00:00Z",
  },
  {
    id: "account-bank",
    accountType: "bank_card" as const,
    name: "工资卡尾号 1234",
    nameSource: "custom" as const,
    note: "工资",
    bankName: "浦发银行",
    branchName: "北京中关村支行",
    cardNumber: "6222000000001234",
    nickname: null,
    phone: null,
    email: null,
    archived: false,
    version: 1,
    usageCount: 1,
    openingBalanceCents: 0,
    balanceCents: 0,
    createdAt: "2026-08-02T00:00:00Z",
    updatedAt: "2026-08-02T00:00:00Z",
  },
]

// 页面名和主操作住在顶栏插槽里（kiln：Title Authority），裸渲染工作区看不到它们。
function TopbarHarness({ children }: { children: ReactNode }) {
  const [slots, setSlots] = useState<TopbarSlots>()
  return <TopbarSlotContext.Provider value={setSlots}>
    {slots?.title ? <h1 className="topbar-title">{slots.title}</h1> : null}
    <div data-testid="topbar-actions">{slots?.actions}</div>
    {children}
  </TopbarSlotContext.Provider>
}

function renderWorkspace() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })
  return render(<QueryClientProvider client={client}><AppToastProvider><TopbarHarness><AccountWorkspace /></TopbarHarness></AppToastProvider></QueryClientProvider>)
}

describe("AccountWorkspace", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.mocked(api.ledgerAccounts).mockResolvedValue(accounts)
    vi.mocked(api.createLedgerAccount).mockResolvedValue(accounts[0])
    vi.mocked(api.updateLedgerAccount).mockResolvedValue(accounts[1])
  })

  it("shows structured account details and searches by type or detail", async () => {
    const user = userEvent.setup()
    renderWorkspace()

    expect(screen.getByRole("columnheader", { name: "账户类型" })).toBeInTheDocument()
    expect(screen.getByRole("columnheader", { name: "账户" })).toBeInTheDocument()
    expect(screen.getByRole("columnheader", { name: "账户信息" })).toBeInTheDocument()
    await screen.findAllByText("日常零钱")
    expect(screen.getAllByText("微信零钱").length).toBeGreaterThan(0)
    expect(screen.getAllByText("银行卡").length).toBeGreaterThan(0)
    expect(screen.getAllByText("浦发银行").length).toBeGreaterThan(0)
    expect(screen.getAllByText("北京中关村支行").length).toBeGreaterThan(0)
    expect(screen.getAllByText("13800138000").length).toBeGreaterThan(0)

    const search = screen.getByPlaceholderText("搜索账户类型、账户、账户信息或备注")
    await user.type(search, "微信零钱")
    expect(screen.getAllByText("日常零钱").length).toBeGreaterThan(0)
    expect(screen.queryByText("工资卡尾号 1234")).not.toBeInTheDocument()

    await user.clear(search)
    await user.type(search, "中关村支行")
    expect(screen.queryByText("日常零钱")).not.toBeInTheDocument()
    expect(screen.getAllByText("工资卡尾号 1234").length).toBeGreaterThan(0)
  })

  it("requires an account type and submits a WeChat nickname with an empty custom name", async () => {
    const user = userEvent.setup()
    renderWorkspace()

    await user.click(await screen.findByRole("button", { name: "新增账户" }))
    const dialog = screen.getByRole("dialog", { name: "新增账户" })
    await user.click(within(dialog).getByRole("button", { name: "保存" }))
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("请选择账户类型")
    expect(api.createLedgerAccount).not.toHaveBeenCalled()

    await user.click(within(dialog).getByRole("combobox", { name: "账户类型" }))
    await user.click(screen.getByRole("option", { name: "微信零钱" }))
    expect(within(dialog).getByLabelText("昵称（可选）")).toBeInTheDocument()
    expect(within(dialog).getByLabelText("手机号（可选）")).toHaveAttribute("type", "tel")
    expect(within(dialog).getByLabelText("手机号（可选）")).toHaveAttribute("maxlength", "64")
    expect(within(dialog).queryByRole("combobox", { name: "银行（可选）" })).not.toBeInTheDocument()
    expect(within(dialog).queryByLabelText("邮箱（可选）")).not.toBeInTheDocument()
    await user.type(within(dialog).getByLabelText("昵称（可选）"), " 小余 ")
    expect(within(dialog).getByLabelText("自定义名称（可选）")).toHaveValue("")
    await user.click(within(dialog).getByRole("button", { name: "保存" }))

    await waitFor(() => expect(api.createLedgerAccount).toHaveBeenCalledWith({
      accountType: "wechat_balance",
      name: "",
      note: "",
      bankName: null,
      branchName: null,
      cardNumber: null,
      nickname: "小余",
      phone: null,
      email: null,
      openingBalanceCents: 0,
    }, expect.objectContaining({ idempotencyKey: expect.any(String) })))
  })

  it("keeps a name entered before the first account type selection", async () => {
    const user = userEvent.setup()
    renderWorkspace()

    await user.click(await screen.findByRole("button", { name: "新增账户" }))
    const dialog = screen.getByRole("dialog", { name: "新增账户" })
    await user.type(within(dialog).getByLabelText("账户名称"), "日常号")
    await user.click(within(dialog).getByRole("combobox", { name: "账户类型" }))
    await user.click(screen.getByRole("option", { name: "微信零钱" }))

    expect(within(dialog).getByLabelText("自定义名称（可选）")).toHaveValue("日常号")
  })

  it("submits a selected bank and branch, and supports a real name for other banks", async () => {
    const user = userEvent.setup()
    renderWorkspace()

    await user.click(await screen.findByRole("button", { name: "新增账户" }))
    const dialog = screen.getByRole("dialog", { name: "新增账户" })
    await user.click(within(dialog).getByRole("combobox", { name: "账户类型" }))
    await user.click(screen.getByRole("option", { name: "银行卡" }))
    expect(within(dialog).getByLabelText("自定义名称（可选）")).toHaveValue("")
    await user.click(within(dialog).getByRole("combobox", { name: "银行（可选）" }))
    await user.click(screen.getByRole("option", { name: "其他银行" }))
    await user.type(within(dialog).getByLabelText("银行名称（可选）"), " 浦发银行 ")
    await user.type(within(dialog).getByLabelText("开户行（可选）"), " 上海陆家嘴支行 ")
    await user.type(within(dialog).getByLabelText("银行卡号（可选）"), " 6222 0000-0000 1234 ")
    await user.click(within(dialog).getByRole("button", { name: "保存" }))

    await waitFor(() => expect(api.createLedgerAccount).toHaveBeenCalledWith({
      accountType: "bank_card",
      name: "",
      note: "",
      bankName: "浦发银行",
      branchName: "上海陆家嘴支行",
      cardNumber: "6222000000001234",
      nickname: null,
      phone: null,
      email: null,
      openingBalanceCents: 0,
    }, expect.objectContaining({ idempotencyKey: expect.any(String) })))
  })

  it("validates and normalizes Alipay phone and email", async () => {
    const user = userEvent.setup()
    renderWorkspace()

    await user.click(await screen.findByRole("button", { name: "新增账户" }))
    const dialog = screen.getByRole("dialog", { name: "新增账户" })
    await user.click(within(dialog).getByRole("combobox", { name: "账户类型" }))
    await user.click(screen.getByRole("option", { name: "支付宝余额" }))
    expect(within(dialog).getByLabelText("自定义名称（可选）")).toHaveValue("")
    await user.type(within(dialog).getByLabelText("昵称（可选）"), " 小余 ")
    await user.type(within(dialog).getByLabelText("手机号（可选）"), "abc")
    await user.type(within(dialog).getByLabelText("邮箱（可选）"), "not-an-email")
    await user.click(within(dialog).getByRole("button", { name: "保存" }))
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("手机号须包含 7–20 位数字")
    expect(api.createLedgerAccount).not.toHaveBeenCalled()

    await user.clear(within(dialog).getByLabelText("手机号（可选）"))
    await user.type(within(dialog).getByLabelText("手机号（可选）"), "+86 138-0013-8000")
    await user.click(within(dialog).getByRole("button", { name: "保存" }))
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("邮箱格式不正确")

    await user.clear(within(dialog).getByLabelText("邮箱（可选）"))
    await user.type(within(dialog).getByLabelText("邮箱（可选）"), " USER@Example.com ")
    await user.click(within(dialog).getByRole("button", { name: "保存" }))

    await waitFor(() => expect(api.createLedgerAccount).toHaveBeenCalledWith({
      accountType: "alipay_balance",
      name: "",
      note: "",
      bankName: null,
      branchName: null,
      cardNumber: null,
      nickname: "小余",
      phone: "+86 138-0013-8000",
      email: "user@example.com",
      openingBalanceCents: 0,
    }, expect.objectContaining({ idempotencyKey: expect.any(String) })))
  })

  it.each(["现金", "数字人民币", "其他账户"])("still requires a name for %s", async (typeLabel) => {
    const user = userEvent.setup()
    renderWorkspace()

    await user.click(await screen.findByRole("button", { name: "新增账户" }))
    const dialog = screen.getByRole("dialog", { name: "新增账户" })
    await user.click(within(dialog).getByRole("combobox", { name: "账户类型" }))
    await user.click(screen.getByRole("option", { name: typeLabel }))
    expect(within(dialog).getByLabelText("账户名称")).toHaveValue("")
    expect(within(dialog).queryByLabelText("自定义名称（可选）")).not.toBeInTheDocument()

    await user.click(within(dialog).getByRole("button", { name: "保存" }))
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("请输入账户名称")
    expect(api.createLedgerAccount).not.toHaveBeenCalled()
  })

  it("clears fields that become incompatible when the account type changes", async () => {
    const user = userEvent.setup()
    renderWorkspace()

    await user.click(await screen.findByRole("button", { name: "新增账户" }))
    const dialog = screen.getByRole("dialog", { name: "新增账户" })
    const accountType = within(dialog).getByRole("combobox", { name: "账户类型" })
    await user.click(accountType)
    await user.click(screen.getByRole("option", { name: "银行卡" }))
    await user.click(within(dialog).getByRole("combobox", { name: "银行（可选）" }))
    await user.click(screen.getByRole("option", { name: "招商银行" }))
    await user.type(within(dialog).getByLabelText("开户行（可选）"), "北京分行")
    await user.type(within(dialog).getByLabelText("银行卡号（可选）"), "6222000000001234")
    await user.type(within(dialog).getByLabelText("自定义名称（可选）"), "工资卡")

    await user.click(accountType)
    await user.click(screen.getByRole("option", { name: "微信零钱" }))
    expect(within(dialog).queryByRole("combobox", { name: "银行（可选）" })).not.toBeInTheDocument()
    expect(within(dialog).getByLabelText("自定义名称（可选）")).toHaveValue("")
    await user.type(within(dialog).getByLabelText("昵称（可选）"), "小余")
    await user.type(within(dialog).getByLabelText("手机号（可选）"), "13800138000")
    await user.type(within(dialog).getByLabelText("自定义名称（可选）"), "微信日常号")

    await user.click(accountType)
    await user.click(screen.getByRole("option", { name: "银行卡" }))
    expect(within(dialog).getByRole("combobox", { name: "银行（可选）" })).toHaveTextContent("请选择银行")
    expect(within(dialog).getByLabelText("开户行（可选）")).toHaveValue("")
    expect(within(dialog).getByLabelText("银行卡号（可选）")).toHaveValue("")
    expect(within(dialog).getByLabelText("自定义名称（可选）")).toHaveValue("")

    await user.click(accountType)
    await user.click(screen.getByRole("option", { name: "支付宝余额" }))
    expect(within(dialog).getByLabelText("昵称（可选）")).toHaveValue("")
    expect(within(dialog).getByLabelText("手机号（可选）")).toHaveValue("")
    await user.type(within(dialog).getByLabelText("昵称（可选）"), "支付宝小余")
    await user.type(within(dialog).getByLabelText("手机号（可选）"), "13800138000")
    await user.type(within(dialog).getByLabelText("邮箱（可选）"), "alipay@example.com")

    await user.click(accountType)
    await user.click(screen.getByRole("option", { name: "微信零钱" }))
    expect(within(dialog).getByLabelText("昵称（可选）")).toHaveValue("")
    expect(within(dialog).getByLabelText("手机号（可选）")).toHaveValue("")
    expect(within(dialog).queryByLabelText("邮箱（可选）")).not.toBeInTheDocument()

    await user.click(accountType)
    await user.click(screen.getByRole("option", { name: "支付宝余额" }))
    expect(within(dialog).getByLabelText("邮箱（可选）")).toHaveValue("")
  })

  it("prefills and updates structured details when editing", async () => {
    const user = userEvent.setup()
    renderWorkspace()
    await screen.findAllByText("工资卡尾号 1234")

    await user.click(screen.getAllByRole("button", { name: "操作 浦发银行 ····1234 · 工资卡尾号 1234" })[0])
    await user.click(await screen.findByRole("menuitem", { name: "编辑账户" }))
    const dialog = screen.getByRole("dialog", { name: "编辑账户" })
    expect(within(dialog).getByRole("combobox", { name: "银行（可选）" })).toHaveTextContent("其他银行")
    expect(within(dialog).getByLabelText("银行名称（可选）")).toHaveValue("浦发银行")
    expect(within(dialog).getByLabelText("开户行（可选）")).toHaveValue("北京中关村支行")
    expect(within(dialog).getByLabelText("银行卡号（可选）")).toHaveValue("6222000000001234")
    expect(within(dialog).getByLabelText("自定义名称（可选）")).toHaveValue("工资卡尾号 1234")

    await user.clear(within(dialog).getByLabelText("开户行（可选）"))
    await user.type(within(dialog).getByLabelText("开户行（可选）"), "深圳科技园支行")
    await user.click(within(dialog).getByRole("button", { name: "保存" }))

    await waitFor(() => expect(api.updateLedgerAccount).toHaveBeenCalledWith("account-bank", {
      accountType: "bank_card",
      name: "工资卡尾号 1234",
      note: "工资",
      bankName: "浦发银行",
      branchName: "深圳科技园支行",
      cardNumber: "6222000000001234",
      nickname: null,
      phone: null,
      email: null,
      version: 1,
      openingBalanceCents: 0,
    }, expect.objectContaining({ idempotencyKey: expect.any(String) })))
  })

  it("groups every structured account form with the Kiln field rhythm", async () => {
    const user = userEvent.setup()
    renderWorkspace()
    await screen.findAllByText("工资卡尾号 1234")

    await user.click(screen.getAllByRole("button", { name: "操作 浦发银行 ····1234 · 工资卡尾号 1234" })[0])
    await user.click(await screen.findByRole("menuitem", { name: "编辑账户" }))
    const dialog = screen.getByRole("dialog", { name: "编辑账户" })
    expect(dialog).toHaveClass("dialog-wide")
    expect(within(dialog).getByRole("heading", { name: "基本信息" })).toBeInTheDocument()
    expect(within(dialog).getByRole("heading", { name: "银行卡信息" })).toBeInTheDocument()
    expect(within(dialog).getByRole("heading", { name: "补充信息" })).toBeInTheDocument()
    expect(within(dialog).getByLabelText("账户类型").closest(".account-form-row")).toContainElement(within(dialog).getByLabelText("自定义名称（可选）"))
    expect(within(dialog).getByLabelText("开户行（可选）").closest(".account-form-row")).toContainElement(within(dialog).getByLabelText("银行卡号（可选）"))
    expect(within(dialog).getByLabelText("备注（可选）")).toHaveAttribute("rows", "3")

    const accountType = within(dialog).getByRole("combobox", { name: "账户类型" })
    await user.click(accountType)
    await user.click(screen.getByRole("option", { name: "支付宝余额" }))
    expect(within(dialog).getByRole("heading", { name: "支付宝信息" })).toBeInTheDocument()
    expect(within(dialog).getByLabelText("昵称（可选）").closest(".account-form-row")).toContainElement(within(dialog).getByLabelText("手机号（可选）"))
    expect(within(dialog).getByLabelText("邮箱（可选）")).toBeInTheDocument()

    await user.click(accountType)
    await user.click(screen.getByRole("option", { name: "微信零钱" }))
    expect(within(dialog).getByRole("heading", { name: "微信信息" })).toBeInTheDocument()
    expect(within(dialog).queryByLabelText("邮箱（可选）")).not.toBeInTheDocument()
  })

  it("shows a resolved derived name without repeating it in details and edits it as an empty custom name", async () => {
    const user = userEvent.setup()
    const derivedAccount = { ...accounts[1], id: "account-derived-bank", name: "浦发银行", nameSource: "derived" as const }
    vi.mocked(api.ledgerAccounts).mockResolvedValue([derivedAccount])
    vi.mocked(api.updateLedgerAccount).mockResolvedValue(derivedAccount)
    renderWorkspace()

    expect(await screen.findAllByText("浦发银行", { exact: true })).toHaveLength(2)
    expect(screen.getAllByText("北京中关村支行", { exact: true })).toHaveLength(2)
    await user.click(screen.getAllByRole("button", { name: "操作 浦发银行 ····1234" })[0])
    await user.click(await screen.findByRole("menuitem", { name: "编辑账户" }))
    const dialog = screen.getByRole("dialog", { name: "编辑账户" })
    expect(within(dialog).getByLabelText("自定义名称（可选）")).toHaveValue("")
    expect(within(dialog).getByLabelText("银行名称（可选）")).toHaveValue("浦发银行")

    await user.click(within(dialog).getByRole("button", { name: "保存" }))
    await waitFor(() => expect(api.updateLedgerAccount).toHaveBeenCalledWith("account-derived-bank", expect.objectContaining({
      name: "",
      bankName: "浦发银行",
      version: 1,
    }), expect.objectContaining({ idempotencyKey: expect.any(String) })))
  })

  it("preserves a custom bank name that matches the internal other-bank value", async () => {
    const user = userEvent.setup()
    const sentinelAccount = { ...accounts[1], id: "account-sentinel-bank", name: "特殊银行账户", bankName: OTHER_BANK_VALUE }
    vi.mocked(api.ledgerAccounts).mockResolvedValue([sentinelAccount])
    vi.mocked(api.updateLedgerAccount).mockResolvedValue(sentinelAccount)
    renderWorkspace()
    await screen.findAllByText("特殊银行账户")

    await user.click(screen.getAllByRole("button", { name: "操作 银行卡 ····1234 · 特殊银行账户" })[0])
    await user.click(await screen.findByRole("menuitem", { name: "编辑账户" }))
    const dialog = screen.getByRole("dialog", { name: "编辑账户" })
    expect(within(dialog).getByRole("combobox", { name: "银行（可选）" })).toHaveTextContent("其他银行")
    expect(within(dialog).getByLabelText("银行名称（可选）")).toHaveValue(OTHER_BANK_VALUE)

    await user.click(within(dialog).getByRole("button", { name: "保存" }))
    await waitFor(() => expect(api.updateLedgerAccount).toHaveBeenCalledWith("account-sentinel-bank", expect.objectContaining({
      bankName: OTHER_BANK_VALUE,
      version: 1,
    }), expect.objectContaining({ idempotencyKey: expect.any(String) })))
  })
})
