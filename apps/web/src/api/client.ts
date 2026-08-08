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

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const response = await fetch(`/api/v1${path}`, {
    ...init,
    credentials: "include",
    headers: {
      ...(init.body ? { "Content-Type": "application/json" } : {}),
      ...init.headers,
    },
  })
  if (response.status === 204) return undefined as T
  const body = await response.json().catch(() => ({}))
  if (!response.ok) throw new ApiClientError(response.status, body)
  return body as T
}

const idempotencyKey = () => crypto.randomUUID()

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
  createLedgerAccount: (input: CreateLedgerAccountInput) =>
    request<LedgerAccount>("/ledger-accounts", {
      method: "POST",
      headers: { "Idempotency-Key": idempotencyKey() },
      body: JSON.stringify(input),
    }),
  updateLedgerAccount: (id: string, input: UpdateLedgerAccountInput) =>
    request<LedgerAccount>(`/ledger-accounts/${id}`, {
      method: "PATCH",
      headers: { "Idempotency-Key": idempotencyKey() },
      body: JSON.stringify(input),
    }),
  archiveLedgerAccount: (id: string, version: number) =>
    request<LedgerAccount>(`/ledger-accounts/${id}/archive`, {
      method: "POST",
      headers: { "Idempotency-Key": idempotencyKey() },
      body: JSON.stringify({ version }),
    }),
  restoreLedgerAccount: (id: string, version: number) =>
    request<LedgerAccount>(`/ledger-accounts/${id}/restore`, {
      method: "POST",
      headers: { "Idempotency-Key": idempotencyKey() },
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
  createDebt: (input: CreateDebtInput) =>
    request<Debt>("/debts", {
      method: "POST",
      headers: { "Idempotency-Key": idempotencyKey() },
      body: JSON.stringify(input),
    }),
  updateDebt: (id: string, input: UpdateDebtInput) =>
    request<Debt>(`/debts/${id}`, {
      method: "PATCH",
      headers: { "Idempotency-Key": idempotencyKey() },
      body: JSON.stringify(input),
    }),
  archiveDebt: (id: string, version: number) =>
    request<Debt>(`/debts/${id}/archive`, {
      method: "POST",
      headers: { "Idempotency-Key": idempotencyKey() },
      body: JSON.stringify({ version }),
    }),
  restoreDebt: (id: string, version: number) =>
    request<Debt>(`/debts/${id}/restore`, {
      method: "POST",
      headers: { "Idempotency-Key": idempotencyKey() },
      body: JSON.stringify({ version }),
    }),
  deleteDebt: (id: string, version: number) =>
    request<void>(`/debts/${id}`, {
      method: "DELETE",
      headers: { "Idempotency-Key": idempotencyKey() },
      body: JSON.stringify({ version }),
    }),
  createRepayment: (id: string, input: CreateRepaymentInput) =>
    request<Debt>(`/debts/${id}/repayments`, {
      method: "POST",
      headers: { "Idempotency-Key": idempotencyKey() },
      body: JSON.stringify(input),
    }),
  createDebtAddition: (id: string, input: CreateDebtAdditionInput) =>
    request<Debt>(`/debts/${id}/additions`, {
      method: "POST",
      headers: { "Idempotency-Key": idempotencyKey() },
      body: JSON.stringify(input),
    }),
  updateDebtAddition: (id: string, input: UpdateDebtAdditionInput) =>
    request<Debt>(`/debt-additions/${id}`, {
      method: "PATCH",
      headers: { "Idempotency-Key": idempotencyKey() },
      body: JSON.stringify(input),
    }),
  updateRepayment: (id: string, input: UpdateRepaymentInput) =>
    request<Debt>(`/repayments/${id}`, {
      method: "PATCH",
      headers: { "Idempotency-Key": idempotencyKey() },
      body: JSON.stringify(input),
    }),
  reverseRepayment: (id: string, input: ReverseRepaymentInput) =>
    request<Debt>(`/repayments/${id}/reversals`, {
      method: "POST",
      headers: { "Idempotency-Key": idempotencyKey() },
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
  createTransaction: (input: CreateTransactionInput) =>
    request<LedgerTransaction>("/transactions", {
      method: "POST",
      headers: { "Idempotency-Key": idempotencyKey() },
      body: JSON.stringify(input),
    }),
  updateTransaction: (id: string, input: UpdateTransactionInput) =>
    request<LedgerTransaction>(`/transactions/${id}`, {
      method: "PATCH",
      headers: { "Idempotency-Key": idempotencyKey() },
      body: JSON.stringify(input),
    }),
  deleteTransaction: (id: string, version: number) =>
    request<void>(`/transactions/${id}`, {
      method: "DELETE",
      headers: { "Idempotency-Key": idempotencyKey() },
      body: JSON.stringify({ version }),
    }),
  restoreTransaction: (id: string, version: number) =>
    request<LedgerTransaction>(`/transactions/${id}/restore`, {
      method: "POST",
      headers: { "Idempotency-Key": idempotencyKey() },
      body: JSON.stringify({ version }),
    }),
  transactionSummary: (month: string) =>
    request<TransactionMonthSummary>(`/transactions/summary?month=${encodeURIComponent(month)}`),
  transactionCategories: () => request<string[]>("/transactions/categories"),
}
