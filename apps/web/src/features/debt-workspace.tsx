import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { createColumnHelper, flexRender, getCoreRowModel, useReactTable } from "@tanstack/react-table"
import {
  ArchiveIcon,
  ArrowDownLeftIcon,
  ArrowUpRightIcon,
  CircleDollarSignIcon,
  HandCoinsIcon,
  PencilIcon,
  PlusIcon,
  RotateCcwIcon,
  SearchIcon,
  Trash2Icon,
  UsersIcon,
} from "lucide-react"
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react"
import { useNavigate, useParams } from "react-router-dom"

import { ApiClientError, api } from "../api/client"
import type { Counterparty, Debt, DebtStatus, LedgerAccount, LedgerAccountRef, Summary } from "../api/types"
import {
  Button,
  ActionMenu,
  CheckboxControl,
  ConfirmDialog,
  DatePicker,
  DirectionTag,
  Field,
  FilterSelect,
  InlineNotice,
  Input,
  Modal,
  Select,
  TablePagination,
  TabsContent,
  TabsList,
  TabsRoot,
  TabsTrigger,
  Textarea,
  useToast,
} from "../components/ui"
import { elapsedCalendarDays } from "./debt-time"
import { ledgerAccountDisplayLabel } from "./ledger-account"

const NEW_COUNTERPARTY = "__new_counterparty__"

const statusLabels: Record<DebtStatus, string> = {
  archived: "已归档",
  settled: "已结清",
  overdue: "已逾期",
  due_soon: "临近到期",
  open: "进行中",
}

const today = () => new Intl.DateTimeFormat("en-CA").format(new Date())
const yuan = (cents: number) => new Intl.NumberFormat("zh-CN", { style: "currency", currency: "CNY" }).format(cents / 100)
const toCents = (value: string) => {
  const amount = Number(value)
  return Number.isFinite(amount) && amount > 0 ? Math.round(amount * 100) : null
}

const principalAccountLabel = (direction: Debt["direction"]) => direction === "borrow_in" ? "收款账户" : "付款账户"
const repaymentAccountLabel = (direction: Debt["direction"]) => direction === "borrow_in" ? "付款账户" : "收款账户"
const visibleNote = (note: string, account: LedgerAccountRef | null) => {
  const value = note.trim()
  if (account) {
    const accountLabels = [account.name, ledgerAccountDisplayLabel(account)]
    if (accountLabels.some((label) => value === `付款账户：${label}` || value === `收款账户：${label}`)) return ""
  }
  return value
}

function MoneyAccountField({
  accounts,
  label,
  loading,
  onManage,
  onValueChange,
  value,
  allowEmpty = false,
}: {
  accounts: LedgerAccount[]
  label: string
  loading: boolean
  onManage: () => void
  onValueChange: (value: string) => void
  value: string
  allowEmpty?: boolean
}) {
  const selectableAccounts = accounts.filter((account) => !account.archived || account.id === value)
  const empty = !loading && selectableAccounts.length === 0
  return <>
    <Field className="field-medium" label={label}>
      <Select
        ariaLabel={label}
        disabled={loading || empty}
        onValueChange={onValueChange}
        options={selectableAccounts.map((account) => ({ value: account.id, label: `${ledgerAccountDisplayLabel(account)}${account.archived ? "（已归档）" : ""}` }))}
        placeholder={loading ? "正在加载账户…" : empty ? allowEmpty ? "历史未指定（可保留）" : "暂无可用账户" : allowEmpty && !value ? "历史未指定（可保留）" : "请选择账户"}
        value={value}
      />
    </Field>
    {empty && !allowEmpty ? <InlineNotice><span>请先新增可用于付款或收款的账户。</span><Button onClick={onManage} size="sm">前往账户管理</Button></InlineNotice> : null}
  </>
}

function StatusBadge({ status }: { status: DebtStatus }) {
  return <span className={`status status-${status}`}>{statusLabels[status]}</span>
}

function DirectionLabel({ direction }: { direction: string }) {
  return <DirectionTag direction={direction === "lend_out" ? "lend_out" : "borrow_in"} />
}

function MetricStrip({ summary, loading }: { summary?: Summary; loading: boolean }) {
  const items = [
    ["待收", summary?.lendOutRemainingCents, "metric-positive"],
    ["待还", summary?.borrowInRemainingCents, "metric-negative"],
    ["净额", summary?.netCents, ""],
    ["逾期", summary ? `${summary.overdueCount} 笔` : undefined, summary?.overdueCount ? "metric-warning" : ""],
  ]
  return <section className="metrics" aria-label="债务汇总">{items.map(([label, value, className]) => <div className="metric" key={String(label)}><span>{label}</span><strong className={String(className)}>{loading ? "—" : typeof value === "number" ? yuan(value) : value}</strong></div>)}</section>
}

