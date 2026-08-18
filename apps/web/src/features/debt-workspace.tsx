import { useQuery, useQueryClient } from "@tanstack/react-query"
import { createColumnHelper, flexRender, getCoreRowModel, useReactTable } from "@tanstack/react-table"
import {
  ArchiveIcon,
  ArrowDownLeftIcon,
  ArrowUpRightIcon,
  CircleDollarSignIcon,
  HandCoinsIcon,
  LinkIcon,
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
import { useIdempotentMutation } from "../api/use-idempotent-mutation"
import type { Counterparty, Debt, DebtStatus, LedgerAccount, LedgerAccountRef, LedgerTransaction, Summary, TransactionLinkCandidate } from "../api/types"
import {
  Button,
  ActionMenu,
  CheckboxControl,
  ConfirmDialog,
  CreatableSelect,
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
import { useTopbarSlots } from "../components/topbar-slots"
import { elapsedCalendarDays } from "./debt-time"
import { ledgerAccountDisplayLabel } from "./ledger-account"
import { TransactionLinkPicker } from "./transaction-link-picker"
import { debtDraftCounterparty, transactionDebtDirection } from "./transaction-debt"

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
const debtOriginAccountText = (debt: { account: LedgerAccountRef | null; originKind: Debt["originKind"] }) => {
  if (debt.account) return ledgerAccountDisplayLabel(debt.account)
  return debt.originKind === "no_cash_movement" ? "无资金进出" : "历史未指定"
}
const debtOriginAccountLabel = (debt: { account: LedgerAccountRef | null; direction: Debt["direction"]; originKind: Debt["originKind"] }) =>
  !debt.account && debt.originKind === "no_cash_movement" ? "资金往来" : principalAccountLabel(debt.direction)
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
  disabled = false,
}: {
  accounts: LedgerAccount[]
  label: string
  loading: boolean
  onManage: () => void
  onValueChange: (value: string) => void
  value: string
  allowEmpty?: boolean
  disabled?: boolean
}) {
  const selectableAccounts = accounts.filter((account) => !account.archived || account.id === value)
  const empty = !loading && selectableAccounts.length === 0
  return <>
    <Field className="field-medium" label={label}>
      <Select
        ariaLabel={label}
        disabled={disabled || loading || empty}
        onValueChange={onValueChange}
        options={selectableAccounts.map((account) => ({ value: account.id, label: `${ledgerAccountDisplayLabel(account)}${account.archived ? "（已归档）" : ""}` }))}
        placeholder={loading ? "正在加载账户…" : empty ? allowEmpty ? "历史未指定（可保留）" : "暂无可用账户" : allowEmpty && !value ? "历史未指定（可保留）" : "请选择账户"}
        value={value}
      />
    </Field>
    {empty && !allowEmpty ? <InlineNotice><span>请先新增可用于付款或收款的账户。</span><Button onClick={onManage} size="sm">前往账户</Button></InlineNotice> : null}
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
  return <section className="summary-strip" aria-label="债务汇总">{items.map(([label, value, className]) => <div className="summary-strip-item" key={String(label)}><span>{label}</span><strong className={String(className)}>{loading ? "—" : typeof value === "number" ? yuan(value) : value}</strong></div>)}</section>
}

