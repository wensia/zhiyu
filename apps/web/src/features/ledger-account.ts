import type { LedgerAccountType } from "../api/types"

export type LedgerAccountDetails = {
  accountType: LedgerAccountType
  name?: string
  nameSource?: "custom" | "derived"
  bankName?: string | null
  branchName?: string | null
  cardNumber?: string | null
  nickname?: string | null
  phone?: string | null
  email?: string | null
}

export type LedgerAccountDetailItem = {
  label: string
  value: string
}

export const LEDGER_ACCOUNT_TYPE_LABELS: Record<LedgerAccountType, string> = {
  wechat_balance: "微信零钱",
  alipay_balance: "支付宝余额",
  bank_card: "银行卡",
  cash: "现金",
  digital_cny: "数字人民币",
  other: "其他账户",
}

export const LEDGER_ACCOUNT_TYPE_OPTIONS = (Object.entries(LEDGER_ACCOUNT_TYPE_LABELS) as Array<[LedgerAccountType, string]>).map(([value, label]) => ({ value, label }))

export const OTHER_BANK_VALUE = "__other_bank__"

export const BANK_NAME_OPTIONS = [
  "中国工商银行",
  "中国农业银行",
  "中国银行",
  "中国建设银行",
  "交通银行",
  "中国邮政储蓄银行",
  "招商银行",
  "平安银行",
].map((name) => ({ value: name, label: name })).concat({ value: OTHER_BANK_VALUE, label: "其他银行" })

export function ledgerAccountTypeLabel(accountType: LedgerAccountType) {
  return LEDGER_ACCOUNT_TYPE_LABELS[accountType]
}

export function ledgerAccountDisplayLabel(account: { accountType: LedgerAccountType; name: string } | null | undefined) {
  if (!account) return "历史未指定"
  const type = ledgerAccountTypeLabel(account.accountType)
  const name = account.name.trim()
  if (!name || name === type) return type
  return `${type} · ${name}`
}

export function ledgerAccountDetailItems(account: LedgerAccountDetails): LedgerAccountDetailItem[] {
  const derivedName = account.nameSource === "derived" ? account.name?.trim() : ""
  const candidateItems = account.accountType === "bank_card"
    ? [["银行卡号", account.cardNumber], ["银行", account.bankName], ["开户行", account.branchName]]
    : account.accountType === "wechat_balance"
      ? [["昵称", account.nickname], ["手机号", account.phone]]
      : account.accountType === "alipay_balance"
        ? [["昵称", account.nickname], ["手机号", account.phone], ["邮箱", account.email]]
        : []

  return candidateItems.flatMap(([label, rawValue]) => {
    const value = rawValue?.trim()
    return value && value !== derivedName ? [{ label: label || "", value }] : []
  })
}

export function ledgerAccountSearchText(account: LedgerAccountDetails & { name: string; note: string }) {
  const details = ledgerAccountDetailItems({ ...account, nameSource: "custom" }).map(({ label, value }) => `${label} ${value}`).join(" ")
  return `${ledgerAccountTypeLabel(account.accountType)} ${account.accountType} ${account.name} ${details} ${account.note}`
}
