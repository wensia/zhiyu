import { HandCoinsIcon } from "lucide-react"
import { createElement, type ReactNode } from "react"

import type { DashboardWidget } from "../api/types"
import { DebtsOverviewWidget } from "../features/statistics/widgets"
import type { NavigationItem } from "../navigation"

export type PluginRegistration = {
  id: string
  name: string
  description: string
  ownsTransactions: boolean
  routePrefixes: string[]
  navigationItems?: NavigationItem[]
  linkHref?: (link: { kind: string; refId: string }) => string | undefined
  linkLabel?: (link: { kind: string; refId: string }) => string
  widgets?: PluginWidgetRegistration[]
  renderWidget?: (type: string, config: unknown, context: PluginWidgetRenderContext) => ReactNode
}

export type PluginWidgetRenderContext = {
  month: Date
  widget: DashboardWidget
}

export type PluginWidgetRegistration = {
  id: string
  name: string
  description: string
  defaultW: number
  defaultH: number
  minW: number
  minH: number
}

export const pluginRegistry: PluginRegistration[] = [
  {
    id: "debts",
    name: "债务",
    description: "记录借入、借出及还款进度。",
    ownsTransactions: true,
    routePrefixes: ["/app/debts"],
    linkHref: ({ refId }) => `/app/debts/${refId}`,
    linkLabel: () => "债务往来",
    widgets: [
      {
        id: "overview",
        name: "谁欠我多少",
        description: "汇总当前债务余额。",
        defaultW: 4,
        defaultH: 3,
        minW: 3,
        minH: 2,
      },
    ],
    renderWidget: (type, _config, context) => type === "overview"
      ? createElement(DebtsOverviewWidget, { widget: context.widget })
      : undefined,
    navigationItems: [
      { path: "/app/debts", label: "债务", mobileLabel: "债务", icon: HandCoinsIcon, group: "个人账本" },
    ],
  },
  {
    id: "bill-imports",
    name: "账单导入",
    description: "从受支持的账单来源导入流水。",
    ownsTransactions: false,
    routePrefixes: ["/app/transactions/imports"],
    linkHref: ({ refId }) => `/app/transactions/imports/${refId}`,
    linkLabel: () => "账单导入",
    widgets: [],
  },
  {
    id: "auto-categorize",
    name: "自动分类",
    description: "按规则为流水自动匹配分类。",
    ownsTransactions: false,
    routePrefixes: [],
    widgets: [],
  },
]

export function pluginNavigationItems(enabledPluginIds?: ReadonlySet<string>) {
  return pluginRegistry.flatMap((plugin) => (
    enabledPluginIds && !enabledPluginIds.has(plugin.id) ? [] : plugin.navigationItems ?? []
  ))
}
