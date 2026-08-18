import { useQuery } from "@tanstack/react-query"
import { Link } from "react-router-dom"

import { api } from "../../api/client"
import type { DashboardWidget, StatisticsAggregateItem } from "../../api/types"
import { ledgerAccountDisplayLabel } from "../ledger-account"
import { useStatisticsPeriod } from "./period"
import { WidgetBodyState } from "./shared"
import { accountsUnarchivedOnly, categoryKind, monthKeyOf, yuan } from "./utils"

const CHART_COLORS = ["var(--chart-2)", "var(--chart-1)", "var(--chart-3)", "var(--chart-4)", "var(--chart-5)"]

function TrendChart({ data, month }: { data: StatisticsAggregateItem[]; month: Date }) {
  const width = 640
  const height = 220
  const pad = { top: 12, right: 12, bottom: 24, left: 12 }
  const daysInMonth = new Date(month.getFullYear(), month.getMonth() + 1, 0).getDate()
  const byDay = new Map(data.map((item) => [item.key, item]))
  const series = Array.from({ length: daysInMonth }, (_, index) => {
    const day = index + 1
    const item = byDay.get(`${monthKeyOf(month)}-${String(day).padStart(2, "0")}`)
    return { day, income: item?.incomeCents ?? 0, expense: item?.expenseCents ?? 0 }
  })
  const plotWidth = width - pad.left - pad.right
  const plotHeight = height - pad.top - pad.bottom
  const maxBar = Math.max(...series.map((item) => Math.max(item.income, item.expense)), 1)
  let cumulative = 0
  const cumulativePoints = series.map((item) => {
    cumulative += item.income - item.expense
    return cumulative
  })
  const maxCumulative = Math.max(...cumulativePoints.map((value) => Math.abs(value)), 1)
  const slot = plotWidth / daysInMonth
  const barWidth = Math.max(2, (slot - 4) / 2)
  const barTop = (value: number) => pad.top + plotHeight * (1 - value / maxBar)
  const lineY = (value: number) => pad.top + (plotHeight / 2) * (1 - value / maxCumulative)
  const centerX = (index: number) => pad.left + slot * index + slot / 2
  const linePath = cumulativePoints.map((value, index) => (
    `${index === 0 ? "M" : "L"}${centerX(index).toFixed(1)},${lineY(value).toFixed(1)}`
  )).join(" ")
  const labelDays = new Set([1, 5, 10, 15, 20, 25, daysInMonth])
  return (
    <div aria-label="每日趋势" className="statistics-trend-chart">
      <svg aria-label="当月每日收支柱状图与累计结余折线" role="img" viewBox={`0 0 ${width} ${height}`}>
        {[0.25, 0.5, 0.75].map((ratio) => (
          <line className="statistics-chart-grid-line" key={ratio} x1={pad.left} x2={width - pad.right} y1={pad.top + plotHeight * ratio} y2={pad.top + plotHeight * ratio} />
        ))}
        <line className="statistics-chart-zero-line" x1={pad.left} x2={width - pad.right} y1={lineY(0)} y2={lineY(0)} />
        {series.map((item, index) => (
          <g key={item.day}>
            {item.income ? <rect className="statistics-chart-income" height={Math.max(1, pad.top + plotHeight - barTop(item.income))} width={barWidth} x={centerX(index) - barWidth - 1} y={barTop(item.income)} /> : null}
            {item.expense ? <rect className="statistics-chart-expense" height={Math.max(1, pad.top + plotHeight - barTop(item.expense))} width={barWidth} x={centerX(index) + 1} y={barTop(item.expense)} /> : null}
            {labelDays.has(item.day) ? <text className="statistics-chart-label" textAnchor="middle" x={centerX(index)} y={height - 7}>{item.day}</text> : null}
          </g>
        ))}
        <path className="statistics-chart-net-line" d={linePath} />
      </svg>
      <div className="statistics-chart-legend">
        <span><i className="statistics-legend-income" />收入</span>
        <span><i className="statistics-legend-expense" />支出</span>
        <span><i className="statistics-legend-net" />累计结余</span>
      </div>
    </div>
  )
}

