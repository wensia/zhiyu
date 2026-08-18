import type { components } from "./generated"

export type User = components["schemas"]["UserView"]
export type Plugin = components["schemas"]["PluginView"]
export type UpdatePluginResult = components["schemas"]["UpdatePluginResponse"]
export type Dashboard = components["schemas"]["DashboardView"]
export type DashboardWidget = components["schemas"]["DashboardWidgetView"]
export type CreateDashboardInput = components["schemas"]["CreateDashboardRequest"]
export type UpdateDashboardInput = components["schemas"]["UpdateDashboardRequest"]
export type DashboardWidgetInput = components["schemas"]["DashboardWidgetInput"]
export type WidgetTypes = components["schemas"]["WidgetTypesResponse"]
export type StatisticsAggregateItem = components["schemas"]["AggregateItem"]
export type StatisticsAggregateParams = {
  from: string
  to: string
  groupBy: "day" | "month" | "category" | "account"
  accountId?: string
  categoryId?: string
  kind?: "income" | "expense"
}
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
export type CreateDebtAdditionInput = components["schemas"]["CreateDebtAdditionRequest"]
export type UpdateDebtInput = components["schemas"]["UpdateDebtRequest"]
export type CreateRepaymentInput = components["schemas"]["CreateRepaymentRequest"]
export type UpdateDebtAdditionInput = components["schemas"]["UpdateDebtAdditionRequest"]
export type UpdateRepaymentInput = components["schemas"]["UpdateRepaymentRequest"]
export type ReverseRepaymentInput = components["schemas"]["ReverseRepaymentRequest"]
export type CreateLedgerAccountInput = components["schemas"]["CreateLedgerAccountRequest"]
export type UpdateLedgerAccountInput = components["schemas"]["UpdateLedgerAccountRequest"]
export type TransactionKind = components["schemas"]["TransactionKind"]
export type PnlScope = components["schemas"]["PnlScope"]
type GeneratedTransaction = components["schemas"]["LedgerTransactionView"]
export type LedgerTransaction = Omit<GeneratedTransaction, "account"> & {
  account: LedgerAccountRef | null
}
export type TransactionLinkCandidate = Omit<components["schemas"]["TransactionLinkCandidate"], "account"> & { account: LedgerAccountRef | null }
export type TransactionList = Omit<components["schemas"]["TransactionListResponse"], "items"> & {
  items: LedgerTransaction[]
}
export type TransactionDaySummary = components["schemas"]["TransactionDaySummary"]
export type TransactionCategorySummary = components["schemas"]["TransactionCategorySummary"]
export type TransactionMonthSummary = components["schemas"]["TransactionMonthSummary"]
export type CreateTransactionInput = components["schemas"]["CreateTransactionRequest"]
export type UpdateTransactionInput = components["schemas"]["UpdateTransactionRequest"]
export type Category = components["schemas"]["CategoryView"]
export type CreateCategoryInput = components["schemas"]["CreateCategoryRequest"]
export type CategoryRule = components["schemas"]["CategoryRuleView"]
export type CreateCategoryRuleInput = components["schemas"]["CreateCategoryRuleRequest"]
export type RecategorizeResult = components["schemas"]["RecategorizeResponse"]
export type ImportBatch = components["schemas"]["ImportBatchListItem"]
export type ImportList = components["schemas"]["ImportListResponse"]
export type ImportDetail = components["schemas"]["ImportDetailResponse"]
export type ImportRecord = components["schemas"]["ImportRecordView"]
export type ImportSummary = components["schemas"]["ImportSummary"]
export type ImportSummaryItem = components["schemas"]["ImportSummaryItem"]
export type ImportIssue = components["schemas"]["UnknownIssue"]
export type CommitImportResult = components["schemas"]["CommitImportResponse"]
export type DiscardImportResult = components["schemas"]["DiscardImportResponse"]
export type CommitImportInput = components["schemas"]["CommitImportRequest"]
export type BindImportAccountInput = components["schemas"]["BindImportAccountRequest"]
export type BindImportAccountResult = components["schemas"]["BindImportAccountResponse"]
export type ImportPayMethod = components["schemas"]["ImportPayMethodSummary"]
export type UpsertImportAccountMappingInput = components["schemas"]["UpsertImportAccountMappingRequest"]
export type ImportAccountMapping = components["schemas"]["ImportAccountMappingResponse"]
export type DuplicateSuspicion = components["schemas"]["DuplicateSuspicionView"]
export type DuplicateSuspicionList = components["schemas"]["DuplicateSuspicionListResponse"]
export type UpdateDuplicateSuspicionInput = components["schemas"]["UpdateDuplicateSuspicionRequest"]

export type ImportChannel = "alipay" | "wechat"
export type UploadImportInput = { file: File; channel?: ImportChannel }
export type ImportListParams = { page?: number; pageSize?: number }
export type ImportDetailParams = ImportListParams & { disposition?: string; direction?: string }
export type DuplicateSuspicionListParams = { page?: number; pageSize?: number }