export function DebtWorkspace() {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const [view, setView] = useState("debts")
  const [search, setSearch] = useState("")
  const [direction, setDirection] = useState("")
  const [status, setStatus] = useState("")
  const [archived, setArchived] = useState(false)
  const [page, setPage] = useState(1)
  const [createOpen, setCreateOpen] = useState(false)

  useEffect(() => setPage(1), [search, direction, status, archived])
  const debtsQuery = useQuery({ queryKey: ["debts", search, direction, status, archived, page], queryFn: () => api.debts({ query: search, direction, status, archived, page, pageSize: 20 }) })
  const summaryQuery = useQuery({ queryKey: ["summary"], queryFn: api.summary })
  const counterpartiesQuery = useQuery({ queryKey: ["counterparties"], queryFn: api.counterparties })
  const accountsQuery = useQuery({ queryKey: ["ledger-accounts"], queryFn: api.ledgerAccounts })

  const refresh = async () => Promise.all([queryClient.invalidateQueries({ queryKey: ["debts"] }), queryClient.invalidateQueries({ queryKey: ["summary"] }), queryClient.invalidateQueries({ queryKey: ["counterparties"] }), queryClient.invalidateQueries({ queryKey: ["ledger-accounts"] })])

  return (
    <main className="workspace">
      <header className="page-header">
        <div><p className="eyebrow">个人往来</p><h1>债务管理</h1></div>
        <Button aria-label="新增债务" onClick={() => setCreateOpen(true)} variant="primary"><PlusIcon /><span className="button-label">新增债务</span></Button>
      </header>
      <MetricStrip loading={summaryQuery.isLoading} summary={summaryQuery.data} />
      <TabsRoot className="workspace-tabs" value={view} onValueChange={setView}>
        <TabsList aria-label="债务视图">
          <TabsTrigger value="debts"><HandCoinsIcon /> 按债务</TabsTrigger>
          <TabsTrigger value="contacts"><UsersIcon /> 按联系人</TabsTrigger>
        </TabsList>
        <TabsContent className="tab-content" value="debts">
          <section className="toolbar" aria-label="债务筛选">
            <label className="search-box"><SearchIcon /><span className="sr-only">搜索</span><input value={search} onChange={(event) => setSearch(event.target.value)} placeholder="搜索联系人或备注" /></label>
            <FilterSelect
              ariaLabel="方向"
              onValueChange={setDirection}
              options={[{ value: "lend_out", label: "借出" }, { value: "borrow_in", label: "借入" }]}
              placeholder="全部方向"
              value={direction}
            />
            <FilterSelect
              ariaLabel="状态"
              onValueChange={setStatus}
              options={[{ value: "open", label: "进行中" }, { value: "due_soon", label: "临近到期" }, { value: "overdue", label: "已逾期" }, { value: "settled", label: "已结清" }]}
              placeholder="全部状态"
              value={status}
            />
            <CheckboxControl checked={archived} label="查看归档" onChange={(event) => setArchived(event.target.checked)} />
          </section>
          {debtsQuery.error ? <InlineNotice type="error">{debtsQuery.error.message}<Button size="sm" onClick={() => debtsQuery.refetch()}>重试</Button></InlineNotice> : null}
          <DebtTable loading={debtsQuery.isLoading} items={debtsQuery.data?.items || []} onOpen={(id) => navigate(`/app/debts/${id}`)} />
          <TablePagination disabled={debtsQuery.isLoading} page={page} pageSize={20} total={debtsQuery.data?.total || 0} onPageChange={setPage} />
        </TabsContent>
        <TabsContent className="tab-content" value="contacts">
          <div className="contact-scroll"><ContactGrid loading={counterpartiesQuery.isLoading} items={counterpartiesQuery.data || []} /></div>
        </TabsContent>
      </TabsRoot>
      <DebtFormModal accounts={accountsQuery.data || []} accountsLoading={accountsQuery.isLoading} counterparties={counterpartiesQuery.data || []} onManageAccounts={() => navigate("/app/accounts")} open={createOpen} onOpenChange={setCreateOpen} onSaved={refresh} />
    </main>
  )
}

const columnHelper = createColumnHelper<Debt>()

