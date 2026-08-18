import { useEffect, useState } from "react"

import type { Dashboard, DashboardWidget, WidgetTypes } from "../../api/types"
import {
  ActionMenu,
  Button,
  CheckboxControl,
  Field,
  Input,
  Modal,
  Select,
  Sheet,
  TabsList,
  TabsRoot,
  TabsTrigger,
} from "../../components/ui"
import { pluginRegistry } from "../../plugins/registry"
import { accountsUnarchivedOnly, categoryKind } from "./utils"

export function DashboardTabs({
  dashboards,
  activeId,
  editing,
  onChange,
  onCreate,
  onRename,
  onDelete,
}: {
  dashboards: Dashboard[]
  activeId: string
  editing: boolean
  onChange: (id: string) => void
  onCreate: () => void
  onRename: () => void
  onDelete: () => void
}) {
  const lastDashboard = dashboards.length <= 1
  const active = dashboards.find((dashboard) => dashboard.id === activeId)
  return (
    <div className="dashboard-tabs">
      <TabsRoot className="workspace-tabs dashboard-tabs-root" onValueChange={onChange} value={activeId}>
        <TabsList aria-label="统计面板">
          {dashboards.map((dashboard) => <TabsTrigger key={dashboard.id} value={dashboard.id}>{dashboard.name}</TabsTrigger>)}
        </TabsList>
      </TabsRoot>
      {editing ? (
        <>
          <ActionMenu
            items={[
              { label: "重命名", onSelect: onRename },
              { label: "删除面板", onSelect: onDelete, destructive: true, disabled: lastDashboard, title: lastDashboard ? "至少保留一页" : undefined },
            ]}
            label={`操作面板 ${active?.name ?? ""}`}
            quiet
          />
          <Button aria-label="新建面板" className="dashboard-tab-add" onClick={onCreate} size="icon-sm" title="新建面板" variant="ghost">+</Button>
        </>
      ) : null}
    </div>
  )
}

export function AddWidgetSheet({
  open,
  widgetTypes,
  onOpenChange,
  onAdd,
}: {
  open: boolean
  widgetTypes?: WidgetTypes
  onOpenChange: (open: boolean) => void
  onAdd: (definition: WidgetTypes["core"][number], pluginId?: string) => void
}) {
  return (
    <Sheet onOpenChange={onOpenChange} open={open} title="添加组件">
      <div className="statistics-widget-picker">
        <section>
          <h3>核心</h3>
          <div>
            {(widgetTypes?.core ?? []).map((definition) => (
              <button className="statistics-widget-picker-item" key={definition.id} onClick={() => onAdd(definition)} type="button">
                <span><strong>{definition.name}</strong><small>{definition.description}</small></span>
                <span>{definition.defaultW}×{definition.defaultH}</span>
              </button>
            ))}
          </div>
        </section>
        {(widgetTypes?.plugins ?? []).filter((group) => group.widgets.length > 0).map((group) => {
          const plugin = pluginRegistry.find((candidate) => candidate.id === group.pluginId)
          return (
            <section data-disabled={!group.enabled || undefined} key={group.pluginId}>
              <h3>{plugin?.name ?? group.pluginId}{group.enabled ? "" : " · 已关闭"}</h3>
              <div>
                {group.widgets.map((definition) => (
                  <button className="statistics-widget-picker-item" disabled={!group.enabled} key={definition.id} onClick={() => onAdd(definition, group.pluginId)} type="button">
                    <span><strong>{definition.name}</strong><small>{definition.description}</small></span>
                    <span>{definition.defaultW}×{definition.defaultH}</span>
                  </button>
                ))}
              </div>
            </section>
          )
        })}
      </div>
    </Sheet>
  )
}

export function DashboardNameDialog({
  open,
  title,
  initialName,
  pending,
  submitLabel,
  onOpenChange,
  onSubmit,
}: {
  open: boolean
  title: string
  initialName: string
  pending: boolean
  submitLabel: string
  onOpenChange: (open: boolean) => void
  onSubmit: (name: string) => void
}) {
  const [name, setName] = useState(initialName)
  useEffect(() => { if (open) setName(initialName) }, [initialName, open])
  return (
    <Modal
      footer={<><Button disabled={pending} onClick={() => onOpenChange(false)}>取消</Button><Button disabled={pending || !name.trim()} onClick={() => onSubmit(name.trim())} variant="default">{pending ? "正在处理…" : submitLabel}</Button></>}
      onOpenChange={onOpenChange}
      open={open}
      title={title}
    >
      <Field label="名称"><Input autoFocus maxLength={60} onChange={(event) => setName(event.target.value)} value={name} /></Field>
    </Modal>
  )
}

export function WidgetConfigDialog({
  widget,
  onOpenChange,
  onSubmit,
}: {
  widget?: DashboardWidget
  onOpenChange: (open: boolean) => void
  onSubmit: (config: Record<string, unknown>) => void
}) {
  const [kind, setKind] = useState<"expense" | "income">("expense")
  const [unarchivedOnly, setUnarchivedOnly] = useState(true)
  useEffect(() => {
    if (!widget) return
    setKind(categoryKind(widget.config))
    setUnarchivedOnly(accountsUnarchivedOnly(widget.config))
  }, [widget])
  const category = widget?.widgetType === "core:category-share"
  const accounts = widget?.widgetType === "core:account-balances"
  return (
    <Modal
      footer={<><Button onClick={() => onOpenChange(false)}>取消</Button><Button onClick={() => onSubmit(category ? { kind } : { unarchivedOnly })} variant="default">确认</Button></>}
      onOpenChange={onOpenChange}
      open={Boolean(widget)}
      title={`配置${category ? "分类占比" : accounts ? "账户余额" : "组件"}`}
    >
      {category ? (
        <Field label="统计类型">
          <Select ariaLabel="统计类型" onValueChange={(value) => setKind(value as "expense" | "income")} options={[{ value: "expense", label: "支出" }, { value: "income", label: "收入" }]} value={kind} />
        </Field>
      ) : null}
      {accounts ? <CheckboxControl checked={unarchivedOnly} label="只看未归档账户" onChange={(event) => setUnarchivedOnly(event.target.checked)} /> : null}
    </Modal>
  )
}

export function EmptyDashboardState({
  pending,
  onDefault,
  onBlank,
}: {
  pending: boolean
  onDefault: () => void
  onBlank: () => void
}) {
  return (
    <section className="statistics-empty-dashboard">
      <h2>还没有统计面板</h2>
      <p>创建一页后，就能按自己的习惯查看账本。</p>
      <div>
        <Button disabled={pending} onClick={onDefault} variant="default">使用默认布局</Button>
        <Button disabled={pending} onClick={onBlank} variant="outline">从空白开始</Button>
      </div>
    </section>
  )
}
