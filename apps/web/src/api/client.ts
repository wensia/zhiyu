import type {
  Counterparty,
  CreateDebtAdditionInput,
  CreateDebtInput,
  CreateLedgerAccountInput,
  CreateRepaymentInput,
  CreateTransactionInput,
  Debt,
  DebtList,
  LedgerAccount,
  LedgerTransaction,
  ReverseRepaymentInput,
  Summary,
  TransactionList,
  TransactionMonthSummary,
  UpdateDebtInput,
  UpdateDebtAdditionInput,
  UpdateLedgerAccountInput,
  UpdateRepaymentInput,
  UpdateTransactionInput,
  User,
} from "./types"

export class ApiClientError extends Error {
  code: string
  status: number
  fieldErrors?: Record<string, string>

  constructor(status: number, body: { code?: string; message?: string; fieldErrors?: Record<string, string> }) {
    super(body.message || "请求失败")
    this.name = "ApiClientError"
    this.code = body.code || "request_failed"
    this.status = status
    this.fieldErrors = body.fieldErrors
  }
}

// 页面从回环地址来，说明这是 `pnpm tauri dev`：所有请求都要过本地 Vite 的代理，
// 而失败的头号成因就是 Vite 那半边先退出了，只剩 Tauri 窗口指着一个没人监听的端口。
// 页面看着完好，每个请求却都连不上——直说这件事，比让人去猜网络强。
const IS_LOCAL_DEV_PAGE = ["127.0.0.1", "localhost"].includes(location.hostname)
const NETWORK_UNREACHABLE_MESSAGE = IS_LOCAL_DEV_PAGE
  ? "无法连接服务器：本地 Vite 代理可能已退出，请重新运行 pnpm tauri dev"
  : "无法连接服务器，请检查网络后重试"

/**
 * 请求根本没到达服务器：断网、DNS 失败、连接被拒、代理进程没了。
 *
 * 这种情况下 fetch 抛的是 `TypeError`，message 由引擎决定且只有英文——WKWebView
 * 说 "Load failed"，Chrome 说 "Failed to fetch"。界面统一渲染 `error.message`，
 * 于是这句英文会原样贴到用户脸上，既看不懂也指不出问题在哪。这里换成中文，原始
 * 错误挂在 `cause` 上留给控制台。
 *
 * 状态码留 0：语义上「一个响应都没有」，也让 `status === 409` 这类判断自然落空。
 */
export class ApiNetworkError extends ApiClientError {
  constructor(cause: unknown) {
    super(0, { code: "network_unreachable", message: NETWORK_UNREACHABLE_MESSAGE })
    this.name = "ApiNetworkError"
    this.cause = cause
  }
}

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  let response: Response
  try {
    response = await fetch(`/api/v1${path}`, {
      ...init,
      credentials: "include",
      headers: {
        ...(init.body ? { "Content-Type": "application/json" } : {}),
        ...init.headers,
      },
    })
  } catch (cause) {
    throw new ApiNetworkError(cause)
  }
  if (response.status === 204) return undefined as T
  const body = await response.json().catch(() => ({}))
  if (!response.ok) throw new ApiClientError(response.status, body)
  return body as T
}

/**
 * 写请求的幂等键。
 *
 * 服务端靠它认出重复提交（`replay_idempotency`）：同一把键来两次，第二次直接返回
 * 第一次的结果，不会重复记账。所以键必须绑定「这一次用户意图」，而不是「这一次
 * 网络请求」——否则以下场景会记两笔：
 *
 *     用户点保存 ──▶ 服务端写入成功 ──▶ 响应在回程丢了
 *                                          │
 *     前端显示失败 ◀────────────────────────┘
 *     用户再点一次 ──▶ 新键 ──▶ 服务端认不出 ──▶ 第二笔
 *
 * 调用方（见 useIdempotentMutation）在整个重试过程中传同一个 key；不传则退化为
 * 每次现算，只对「确定不会重试」的调用安全。
 */
export type WriteOptions = { idempotencyKey?: string }

const writeHeaders = (options?: WriteOptions) => ({
  "Idempotency-Key": options?.idempotencyKey ?? crypto.randomUUID(),
})

