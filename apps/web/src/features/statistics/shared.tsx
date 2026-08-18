import type { ReactNode } from "react"
import { Link } from "react-router-dom"

import type { TransactionMonthSummary } from "../../api/types"
import { ActionMenu, Button, InlineNotice } from "../../components/ui"
import { yuan } from "./utils"

export function MetricStrip({ summary, loading }: { summary?: TransactionMonthSummary; loading: boolean }) {
  const net = summary?.netCents ?? 0
  const items: Array<[string, string, string?]> = [
    ["本月收入", yuan(summary?.incomeCents ?? 0), "metric-positive"],
    ["本月支出", yuan(summary?.expenseCents ?? 0)],
    ["本月结余", `${net > 0 ? "+" : ""}${yuan(net)}`, net < 0 ? "metric-negative" : "metric-positive"],
    ["本月笔数", `${summary?.transactionCount ?? 0} 笔`],
  ]
  return (
    <section aria-label="本月收支汇总" className="summary-strip statistics-summary-strip">
      {items.map(([label, value, className]) => (
        <div className="summary-strip-item" key={label}>
          <span>{label}</span>
          <strong className={className}>{loading ? "—" : value}</strong>
        </div>
      ))}
    </section>
  )
}

export type WidgetCardProps = {
  title: string
  editing: boolean
  configurable?: boolean
  mutedTitle?: boolean
  onConfigure?: () => void
  onDelete: () => void
  children: ReactNode
}

export function WidgetCard({
  title,
  editing,
  configurable = false,
  mutedTitle = false,
  onConfigure,
  onDelete,
  children,
}: WidgetCardProps) {
  return (
    <section className={`statistics-widget-card${editing ? " statistics-widget-card-editing" : ""}`}>
      <header className="statistics-widget-header">
        <h2 className={mutedTitle ? "statistics-widget-title-muted" : undefined}>{title}</h2>
        {editing ? (
          <ActionMenu
            items={[
              ...(configurable && onConfigure ? [{ label: "配置", onSelect: onConfigure }] : []),
              { label: "删除", onSelect: onDelete, destructive: true },
            ]}
            label={`操作组件 ${title}`}
            quiet
          />
        ) : null}
      </header>
      <div className="statistics-widget-body">{children}</div>
    </section>
  )
}

export function WidgetBodyState({
  loading,
  error,
  empty,
  onRetry,
  children,
}: {
  loading: boolean
  error?: Error | null
  empty?: boolean
  onRetry: () => void
  children: ReactNode
}) {
  if (loading) {
    return (
      <div aria-busy="true" aria-label="正在加载组件" className="statistics-widget-skeleton">
        <span /><span /><span />
      </div>
    )
  }
  if (error) {
    return (
      <InlineNotice type="error">
        <span>{error.message}</span>
        <Button onClick={onRetry} size="sm">重试</Button>
      </InlineNotice>
    )
  }
  if (empty) {
    return (
      <div className="statistics-widget-empty">
        <p>本月暂无数据</p>
        <Link className="statistics-secondary-link" to="/app/transactions">记一笔</Link>
      </div>
    )
  }
  return children
}

export function WidgetPlaceholder({
  title,
  pluginName,
  editing,
  onDelete,
}: {
  title: string
  pluginName: string
  editing: boolean
  onDelete: () => void
}) {
  return (
    <WidgetCard editing={editing} mutedTitle onDelete={onDelete} title={title}>
      <div className="statistics-widget-placeholder">
        <span>〈{pluginName}〉已关闭</span>
        <Link to="/app/settings/plugins">去开启</Link>
      </div>
    </WidgetCard>
  )
}