export function IncomeExpenseTrendWidget() {
  const { month, monthKey, from, to } = useStatisticsPeriod()
  const query = useQuery({
    queryKey: ["statistics-aggregate", "day", monthKey],
    queryFn: () => api.statisticsAggregate({ from, to, groupBy: "day" }),
  })
  return (
    <WidgetBodyState empty={!query.data?.some((item) => item.incomeCents || item.expenseCents)} error={query.error} loading={query.isLoading} onRetry={() => void query.refetch()}>
      <TrendChart data={query.data ?? []} month={month} />
    </WidgetBodyState>
  )
}

function CategoryShare({ data, kind }: { data: StatisticsAggregateItem[]; kind: "expense" | "income" }) {
  const amount = (item: StatisticsAggregateItem) => kind === "expense" ? item.expenseCents : item.incomeCents
  const entries = data.filter((item) => amount(item) > 0).sort((a, b) => amount(b) - amount(a))
  const total = entries.reduce((sum, item) => sum + amount(item), 0)
  const head = entries.slice(0, 8)
  const tail = entries.slice(8)
  const rows = tail.length ? [
    ...head,
    { key: "other", label: "其他", incomeCents: kind === "income" ? tail.reduce((sum, item) => sum + item.incomeCents, 0) : 0, expenseCents: kind === "expense" ? tail.reduce((sum, item) => sum + item.expenseCents, 0) : 0, count: tail.reduce((sum, item) => sum + item.count, 0) },
  ] : head
  return (
    <div aria-label="分类占比" className="statistics-category-share">
      {rows.map((item, index) => {
        const value = amount(item)
        const ratio = Math.round((value / total) * 100)
        return (
          <div className="statistics-category-row" key={item.key || item.label}>
            <span className="statistics-category-name">{item.label || "未分类"}</span>
            <span className="statistics-category-bar"><i style={{ background: CHART_COLORS[index % CHART_COLORS.length], width: `${Math.max(2, ratio)}%` }} /></span>
            <span className="statistics-category-value">{yuan(value)} · {ratio}%</span>
          </div>
        )
      })}
    </div>
  )
}

export function CategoryShareWidget({ config }: { config: unknown }) {
  const { monthKey, from, to } = useStatisticsPeriod()
  const kind = categoryKind(config)
  const query = useQuery({
    queryKey: ["statistics-aggregate", "category", monthKey, kind],
    queryFn: () => api.statisticsAggregate({ from, to, groupBy: "category", kind }),
  })
  return (
    <WidgetBodyState empty={!query.data?.some((item) => item.incomeCents || item.expenseCents)} error={query.error} loading={query.isLoading} onRetry={() => void query.refetch()}>
      <CategoryShare data={query.data ?? []} kind={kind} />
    </WidgetBodyState>
  )
}

export function AccountBalancesWidget({ config }: { config: unknown }) {
  const unarchivedOnly = accountsUnarchivedOnly(config)
  const query = useQuery({ queryKey: ["ledger-accounts"], queryFn: api.ledgerAccounts })
  const accounts = (query.data ?? []).filter((account) => !unarchivedOnly || !account.archived)
  return (
    <WidgetBodyState empty={!accounts.length} error={query.error} loading={query.isLoading} onRetry={() => void query.refetch()}>
      <div aria-label="账户余额列表" className="statistics-account-list" data-scrollable={accounts.length > 6 || undefined}>
        {accounts.map((account) => (
          <div className="statistics-account-row" key={account.id}>
            <span title={ledgerAccountDisplayLabel(account)}>{ledgerAccountDisplayLabel(account)}</span>
            <strong className={account.balanceCents < 0 ? "metric-negative" : undefined}>{yuan(account.balanceCents)}</strong>
          </div>
        ))}
      </div>
    </WidgetBodyState>
  )
}

function sixMonthRange(month: Date) {
  const first = new Date(month.getFullYear(), month.getMonth() - 5, 1)
  const next = new Date(month.getFullYear(), month.getMonth() + 1, 1)
  return {
    from: `${monthKeyOf(first)}-01`,
    to: `${monthKeyOf(next)}-01`,
    keys: Array.from({ length: 6 }, (_, index) => monthKeyOf(new Date(first.getFullYear(), first.getMonth() + index, 1))),
  }
}

