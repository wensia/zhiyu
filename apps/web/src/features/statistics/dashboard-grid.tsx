import ReactGridLayout from "react-grid-layout"
import type { CSSProperties } from "react"

import type { DashboardWidget, WidgetTypes } from "../../api/types"
import { usePluginState } from "../../plugins/context"
import { pluginRegistry } from "../../plugins/registry"
import { useStatisticsPeriod } from "./period"
import { WidgetCard, WidgetPlaceholder } from "./shared"
import {
  AccountBalancesWidget,
  CategoryShareWidget,
  IncomeExpenseTrendWidget,
  MonthCompareWidget,
} from "./widgets"

const MeasuredGridLayout = ReactGridLayout.WidthProvider(ReactGridLayout)
type Layout = ReactGridLayout.Layout
type WidgetDefinition = WidgetTypes["core"][number]

const ROW_HEIGHT = 72
const GRID_MARGIN: [number, number] = [16, 16]

const CORE_TITLES: Record<string, string> = {
  "core:income-expense-trend": "收支趋势",
  "core:category-share": "分类占比",
  "core:account-balances": "账户余额",
  "core:month-compare": "月度对比",
}

function sixColumnLayout(layout: Layout[]): Layout[] {
  return layout.map((item) => {
    const w = Math.max(1, Math.ceil(item.w / 2))
    return { ...item, x: Math.min(6 - w, Math.floor(item.x / 2)), w, minW: Math.max(1, Math.ceil((item.minW ?? 1) / 2)) }
  })
}

function singleColumnLayout(layout: Layout[]): Layout[] {
  let y = 0
  return [...layout].sort((a, b) => a.y - b.y || a.x - b.x).map((item) => {
    const next = { ...item, x: 0, y, w: 1, h: Math.max(2, item.h), minW: 1 }
    y += next.h
    return next
  })
}

function coreWidgetBody(widget: DashboardWidget) {
  if (widget.widgetType === "core:income-expense-trend") return <IncomeExpenseTrendWidget />
  if (widget.widgetType === "core:category-share") return <CategoryShareWidget config={widget.config} />
  if (widget.widgetType === "core:account-balances") return <AccountBalancesWidget config={widget.config} />
  if (widget.widgetType === "core:month-compare") return <MonthCompareWidget />
  return undefined
}

function findDefinition(widget: DashboardWidget, widgetTypes?: WidgetTypes): WidgetDefinition | undefined {
  if (widget.widgetType.startsWith("core:")) {
    return widgetTypes?.core.find((definition) => `core:${definition.id}` === widget.widgetType)
  }
  const [, pluginId, type] = widget.widgetType.split(":")
  return widgetTypes?.plugins.find((plugin) => plugin.pluginId === pluginId)?.widgets.find((definition) => definition.id === type)
}

export function DashboardGrid({
  widgets,
  widgetTypes,
  editing,
  viewportWidth,
  onLayoutChange,
  onConfigure,
  onDelete,
}: {
  widgets: DashboardWidget[]
  widgetTypes?: WidgetTypes
  editing: boolean
  viewportWidth: number
  onLayoutChange: (widgets: DashboardWidget[]) => void
  onConfigure: (widget: DashboardWidget) => void
  onDelete: (widget: DashboardWidget) => void
}) {
  const { month } = useStatisticsPeriod()
  const { plugins } = usePluginState()
  const desktop = viewportWidth >= 1024
  const layout: Layout[] = widgets.map((widget) => {
    const definition = findDefinition(widget, widgetTypes)
    return {
      i: widget.id,
      x: widget.x,
      y: widget.y,
      w: widget.w,
      h: widget.h,
      minW: definition?.minW ?? 1,
      minH: definition?.minH ?? 1,
    }
  })
  const applyLayout = (next: Layout[]) => {
    const byId = new Map(next.map((item) => [item.i, item]))
    const nextWidgets = widgets.map((widget) => {
      const item = byId.get(widget.id)
      return item ? { ...widget, x: item.x, y: item.y, w: item.w, h: item.h } : widget
    })
    if (nextWidgets.every((widget, index) => {
      const current = widgets[index]
      return widget.x === current.x && widget.y === current.y && widget.w === current.w && widget.h === current.h
    })) return
    onLayoutChange(nextWidgets)
  }
  const visibleLayout = desktop ? layout : viewportWidth >= 768 ? sixColumnLayout(layout) : singleColumnLayout(layout)
  const columns = desktop ? 12 : viewportWidth >= 768 ? 6 : 1

  return (
    <div
      className={`statistics-grid-region${editing ? " statistics-grid-region-editing" : ""}`}
      style={editing ? {
        "--statistics-grid-row": `${ROW_HEIGHT + GRID_MARGIN[1]}px`,
        "--statistics-grid-col": `calc((100% + ${GRID_MARGIN[0]}px) / 12)`,
      } as CSSProperties : undefined}
    >
      {!desktop ? <p className="statistics-breakpoint-notice" role="status">请在桌面宽度下编辑布局</p> : null}
      {!widgets.length ? (
        <div className="statistics-empty-grid">
          <p>当前面板还没有组件</p>
          <span>{editing ? "从“添加组件”开始搭建面板。" : "进入编辑态后可以添加组件。"}</span>
        </div>
      ) : (
        <MeasuredGridLayout
          className="statistics-dashboard-grid"
          cols={columns}
          compactType={null}
          draggableCancel="button, a, [role='menu']"
          draggableHandle=".statistics-widget-header"
          isDraggable={editing && desktop}
          isResizable={editing && desktop}
          layout={visibleLayout}
          margin={GRID_MARGIN}
          maxRows={200}
          measureBeforeMount={false}
          onLayoutChange={desktop ? applyLayout : undefined}
          allowOverlap={false}
          preventCollision={true}
          resizeHandles={["se", "e", "s"]}
          rowHeight={ROW_HEIGHT}
        >
          {widgets.map((widget) => {
            const definition = findDefinition(widget, widgetTypes)
            const configurable = widget.widgetType === "core:category-share" || widget.widgetType === "core:account-balances"
            const title = definition?.name ?? CORE_TITLES[widget.widgetType] ?? widget.widgetType
            const coreBody = coreWidgetBody(widget)
            if (coreBody) {
              return (
                <div key={widget.id}>
                  <WidgetCard configurable={configurable} editing={editing} onConfigure={() => onConfigure(widget)} onDelete={() => onDelete(widget)} title={title}>
                    {coreBody}
                  </WidgetCard>
                </div>
              )
            }

            const [, pluginId = widget.pluginId ?? "组件", type = ""] = widget.widgetType.split(":")
            const registration = pluginRegistry.find((plugin) => plugin.id === pluginId)
            const enabled = plugins?.find((plugin) => plugin.id === pluginId)?.enabled
              ?? widgetTypes?.plugins.find((plugin) => plugin.pluginId === pluginId)?.enabled
              ?? false
            const body = enabled ? registration?.renderWidget?.(type, widget.config, { month, widget }) : undefined
            return (
              <div key={widget.id}>
                {body ? (
                  <WidgetCard editing={editing} onDelete={() => onDelete(widget)} title={title}>{body}</WidgetCard>
                ) : (
                  <WidgetPlaceholder editing={editing} onDelete={() => onDelete(widget)} pluginName={registration?.name ?? pluginId} title={title} />
                )}
              </div>
            )
          })}
        </MeasuredGridLayout>
      )}
    </div>
  )
}
