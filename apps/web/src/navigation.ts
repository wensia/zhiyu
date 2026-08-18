import { BarChart3Icon, CalendarDaysIcon, ListIcon, SettingsIcon, WalletCardsIcon, type LucideIcon } from "lucide-react"

import { pluginNavigationItems } from "./plugins/registry"

export type NavigationItem = {
  path: string
  label: string
  mobileLabel: string
  icon: LucideIcon
  group: string
}

const coreNavigationItems: NavigationItem[] = [
  { path: "/app/calendar", label: "日历", mobileLabel: "日历", icon: CalendarDaysIcon, group: "个人账本" },
  { path: "/app/transactions", label: "流水", mobileLabel: "流水", icon: ListIcon, group: "个人账本" },
  { path: "/app/statistics", label: "统计", mobileLabel: "统计", icon: BarChart3Icon, group: "个人账本" },
  { path: "/app/accounts", label: "账户", mobileLabel: "账户", icon: WalletCardsIcon, group: "个人账本" },
  { path: "/app/settings/plugins", label: "设置", mobileLabel: "设置", icon: SettingsIcon, group: "设置" },
]

export function navigationItemsForPlugins(enabledPluginIds?: ReadonlySet<string>): NavigationItem[] {
  const [settings] = coreNavigationItems.slice(-1)
  return [...pluginNavigationItems(enabledPluginIds), ...coreNavigationItems.slice(0, -1), settings]
}

export const navigationItems = navigationItemsForPlugins()

export function navigationShortcut(index: number) {
  return index >= 0 && index < 9 ? String(index + 1) : undefined
}

export function navigationKey(event: Pick<KeyboardEvent, "code" | "key">) {
  const codeMatch = /^Digit([1-9])$/.exec(event.code)
  if (codeMatch) return codeMatch[1]
  return /^[1-9]$/.test(event.key) ? event.key : undefined
}
