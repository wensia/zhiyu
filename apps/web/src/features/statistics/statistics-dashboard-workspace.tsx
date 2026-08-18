import { useQuery } from "@tanstack/react-query"
import { ChevronLeftIcon, ChevronRightIcon, PlusIcon } from "lucide-react"
import { useCallback, useEffect, useMemo, useRef, useState } from "react"

import { api } from "../../api/client"
import { useIdempotentMutation } from "../../api/use-idempotent-mutation"
import type { Dashboard, DashboardWidget, DashboardWidgetInput, WidgetTypes } from "../../api/types"
import { Button, ConfirmDialog, InlineNotice, useToast } from "../../components/ui"
import { useTopbarSlots } from "../../components/topbar-slots"
import { AddWidgetSheet, DashboardNameDialog, DashboardTabs, EmptyDashboardState, WidgetConfigDialog } from "./controls"
import { DashboardGrid } from "./dashboard-grid"
import { StatisticsPeriodProvider } from "./period"
import { MetricStrip } from "./shared"
import { findFreeSlot, monthKeyOf } from "./utils"

function useViewportWidth() {
  const [width, setWidth] = useState(() => typeof window === "undefined" ? 1280 : window.innerWidth)
  useEffect(() => {
    const update = () => setWidth(window.innerWidth)
    window.addEventListener("resize", update)
    return () => window.removeEventListener("resize", update)
  }, [])
  return width
}

function MonthControls({ month, onChange }: { month: Date; onChange: (month: Date) => void }) {
  return (
    <div aria-label={`${month.getFullYear()} 年 ${month.getMonth() + 1} 月`} className="topbar-month-controls statistics-month-controls" role="group">
      <Button aria-label="上一月" onClick={() => onChange(new Date(month.getFullYear(), month.getMonth() - 1, 1))} size="icon" title="上一月" variant="outline"><ChevronLeftIcon /></Button>
      <strong>{monthKeyOf(month)}</strong>
      <Button aria-label="下一月" onClick={() => onChange(new Date(month.getFullYear(), month.getMonth() + 1, 1))} size="icon" title="下一月" variant="outline"><ChevronRightIcon /></Button>
    </div>
  )
}

function widgetInputs(widgets: DashboardWidget[]): DashboardWidgetInput[] {
  return widgets.map((widget) => ({
    id: widget.id,
    widgetType: widget.widgetType,
    pluginId: widget.pluginId,
    x: widget.x,
    y: widget.y,
    w: widget.w,
    h: widget.h,
    config: widget.config,
  }))
}

function defaultConfig(widgetType: string): Record<string, unknown> {
  if (widgetType === "core:category-share") return { kind: "expense" }
  if (widgetType === "core:account-balances") return { unarchivedOnly: true }
  return {}
}