function MonthCompare({ data, keys }: { data: StatisticsAggregateItem[]; keys: string[] }) {
  const width = 640
  const height = 130
  const byMonth = new Map(data.map((item) => [item.key, item]))
  const series = keys.map((key) => ({ key, income: byMonth.get(key)?.incomeCents ?? 0, expense: byMonth.get(key)?.expenseCents ?? 0 }))
  const max = Math.max(...series.flatMap((item) => [item.income, item.expense]), 1)
  const slot = width / series.length
  const barWidth = Math.min(26, slot / 4)
  const top = (value: number) => height - 26 - (value / max) * 94
  return (
    <div aria-label="近六个月收支对比" className="statistics-month-compare">
      <svg aria-label="近六个月收入支出分组柱状图" role="img" viewBox={`0 0 ${width} ${height}`}>
        {series.map((item, index) => {
          const center = slot * index + slot / 2
          return (
            <g key={item.key}>
              <rect className="statistics-chart-income" height={Math.max(item.income ? 1 : 0, height - 26 - top(item.income))} width={barWidth} x={center - barWidth - 2} y={top(item.income)} />
              <rect className="statistics-chart-expense" height={Math.max(item.expense ? 1 : 0, height - 26 - top(item.expense))} width={barWidth} x={center + 2} y={top(item.expense)} />
              <text className="statistics-chart-label" textAnchor="middle" x={center} y={height - 7}>{item.key.slice(5)}月</text>
            </g>
          )
        })}
      </svg>
      <div className="statistics-month-net-row">
        {series.map((item) => {
          const net = item.income - item.expense
          return <span className={net < 0 ? "metric-negative" : undefined} key={item.key} title={`${item.key} 结余 ${yuan(net)}`}>{net > 0 ? "+" : ""}{yuan(net)}</span>
        })}
      </div>
    </div>
  )
}

export function MonthCompareWidget() {
  const { month, monthKey } = useStatisticsPeriod()
  const range = sixMonthRange(month)
  const query = useQuery({
    queryKey: ["statistics-aggregate", "month", monthKey],
    queryFn: () => api.statisticsAggregate({ from: range.from, to: range.to, groupBy: "month" }),
  })
  return (
    <WidgetBodyState empty={!query.data?.some((item) => item.incomeCents || item.expenseCents)} error={query.error} loading={query.isLoading} onRetry={() => void query.refetch()}>
      <MonthCompare data={query.data ?? []} keys={range.keys} />
    </WidgetBodyState>
  )
}

export function DebtsOverviewWidget({ widget }: { widget?: DashboardWidget }) {
  void widget
  const summary = useQuery({ queryKey: ["dashboard-summary"], queryFn: api.summary })
  const debts = useQuery({
    queryKey: ["debts", "dashboard-widget"],
    queryFn: () => api.debts({ archived: false, page: 1, pageSize: 100 }),
  })
  const due = (debts.data?.items ?? [])
    .filter((item) => item.dueOn && item.remainingCents > 0)
    .sort((a, b) => (a.dueOn ?? "").localeCompare(b.dueOn ?? ""))
    .slice(0, 3)
  const error = summary.error || debts.error
  const empty = !summary.data?.lendOutRemainingCents && !summary.data?.borrowInRemainingCents && !due.length
  const retry = () => { void summary.refetch(); void debts.refetch() }
  return (
    <WidgetBodyState empty={empty} error={error} loading={summary.isLoading || debts.isLoading} onRetry={retry}>
      <div className="statistics-debt-overview">
        <div className="statistics-debt-metrics">
          <div><span>借出未收</span><strong>{yuan(summary.data?.lendOutRemainingCents ?? 0)}</strong></div>
          <div><span>借入未还</span><strong>{yuan(summary.data?.borrowInRemainingCents ?? 0)}</strong></div>
        </div>
        <div className="statistics-debt-due-list">
          {due.map((item) => (
            <Link key={item.id} to={`/app/debts/${item.id}`}>
              <span>{item.counterparty.displayName}</span>
              <span>{item.dueOn}</span>
              <strong>{yuan(item.remainingCents)}</strong>
            </Link>
          ))}
        </div>
      </div>
    </WidgetBodyState>
  )
}