export function DebtWorkspace() {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const setTopbarSlots = useTopbarSlots()
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

  // 页面名和主操作都归顶栏（kiln：Title Answer 由产品统一挑一处，这个产品挑的是顶栏，
  // 日历/流水/统计/导入本来就在那儿）。正文里那个 <h1>债务</h1> 是第二个标题权威：
  // 侧边栏当前项已经是高亮的「债务」，顶栏又挂着「个人账本」这个常量，同一个页面名
  // 用户要读三遍，而顶栏的标题槽在这一页白摆着。
  useEffect(() => {
    setTopbarSlots({
      title: "债务",
      actions: <Button aria-label="新增债务" onClick={() => setCreateOpen(true)} variant="primary"><PlusIcon /><span className="button-label">新增债务</span></Button>,
    })
    return () => setTopbarSlots(undefined)
  }, [setTopbarSlots])

  return (
    <main className="workspace debt-workspace">
      <TabsRoot className="workspace-tabs debt-workspace-tabs" value={view} onValueChange={setView}>
        <header className="debt-commandbar">
          <TabsList aria-label="债务查看方式">
            <TabsTrigger value="debts"><HandCoinsIcon /> 按债务</TabsTrigger>
            <TabsTrigger value="contacts"><UsersIcon /> 按联系人</TabsTrigger>
          </TabsList>
        </header>
        {/* 汇总是表格的上下文，不是命令栏的控件：留在命令栏里它既撑高那一行，又只能
            分到中间一段宽度；单独一条通栏后四个数字才等分，也和下面的表格同宽。 */}
        <MetricStrip loading={summaryQuery.isLoading} summary={summaryQuery.data} />
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
          <TablePagination disabled={debtsQuery.isLoading} page={page} pageSize={20} total={debtsQuery.data?.total || 0} unit="笔" onPageChange={setPage} />
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
    columnHelper.accessor("account", { header: "资金账户", cell: ({ row }) => debtOriginAccountText(row.original) }),
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
            <p>{debtOriginAccountLabel(debt)}：{debtOriginAccountText(debt)}</p>
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

type DebtDraft = { direction: "borrow_in" | "lend_out"; originKind: "cash_movement" | "no_cash_movement"; accountId: string; counterpartyId: string; counterpartyName: string; principal: string; occurredOn: string; dueOn: string; note: string }
type InitialDebtMovement = { id: string; amountCents: number; effectiveOn: string; note: string; account: Debt["account"]; createdAt: string; transactionId: string | null }

export function DebtFormModal({ accounts, accountsLoading = false, open, onOpenChange, counterparties, debt, initialTransaction, onManageAccounts = () => undefined, onSaved }: { accounts?: LedgerAccount[]; accountsLoading?: boolean; open: boolean; onOpenChange: (open: boolean) => void; counterparties: Counterparty[]; debt?: Debt; initialTransaction?: LedgerTransaction; onManageAccounts?: () => void; onSaved: () => Promise<unknown> }) {
  const toast = useToast()
  const initial = useMemo<DebtDraft>(() => {
    const counterparty = initialTransaction ? debtDraftCounterparty(initialTransaction, counterparties) : { counterpartyId: "", counterpartyName: "" }
    const direction: DebtDraft["direction"] = (debt?.direction as DebtDraft["direction"] | undefined) || (initialTransaction ? transactionDebtDirection(initialTransaction.kind) : "borrow_in")
    return { direction, originKind: debt && debt.originKind !== "cash_movement" ? "no_cash_movement" : "cash_movement", accountId: debt?.account?.id || initialTransaction?.account?.id || "", counterpartyId: debt?.counterparty.id || counterparty.counterpartyId, counterpartyName: counterparty.counterpartyName, principal: debt ? String(debt.principalCents / 100) : initialTransaction ? String(initialTransaction.amountCents / 100) : "", occurredOn: debt?.occurredOn || initialTransaction?.occurredOn || today(), dueOn: debt?.dueOn || "", note: debt?.note || initialTransaction?.note || "" }
  }, [debt, initialTransaction, counterparties])
  const [draft, setDraft] = useState<DebtDraft>(initial)
  const [linkedTransaction, setLinkedTransaction] = useState<TransactionLinkCandidate | null>(null)
  const [linkCleared, setLinkCleared] = useState(false)
  const [error, setError] = useState("")
  useEffect(() => { if (open) { setDraft(initial); setLinkedTransaction(debt?.transactionId ? { id: debt.transactionId, kind: debt.direction === "lend_out" ? "expense" : "income", amountCents: debt.principalCents, occurredOn: debt.occurredOn, note: debt.note, account: debt.account } : initialTransaction ? { id: initialTransaction.id, kind: initialTransaction.kind, amountCents: initialTransaction.amountCents, occurredOn: initialTransaction.occurredOn, note: initialTransaction.note, account: initialTransaction.account } : null); setLinkCleared(false); setError("") } }, [open, initial, debt, initialTransaction])
  const mutation = useIdempotentMutation({ mutationFn: async (_variables: void, write) => {
    const cents = toCents(draft.principal)
    if (!cents) throw new Error("请输入正确的本金金额")
    if (!draft.counterpartyId && !draft.counterpartyName.trim()) throw new Error("请选择或填写联系人")
    if (draft.originKind === "cash_movement" && !draft.accountId && !linkedTransaction) throw new Error(`请选择${principalAccountLabel(draft.direction)}`)
    const accountId = draft.originKind === "cash_movement" ? draft.accountId : null
    const originKind = draft.originKind
    if (debt) return api.updateDebt(debt.id, { version: debt.version, accountId, originKind, counterpartyId: draft.counterpartyId, principalCents: cents, occurredOn: draft.occurredOn, dueOn: draft.dueOn || null, note: draft.note, ...(linkedTransaction ? { transactionId: linkedTransaction.id } : linkCleared ? { transactionId: null } : {}) }, write)
    return api.createDebt({ direction: draft.direction, accountId, originKind, counterpartyId: draft.counterpartyId || null, counterpartyName: draft.counterpartyId ? null : draft.counterpartyName.trim(), principalCents: cents, occurredOn: draft.occurredOn, dueOn: draft.dueOn || null, note: draft.note, transactionId: linkedTransaction?.id || null }, write)
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
    : activeCounterparties.map((item) => ({ value: item.id, label: item.displayName }))
  const principalLocked = Boolean(debt && (debt.repayments.length > 0 || (debt.additions?.length || 0) > 0))
  const activeAccounts = (accounts || []).filter((account) => !account.archived)
  const needsAccount = draft.originKind === "cash_movement"
  const transactionLocked = Boolean(initialTransaction)
  return <Modal open={open} onOpenChange={onOpenChange} title={debt ? "编辑债务" : "新增债务"} footer={<><Button disabled={mutation.isPending} onClick={() => onOpenChange(false)}>取消</Button><Button disabled={mutation.isPending || accountsLoading || (needsAccount && activeAccounts.length === 0)} onClick={() => mutation.mutate()} variant="default">{mutation.isPending ? "正在保存…" : "保存"}</Button></>}>
    <div className="form-stack">
      <div className="choice-field">
        <span className="field-label">方向</span>
        <div aria-describedby={debt || transactionLocked ? "direction-locked-reason" : undefined} className="segmented"><button aria-pressed={draft.direction === "borrow_in"} disabled={Boolean(debt) || transactionLocked} onClick={() => set("direction", "borrow_in")}><ArrowDownLeftIcon /> 借入（我欠别人）</button><button aria-pressed={draft.direction === "lend_out"} disabled={Boolean(debt) || transactionLocked} onClick={() => set("direction", "lend_out")}><ArrowUpRightIcon /> 借出（别人欠我）</button></div>
        {debt || transactionLocked ? <span className="field-description" id="direction-locked-reason">{transactionLocked ? "方向由关联流水自动确定" : "债务方向创建后不可修改"}</span> : null}
      </div>
      <div className="choice-field">
        <span className="field-label">资金往来</span>
        <div className="segmented"><button aria-pressed={draft.originKind === "cash_movement"} disabled={transactionLocked} onClick={() => set("originKind", "cash_movement")}><CircleDollarSignIcon /> 有实际收付款</button><button aria-pressed={draft.originKind === "no_cash_movement"} disabled={transactionLocked} onClick={() => set("originKind", "no_cash_movement")}><HandCoinsIcon /> 赊账·无资金进出</button></div>
        {needsAccount ? null : <span className="field-description">适用于赊购服务、先消费后付款等没有资金进出的场景，不计入任何账户流水。</span>}
      </div>
      {needsAccount ? <MoneyAccountField accounts={accounts || []} disabled={transactionLocked} label={principalAccountLabel(draft.direction)} loading={accountsLoading} onManage={() => { onOpenChange(false); onManageAccounts() }} onValueChange={(value) => set("accountId", value)} value={draft.accountId} /> : null}
      {needsAccount && !principalLocked && !transactionLocked ? <TransactionLinkPicker debtId={debt?.id} label="从流水选取本金" selected={linkedTransaction} onClear={() => { setLinkedTransaction(null); setLinkCleared(true) }} onSelect={(item) => { setLinkedTransaction(item); setLinkCleared(false); setDraft((current) => ({ ...current, originKind: "cash_movement", accountId: item.account?.id || "", principal: String(item.amountCents / 100), occurredOn: item.occurredOn, counterpartyName: current.counterpartyId ? current.counterpartyName : item.note })) }} /> : null}
      {debt
        ? <Field className="field-medium" label="联系人"><Select ariaLabel="联系人" onValueChange={(value) => set("counterpartyId", value)} options={counterpartyOptions} value={draft.counterpartyId} /></Field>
        : <Field className="field-medium" label="联系人"><CreatableSelect ariaLabel="联系人" onSelect={(value) => set("counterpartyId", value)} onTextChange={(name) => setDraft((current) => ({ ...current, counterpartyId: "", counterpartyName: name }))} options={counterpartyOptions} placeholder="选择或输入姓名" text={draft.counterpartyName} value={draft.counterpartyId} /></Field>}
      <div className="form-grid form-grid-3">
        <Field description={principalLocked ? "已有追加或还款记录，本金不可修改" : undefined} label="本金（元）"><Input disabled={principalLocked || Boolean(linkedTransaction)} inputMode="decimal" value={draft.principal} onChange={(event) => set("principal", event.target.value)} placeholder="0.00" /></Field>
        <Field label="发生日期"><DatePicker ariaLabel="发生日期" disabled={Boolean(linkedTransaction)} onValueChange={(value) => set("occurredOn", value)} value={draft.occurredOn} /></Field>
        <Field label="到期日（可选）"><DatePicker ariaLabel="到期日（可选）" clearable onValueChange={(value) => set("dueOn", value)} placeholder="选择到期日" value={draft.dueOn} /></Field>
      </div>
      <Field label="备注"><Textarea rows={3} value={draft.note} onChange={(event) => set("note", event.target.value)} placeholder="可选" /></Field>
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
  const [linkingInitial, setLinkingInitial] = useState(false)
  const [linkingEvent, setLinkingEvent] = useState<{ kind: "addition" | "repayment"; event: Debt["additions"][number] | Debt["repayments"][number] } | null>(null)
  const [confirm, setConfirm] = useState<"delete" | "archive" | null>(null)
  const refresh = async () => Promise.all([queryClient.invalidateQueries({ queryKey: ["debts"] }), queryClient.invalidateQueries({ queryKey: ["summary"] }), queryClient.invalidateQueries({ queryKey: ["counterparties"] }), queryClient.invalidateQueries({ queryKey: ["ledger-accounts"] })])
  // 动作和归档状态在点确认那一刻就冻结成参数传进来：确认框一关，Radix 就会把
  // confirm 置空，晚一步再读 state 会读到 null，整个 mutation 静默变成空操作。
  const mutation = useIdempotentMutation({ mutationFn: async ({ action, wasArchived }: { action: "delete" | "archive"; wasArchived: boolean }, write) => {
    if (!debt) return
    if (action === "delete") return api.deleteDebt(debt.id, debt.version, write)
    return wasArchived ? api.restoreDebt(debt.id, debt.version, write) : api.archiveDebt(debt.id, debt.version, write)
  }, onSuccess: async (_data, { action: completedAction, wasArchived }) => {
    // 删除和归档之后这个页面都没有意义了：删除后详情必然 404，归档后停在这里
    // 盯着一笔已归档的债务也无事可做。只有「恢复」应该留下——用户要看恢复的结果。
    const shouldLeave = completedAction === "delete" || !wasArchived
    setConfirm(null)
    // 顺序要紧：先离开，再刷数据。反过来的话详情查询会先跑一遍，把
    //「找不到该笔债务」渲染出来，用户在跳走前先看到一闪而过的报错。
    if (shouldLeave) navigate("/app/debts", { replace: true })
    await refresh()
    await queryClient.invalidateQueries({ queryKey: ["debt", debtId] })
    toast({ title: completedAction === "delete" ? "债务已删除" : wasArchived ? "债务已恢复" : "债务已归档", type: "success" })
  }, onError: (error) => toast({ title: "操作失败", description: error.message, type: "error" }) })
  const saved = async () => { await refresh(); await query.refetch() }
  const additions = debt?.additions || []
  const hasFollowUpHistory = Boolean(debt && (additions.length > 0 || debt.repayments.length > 0))
  const initialPrincipalCents = debt ? debt.principalCents - additions.reduce((sum, event) => sum + event.amountCents, 0) : 0
  const activities = debt ? [
    { type: "initial" as const, event: { id: debt.id, amountCents: initialPrincipalCents, effectiveOn: debt.occurredOn, note: debt.note, account: debt.account, createdAt: debt.createdAt, transactionId: debt.transactionId || null } },
    ...additions.map((event) => ({ type: "addition" as const, event })),
    ...debt.repayments.map((event) => ({ type: "repayment" as const, event })),
  ].sort((left, right) => right.event.effectiveOn.localeCompare(left.event.effectiveOn)
    || right.event.createdAt.localeCompare(left.event.createdAt)
    || (left.type === "initial" ? 1 : right.type === "initial" ? -1 : 0)
    || right.event.id.localeCompare(left.event.id)) : []
  {/* 详情路由的页面名就是那条记录，所以 h1 归概览卡里的联系人名（kiln：记录路由由
      主记录面持有标题）。原来这里挂的是一个 sr-only 的「债务详情」——它命名的是模板
      而不是记录，读屏用户在七条债务之间听到的是同一句话。 */}
  return <main className="workspace debt-detail-page">
    {query.isLoading ? <div className="table-state debt-detail-state"><div className="spinner" /></div> : query.error ? <InlineNotice type="error">{query.error.message}</InlineNotice> : debt ? <div className="debt-detail-content">
      <section className="detail-overview-card" aria-label="债务概览">
        <header className="detail-overview-header">
          <div className="detail-contact-copy">
            <div className="detail-title-row">
              <h1 className="detail-contact-name">{debt.counterparty.displayName}</h1>
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
            <div><dt>{debtOriginAccountLabel(debt)}</dt><dd>{debtOriginAccountText(debt)}</dd></div>
          </dl>
        </aside>
        <div className="detail-main">
          <section className="detail-history" aria-label="债务往来记录">
            <div className="section-heading"><div><div className="section-title-row"><h3>往来记录</h3><span>{activities.length} 条</span></div></div></div>
            <ol className="timeline-list">{activities.map((activity) => activity.type === "initial" ? <InitialDebtRow debt={debt} event={activity.event} key={`initial:${activity.event.id}`} onEdit={() => setEditOpen(true)} onLink={() => setLinkingInitial(true)} /> : activity.type === "addition" ? <DebtAdditionRow debt={debt} event={activity.event} key={`addition:${activity.event.id}`} onEdit={setEditingAddition} onLink={(event) => setLinkingEvent({ kind: "addition", event })} /> : <RepaymentRow debt={debt} event={activity.event} key={`repayment:${activity.event.id}`} onEdit={setEditingRepayment} onLink={(event) => setLinkingEvent({ kind: "repayment", event })} onSaved={saved} />)}</ol>
          </section>
        </div>
      </div>
    </div> : null}
    {debt ? <DebtFormModal accounts={accounts.data || []} accountsLoading={accounts.isLoading} counterparties={counterparties.data || []} debt={debt} onManageAccounts={() => navigate("/app/accounts")} open={editOpen} onOpenChange={setEditOpen} onSaved={saved} /> : null}
    {debt ? <DebtMovementModal accounts={accounts.data || []} accountsLoading={accounts.isLoading} debt={debt} open={movementOpen} onOpenChange={setMovementOpen} onSaved={saved} /> : null}
    {debt && editingAddition ? <DebtAdditionModal accounts={accounts.data || []} accountsLoading={accounts.isLoading} debt={debt} event={editingAddition} open onOpenChange={(open) => !open && setEditingAddition(null)} onSaved={saved} /> : null}
    {debt && editingRepayment ? <RepaymentModal accounts={accounts.data || []} accountsLoading={accounts.isLoading} debt={debt} event={editingRepayment} open onOpenChange={(open) => !open && setEditingRepayment(null)} onSaved={saved} /> : null}
    {debt && linkingInitial ? <InitialTransactionLinkModal debt={debt} open onOpenChange={setLinkingInitial} onSaved={saved} /> : null}
    {debt && linkingEvent ? <TimelineLinkModal debt={debt} kind={linkingEvent.kind} event={linkingEvent.event} open onOpenChange={(open) => !open && setLinkingEvent(null)} onSaved={saved} /> : null}
    <ConfirmDialog destructive={confirm === "delete"} open={Boolean(confirm)} onOpenChange={(open) => !open && setConfirm(null)} title={confirm === "delete" ? `删除“${debt?.counterparty.displayName || "该联系人"}”的债务？` : debt?.archived ? `恢复“${debt?.counterparty.displayName || "该联系人"}”的债务？` : `归档“${debt?.counterparty.displayName || "该联系人"}”的债务？`} description={confirm === "delete" ? "删除后无法恢复。只有没有追加或还款记录的债务可以删除。" : debt?.archived ? "恢复后，该笔债务会重新出现在默认列表中。" : "归档不会删除往来历史，可随时恢复。"} confirmLabel={confirm === "delete" ? "确认删除" : debt?.archived ? "确认恢复" : "确认归档"} onConfirm={() => confirm && mutation.mutate({ action: confirm, wasArchived: Boolean(debt?.archived) })} pending={mutation.isPending} />
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
  const [linkedTransaction, setLinkedTransaction] = useState<TransactionLinkCandidate | null>(null)
  const [error, setError] = useState("")
  const additionLabel = debt.direction === "lend_out" ? "追加借出" : "追加借入"
  const isAddition = action === "addition"
  const actionLabel = isAddition ? additionLabel : "登记还款"
  const accountLabel = isAddition ? principalAccountLabel(debt.direction) : repaymentAccountLabel(debt.direction)
  const activeAccounts = accounts.filter((account) => !account.archived)
  const mutation = useIdempotentMutation({ mutationFn: (_variables: void, write) => {
    const cents = toCents(amount)
    if (!cents) throw new Error(`请输入正确的${isAddition ? additionLabel : "还款"}金额`)
    if (!accountId && !linkedTransaction) throw new Error(`请选择${accountLabel}`)
    return isAddition
      ? api.createDebtAddition(debt.id, { accountId: accountId || null, amountCents: cents, effectiveOn: date, note, ...(linkedTransaction ? { transactionId: linkedTransaction.id } : {}) }, write)
      : api.createRepayment(debt.id, { accountId: accountId || null, amountCents: cents, effectiveOn: date, note, ...(linkedTransaction ? { transactionId: linkedTransaction.id } : {}) }, write)
  }, onSuccess: async () => { await onSaved(); onOpenChange(false); toast({ title: `${actionLabel}已登记`, type: "success" }) }, onError: (cause) => { setError(cause instanceof Error ? cause.message : "登记失败"); if (cause instanceof ApiClientError && cause.status === 409) void onSaved() } })
  useEffect(() => { if (open) { setAction("addition"); setAmount(""); setDate(today()); setNote(""); setAccountId(""); setLinkedTransaction(null); setError("") } }, [open])
  return <Modal open={open} onOpenChange={onOpenChange} title="登记往来" description="选择本次往来动作后，可手动填写或从已入账流水选取。" footer={<><Button disabled={mutation.isPending} onClick={() => onOpenChange(false)}>取消</Button><Button disabled={mutation.isPending || accountsLoading || (!linkedTransaction && activeAccounts.length === 0)} onClick={() => mutation.mutate()} variant="default">{mutation.isPending ? "正在登记…" : "确认登记"}</Button></>}><div className="form-stack"><Field className="field-medium" label="动作"><Select ariaLabel="动作" onValueChange={(value) => { setAction(value as "addition" | "repayment"); setLinkedTransaction(null); setError("") }} options={[{ value: "addition", label: additionLabel }, { value: "repayment", label: "登记还款" }]} value={action} /></Field><TransactionLinkPicker debtId={debt.id} selected={linkedTransaction} onClear={() => setLinkedTransaction(null)} onSelect={(item) => { setLinkedTransaction(item); setAmount(String(item.amountCents / 100)); setDate(item.occurredOn); setAccountId(item.account?.id || "") }} /><MoneyAccountField accounts={accounts} label={accountLabel} loading={accountsLoading} onManage={() => { onOpenChange(false); navigate("/app/accounts") }} onValueChange={setAccountId} value={accountId} /><Field className="field-number" label={`${isAddition ? additionLabel : "还款"}金额（元）`}><Input autoFocus disabled={Boolean(linkedTransaction)} inputMode="decimal" value={amount} onChange={(event) => setAmount(event.target.value)} placeholder="0.00" /></Field><Field label={isAddition ? "追加日期" : "还款日期"}><DatePicker ariaLabel={isAddition ? "追加日期" : "还款日期"} disabled={Boolean(linkedTransaction)} onValueChange={setDate} value={date} /></Field><Field label="备注"><Textarea value={note} onChange={(event) => setNote(event.target.value)} placeholder="可选" /></Field>{error ? <InlineNotice type="error">{error}</InlineNotice> : null}</div></Modal>
}

function DebtAdditionModal({ accounts, accountsLoading, debt, event, open, onOpenChange, onSaved }: { accounts: LedgerAccount[]; accountsLoading: boolean; debt: Debt; event?: Debt["additions"][number]; open: boolean; onOpenChange: (open: boolean) => void; onSaved: () => Promise<unknown> }) {
  const navigate = useNavigate()
  const toast = useToast()
  const [amount, setAmount] = useState("")
  const [date, setDate] = useState(today())
  const [note, setNote] = useState("")
  const [accountId, setAccountId] = useState("")
  const [movementType, setMovementType] = useState<"addition" | "repayment">("addition")
  const [linkedTransaction, setLinkedTransaction] = useState<TransactionLinkCandidate | null>(null)
  const [linkCleared, setLinkCleared] = useState(false)
  const [error, setError] = useState("")
  const actionLabel = debt.direction === "lend_out" ? "追加借出" : "追加借入"
  const editing = Boolean(event)
  const convertsToRepayment = editing && movementType === "repayment"
  const displayedActionLabel = convertsToRepayment ? "登记还款" : actionLabel
  const activeAccounts = accounts.filter((account) => !account.archived)
  const mutation = useIdempotentMutation({ mutationFn: (_variables: void, write) => {
    const cents = toCents(amount)
    if (!cents) throw new Error(`请输入正确的${actionLabel}金额`)
    if (event) return api.updateDebtAddition(event.id, { version: debt.version, amountCents: cents, effectiveOn: date, note, accountId: accountId || undefined, movementType: convertsToRepayment ? "repayment" : undefined, ...(linkedTransaction ? { transactionId: linkedTransaction.id } : linkCleared ? { transactionId: null } : {}) }, write)
    if (!accountId) throw new Error(`请选择${principalAccountLabel(debt.direction)}`)
    return api.createDebtAddition(debt.id, { accountId, amountCents: cents, effectiveOn: date, note }, write)
  }, onSuccess: async () => { await onSaved(); onOpenChange(false); toast({ title: editing ? `${actionLabel}记录已更新` : `${actionLabel}已登记`, type: "success" }) }, onError: (cause) => { setError(cause instanceof Error ? cause.message : editing ? "更新失败" : "追加失败"); if (cause instanceof ApiClientError && cause.status === 409) void onSaved() } })
  useEffect(() => { if (open) { setAmount(event ? String(event.amountCents / 100) : ""); setDate(event?.effectiveOn || today()); setNote(event?.note || ""); setAccountId(event?.account?.id || ""); setMovementType("addition"); setLinkedTransaction(event?.transactionId && !event.transactionAutoCreated ? { id: event.transactionId, kind: debt.direction === "lend_out" ? "expense" : "income", amountCents: event.amountCents, occurredOn: event.effectiveOn, note: event.note, account: event.account } : null); setLinkCleared(false); setError("") } }, [open, event, debt.direction])
  return <Modal open={open} onOpenChange={onOpenChange} title={editing ? `编辑${displayedActionLabel}` : actionLabel} description={editing ? "可修改金额、账户、日期、备注和往来类型。" : "追加后，本金与剩余金额都会增加。"} footer={<><Button disabled={mutation.isPending} onClick={() => onOpenChange(false)}>取消</Button><Button disabled={mutation.isPending || accountsLoading || (!editing && activeAccounts.length === 0)} onClick={() => mutation.mutate()} variant="default">{mutation.isPending ? editing ? "正在保存…" : "正在追加…" : editing ? "保存修改" : "确认追加"}</Button></>}><div className="form-stack">{editing ? <Field className="field-medium" label="动作"><Select ariaLabel="动作" onValueChange={(value) => setMovementType(value as "addition" | "repayment")} options={[{ value: "addition", label: actionLabel }, { value: "repayment", label: "登记还款" }]} value={movementType} /></Field> : null}<MoneyAccountField accounts={accounts} allowEmpty={editing} label={convertsToRepayment ? repaymentAccountLabel(debt.direction) : principalAccountLabel(debt.direction)} loading={accountsLoading} onManage={() => { onOpenChange(false); navigate("/app/accounts") }} onValueChange={setAccountId} value={accountId} /><Field className="field-number" label={`${convertsToRepayment ? "还款" : actionLabel}金额（元）`}><Input autoFocus disabled={Boolean(linkedTransaction)} inputMode="decimal" value={amount} onChange={(event) => setAmount(event.target.value)} placeholder="0.00" /></Field><Field label={convertsToRepayment ? "还款日期" : "追加日期"}><DatePicker ariaLabel={convertsToRepayment ? "还款日期" : "追加日期"} disabled={Boolean(linkedTransaction)} onValueChange={setDate} value={date} /></Field><Field label="备注"><Textarea value={note} onChange={(event) => setNote(event.target.value)} placeholder="可选" /></Field>{error ? <InlineNotice type="error">{error}</InlineNotice> : null}</div></Modal>
}

function RepaymentModal({ accounts, accountsLoading, debt, event, open, onOpenChange, onSaved }: { accounts: LedgerAccount[]; accountsLoading: boolean; debt: Debt; event?: Debt["repayments"][number]; open: boolean; onOpenChange: (open: boolean) => void; onSaved: () => Promise<unknown> }) {
  const navigate = useNavigate()
  const toast = useToast()
  const [amount, setAmount] = useState("")
  const [date, setDate] = useState(today())
  const [note, setNote] = useState("")
  const [accountId, setAccountId] = useState("")
  const [movementType, setMovementType] = useState<"addition" | "repayment">("repayment")
  const [linkedTransaction, setLinkedTransaction] = useState<TransactionLinkCandidate | null>(null)
  const [error, setError] = useState("")
  const editing = Boolean(event)
  const additionLabel = debt.direction === "lend_out" ? "追加借出" : "追加借入"
  const convertsToAddition = editing && movementType === "addition"
  const activeAccounts = accounts.filter((account) => !account.archived)
  const mutation = useIdempotentMutation({ mutationFn: (_variables: void, write) => {
    const cents = toCents(amount)
    if (!cents) throw new Error("请输入正确的还款金额")
    if (event) return api.updateRepayment(event.id, { version: debt.version, amountCents: cents, effectiveOn: date, note, accountId: accountId || undefined, movementType: convertsToAddition ? "addition" : undefined, ...(linkedTransaction ? { transactionId: linkedTransaction.id } : {}) }, write)
    if (!accountId) throw new Error(`请选择${repaymentAccountLabel(debt.direction)}`)
    return api.createRepayment(debt.id, { accountId, amountCents: cents, effectiveOn: date, note }, write)
  }, onSuccess: async () => { await onSaved(); onOpenChange(false); toast({ title: editing ? "还款记录已更新" : "还款已登记", type: "success" }) }, onError: (cause) => { setError(cause instanceof Error ? cause.message : editing ? "更新失败" : "登记失败"); if (cause instanceof ApiClientError && cause.status === 409) void onSaved() } })
  useEffect(() => { if (open) { setAmount(event ? String(event.amountCents / 100) : ""); setDate(event?.effectiveOn || today()); setNote(event?.note || ""); setAccountId(event?.account?.id || ""); setMovementType("repayment"); setLinkedTransaction(event?.transactionId && !event.transactionAutoCreated ? { id: event.transactionId, kind: debt.direction === "lend_out" ? "income" : "expense", amountCents: event.amountCents, occurredOn: event.effectiveOn, note: event.note, account: event.account } : null); setError("") } }, [open, event, debt.direction])
  return <Modal open={open} onOpenChange={onOpenChange} title={editing ? convertsToAddition ? `编辑${additionLabel}` : "编辑还款记录" : "登记还款"} description={editing ? "可修改金额、账户、日期、备注和往来类型。" : `当前剩余 ${yuan(debt.remainingCents)}，禁止超额还款。`} footer={<><Button disabled={mutation.isPending} onClick={() => onOpenChange(false)}>取消</Button><Button disabled={mutation.isPending || accountsLoading || (!editing && activeAccounts.length === 0)} onClick={() => mutation.mutate()} variant="default">{mutation.isPending ? editing ? "正在保存…" : "正在登记…" : editing ? "保存修改" : "确认登记"}</Button></>}><div className="form-stack">{editing ? <Field className="field-medium" label="动作"><Select ariaLabel="动作" onValueChange={(value) => setMovementType(value as "addition" | "repayment")} options={[{ value: "repayment", label: "登记还款" }, { value: "addition", label: additionLabel }]} value={movementType} /></Field> : null}<MoneyAccountField accounts={accounts} allowEmpty={editing} label={convertsToAddition ? principalAccountLabel(debt.direction) : repaymentAccountLabel(debt.direction)} loading={accountsLoading} onManage={() => { onOpenChange(false); navigate("/app/accounts") }} onValueChange={setAccountId} value={accountId} /><Field label={`${convertsToAddition ? additionLabel : "还款"}金额（元）`}><Input disabled={Boolean(linkedTransaction)} inputMode="decimal" value={amount} onChange={(event) => setAmount(event.target.value)} /></Field><Field label={convertsToAddition ? "追加日期" : "还款日期"}><DatePicker ariaLabel={convertsToAddition ? "追加日期" : "还款日期"} disabled={Boolean(linkedTransaction)} onValueChange={setDate} value={date} /></Field><Field label="备注"><Textarea value={note} onChange={(event) => setNote(event.target.value)} /></Field>{error ? <InlineNotice type="error">{error}</InlineNotice> : null}</div></Modal>
}

function TimelineLinkModal({ debt, kind, event, open, onOpenChange, onSaved }: { debt: Debt; kind: "addition" | "repayment"; event: Debt["additions"][number] | Debt["repayments"][number]; open: boolean; onOpenChange: (open: boolean) => void; onSaved: () => Promise<unknown> }) {
  const toast = useToast()
  const linkedKind = kind === "repayment" ? debt.direction === "lend_out" ? "income" : "expense" : debt.direction === "lend_out" ? "expense" : "income"
  const initial = event.transactionId ? { id: event.transactionId, kind: linkedKind, amountCents: event.amountCents, occurredOn: event.effectiveOn, note: event.note, account: event.account } satisfies TransactionLinkCandidate : null
  const [selected, setSelected] = useState<TransactionLinkCandidate | null>(initial)
  const mutation = useIdempotentMutation({
    mutationFn: (_variables: void, write) => kind === "addition"
      ? api.updateDebtAddition(event.id, { version: debt.version, amountCents: event.amountCents, effectiveOn: event.effectiveOn, note: event.note, accountId: event.account?.id, transactionId: selected?.id || null }, write)
      : api.updateRepayment(event.id, { version: debt.version, amountCents: event.amountCents, effectiveOn: event.effectiveOn, note: event.note, accountId: event.account?.id, transactionId: selected?.id || null }, write),
    onSuccess: async () => { await onSaved(); onOpenChange(false); toast({ title: selected ? "流水已关联" : "已使用自动流水", type: "success" }) },
    onError: (error) => toast({ title: "关联失败", description: error.message, type: "error" }),
  })
  return <Modal open={open} onOpenChange={onOpenChange} title={event.transactionId ? "管理流水" : "关联流水"} description="现金变动始终保留一笔流水；可改用另一笔已有流水，或恢复自动流水。" footer={<><Button disabled={mutation.isPending} onClick={() => onOpenChange(false)}>取消</Button><Button disabled={mutation.isPending || (!selected && !event.transactionId)} onClick={() => mutation.mutate()} variant="default">{mutation.isPending ? "正在保存…" : "保存"}</Button></>}><TransactionLinkPicker debtId={debt.id} amountCents={event.amountCents} clearLabel="使用自动流水" selected={selected} onClear={() => setSelected(null)} onSelect={setSelected} /></Modal>
}

function InitialTransactionLinkModal({ debt, open, onOpenChange, onSaved }: { debt: Debt; open: boolean; onOpenChange: (open: boolean) => void; onSaved: () => Promise<unknown> }) {
  const toast = useToast()
  const initialPrincipalCents = debt.principalCents - debt.additions.reduce((total, event) => total + event.amountCents, 0)
  const initial = debt.transactionId ? { id: debt.transactionId, kind: debt.direction === "lend_out" ? "expense" as const : "income" as const, amountCents: initialPrincipalCents, occurredOn: debt.occurredOn, note: debt.note, account: debt.account } satisfies TransactionLinkCandidate : null
  const [selected, setSelected] = useState<TransactionLinkCandidate | null>(initial)
  const mutation = useIdempotentMutation({
    mutationFn: (_variables: void, write) => api.updateDebt(debt.id, { version: debt.version, accountId: debt.account?.id || null, originKind: debt.originKind, counterpartyId: debt.counterparty.id, principalCents: debt.principalCents, occurredOn: debt.occurredOn, dueOn: debt.dueOn, note: debt.note, transactionId: selected?.id || null }, write),
    onSuccess: async () => { await onSaved(); onOpenChange(false); toast({ title: selected ? "流水已关联" : "已使用自动流水", type: "success" }) },
    onError: (error) => toast({ title: "关联失败", description: error.message, type: "error" }),
  })
  return <Modal open={open} onOpenChange={onOpenChange} title={debt.transactionId ? "管理流水" : "关联流水"} description="现金变动始终保留一笔流水；可改用另一笔已有流水，或恢复自动流水。" footer={<><Button disabled={mutation.isPending} onClick={() => onOpenChange(false)}>取消</Button><Button disabled={mutation.isPending || (!selected && !debt.transactionId)} onClick={() => mutation.mutate()} variant="default">{mutation.isPending ? "正在保存…" : "保存"}</Button></>}><TransactionLinkPicker debtId={debt.id} amountCents={initialPrincipalCents} clearLabel="使用自动流水" selected={selected} onClear={() => setSelected(null)} onSelect={setSelected} /></Modal>
}

function RepaymentRow({ debt, event, onEdit, onLink, onSaved }: { debt: Debt; event: Debt["repayments"][number]; onEdit: (event: Debt["repayments"][number]) => void; onLink: (event: Debt["repayments"][number]) => void; onSaved: () => Promise<unknown> }) {
  const [confirm, setConfirm] = useState(false)
  const toast = useToast()
  const mutation = useIdempotentMutation({ mutationFn: (_variables: void, write) => api.reverseRepayment(event.id, { effectiveOn: today(), note: "撤销错误还款" }, write), onSuccess: async () => { await onSaved(); setConfirm(false); toast({ title: "还款已撤销", type: "success" }) }, onError: (error) => toast({ title: "撤销失败", description: error.message, type: "error" }) })
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
        { label: event.transactionId ? "管理流水" : "关联流水", icon: <LinkIcon />, onSelect: () => onLink(event) },
        { label: "撤销还款", icon: <RotateCcwIcon />, onSelect: () => setConfirm(true) },
      ] : undefined}
      actionLabel={`操作 ${event.effectiveOn} ${title} ${yuan(event.amountCents)}`}
      amountCents={event.amountCents}
      date={event.effectiveOn}
      icon={isPayment ? <CircleDollarSignIcon /> : <RotateCcwIcon />}
      reversed={event.reversed}
      transactionId={event.transactionId}
      note={note}
      title={title}
      tone="repayment"
    />
    <ConfirmDialog destructive={false} open={confirm} onOpenChange={setConfirm} title={`撤销 ${yuan(event.amountCents)} 的还款？`} description="撤销会新增一条补偿记录，并重新增加债务剩余金额。历史记录不会删除。" confirmLabel="确认撤销" onConfirm={() => mutation.mutate()} pending={mutation.isPending} />
  </>
}

function InitialDebtRow({ debt, event, onEdit, onLink }: { debt: Debt; event: InitialDebtMovement; onEdit: () => void; onLink: () => void }) {
  const isLendOut = debt.direction === "lend_out"
  const note = visibleNote(event.note, event.account)
  const cashless = debt.originKind === "no_cash_movement" && !event.account
  const title = cashless ? (isLendOut ? "确认应收" : "确认应付") : (isLendOut ? "初始借出" : "初始借入")
  const actions = !debt.archived ? [
    { label: "编辑记录", icon: <PencilIcon />, onSelect: onEdit },
    ...(!cashless ? [{ label: event.transactionId ? "管理流水" : "关联流水", icon: <LinkIcon />, onSelect: onLink }] : []),
  ] : undefined
  return <MovementTimelineRow account={event.account} accountDisplay={cashless ? "无资金进出" : undefined} accountLabel={cashless ? "资金往来" : principalAccountLabel(debt.direction)} actions={actions} actionLabel={`操作 ${debt.occurredOn} ${title} ${yuan(debt.principalCents)}`} amountCents={event.amountCents} date={event.effectiveOn} icon={isLendOut ? <ArrowUpRightIcon /> : <ArrowDownLeftIcon />} note={note} title={title} tone={isLendOut ? "lend" : "borrow"} transactionId={event.transactionId} />
}

function DebtAdditionRow({ debt, event, onEdit, onLink }: { debt: Debt; event: Debt["additions"][number]; onEdit: (event: Debt["additions"][number]) => void; onLink: (event: Debt["additions"][number]) => void }) {
  const isLendOut = debt.direction === "lend_out"
  const note = visibleNote(event.note, event.account)
  const title = isLendOut ? "追加借出" : "追加借入"
  return <MovementTimelineRow account={event.account} accountLabel={principalAccountLabel(debt.direction)} actions={!debt.archived ? [{ label: "编辑记录", icon: <PencilIcon />, onSelect: () => onEdit(event) }, { label: event.transactionId ? "管理流水" : "关联流水", icon: <LinkIcon />, onSelect: () => onLink(event) }] : undefined} actionLabel={`操作 ${event.effectiveOn} ${title} ${yuan(event.amountCents)}`} amountCents={event.amountCents} date={event.effectiveOn} icon={isLendOut ? <ArrowUpRightIcon /> : <ArrowDownLeftIcon />} note={note} title={title} tone={isLendOut ? "lend" : "borrow"} transactionId={event.transactionId} />
}

type TimelineAction = { label: string; icon: ReactNode; onSelect: () => void }

function MovementTimelineRow({
  account,
  accountDisplay,
  accountLabel,
  actionLabel,
  actions,
  amountCents,
  date,
  icon,
  note,
  reversed = false,
  transactionId,
  title,
  tone,
}: {
  account: LedgerAccountRef | null
  accountDisplay?: string
  accountLabel: string
  actionLabel?: string
  actions?: TimelineAction[]
  amountCents: number
  date: string
  icon: ReactNode
  note: string
  reversed?: boolean
  transactionId?: string | null
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
          {transactionId ? <span className="linked-transaction-line">已关联流水 · {date} · {tone === "lend" ? "-" : "+"}{yuan(amountCents)}</span> : null}
          <span className="timeline-account"><span>{accountLabel}：</span><span>{accountDisplay ?? ledgerAccountDisplayLabel(account)}</span></span>
          {note ? <span className="timeline-note"><span>备注：</span><span>{note}</span></span> : null}
        </div>
      </div>
      {actions?.length && actionLabel ? <ActionMenu items={actions} label={actionLabel} quiet /> : <span aria-hidden="true" className="timeline-action-spacer" />}
    </div>
  </li>
}