function DebtTable({ items, loading, onOpen }: { items: Debt[]; loading: boolean; onOpen: (id: string) => void }) {
  const scrollRef = useRef<HTMLDivElement>(null)
  const columns = useMemo(() => [
    columnHelper.accessor("counterparty.displayName", { header: "联系人", cell: ({ row, getValue }) => <button className="row-link" onClick={(event) => { event.stopPropagation(); onOpen(row.original.id) }}>{getValue()}</button> }),
    columnHelper.accessor("direction", { header: "方向", cell: ({ getValue }) => <DirectionLabel direction={getValue()} /> }),
    columnHelper.accessor("account", { header: "资金账户", cell: ({ getValue }) => ledgerAccountDisplayLabel(getValue()) }),
    columnHelper.display({ id: "overview", header: "债务概况", cell: ({ row }) => <div className="debt-overview-cell">
      <div className="debt-overview-line debt-overview-amounts">
        <span><span>本金</span>{" "}{yuan(row.original.principalCents)}</span>
        <span><span>已还</span>{" "}{yuan(row.original.paidCents)}</span>
        <span><span>剩余</span>{" "}<strong>{yuan(row.original.remainingCents)}</strong></span>
      </div>
      <div className="debt-overview-line debt-overview-time">
        <span><span>发生日期</span>{" "}<time dateTime={row.original.occurredOn}>{row.original.occurredOn}</time></span>
        <span><span>间隔</span>{" "}<strong className="debt-age">{elapsedCalendarDays(row.original.occurredOn)} 天</strong></span>
      </div>
    </div> }),
    columnHelper.accessor("dueOn", { header: "到期日", cell: ({ getValue }) => getValue() || "未设置" }),
    columnHelper.accessor("status", { header: "状态", cell: ({ getValue }) => <StatusBadge status={getValue()} /> }),
    columnHelper.display({ id: "actions", header: () => <span className="sr-only">操作</span>, cell: ({ row }) => <RowMenu debt={row.original} onOpen={onOpen} /> }),
  ], [onOpen])
  const table = useReactTable({ data: items, columns, getCoreRowModel: getCoreRowModel() })
  useEffect(() => {
    const frame = scrollRef.current
    if (!frame) return
    const updateEdges = () => {
      const max = Math.max(0, frame.scrollWidth - frame.clientWidth)
      frame.dataset.atStart = String(frame.scrollLeft <= 1)
      frame.dataset.atEnd = String(frame.scrollLeft >= max - 1)
    }
    updateEdges()
    frame.addEventListener("scroll", updateEdges, { passive: true })
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(updateEdges)
    observer?.observe(frame)
    const tableElement = frame.querySelector("table")
    if (tableElement) observer?.observe(tableElement)
    return () => {
      frame.removeEventListener("scroll", updateEdges)
      observer?.disconnect()
    }
  }, [items, loading])

  const header = table.getHeaderGroups().map((group) => (
    <tr key={group.id}>
      {group.headers.map((headerCell) => <th data-sticky-cell={headerCell.column.id === "actions" ? "right" : undefined} key={headerCell.id}>{flexRender(headerCell.column.columnDef.header, headerCell.getContext())}</th>)}
    </tr>
  ))
  const skeletonRows = Array.from({ length: 6 }, (_, index) => (
    <tr aria-hidden="true" className="skeleton-row" key={`skeleton-${index}`}>
      {columns.map((_, cellIndex) => <td data-sticky-cell={cellIndex === columns.length - 1 ? "right" : undefined} key={cellIndex}><span className="skeleton-line" /></td>)}
    </tr>
  ))
  return (
    <div aria-busy={loading} className="data-dock">
      <div className="desktop-table" data-at-end="true" data-at-start="true" ref={scrollRef}>
        <table className="debt-table" data-slot="table">
          <thead>{header}</thead>
          <tbody>
            {loading ? skeletonRows : !items.length ? (
              <tr className="table-state-row"><td colSpan={columns.length}><div className="table-state"><CircleDollarSignIcon /><h2>暂无符合条件的债务</h2><p>调整筛选条件，或从“新增债务”开始记录。</p></div></td></tr>
            ) : table.getRowModel().rows.map((row) => (
              <tr data-slot="table-row" key={row.id} onClick={() => onOpen(row.original.id)} onKeyDown={(event) => { if (event.target === event.currentTarget && (event.key === "Enter" || event.key === " ")) { event.preventDefault(); onOpen(row.original.id) } }} tabIndex={0}>
                {row.getVisibleCells().map((cell) => (
                  <td data-sticky-cell={cell.column.id === "actions" ? "right" : undefined} key={cell.id}>
                    {flexRender(cell.column.columnDef.cell, cell.getContext())}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div className="mobile-debt-list">
        {loading ? Array.from({ length: 3 }, (_, index) => <div aria-hidden="true" className="debt-card debt-card-skeleton" key={index}><span className="skeleton-line" /><span className="skeleton-line" /><span className="skeleton-line" /></div>) : !items.length ? <div className="table-state"><CircleDollarSignIcon /><h2>暂无符合条件的债务</h2><p>调整筛选条件，或从“新增债务”开始记录。</p></div> : items.map((debt) => (
          <article className="debt-card" key={debt.id}>
            <div><strong>{debt.counterparty.displayName}</strong><StatusBadge status={debt.status} /></div>
            <div><DirectionLabel direction={debt.direction} /><strong>{yuan(debt.remainingCents)}</strong></div>
            <p className="debt-card-amounts"><span>本金 {yuan(debt.principalCents)}</span><span>已还 {yuan(debt.paidCents)}</span></p>
            <p>{debt.direction === "lend_out" ? "借出" : "借入"}日期 {debt.occurredOn} · 间隔 {elapsedCalendarDays(debt.occurredOn)} 天</p>
            <p>{debt.dueOn ? `${debt.dueOn} 到期` : "未设置到期日"}</p>
            <p>{principalAccountLabel(debt.direction)}：{ledgerAccountDisplayLabel(debt.account)}</p>
            <Button aria-label={`查看 ${debt.counterparty.displayName} 的债务详情`} onClick={() => onOpen(debt.id)} size="sm">查看详情</Button>
          </article>
        ))}
      </div>
    </div>
  )
}

function RowMenu({ debt, onOpen }: { debt: Debt; onOpen: (id: string) => void }) {
  return <div onClick={(event) => event.stopPropagation()}><ActionMenu items={[{ label: "查看与编辑", icon: <PencilIcon />, onSelect: () => onOpen(debt.id) }]} label={`操作 ${debt.counterparty.displayName}`} quiet /></div>
}

function ContactGrid({ items, loading }: { items: Counterparty[]; loading: boolean }) {
  if (loading) return <div className="table-state"><div className="spinner" /><p>正在汇总联系人…</p></div>
  if (!items.length) return <div className="table-state"><UsersIcon /><h2>暂无联系人</h2><p>创建债务时可同时建立联系人。</p></div>
  return <div className="contact-grid">{items.map((item) => <article className="contact-card" key={item.id}><div><span className="contact-avatar">{item.displayName.slice(0, 1)}</span><div><h2>{item.displayName}</h2><p>{item.activeDebtCount} 笔进行中{item.overdueCount ? ` · ${item.overdueCount} 笔逾期` : ""}</p></div></div><dl><div><dt>待收</dt><dd>{yuan(item.lendOutRemainingCents)}</dd></div><div><dt>待还</dt><dd>{yuan(item.borrowInRemainingCents)}</dd></div><div><dt>净额</dt><dd>{yuan(item.netCents)}</dd></div></dl></article>)}</div>
}

type DebtDraft = { direction: "borrow_in" | "lend_out"; accountId: string; counterpartyId: string; counterpartyName: string; principal: string; occurredOn: string; dueOn: string; note: string }
type InitialDebtMovement = { id: string; amountCents: number; effectiveOn: string; note: string; account: Debt["account"]; createdAt: string }

export function DebtFormModal({ accounts, accountsLoading = false, open, onOpenChange, counterparties, debt, onManageAccounts = () => undefined, onSaved }: { accounts?: LedgerAccount[]; accountsLoading?: boolean; open: boolean; onOpenChange: (open: boolean) => void; counterparties: Counterparty[]; debt?: Debt; onManageAccounts?: () => void; onSaved: () => Promise<unknown> }) {
  const toast = useToast()
  const initial = useMemo<DebtDraft>(() => ({ direction: (debt?.direction as DebtDraft["direction"]) || "borrow_in", accountId: debt?.account?.id || "", counterpartyId: debt?.counterparty.id || "", counterpartyName: "", principal: debt ? String(debt.principalCents / 100) : "", occurredOn: debt?.occurredOn || today(), dueOn: debt?.dueOn || "", note: debt?.note || "" }), [debt])
  const [draft, setDraft] = useState<DebtDraft>(initial)
  const [error, setError] = useState("")
  useEffect(() => { if (open) { setDraft(initial); setError("") } }, [open, initial])
  const mutation = useMutation({ mutationFn: async () => {
    const cents = toCents(draft.principal)
    if (!cents) throw new Error("请输入正确的本金金额")
    if (!draft.counterpartyId && !draft.counterpartyName.trim()) throw new Error("请选择或填写联系人")
    if (!draft.accountId) throw new Error(`请选择${principalAccountLabel(draft.direction)}`)
    if (debt) return api.updateDebt(debt.id, { version: debt.version, accountId: draft.accountId, counterpartyId: draft.counterpartyId, principalCents: cents, occurredOn: draft.occurredOn, dueOn: draft.dueOn || null, note: draft.note })
    return api.createDebt({ direction: draft.direction, accountId: draft.accountId, counterpartyId: draft.counterpartyId || null, counterpartyName: draft.counterpartyId ? null : draft.counterpartyName.trim(), principalCents: cents, occurredOn: draft.occurredOn, dueOn: draft.dueOn || null, note: draft.note })
  }, onSuccess: async () => {
    await onSaved()
    onOpenChange(false)
    toast({ title: debt ? "债务已更新" : "债务已新增", type: "success" })
  }, onError: (cause) => {
    setError(cause instanceof Error ? cause.message : "保存失败")
    if (cause instanceof ApiClientError && cause.status === 409) void onSaved()
  } })
  const set = <K extends keyof DebtDraft>(key: K, value: DebtDraft[K]) => setDraft((current) => ({ ...current, [key]: value }))
  const activeCounterparties = counterparties.filter((item) => !item.archived)
  const selectableCounterparties = debt && !activeCounterparties.some((item) => item.id === debt.counterparty.id)
    ? [debt.counterparty, ...activeCounterparties]
    : activeCounterparties
  const counterpartyOptions = debt
    ? selectableCounterparties.map((item) => ({ value: item.id, label: item.displayName }))
    : [{ value: NEW_COUNTERPARTY, label: "新联系人" }, ...activeCounterparties.map((item) => ({ value: item.id, label: item.displayName }))]
  const principalLocked = Boolean(debt && (debt.repayments.length > 0 || (debt.additions?.length || 0) > 0))
  const activeAccounts = (accounts || []).filter((account) => !account.archived)
  return <Modal open={open} onOpenChange={onOpenChange} title={debt ? "编辑债务" : "新增债务"} footer={<><Button disabled={mutation.isPending} onClick={() => onOpenChange(false)}>取消</Button><Button disabled={mutation.isPending || accountsLoading || activeAccounts.length === 0} onClick={() => mutation.mutate()} variant="default">{mutation.isPending ? "正在保存…" : "保存"}</Button></>}>
    <div className="form-stack">
      <div className="choice-field">
        <span className="field-label">方向</span>
        <div aria-describedby={debt ? "direction-locked-reason" : undefined} className="segmented"><button aria-pressed={draft.direction === "borrow_in"} disabled={Boolean(debt)} onClick={() => set("direction", "borrow_in")}><ArrowDownLeftIcon /> 借入（我欠别人）</button><button aria-pressed={draft.direction === "lend_out"} disabled={Boolean(debt)} onClick={() => set("direction", "lend_out")}><ArrowUpRightIcon /> 借出（别人欠我）</button></div>
        {debt ? <span className="field-description" id="direction-locked-reason">债务方向创建后不可修改</span> : null}
      </div>
      <MoneyAccountField accounts={accounts || []} label={principalAccountLabel(draft.direction)} loading={accountsLoading} onManage={() => { onOpenChange(false); onManageAccounts() }} onValueChange={(value) => set("accountId", value)} value={draft.accountId} />
      <Field className="field-medium" label="联系人"><Select ariaLabel="联系人" onValueChange={(value) => set("counterpartyId", value === NEW_COUNTERPARTY ? "" : value)} options={counterpartyOptions} value={draft.counterpartyId || NEW_COUNTERPARTY} /></Field>
      {!draft.counterpartyId ? <Field className="field-short" label="联系人名称"><Input value={draft.counterpartyName} onChange={(event) => set("counterpartyName", event.target.value)} placeholder="姓名或称呼" /></Field> : null}
      <Field className="field-number" description={principalLocked ? "已有追加或还款记录，本金不可修改" : undefined} label="本金（元）"><Input disabled={principalLocked} inputMode="decimal" value={draft.principal} onChange={(event) => set("principal", event.target.value)} placeholder="0.00" /></Field>
      <div className="form-grid"><Field label="发生日期"><DatePicker ariaLabel="发生日期" onValueChange={(value) => set("occurredOn", value)} value={draft.occurredOn} /></Field><Field label="到期日（可选）"><DatePicker ariaLabel="到期日（可选）" clearable onValueChange={(value) => set("dueOn", value)} placeholder="选择到期日" value={draft.dueOn} /></Field></div>
      <Field label="备注"><Textarea rows={4} value={draft.note} onChange={(event) => set("note", event.target.value)} placeholder="可选" /></Field>
      {error ? <InlineNotice type="error">{error}</InlineNotice> : null}
    </div>
  </Modal>
}

export function DebtDetailPage() {
  const { id: debtId } = useParams()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const toast = useToast()
  const query = useQuery({ queryKey: ["debt", debtId], queryFn: () => api.debt(debtId!), enabled: Boolean(debtId) })
  const counterparties = useQuery({ queryKey: ["counterparties"], queryFn: api.counterparties, enabled: Boolean(debtId) })
  const accounts = useQuery({ queryKey: ["ledger-accounts"], queryFn: api.ledgerAccounts, enabled: Boolean(debtId) })
  const debt = query.data
  const [editOpen, setEditOpen] = useState(false)
  const [movementOpen, setMovementOpen] = useState(false)
  const [editingAddition, setEditingAddition] = useState<Debt["additions"][number] | null>(null)
  const [editingRepayment, setEditingRepayment] = useState<Debt["repayments"][number] | null>(null)
  const [confirm, setConfirm] = useState<"delete" | "archive" | null>(null)
  const refresh = async () => Promise.all([queryClient.invalidateQueries({ queryKey: ["debts"] }), queryClient.invalidateQueries({ queryKey: ["summary"] }), queryClient.invalidateQueries({ queryKey: ["counterparties"] }), queryClient.invalidateQueries({ queryKey: ["ledger-accounts"] })])
  const mutation = useMutation({ mutationFn: async () => {
    if (!debt || !confirm) return
    if (confirm === "delete") return api.deleteDebt(debt.id, debt.version)
    return debt.archived ? api.restoreDebt(debt.id, debt.version) : api.archiveDebt(debt.id, debt.version)
  }, onSuccess: async () => {
    const completedAction = confirm
    await refresh()
    await queryClient.invalidateQueries({ queryKey: ["debt", debtId] })
    setConfirm(null)
    if (completedAction === "delete") navigate("/app/debts", { replace: true })
    toast({ title: completedAction === "delete" ? "债务已删除" : debt?.archived ? "债务已恢复" : "债务已归档", type: "success" })
  }, onError: (error) => toast({ title: "操作失败", description: error.message, type: "error" }) })
  const saved = async () => { await refresh(); await query.refetch() }
  const additions = debt?.additions || []
  const hasFollowUpHistory = Boolean(debt && (additions.length > 0 || debt.repayments.length > 0))
  const initialPrincipalCents = debt ? debt.principalCents - additions.reduce((sum, event) => sum + event.amountCents, 0) : 0
  const activities = debt ? [
    { type: "initial" as const, event: { id: debt.id, amountCents: initialPrincipalCents, effectiveOn: debt.occurredOn, note: debt.note, account: debt.account, createdAt: debt.createdAt } },
    ...additions.map((event) => ({ type: "addition" as const, event })),
    ...debt.repayments.map((event) => ({ type: "repayment" as const, event })),
  ].sort((left, right) => right.event.effectiveOn.localeCompare(left.event.effectiveOn)
    || right.event.createdAt.localeCompare(left.event.createdAt)
    || (left.type === "initial" ? 1 : right.type === "initial" ? -1 : 0)
    || right.event.id.localeCompare(left.event.id)) : []
  return <main className="workspace debt-detail-page">
    <h1 className="sr-only">债务详情</h1>
    {query.isLoading ? <div className="table-state debt-detail-state"><div className="spinner" /></div> : query.error ? <InlineNotice type="error">{query.error.message}</InlineNotice> : debt ? <div className="debt-detail-content">
      <section className="detail-overview-card" aria-label="债务概览">
        <header className="detail-overview-header">
          <div className="detail-contact-copy">
            <div className="detail-title-row">
              <h2 className="detail-contact-name">{debt.counterparty.displayName}</h2>
              <div className="detail-identity"><DirectionLabel direction={debt.direction} /><StatusBadge status={debt.status} /></div>
            </div>
            {debt.note ? <p className="detail-note">{debt.note}</p> : null}
          </div>
          <div className="detail-actions">
            {!debt.archived ? <Button className="detail-primary-action" onClick={() => setMovementOpen(true)} variant="primary"><CircleDollarSignIcon /> 登记往来</Button> : null}
            <Button onClick={() => setEditOpen(true)} variant="outline"><PencilIcon /> 编辑债务</Button>
            <ActionMenu label="更多债务操作" items={[
              { label: debt.archived ? "恢复债务" : "归档债务", icon: debt.archived ? <RotateCcwIcon /> : <ArchiveIcon />, onSelect: () => setConfirm("archive") },
              ...(!hasFollowUpHistory ? [{ label: "删除债务", icon: <Trash2Icon />, onSelect: () => setConfirm("delete"), destructive: true }] : []),
            ]} />
          </div>
        </header>
        <dl className="detail-metrics">
          <div className="detail-balance-metric"><dt>当前待{debt.direction === "lend_out" ? "收" : "还"}</dt><dd>{yuan(debt.remainingCents)}</dd></div>
          <div><dt>累计本金</dt><dd>{yuan(debt.principalCents)}</dd></div>
          <div><dt>净已还</dt><dd>{yuan(debt.paidCents)}</dd></div>
        </dl>
      </section>
      <div className="detail-workbench">
        <aside className="detail-information-card" aria-label="债务信息">
          <div className="detail-information-heading"><h3>债务信息</h3></div>
          <dl className="detail-information-list">
            <div><dt>发生日期</dt><dd>{debt.occurredOn}</dd></div>
            <div><dt>到期日期</dt><dd>{debt.dueOn || "未设置"}</dd></div>
            <div><dt>{principalAccountLabel(debt.direction)}</dt><dd>{ledgerAccountDisplayLabel(debt.account)}</dd></div>
          </dl>
        </aside>
        <div className="detail-main">
          <section className="detail-history" aria-label="债务往来记录">
            <div className="section-heading"><div><div className="section-title-row"><h3>往来记录</h3><span>{activities.length} 条</span></div></div></div>
            <ol className="timeline-list">{activities.map((activity) => activity.type === "initial" ? <InitialDebtRow debt={debt} event={activity.event} key={`initial:${activity.event.id}`} /> : activity.type === "addition" ? <DebtAdditionRow debt={debt} event={activity.event} key={`addition:${activity.event.id}`} onEdit={setEditingAddition} /> : <RepaymentRow debt={debt} event={activity.event} key={`repayment:${activity.event.id}`} onEdit={setEditingRepayment} onSaved={saved} />)}</ol>
          </section>
        </div>
      </div>
    </div> : null}
    {debt ? <DebtFormModal accounts={accounts.data || []} accountsLoading={accounts.isLoading} counterparties={counterparties.data || []} debt={debt} onManageAccounts={() => navigate("/app/accounts")} open={editOpen} onOpenChange={setEditOpen} onSaved={saved} /> : null}
    {debt ? <DebtMovementModal accounts={accounts.data || []} accountsLoading={accounts.isLoading} debt={debt} open={movementOpen} onOpenChange={setMovementOpen} onSaved={saved} /> : null}
    {debt && editingAddition ? <DebtAdditionModal accounts={accounts.data || []} accountsLoading={accounts.isLoading} debt={debt} event={editingAddition} open onOpenChange={(open) => !open && setEditingAddition(null)} onSaved={saved} /> : null}
    {debt && editingRepayment ? <RepaymentModal accounts={accounts.data || []} accountsLoading={accounts.isLoading} debt={debt} event={editingRepayment} open onOpenChange={(open) => !open && setEditingRepayment(null)} onSaved={saved} /> : null}
    <ConfirmDialog destructive={confirm === "delete"} open={Boolean(confirm)} onOpenChange={(open) => !open && setConfirm(null)} title={confirm === "delete" ? `删除“${debt?.counterparty.displayName || "该联系人"}”的债务？` : debt?.archived ? `恢复“${debt?.counterparty.displayName || "该联系人"}”的债务？` : `归档“${debt?.counterparty.displayName || "该联系人"}”的债务？`} description={confirm === "delete" ? "删除后无法恢复。只有没有追加或还款记录的债务可以删除。" : debt?.archived ? "恢复后，该笔债务会重新出现在默认列表中。" : "归档不会删除往来历史，可随时恢复。"} confirmLabel={confirm === "delete" ? "确认删除" : debt?.archived ? "确认恢复" : "确认归档"} onConfirm={() => mutation.mutate()} pending={mutation.isPending} />
  </main>
}

function DebtMovementModal({ accounts, accountsLoading, debt, open, onOpenChange, onSaved }: { accounts: LedgerAccount[]; accountsLoading: boolean; debt: Debt; open: boolean; onOpenChange: (open: boolean) => void; onSaved: () => Promise<unknown> }) {
  const navigate = useNavigate()
  const toast = useToast()
  const [action, setAction] = useState<"addition" | "repayment">("addition")
  const [amount, setAmount] = useState("")
  const [date, setDate] = useState(today())
  const [note, setNote] = useState("")
  const [accountId, setAccountId] = useState("")
  const [error, setError] = useState("")
  const additionLabel = debt.direction === "lend_out" ? "追加借出" : "追加借入"
  const isAddition = action === "addition"
  const actionLabel = isAddition ? additionLabel : "登记还款"
  const accountLabel = isAddition ? principalAccountLabel(debt.direction) : repaymentAccountLabel(debt.direction)
  const activeAccounts = accounts.filter((account) => !account.archived)
  const mutation = useMutation({ mutationFn: () => {
    const cents = toCents(amount)
    if (!cents) throw new Error(`请输入正确的${isAddition ? additionLabel : "还款"}金额`)
    if (!accountId) throw new Error(`请选择${accountLabel}`)
    return isAddition
      ? api.createDebtAddition(debt.id, { accountId, amountCents: cents, effectiveOn: date, note })
      : api.createRepayment(debt.id, { accountId, amountCents: cents, effectiveOn: date, note })
  }, onSuccess: async () => { await onSaved(); onOpenChange(false); toast({ title: `${actionLabel}已登记`, type: "success" }) }, onError: (cause) => { setError(cause instanceof Error ? cause.message : "登记失败"); if (cause instanceof ApiClientError && cause.status === 409) void onSaved() } })
  useEffect(() => { if (open) { setAction("addition"); setAmount(""); setDate(today()); setNote(""); setAccountId(""); setError("") } }, [open])
  return <Modal open={open} onOpenChange={onOpenChange} title="登记往来" description="选择本次往来动作后，填写金额、账户和日期。" footer={<><Button disabled={mutation.isPending} onClick={() => onOpenChange(false)}>取消</Button><Button disabled={mutation.isPending || accountsLoading || activeAccounts.length === 0} onClick={() => mutation.mutate()} variant="default">{mutation.isPending ? "正在登记…" : "确认登记"}</Button></>}><div className="form-stack"><Field className="field-medium" label="动作"><Select ariaLabel="动作" onValueChange={(value) => { setAction(value as "addition" | "repayment"); setError("") }} options={[{ value: "addition", label: additionLabel }, { value: "repayment", label: "登记还款" }]} value={action} /></Field><MoneyAccountField accounts={accounts} label={accountLabel} loading={accountsLoading} onManage={() => { onOpenChange(false); navigate("/app/accounts") }} onValueChange={setAccountId} value={accountId} /><Field className="field-number" label={`${isAddition ? additionLabel : "还款"}金额（元）`}><Input autoFocus inputMode="decimal" value={amount} onChange={(event) => setAmount(event.target.value)} placeholder="0.00" /></Field><Field label={isAddition ? "追加日期" : "还款日期"}><DatePicker ariaLabel={isAddition ? "追加日期" : "还款日期"} onValueChange={setDate} value={date} /></Field><Field label="备注"><Textarea value={note} onChange={(event) => setNote(event.target.value)} placeholder="可选" /></Field>{error ? <InlineNotice type="error">{error}</InlineNotice> : null}</div></Modal>
}

function DebtAdditionModal({ accounts, accountsLoading, debt, event, open, onOpenChange, onSaved }: { accounts: LedgerAccount[]; accountsLoading: boolean; debt: Debt; event?: Debt["additions"][number]; open: boolean; onOpenChange: (open: boolean) => void; onSaved: () => Promise<unknown> }) {
  const navigate = useNavigate()
  const toast = useToast()
  const [amount, setAmount] = useState("")
  const [date, setDate] = useState(today())
  const [note, setNote] = useState("")
  const [accountId, setAccountId] = useState("")
  const [movementType, setMovementType] = useState<"addition" | "repayment">("addition")
  const [error, setError] = useState("")
  const actionLabel = debt.direction === "lend_out" ? "追加借出" : "追加借入"
  const editing = Boolean(event)
  const convertsToRepayment = editing && movementType === "repayment"
  const displayedActionLabel = convertsToRepayment ? "登记还款" : actionLabel
  const activeAccounts = accounts.filter((account) => !account.archived)
  const mutation = useMutation({ mutationFn: () => {
    const cents = toCents(amount)
    if (!cents) throw new Error(`请输入正确的${actionLabel}金额`)
    if (event) return api.updateDebtAddition(event.id, { version: debt.version, amountCents: cents, effectiveOn: date, note, accountId: accountId || undefined, movementType: convertsToRepayment ? "repayment" : undefined })
    if (!accountId) throw new Error(`请选择${principalAccountLabel(debt.direction)}`)
    return api.createDebtAddition(debt.id, { accountId, amountCents: cents, effectiveOn: date, note })
  }, onSuccess: async () => { await onSaved(); onOpenChange(false); toast({ title: editing ? `${actionLabel}记录已更新` : `${actionLabel}已登记`, type: "success" }) }, onError: (cause) => { setError(cause instanceof Error ? cause.message : editing ? "更新失败" : "追加失败"); if (cause instanceof ApiClientError && cause.status === 409) void onSaved() } })
  useEffect(() => { if (open) { setAmount(event ? String(event.amountCents / 100) : ""); setDate(event?.effectiveOn || today()); setNote(event?.note || ""); setAccountId(event?.account?.id || ""); setMovementType("addition"); setError("") } }, [open, event])
  return <Modal open={open} onOpenChange={onOpenChange} title={editing ? `编辑${displayedActionLabel}` : actionLabel} description={editing ? "可修改金额、账户、日期、备注和往来类型。" : "追加后，本金与剩余金额都会增加。"} footer={<><Button disabled={mutation.isPending} onClick={() => onOpenChange(false)}>取消</Button><Button disabled={mutation.isPending || accountsLoading || (!editing && activeAccounts.length === 0)} onClick={() => mutation.mutate()} variant="default">{mutation.isPending ? editing ? "正在保存…" : "正在追加…" : editing ? "保存修改" : "确认追加"}</Button></>}><div className="form-stack">{editing ? <Field className="field-medium" label="动作"><Select ariaLabel="动作" onValueChange={(value) => setMovementType(value as "addition" | "repayment")} options={[{ value: "addition", label: actionLabel }, { value: "repayment", label: "登记还款" }]} value={movementType} /></Field> : null}<MoneyAccountField accounts={accounts} allowEmpty={editing} label={convertsToRepayment ? repaymentAccountLabel(debt.direction) : principalAccountLabel(debt.direction)} loading={accountsLoading} onManage={() => { onOpenChange(false); navigate("/app/accounts") }} onValueChange={setAccountId} value={accountId} /><Field className="field-number" label={`${convertsToRepayment ? "还款" : actionLabel}金额（元）`}><Input autoFocus inputMode="decimal" value={amount} onChange={(event) => setAmount(event.target.value)} placeholder="0.00" /></Field><Field label={convertsToRepayment ? "还款日期" : "追加日期"}><DatePicker ariaLabel={convertsToRepayment ? "还款日期" : "追加日期"} onValueChange={setDate} value={date} /></Field><Field label="备注"><Textarea value={note} onChange={(event) => setNote(event.target.value)} placeholder="可选" /></Field>{error ? <InlineNotice type="error">{error}</InlineNotice> : null}</div></Modal>
}

function RepaymentModal({ accounts, accountsLoading, debt, event, open, onOpenChange, onSaved }: { accounts: LedgerAccount[]; accountsLoading: boolean; debt: Debt; event?: Debt["repayments"][number]; open: boolean; onOpenChange: (open: boolean) => void; onSaved: () => Promise<unknown> }) {
  const navigate = useNavigate()
  const toast = useToast()
  const [amount, setAmount] = useState("")
  const [date, setDate] = useState(today())
  const [note, setNote] = useState("")
  const [accountId, setAccountId] = useState("")
  const [movementType, setMovementType] = useState<"addition" | "repayment">("repayment")
  const [error, setError] = useState("")
  const editing = Boolean(event)
  const additionLabel = debt.direction === "lend_out" ? "追加借出" : "追加借入"
  const convertsToAddition = editing && movementType === "addition"
  const activeAccounts = accounts.filter((account) => !account.archived)
  const mutation = useMutation({ mutationFn: () => {
    const cents = toCents(amount)
    if (!cents) throw new Error("请输入正确的还款金额")
    if (event) return api.updateRepayment(event.id, { version: debt.version, amountCents: cents, effectiveOn: date, note, accountId: accountId || undefined, movementType: convertsToAddition ? "addition" : undefined })
    if (!accountId) throw new Error(`请选择${repaymentAccountLabel(debt.direction)}`)
    return api.createRepayment(debt.id, { accountId, amountCents: cents, effectiveOn: date, note })
  }, onSuccess: async () => { await onSaved(); onOpenChange(false); toast({ title: editing ? "还款记录已更新" : "还款已登记", type: "success" }) }, onError: (cause) => { setError(cause instanceof Error ? cause.message : editing ? "更新失败" : "登记失败"); if (cause instanceof ApiClientError && cause.status === 409) void onSaved() } })
  useEffect(() => { if (open) { setAmount(event ? String(event.amountCents / 100) : ""); setDate(event?.effectiveOn || today()); setNote(event?.note || ""); setAccountId(event?.account?.id || ""); setMovementType("repayment"); setError("") } }, [open, event])
  return <Modal open={open} onOpenChange={onOpenChange} title={editing ? convertsToAddition ? `编辑${additionLabel}` : "编辑还款记录" : "登记还款"} description={editing ? "可修改金额、账户、日期、备注和往来类型。" : `当前剩余 ${yuan(debt.remainingCents)}，禁止超额还款。`} footer={<><Button disabled={mutation.isPending} onClick={() => onOpenChange(false)}>取消</Button><Button disabled={mutation.isPending || accountsLoading || (!editing && activeAccounts.length === 0)} onClick={() => mutation.mutate()} variant="default">{mutation.isPending ? editing ? "正在保存…" : "正在登记…" : editing ? "保存修改" : "确认登记"}</Button></>}><div className="form-stack">{editing ? <Field className="field-medium" label="动作"><Select ariaLabel="动作" onValueChange={(value) => setMovementType(value as "addition" | "repayment")} options={[{ value: "repayment", label: "登记还款" }, { value: "addition", label: additionLabel }]} value={movementType} /></Field> : null}<MoneyAccountField accounts={accounts} allowEmpty={editing} label={convertsToAddition ? principalAccountLabel(debt.direction) : repaymentAccountLabel(debt.direction)} loading={accountsLoading} onManage={() => { onOpenChange(false); navigate("/app/accounts") }} onValueChange={setAccountId} value={accountId} /><Field label={`${convertsToAddition ? additionLabel : "还款"}金额（元）`}><Input inputMode="decimal" value={amount} onChange={(event) => setAmount(event.target.value)} /></Field><Field label={convertsToAddition ? "追加日期" : "还款日期"}><DatePicker ariaLabel={convertsToAddition ? "追加日期" : "还款日期"} onValueChange={setDate} value={date} /></Field><Field label="备注"><Textarea value={note} onChange={(event) => setNote(event.target.value)} /></Field>{error ? <InlineNotice type="error">{error}</InlineNotice> : null}</div></Modal>
}

function RepaymentRow({ debt, event, onEdit, onSaved }: { debt: Debt; event: Debt["repayments"][number]; onEdit: (event: Debt["repayments"][number]) => void; onSaved: () => Promise<unknown> }) {
  const [confirm, setConfirm] = useState(false)
  const toast = useToast()
  const mutation = useMutation({ mutationFn: () => api.reverseRepayment(event.id, { effectiveOn: today(), note: "撤销错误还款" }), onSuccess: async () => { await onSaved(); setConfirm(false); toast({ title: "还款已撤销", type: "success" }) }, onError: (error) => toast({ title: "撤销失败", description: error.message, type: "error" }) })
  const isPayment = event.kind === "payment"
  const canEdit = isPayment && !event.reversed && !debt.archived
  const label = `${isPayment ? "" : "原"}${repaymentAccountLabel(debt.direction)}`
  const note = visibleNote(event.note, event.account)
  const title = isPayment ? "还款" : "撤销还款"
  return <>
    <MovementTimelineRow
      account={event.account}
      accountLabel={label}
      actions={canEdit ? [
        { label: "编辑记录", icon: <PencilIcon />, onSelect: () => onEdit(event) },
        { label: "撤销还款", icon: <RotateCcwIcon />, onSelect: () => setConfirm(true) },
      ] : undefined}
      actionLabel={`操作 ${event.effectiveOn} ${title} ${yuan(event.amountCents)}`}
      amountCents={event.amountCents}
      date={event.effectiveOn}
      icon={isPayment ? <CircleDollarSignIcon /> : <RotateCcwIcon />}
      reversed={event.reversed}
      note={note}
      title={title}
      tone="repayment"
    />
    <ConfirmDialog destructive={false} open={confirm} onOpenChange={setConfirm} title={`撤销 ${yuan(event.amountCents)} 的还款？`} description="撤销会新增一条补偿记录，并重新增加债务剩余金额。历史记录不会删除。" confirmLabel="确认撤销" onConfirm={() => mutation.mutate()} pending={mutation.isPending} />
  </>
}

function InitialDebtRow({ debt, event }: { debt: Debt; event: InitialDebtMovement }) {
  const isLendOut = debt.direction === "lend_out"
  const note = visibleNote(event.note, event.account)
  return <MovementTimelineRow account={event.account} accountLabel={principalAccountLabel(debt.direction)} amountCents={event.amountCents} date={event.effectiveOn} icon={isLendOut ? <ArrowUpRightIcon /> : <ArrowDownLeftIcon />} note={note} title={isLendOut ? "初始借出" : "初始借入"} tone={isLendOut ? "lend" : "borrow"} />
}

function DebtAdditionRow({ debt, event, onEdit }: { debt: Debt; event: Debt["additions"][number]; onEdit: (event: Debt["additions"][number]) => void }) {
  const isLendOut = debt.direction === "lend_out"
  const note = visibleNote(event.note, event.account)
  const title = isLendOut ? "追加借出" : "追加借入"
  return <MovementTimelineRow account={event.account} accountLabel={principalAccountLabel(debt.direction)} actions={!debt.archived ? [{ label: "编辑记录", icon: <PencilIcon />, onSelect: () => onEdit(event) }] : undefined} actionLabel={`操作 ${event.effectiveOn} ${title} ${yuan(event.amountCents)}`} amountCents={event.amountCents} date={event.effectiveOn} icon={isLendOut ? <ArrowUpRightIcon /> : <ArrowDownLeftIcon />} note={note} title={title} tone={isLendOut ? "lend" : "borrow"} />
}

type TimelineAction = { label: string; icon: ReactNode; onSelect: () => void }

function MovementTimelineRow({
  account,
  accountLabel,
  actionLabel,
  actions,
  amountCents,
  date,
  icon,
  note,
  reversed = false,
  title,
  tone,
}: {
  account: LedgerAccountRef | null
  accountLabel: string
  actionLabel?: string
  actions?: TimelineAction[]
  amountCents: number
  date: string
  icon: ReactNode
  note: string
  reversed?: boolean
  title: string
  tone: "repayment" | "borrow" | "lend"
}) {
  return <li className={`timeline-row timeline-row-${tone} ${reversed ? "is-reversed" : ""}`}>
    <time className="timeline-date" dateTime={date}>{date}</time>
    <span aria-hidden="true" className="timeline-icon">{icon}</span>
    <div className="timeline-entry">
      <div className="timeline-copy">
        <div className="timeline-heading"><strong>{title} {yuan(amountCents)}</strong>{reversed ? <span className="timeline-reversed-badge">已撤销</span> : null}</div>
        <div className="timeline-meta">
          <span className="timeline-account"><span>{accountLabel}：</span><span>{ledgerAccountDisplayLabel(account)}</span></span>
          {note ? <span className="timeline-note"><span>备注：</span><span>{note}</span></span> : null}
        </div>
      </div>
      {actions?.length && actionLabel ? <ActionMenu items={actions} label={actionLabel} quiet /> : <span aria-hidden="true" className="timeline-action-spacer" />}
    </div>
  </li>
}