export const api = {
  register: (input: { email: string; password: string; timezone: string }) =>
    request<{ message: string }>("/auth/register", { method: "POST", body: JSON.stringify(input) }),
  verifyEmail: (token: string) =>
    request<{ message: string }>("/auth/verify-email", { method: "POST", body: JSON.stringify({ token }) }),
  resendVerification: (email: string) =>
    request<{ message: string }>("/auth/resend-verification", { method: "POST", body: JSON.stringify({ email }) }),
  login: (email: string, password: string) =>
    request<User>("/auth/login", { method: "POST", body: JSON.stringify({ email, password }) }),
  logout: () => request<{ message: string }>("/auth/logout", { method: "POST" }),
  me: () => request<User>("/auth/me"),
  forgotPassword: (email: string) =>
    request<{ message: string }>("/auth/forgot-password", { method: "POST", body: JSON.stringify({ email }) }),
  resetPassword: (token: string, newPassword: string) =>
    request<{ message: string }>("/auth/reset-password", {
      method: "POST",
      body: JSON.stringify({ token, newPassword }),
    }),
  ledgerAccounts: () => request<LedgerAccount[]>("/ledger-accounts"),
  createLedgerAccount: (input: CreateLedgerAccountInput, options?: WriteOptions) =>
    request<LedgerAccount>("/ledger-accounts", {
      method: "POST",
      headers: writeHeaders(options),
      body: JSON.stringify(input),
    }),
  updateLedgerAccount: (id: string, input: UpdateLedgerAccountInput, options?: WriteOptions) =>
    request<LedgerAccount>(`/ledger-accounts/${id}`, {
      method: "PATCH",
      headers: writeHeaders(options),
      body: JSON.stringify(input),
    }),
  archiveLedgerAccount: (id: string, version: number, options?: WriteOptions) =>
    request<LedgerAccount>(`/ledger-accounts/${id}/archive`, {
      method: "POST",
      headers: writeHeaders(options),
      body: JSON.stringify({ version }),
    }),
  restoreLedgerAccount: (id: string, version: number, options?: WriteOptions) =>
    request<LedgerAccount>(`/ledger-accounts/${id}/restore`, {
      method: "POST",
      headers: writeHeaders(options),
      body: JSON.stringify({ version }),
    }),
  summary: () => request<Summary>("/dashboard/summary"),
  debts: (params: Record<string, string | number | boolean | undefined>) => {
    const query = new URLSearchParams()
    Object.entries(params).forEach(([key, value]) => {
      if (value !== undefined && value !== "") query.set(key, String(value))
    })
    return request<DebtList>(`/debts?${query}`)
  },
  debt: (id: string) => request<Debt>(`/debts/${id}`),
  createDebt: (input: CreateDebtInput, options?: WriteOptions) =>
    request<Debt>("/debts", {
      method: "POST",
      headers: writeHeaders(options),
      body: JSON.stringify(input),
    }),
  updateDebt: (id: string, input: UpdateDebtInput, options?: WriteOptions) =>
    request<Debt>(`/debts/${id}`, {
      method: "PATCH",
      headers: writeHeaders(options),
      body: JSON.stringify(input),
    }),
  archiveDebt: (id: string, version: number, options?: WriteOptions) =>
    request<Debt>(`/debts/${id}/archive`, {
      method: "POST",
      headers: writeHeaders(options),
      body: JSON.stringify({ version }),
    }),
  restoreDebt: (id: string, version: number, options?: WriteOptions) =>
    request<Debt>(`/debts/${id}/restore`, {
      method: "POST",
      headers: writeHeaders(options),
      body: JSON.stringify({ version }),
    }),
  deleteDebt: (id: string, version: number, options?: WriteOptions) =>
    request<void>(`/debts/${id}`, {
      method: "DELETE",
      headers: writeHeaders(options),
      body: JSON.stringify({ version }),
    }),
  createRepayment: (id: string, input: CreateRepaymentInput, options?: WriteOptions) =>
    request<Debt>(`/debts/${id}/repayments`, {
      method: "POST",
      headers: writeHeaders(options),
      body: JSON.stringify(input),
    }),
  createDebtAddition: (id: string, input: CreateDebtAdditionInput, options?: WriteOptions) =>
    request<Debt>(`/debts/${id}/additions`, {
      method: "POST",
      headers: writeHeaders(options),
      body: JSON.stringify(input),
    }),
  updateDebtAddition: (id: string, input: UpdateDebtAdditionInput, options?: WriteOptions) =>
    request<Debt>(`/debt-additions/${id}`, {
      method: "PATCH",
      headers: writeHeaders(options),
      body: JSON.stringify(input),
    }),
  updateRepayment: (id: string, input: UpdateRepaymentInput, options?: WriteOptions) =>
    request<Debt>(`/repayments/${id}`, {
      method: "PATCH",
      headers: writeHeaders(options),
      body: JSON.stringify(input),
    }),
  reverseRepayment: (id: string, input: ReverseRepaymentInput, options?: WriteOptions) =>
    request<Debt>(`/repayments/${id}/reversals`, {
      method: "POST",
      headers: writeHeaders(options),
      body: JSON.stringify(input),
    }),
  counterparties: () => request<Counterparty[]>("/counterparties"),
  transactions: (params: Record<string, string | number | undefined>) => {
    const query = new URLSearchParams()
    Object.entries(params).forEach(([key, value]) => {
      if (value !== undefined && value !== "") query.set(key, String(value))
    })
    return request<TransactionList>(`/transactions?${query}`)
  },
  createTransaction: (input: CreateTransactionInput, options?: WriteOptions) =>
    request<LedgerTransaction>("/transactions", {
      method: "POST",
      headers: writeHeaders(options),
      body: JSON.stringify(input),
    }),
  updateTransaction: (id: string, input: UpdateTransactionInput, options?: WriteOptions) =>
    request<LedgerTransaction>(`/transactions/${id}`, {
      method: "PATCH",
      headers: writeHeaders(options),
      body: JSON.stringify(input),
    }),
  deleteTransaction: (id: string, version: number, options?: WriteOptions) =>
    request<void>(`/transactions/${id}`, {
      method: "DELETE",
      headers: writeHeaders(options),
      body: JSON.stringify({ version }),
    }),
  restoreTransaction: (id: string, version: number, options?: WriteOptions) =>
    request<LedgerTransaction>(`/transactions/${id}/restore`, {
      method: "POST",
      headers: writeHeaders(options),
      body: JSON.stringify({ version }),
    }),
  transactionSummary: (month: string) =>
    request<TransactionMonthSummary>(`/transactions/summary?month=${encodeURIComponent(month)}`),
  transactionCategories: () => request<string[]>("/transactions/categories"),
}
