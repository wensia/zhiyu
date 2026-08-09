import { HandCoinsIcon, NotebookPenIcon, WalletCardsIcon, type LucideIcon } from "lucide-react"

type NavigationItem = {
  path: string
  label: string
  mobileLabel: string
  icon: LucideIcon
  group: string
}

export const navigationItems: NavigationItem[] = [
  { path: "/app/debts", label: "债务管理", mobileLabel: "债务", icon: HandCoinsIcon, group: "个人账本" },
  { path: "/app/transactions", label: "记账", mobileLabel: "记账", icon: NotebookPenIcon, group: "个人账本" },
  { path: "/app/accounts", label: "账户管理", mobileLabel: "账户", icon: WalletCardsIcon, group: "个人账本" },
]

export function navigationShortcut(index: number) {
  return index >= 0 && index < 9 ? String(index + 1) : undefined
}

export function navigationKey(event: Pick<KeyboardEvent, "code" | "key">) {
  const codeMatch = /^Digit([1-9])$/.exec(event.code)
  if (codeMatch) return codeMatch[1]
  return /^[1-9]$/.test(event.key) ? event.key : undefined
}
