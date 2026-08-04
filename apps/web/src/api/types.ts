import type { components } from "./generated"

export type User = components["schemas"]["UserView"]
export type LedgerAccountType = components["schemas"]["AccountType"]
export type LedgerAccountRef = components["schemas"]["LedgerAccountBrief"]
export type LedgerAccount = components["schemas"]["LedgerAccountView"]
type GeneratedDebt = components["schemas"]["DebtView"]
type GeneratedDebtAddition = GeneratedDebt["additions"][number]
type GeneratedRepayment = GeneratedDebt["repayments"][number]
export type DebtAddition = Omit<GeneratedDebtAddition, "account"> & { account: LedgerAccountRef | null }
export type Repayment = Omit<GeneratedRepayment, "account"> & { account: LedgerAccountRef | null }
export type Debt = Omit<GeneratedDebt, "account" | "additions" | "repayments"> & {
  account: LedgerAccountRef | null
  additions: DebtAddition[]
  repayments: Repayment[]
}
export type DebtList = Omit<components["schemas"]["DebtListResponse"], "items"> & { items: Debt[] }
export type DebtStatus = components["schemas"]["DebtStatus"]
export type DebtOriginKind = components["schemas"]["DebtOriginKind"]
export type Counterparty = components["schemas"]["CounterpartyView"]
export type Summary = components["schemas"]["DashboardSummary"]
export type CreateDebtInput = components["schemas"]["CreateDebtRequest"]
export type CreateDebtAdditionInput = components["schemas"]["CreateDebtAdditionRequest"] & { accountId: string }
export type UpdateDebtInput = components["schemas"]["UpdateDebtRequest"]
export type CreateRepaymentInput = components["schemas"]["CreateRepaymentRequest"] & { accountId: string }
export type UpdateDebtAdditionInput = components["schemas"]["UpdateDebtAdditionRequest"]
export type UpdateRepaymentInput = components["schemas"]["UpdateRepaymentRequest"]
export type ReverseRepaymentInput = components["schemas"]["ReverseRepaymentRequest"]
export type CreateLedgerAccountInput = components["schemas"]["CreateLedgerAccountRequest"]
export type UpdateLedgerAccountInput = components["schemas"]["UpdateLedgerAccountRequest"]