export function StatisticsDashboardWorkspace() {
  const toast = useToast()
  const setTopbarSlots = useTopbarSlots()
  const viewportWidth = useViewportWidth()
  const desktop = viewportWidth >= 1024
  const [month, setMonth] = useState(() => new Date(new Date().getFullYear(), new Date().getMonth(), 1))
  const [dashboards, setDashboards] = useState<Dashboard[]>([])
  const [activeId, setActiveId] = useState("")
  const [editing, setEditing] = useState(false)
  const [addOpen, setAddOpen] = useState(false)
  const [nameDialog, setNameDialog] = useState<"create" | "rename" | undefined>()
  const [deletingDashboard, setDeletingDashboard] = useState(false)
  const [configuringWidget, setConfiguringWidget] = useState<DashboardWidget>()
  const initialized = useRef(false)
  const lastSaved = useRef(new Map<string, DashboardWidget[]>())
  const saveSignatures = useRef(new Map<string, string>())
  const saveTimers = useRef(new Map<string, number>())
  const saveGenerations = useRef(new Map<string, number>())
  const pendingSaves = useRef(new Map<string, {
    widgets: DashboardWidget[]
    generation: number
    idempotencyKey: string
  }>())

  const dashboardsQuery = useQuery({ queryKey: ["dashboards"], queryFn: api.dashboards })
  const widgetTypesQuery = useQuery({ queryKey: ["dashboard-widget-types"], queryFn: api.dashboardWidgetTypes })
  const activeDashboard = dashboards.find((dashboard) => dashboard.id === activeId) ?? dashboards[0]
  const monthKey = monthKeyOf(month)
  const summaryQuery = useQuery({
    queryKey: ["transaction-summary", monthKey],
    queryFn: () => api.transactionSummary(monthKey),
    enabled: Boolean(activeDashboard),
  })

  useEffect(() => {
    if (!dashboardsQuery.data || initialized.current) return
    initialized.current = true
    const sorted = [...dashboardsQuery.data].sort((a, b) => a.position - b.position)
    setDashboards(sorted)
    setActiveId(sorted[0]?.id ?? "")
    for (const dashboard of sorted) lastSaved.current.set(dashboard.id, dashboard.widgets)
  }, [dashboardsQuery.data])

  useEffect(() => {
    if (desktop) return
    setEditing(false)
    setAddOpen(false)
  }, [desktop])

  const updateLocalWidgets = useCallback((dashboardId: string, widgets: DashboardWidget[]) => {
    setDashboards((current) => current.map((dashboard) => dashboard.id === dashboardId ? { ...dashboard, widgets } : dashboard))
  }, [])

  const persistWidgets = useCallback((dashboardId: string, widgets: DashboardWidget[], generation: number, idempotencyKey: string) => {
    void api.replaceDashboardWidgets(dashboardId, widgetInputs(widgets), { idempotencyKey }).then((saved) => {
      lastSaved.current.set(dashboardId, saved.widgets)
      if (saveGenerations.current.get(dashboardId) === generation) {
        setDashboards((current) => current.map((dashboard) => dashboard.id === dashboardId ? saved : dashboard))
      }
    }).catch(() => {
      if (saveGenerations.current.get(dashboardId) !== generation) return
      const fallback = lastSaved.current.get(dashboardId) ?? []
      updateLocalWidgets(dashboardId, fallback)
      toast({
        title: "布局未保存，重试",
        type: "error",
        action: {
          label: "重试",
          onSelect: () => {
            updateLocalWidgets(dashboardId, widgets)
            persistWidgets(dashboardId, widgets, generation, idempotencyKey)
          },
        },
      })
    })
  }, [toast, updateLocalWidgets])

  const queueWidgetSave = useCallback((dashboardId: string, widgets: DashboardWidget[]) => {
    const signature = JSON.stringify(widgetInputs(widgets))
    if (saveSignatures.current.get(dashboardId) === signature) return
    saveSignatures.current.set(dashboardId, signature)
    updateLocalWidgets(dashboardId, widgets)
    const previous = saveTimers.current.get(dashboardId)
    if (previous !== undefined) window.clearTimeout(previous)
    const generation = (saveGenerations.current.get(dashboardId) ?? 0) + 1
    saveGenerations.current.set(dashboardId, generation)
    const idempotencyKey = crypto.randomUUID()
    pendingSaves.current.set(dashboardId, { widgets, generation, idempotencyKey })
    const timer = window.setTimeout(() => {
      saveTimers.current.delete(dashboardId)
      pendingSaves.current.delete(dashboardId)
      persistWidgets(dashboardId, widgets, generation, idempotencyKey)
    }, 500)
    saveTimers.current.set(dashboardId, timer)
  }, [persistWidgets, updateLocalWidgets])

  const flushPendingSaves = useCallback(() => {
    for (const timer of saveTimers.current.values()) window.clearTimeout(timer)
    saveTimers.current.clear()
    const pending = [...pendingSaves.current.entries()]
    pendingSaves.current.clear()
    for (const [dashboardId, save] of pending) {
      persistWidgets(dashboardId, save.widgets, save.generation, save.idempotencyKey)
    }
  }, [persistWidgets])

  useEffect(() => () => flushPendingSaves(), [flushPendingSaves])

  const createMutation = useIdempotentMutation({
    mutationFn: (name: string, write) => api.createDashboard({ name }, write),
    onSuccess: (created) => {
      setDashboards((current) => [...current, created])
      lastSaved.current.set(created.id, created.widgets)
      setActiveId(created.id)
      setNameDialog(undefined)
    },
    onError: (error) => toast({ title: "面板未创建", description: error.message, type: "error" }),
  })
  const defaultMutation = useIdempotentMutation({
    mutationFn: (_: void, write) => api.createDefaultDashboard(write),
    onSuccess: (created) => {
      setDashboards([created])
      lastSaved.current.set(created.id, created.widgets)
      setActiveId(created.id)
    },
    onError: (error) => toast({ title: "默认布局未创建", description: error.message, type: "error" }),
  })
  const renameMutation = useIdempotentMutation({
    mutationFn: (name: string, write) => api.updateDashboard(activeDashboard!.id, { name }, write),
    onSuccess: (updated) => {
      setDashboards((current) => current.map((dashboard) => dashboard.id === updated.id ? updated : dashboard))
      setNameDialog(undefined)
    },
    onError: (error) => toast({ title: "面板未重命名", description: error.message, type: "error" }),
  })
  const deleteMutation = useIdempotentMutation({
    mutationFn: (_: void, write) => api.deleteDashboard(activeDashboard!.id, write),
    onSuccess: () => {
      const remaining = dashboards.filter((dashboard) => dashboard.id !== activeDashboard?.id)
      setDashboards(remaining)
      setActiveId(remaining[0]?.id ?? "")
      setDeletingDashboard(false)
    },
    onError: (error) => toast({ title: "面板未删除", description: error.message, type: "error" }),
  })

  const addWidget = (definition: WidgetTypes["core"][number], pluginId?: string) => {
    if (!activeDashboard) return
    const widgetType = pluginId ? `plugin:${pluginId}:${definition.id}` : `core:${definition.id}`
    const position = findFreeSlot(activeDashboard.widgets, definition.defaultW, definition.defaultH)
    const next: DashboardWidget = {
      id: crypto.randomUUID(),
      widgetType,
      pluginId: pluginId ?? null,
      x: position.x,
      y: position.y,
      w: definition.defaultW,
      h: definition.defaultH,
      config: defaultConfig(widgetType),
    }
    queueWidgetSave(activeDashboard.id, [...activeDashboard.widgets, next])
  }

  const deleteWidget = (widget: DashboardWidget) => {
    if (!activeDashboard) return
    queueWidgetSave(activeDashboard.id, activeDashboard.widgets.filter((candidate) => candidate.id !== widget.id))
  }

  const updateWidgetConfig = (config: Record<string, unknown>) => {
    if (!activeDashboard || !configuringWidget) return
    queueWidgetSave(activeDashboard.id, activeDashboard.widgets.map((widget) => widget.id === configuringWidget.id ? { ...widget, config } : widget))
    setConfiguringWidget(undefined)
  }

  const topbarActions = useMemo(() => (
    <>
      {activeDashboard ? (
        <DashboardTabs
          activeId={activeDashboard.id}
          dashboards={dashboards}
          editing={editing}
          onChange={setActiveId}
          onCreate={() => setNameDialog("create")}
          onDelete={() => setDeletingDashboard(true)}
          onRename={() => setNameDialog("rename")}
        />
      ) : null}
      <MonthControls month={month} onChange={setMonth} />
      {activeDashboard && editing ? <Button onClick={() => setAddOpen(true)} variant="default"><PlusIcon />添加组件</Button> : null}
      {activeDashboard ? (
        <Button disabled={!desktop} onClick={() => setEditing((value) => !value)} title={!desktop ? "请在桌面宽度下编辑布局" : undefined} variant="outline">
          {editing ? "完成" : "编辑"}
        </Button>
      ) : null}
    </>
  ), [activeDashboard, dashboards, desktop, editing, month])

  useEffect(() => {
    setTopbarSlots({ title: "统计", actions: topbarActions })
    return () => setTopbarSlots(undefined)
  }, [setTopbarSlots, topbarActions])

  if (dashboardsQuery.isLoading) {
    return <main className="workspace statistics-dashboard-workspace"><div aria-busy="true" className="statistics-workspace-loading">正在加载统计面板…</div></main>
  }
  if (dashboardsQuery.error) {
    return <main className="workspace statistics-dashboard-workspace"><InlineNotice type="error"><span>{dashboardsQuery.error.message}</span><Button onClick={() => void dashboardsQuery.refetch()} size="sm">重试</Button></InlineNotice></main>
  }

  const pendingCreate = defaultMutation.isPending || createMutation.isPending
  return (
    <StatisticsPeriodProvider month={month}>
      <main className="workspace statistics-dashboard-workspace">
        {!activeDashboard ? (
          <EmptyDashboardState onBlank={() => createMutation.mutate("面板 1")} onDefault={() => defaultMutation.mutate()} pending={pendingCreate} />
        ) : (
          <>
            {summaryQuery.error ? <InlineNotice type="error"><span>{summaryQuery.error.message}</span><Button onClick={() => void summaryQuery.refetch()} size="sm">重试</Button></InlineNotice> : null}
            <MetricStrip loading={summaryQuery.isLoading} summary={summaryQuery.data} />
            <DashboardGrid
              editing={editing}
              onConfigure={setConfiguringWidget}
              onDelete={deleteWidget}
              onLayoutChange={(widgets) => queueWidgetSave(activeDashboard.id, widgets)}
              viewportWidth={viewportWidth}
              widgetTypes={widgetTypesQuery.data}
              widgets={activeDashboard.widgets}
            />
          </>
        )}
        <AddWidgetSheet onAdd={addWidget} onOpenChange={setAddOpen} open={addOpen} widgetTypes={widgetTypesQuery.data} />
        <WidgetConfigDialog onOpenChange={(open) => !open && setConfiguringWidget(undefined)} onSubmit={updateWidgetConfig} widget={configuringWidget} />
        <DashboardNameDialog
          initialName={nameDialog === "rename" ? activeDashboard?.name ?? "" : `面板 ${dashboards.length + 1}`}
          onOpenChange={(open) => !open && setNameDialog(undefined)}
          onSubmit={(name) => nameDialog === "rename" ? renameMutation.mutate(name) : createMutation.mutate(name)}
          open={Boolean(nameDialog)}
          pending={createMutation.isPending || renameMutation.isPending}
          submitLabel={nameDialog === "rename" ? "确认" : "创建"}
          title={nameDialog === "rename" ? "重命名面板" : "新建面板"}
        />
        <ConfirmDialog
          confirmLabel="删除面板"
          description="删除后无法恢复，组件布局也会一并删除。"
          onConfirm={() => deleteMutation.mutate()}
          onOpenChange={setDeletingDashboard}
          open={deletingDashboard}
          pending={deleteMutation.isPending}
          title={`删除“${activeDashboard?.name ?? ""}”？`}
        />
      </main>
    </StatisticsPeriodProvider>
  )
}
