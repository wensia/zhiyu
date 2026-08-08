import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import {
  CalendarDaysIcon,
  ChevronLeftIcon,
  ChevronRightIcon,
  PencilIcon,
  PlusIcon,
  Trash2Icon,
  TrendingUpIcon,
} from "lucide-react"
import { useEffect, useMemo, useState } from "react"

import { ApiClientError, api } from "../api/client"
import type {
  LedgerAccount,
  LedgerTransaction,
  TransactionDaySummary,
  TransactionKind,
  TransactionMonthSummary,
} from "../api/types"
import {
  ActionMenu,
  Button,
  ConfirmDialog,
  CreatableSelect,
  DatePicker,
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
import { formatDate, monthDays, parseDate, startOfMonth } from "../components/date-utils"

const yuan = (cents: number) => new Intl.NumberFormat("zh-CN", { style: "currency", currency: "CNY" }).format(cents / 100)

/** 日历格子用的紧凑金额：¥999 / ¥1.2k / ¥3.4万，防止小格内溢出。 */
function compactYuan(cents: number) {
  const sign = cents < 0 ? "-" : ""
  const abs = Math.abs(cents)
  if (abs >= 1_000_000) return `${sign}¥${(abs / 1_000_000).toFixed(1)}万`
  if (abs >= 100_000) return `${sign}¥${(abs / 100_000).toFixed(1)}k`
  return `${sign}¥${Math.round(abs / 100)}`
}

function toCents(value: string) {
  const normalized = value.trim().replace(/[¥,，\s]/g, "")
  if (!/^\d+(\.\d{1,2})?$/.test(normalized)) throw new Error("金额格式不正确，最多两位小数")
  const cents = Math.round(Number.parseFloat(normalized) * 100)
  if (!Number.isSafeInteger(cents) || cents <= 0) throw new Error("金额必须大于 0")
  return cents
}

const monthKeyOf = (date: Date) => formatDate(date).slice(0, 7)
const WEEKDAYS = ["一", "二", "三", "四", "五", "六", "日"]
const weekdayOf = (date: Date) => WEEKDAYS[(date.getDay() + 6) % 7]
const KIND_LABEL: Record<TransactionKind, string> = { income: "收入", expense: "支出" }
const CHART_COLORS = ["var(--chart-2)", "var(--chart-1)", "var(--chart-3)", "var(--chart-4)", "var(--chart-5)"]

type DayBucket = { income: number; expense: number }

function bucketByDate(days: TransactionDaySummary[] | undefined) {
  const map = new Map<string, DayBucket>()
  for (const day of days || []) map.set(day.date, { income: day.incomeCents, expense: day.expenseCents })
  return map
}

function MetricStrip({ summary, loading }: { summary: TransactionMonthSummary | undefined; loading: boolean }) {
  const netClass = summary && summary.netCents < 0 ? "metric-negative" : "metric-positive"
  const items: Array<[string, string, string?]> = [
    ["本月收入", yuan(summary?.incomeCents ?? 0), "metric-positive"],
    ["本月支出", yuan(summary?.expenseCents ?? 0)],
    ["本月结余", `${summary && summary.netCents > 0 ? "+" : ""}${yuan(summary?.netCents ?? 0)}`, netClass],
    ["记账笔数", String(summary?.transactionCount ?? 0)],
  ]
  return (
    <section aria-label="本月收支汇总" className="metrics">
      {items.map(([label, value, className]) => (
        <div className="metric" key={label}><span>{label}</span><strong className={className}>{loading ? "—" : value}</strong></div>
      ))}
    </section>
  )
}

function CalendarDayCell({
  date,
  bucket,
  currentMonth,
  selected,
  today,
  onSelect,
  onAdd,
}: {
  date: Date
  bucket: DayBucket | undefined
  currentMonth: boolean
  selected: boolean
  today: boolean
  onSelect: (date: string) => void
  onAdd: (date: string) => void
}) {
  const dateValue = formatDate(date)
  const net = (bucket?.income ?? 0) - (bucket?.expense ?? 0)
  const classNames = [
    "tx-day",
    currentMonth ? "" : "tx-day-other-month",
    selected ? "tx-day-selected" : "",
  ].filter(Boolean).join(" ")
  return (
    <button
      aria-label={`${dateValue}${bucket ? `，收入 ${yuan(bucket.income)}，支出 ${yuan(bucket.expense)}` : "，无记录"}`}
      aria-pressed={selected}
      className={classNames}
      onClick={() => (selected ? onAdd(dateValue) : onSelect(dateValue))}
      type="button"
    >
      <span className={`tx-day-number ${today ? "tx-day-today" : ""}`}>{date.getDate()}</span>
      {bucket ? (
        <span className="tx-day-amounts">
          {bucket.income > 0 ? <span className="tx-amount-income">+{compactYuan(bucket.income)}</span> : null}
          {bucket.expense > 0 ? <span className="tx-amount-expense">-{compactYuan(bucket.expense)}</span> : null}
          <span className={net >= 0 ? "tx-amount-net-positive" : "tx-amount-net-negative"}>{net >= 0 ? "+" : ""}{compactYuan(net)}</span>
        </span>
      ) : null}
      <span
        aria-label={`在 ${dateValue} 记一笔`}
        className="tx-day-add"
        onClick={(event) => { event.stopPropagation(); onAdd(dateValue) }}
        onKeyDown={(event) => { if (event.key === "Enter" || event.key === " ") { event.stopPropagation(); onAdd(dateValue) } }}
        role="button"
        tabIndex={-1}
      ><PlusIcon /></span>
    </button>
  )
}

function TransactionCalendar({
  month,
  selectedDay,
  buckets,
  onMonthChange,
  onSelect,
  onAdd,
}: {
  month: Date
  selectedDay: string
  buckets: Map<string, DayBucket>
  onMonthChange: (month: Date) => void
  onSelect: (date: string) => void
  onAdd: (date: string) => void
}) {
  const days = useMemo(() => monthDays(month), [month])
  const today = formatDate(new Date())
  return (
    <section aria-label="记账日历" className="tx-calendar">
      <div className="tx-calendar-header">
        <Button aria-label="上一月" onClick={() => onMonthChange(new Date(month.getFullYear(), month.getMonth() - 1, 1))} size="icon-sm" variant="ghost"><ChevronLeftIcon /></Button>
        <strong className="tx-calendar-title">{month.getFullYear()} 年 {month.getMonth() + 1} 月</strong>
        <div className="tx-calendar-header-side">
          <Button onClick={() => onMonthChange(startOfMonth(new Date()))} size="sm" variant="ghost">今天</Button>
          <Button aria-label="下一月" onClick={() => onMonthChange(new Date(month.getFullYear(), month.getMonth() + 1, 1))} size="icon-sm" variant="ghost"><ChevronRightIcon /></Button>
        </div>
      </div>
      <div className="tx-calendar-weekdays">{WEEKDAYS.map((day) => <span key={day}>{day}</span>)}</div>
      <div className="tx-calendar-grid" role="grid">
        {days.map((date) => (
          <CalendarDayCell
            bucket={buckets.get(formatDate(date))}
            currentMonth={date.getMonth() === month.getMonth()}
            date={date}
            key={formatDate(date)}
            onAdd={onAdd}
            onSelect={onSelect}
            selected={formatDate(date) === selectedDay}
            today={formatDate(date) === today}
          />
        ))}
      </div>
    </section>
  )
}

function TransactionAmount({ transaction }: { transaction: LedgerTransaction }) {
  const income = transaction.kind === "income"
  return (
    <span className={`tx-row-amount ${income ? "tx-amount-income" : ""}`}>
      {income ? "+" : "-"}{yuan(transaction.amountCents)}
    </span>
  )
}

function DayDetailPanel({
  day,
  items,
  loading,
  onEdit,
  onDelete,
  onAdd,
}: {
  day: string
  items: LedgerTransaction[]
  loading: boolean
  onEdit: (transaction: LedgerTransaction) => void
  onDelete: (transaction: LedgerTransaction) => void
  onAdd: (date: string) => void
}) {
  const date = parseDate(day)
  const income = items.filter((item) => item.kind === "income").reduce((sum, item) => sum + item.amountCents, 0)
  const expense = items.filter((item) => item.kind === "expense").reduce((sum, item) => sum + item.amountCents, 0)
  return (
    <section aria-label="当日明细" className="tx-day-detail">
      <div className="tx-day-detail-header">
        <strong>{date ? `${date.getMonth() + 1}月${date.getDate()}日 周${weekdayOf(date)}` : day}</strong>
        <span>收 {yuan(income)} · 支 {yuan(expense)}</span>
      </div>
      {loading ? (
        <div className="tx-rows" aria-hidden="true">{Array.from({ length: 3 }, (_, index) => <div className="tx-row" key={index}><span className="skeleton-line" /></div>)}</div>
      ) : items.length ? (
        <div className="tx-rows">
          {items.map((item) => (
            <div className="tx-row" key={item.id}>
              <div className="tx-row-copy">
                <strong>{item.category || "未分类"}</strong>
                <span>{[item.account?.name, item.note].filter(Boolean).join(" · ") || "—"}</span>
              </div>
              <TransactionAmount transaction={item} />
              <ActionMenu
                items={[
                  { label: "编辑", icon: <PencilIcon />, onSelect: () => onEdit(item) },
                  { label: "删除", icon: <Trash2Icon />, onSelect: () => onDelete(item), destructive: true },
                ]}
                label={`操作 ${item.category || "未分类"} ${yuan(item.amountCents)}`}
                quiet
              />
            </div>
          ))}
        </div>
      ) : (
        <div className="tx-day-empty">
          <p>当日暂无记录</p>
          <Button onClick={() => onAdd(day)} size="sm"><PlusIcon />记一笔</Button>
        </div>
      )}
    </section>
  )
}

function TrendChart({ days, month }: { days: TransactionDaySummary[]; month: Date }) {
  const width = 640
  const height = 240
  const pad = { top: 20, right: 12, bottom: 24, left: 12 }
  const daysInMonth = new Date(month.getFullYear(), month.getMonth() + 1, 0).getDate()
  const buckets = bucketByDate(days)
  const series = Array.from({ length: daysInMonth }, (_, index) => {
    const key = `${monthKeyOf(month)}-${String(index + 1).padStart(2, "0")}`
    const bucket = buckets.get(key)
    return { day: index + 1, income: bucket?.income ?? 0, expense: bucket?.expense ?? 0 }
  })
  const hasData = series.some((item) => item.income > 0 || item.expense > 0)
  if (!hasData) {
    return (
      <section aria-label="每日趋势" className="tx-trend">
        <div className="tx-day-detail-header"><strong>每日趋势</strong><TrendingUpIcon aria-hidden="true" /></div>
        <div className="tx-day-empty"><p>本月暂无收支数据，记账后这里会出现每日趋势。</p></div>
      </section>
    )
  }
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
  const zeroY = lineY(0)
  const linePath = cumulativePoints.map((value, index) => `${index === 0 ? "M" : "L"}${centerX(index).toFixed(1)},${lineY(value).toFixed(1)}`).join(" ")
  const labelDays = new Set([1, 5, 10, 15, 20, 25, daysInMonth])
  return (
    <section aria-label="每日趋势" className="tx-trend">
      <div className="tx-day-detail-header"><strong>每日趋势</strong><span>柱：当日收支 · 线：当月累计净流转</span></div>
      <svg aria-label="当月每日收支柱状图与累计结余折线" className="tx-trend-svg" role="img" viewBox={`0 0 ${width} ${height}`}>
        {[0.25, 0.5, 0.75].map((ratio) => (
          <line className="tx-trend-grid" key={ratio} x1={pad.left} x2={width - pad.right} y1={pad.top + plotHeight * ratio} y2={pad.top + plotHeight * ratio} />
        ))}
        <line className="tx-trend-zero" x1={pad.left} x2={width - pad.right} y1={zeroY} y2={zeroY} />
        {series.map((item, index) => (
          <g key={item.day}>
            {item.income > 0 ? (
              <rect className="tx-trend-bar-income" height={Math.max(1, pad.top + plotHeight - barTop(item.income))} width={barWidth} x={centerX(index) - barWidth - 1} y={barTop(item.income)} />
            ) : null}
            {item.expense > 0 ? (
              <rect className="tx-trend-bar-expense" height={Math.max(1, pad.top + plotHeight - barTop(item.expense))} width={barWidth} x={centerX(index) + 1} y={barTop(item.expense)} />
            ) : null}
            {labelDays.has(item.day) ? (
              <text className="tx-trend-label" textAnchor="middle" x={centerX(index)} y={height - 8}>{item.day}</text>
            ) : null}
          </g>
        ))}
        <path className="tx-trend-line" d={linePath} />
      </svg>
      <div className="tx-trend-legend">
        <span><i className="tx-trend-dot tx-trend-dot-income" />收入</span>
        <span><i className="tx-trend-dot tx-trend-dot-expense" />支出</span>
        <span><i className="tx-trend-dot tx-trend-dot-net" />累计净流转（月初为 0）</span>
      </div>
    </section>
  )
}

function CategoryShare({ summary }: { summary: TransactionMonthSummary | undefined }) {
  const entries = (summary?.byCategory || []).filter((entry) => entry.expenseCents > 0)
  const total = entries.reduce((sum, entry) => sum + entry.expenseCents, 0)
  if (!summary || !entries.length) {
    return (
      <section aria-label="分类占比" className="tx-cat-share">
        <div className="tx-day-detail-header"><strong>本月支出分类</strong></div>
        <div className="tx-day-empty"><p>本月暂无支出分类数据。</p></div>
      </section>
    )
  }
  const top = entries.slice(0, 5)
  const rest = entries.slice(5)
  const rows = rest.length ? [...top, { category: "其他", expenseCents: rest.reduce((sum, entry) => sum + entry.expenseCents, 0), incomeCents: 0, count: rest.reduce((sum, entry) => sum + entry.count, 0) }] : top
  return (
    <section aria-label="分类占比" className="tx-cat-share">
      <div className="tx-day-detail-header"><strong>本月支出分类</strong><span>共 {yuan(total)}</span></div>
      <div className="tx-cat-share-rows">
        {rows.map((entry, index) => {
          const ratio = Math.round((entry.expenseCents / total) * 100)
          return (
            <div className="tx-cat-share-row" key={entry.category}>
              <span className="tx-cat-share-name">{entry.category || "未分类"}</span>
              <span className="tx-cat-share-bar"><i className="tx-cat-share-fill" style={{ width: `${Math.max(2, ratio)}%`, background: CHART_COLORS[index % CHART_COLORS.length] }} /></span>
              <span className="tx-cat-share-value">{yuan(entry.expenseCents)} · {ratio}%</span>
            </div>
          )
        })}
      </div>
    </section>
  )
}

function TransactionListTab({
  monthKey,
  accounts,
  categories,
  onEdit,
  onDelete,
}: {
  monthKey: string
  accounts: LedgerAccount[]
  categories: string[]
  onEdit: (transaction: LedgerTransaction) => void
  onDelete: (transaction: LedgerTransaction) => void
}) {
  const [kind, setKind] = useState("")
  const [category, setCategory] = useState("")
  const [accountId, setAccountId] = useState("")
  const [page, setPage] = useState(1)
  const query = useQuery({
    queryKey: ["transactions", "list", { month: monthKey, kind, category, accountId, page }],
    queryFn: () => api.transactions({ month: monthKey, kind, category, accountId, page, pageSize: 20 }),
  })
  const changeFilter = (setter: (value: string) => void) => (value: string) => { setter(value); setPage(1) }
  const items = query.data?.items || []
  const empty = !query.isLoading && items.length === 0
  const kindOptions = [{ value: "expense", label: "支出" }, { value: "income", label: "收入" }]
  const categoryOptions = categories.map((value) => ({ value, label: value }))
  const accountOptions = accounts.map((account) => ({ value: account.id, label: account.name }))
  return (
    <div className="tx-list">
      <section aria-label="账目筛选" className="toolbar tx-toolbar">
        <FilterSelect ariaLabel="收支类型" onValueChange={changeFilter(setKind)} options={kindOptions} placeholder="全部类型" value={kind} />
        <FilterSelect ariaLabel="分类" onValueChange={changeFilter(setCategory)} options={categoryOptions} placeholder="全部分类" value={category} />
        <FilterSelect ariaLabel="账户" onValueChange={changeFilter(setAccountId)} options={accountOptions} placeholder="全部账户" value={accountId} />
      </section>
      {query.error ? <InlineNotice type="error">{query.error.message}<Button onClick={() => query.refetch()} size="sm">重试</Button></InlineNotice> : null}
      <div aria-busy={query.isLoading} className="data-dock tx-data-dock">
        <div className="desktop-table tx-table-frame">
          <table className="tx-table" data-slot="table">
            <thead><tr><th>日期</th><th>分类</th><th>账户</th><th>备注</th><th>金额</th><th data-sticky-cell="right"><span className="sr-only">操作</span></th></tr></thead>
            <tbody>
              {query.isLoading ? Array.from({ length: 6 }, (_, index) => (
                <tr aria-hidden="true" className="skeleton-row" key={`tx-skeleton-${index}`}>
                  {Array.from({ length: 6 }, (__, cellIndex) => <td key={cellIndex}><span className="skeleton-line" /></td>)}
                </tr>
              )) : empty ? (
                <tr className="table-state-row"><td colSpan={6}><div className="table-state"><CalendarDaysIcon /><h2>当月暂无符合条件的账目</h2><p>调整筛选，或回到日历点格子记一笔。</p></div></td></tr>
              ) : items.map((item) => (
                <tr data-slot="table-row" key={item.id}>
                  <td>{item.occurredOn}</td>
                  <td><strong>{item.category || "未分类"}</strong></td>
                  <td>{item.account?.name || "—"}</td>
                  <td><span className="tx-note">{item.note || "—"}</span></td>
                  <td className="tx-amount-cell"><TransactionAmount transaction={item} /></td>
                  <td data-sticky-cell="right">
                    <ActionMenu
                      items={[
                        { label: "编辑", icon: <PencilIcon />, onSelect: () => onEdit(item) },
                        { label: "删除", icon: <Trash2Icon />, onSelect: () => onDelete(item), destructive: true },
                      ]}
                      label={`操作 ${item.category || "未分类"} ${yuan(item.amountCents)}`}
                      quiet
                    />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        <div className="tx-list-cards">
          {query.isLoading ? Array.from({ length: 3 }, (_, index) => <div aria-hidden="true" className="tx-list-card" key={index}><span className="skeleton-line" /><span className="skeleton-line" /></div>) : empty ? (
            <div className="table-state"><CalendarDaysIcon /><h2>当月暂无符合条件的账目</h2><p>调整筛选，或回到日历点格子记一笔。</p></div>
          ) : items.map((item) => (
            <article className="tx-list-card" key={item.id}>
              <div><strong>{item.category || "未分类"}</strong><TransactionAmount transaction={item} /></div>
              <span>{item.occurredOn} · {[item.account?.name, item.note].filter(Boolean).join(" · ") || KIND_LABEL[item.kind]}</span>
              <div className="tx-list-card-footer">
                <span>{KIND_LABEL[item.kind]}</span>
                <ActionMenu
                  items={[
                    { label: "编辑", icon: <PencilIcon />, onSelect: () => onEdit(item) },
                    { label: "删除", icon: <Trash2Icon />, onSelect: () => onDelete(item), destructive: true },
                  ]}
                  label={`操作 ${item.category || "未分类"} ${yuan(item.amountCents)}`}
                  quiet
                />
              </div>
            </article>
          ))}
        </div>
      </div>
      <TablePagination
        disabled={query.isLoading}
        onPageChange={setPage}
        page={page}
        pageSize={20}
        total={query.data?.total ?? 0}
      />
    </div>
  )
}

function TransactionFormModal({
  transaction,
  defaultDate,
  accounts,
  categories,
  open,
  onOpenChange,
  onSaved,
}: {
  transaction?: LedgerTransaction
  defaultDate: string
  accounts: LedgerAccount[]
  categories: string[]
  open: boolean
  onOpenChange: (open: boolean) => void
  onSaved: () => Promise<unknown>
}) {
  const toast = useToast()
  const [kind, setKind] = useState<TransactionKind>("expense")
  const [amount, setAmount] = useState("")
  const [occurredOn, setOccurredOn] = useState(defaultDate)
  const [categoryValue, setCategoryValue] = useState("")
  const [categoryText, setCategoryText] = useState("")
  const [accountId, setAccountId] = useState("")
  const [note, setNote] = useState("")
  const [error, setError] = useState("")

  useEffect(() => {
    if (!open) return
    setKind(transaction?.kind || "expense")
    setAmount(transaction ? String(transaction.amountCents / 100) : "")
    setOccurredOn(transaction?.occurredOn || defaultDate)
    setCategoryValue(transaction?.category || "")
    setCategoryText("")
    setAccountId(transaction?.account?.id || "")
    setNote(transaction?.note || "")
    setError("")
  }, [open, transaction, defaultDate])

  const categoryOptions = categories.map((value) => ({ value, label: value }))
  const accountOptions = accounts.filter((account) => !account.archived).map((account) => ({ value: account.id, label: `${account.name} · 余额 ${yuan(account.balanceCents)}` }))

  const mutation = useMutation({
    mutationFn: async () => {
      const amountCents = toCents(amount)
      if (!occurredOn) throw new Error("请选择发生日期")
      const category = (categoryValue || categoryText).trim()
      const input = {
        kind,
        amountCents,
        occurredOn,
        category,
        accountId: accountId || null,
        note: note.trim(),
      }
      if (transaction) return api.updateTransaction(transaction.id, { ...input, version: transaction.version })
      return api.createTransaction(input)
    },
    onSuccess: async () => {
      await onSaved()
      onOpenChange(false)
      toast({ title: transaction ? "记账已更新" : "已记一笔", type: "success" })
    },
    onError: (cause) => {
      setError(cause instanceof Error ? cause.message : "保存失败")
      if (cause instanceof ApiClientError && cause.status === 409) void onSaved()
    },
  })

  return (
    <Modal
      footer={<><Button disabled={mutation.isPending} onClick={() => onOpenChange(false)}>取消</Button><Button disabled={mutation.isPending} onClick={() => mutation.mutate()} variant="default">{mutation.isPending ? "正在保存…" : "保存"}</Button></>}
      onOpenChange={onOpenChange}
      open={open}
      title={transaction ? "编辑记账" : "记一笔"}
    >
      <div className="form-stack">
        <div aria-label="收支类型" className="segmented" role="group">
          {(["expense", "income"] as const).map((value) => (
            <button aria-pressed={kind === value} key={value} onClick={() => setKind(value)} type="button">{KIND_LABEL[value]}</button>
          ))}
        </div>
        <div className="form-grid">
          <Field label="金额（元）">
            <Input
              aria-label="金额（元）"
              autoFocus
              inputMode="decimal"
              onChange={(event) => setAmount(event.target.value)}
              placeholder="0.00"
              value={amount}
            />
          </Field>
          <Field label="发生日期">
            <DatePicker ariaLabel="发生日期" onValueChange={setOccurredOn} value={occurredOn} />
          </Field>
        </div>
        <Field label="分类（可输入新建）">
          <CreatableSelect
            ariaLabel="分类"
            emptyHint="暂无历史分类，直接输入新建"
            onSelect={setCategoryValue}
            onTextChange={setCategoryText}
            options={categoryOptions}
            placeholder="例如：餐饮、交通、工资"
            text={categoryText}
            value={categoryValue}
          />
        </Field>
        <Field label="账户（可选）">
          <Select
            ariaLabel="账户"
            clearLabel="清空账户选择"
            clearable
            onValueChange={setAccountId}
            options={accountOptions}
            placeholder="不关联账户"
            value={accountId}
          />
        </Field>
        <Field label="备注（可选）">
          <Textarea maxLength={2000} onChange={(event) => setNote(event.target.value)} placeholder="补充用途或说明" rows={2} value={note} />
        </Field>
        {error ? <InlineNotice type="error">{error}</InlineNotice> : null}
      </div>
    </Modal>
  )
}

export function TransactionWorkspace() {
  const queryClient = useQueryClient()
  const toast = useToast()
  const today = formatDate(new Date())
  const [month, setMonth] = useState(() => startOfMonth(new Date()))
  const [selectedDay, setSelectedDay] = useState(today)
  const [formState, setFormState] = useState<{ transaction?: LedgerTransaction; date: string } | undefined>()
  const [deleting, setDeleting] = useState<LedgerTransaction | undefined>()
  const monthKey = monthKeyOf(month)

  const summaryQuery = useQuery({
    queryKey: ["transaction-summary", monthKey],
    queryFn: () => api.transactionSummary(monthKey),
  })
  const monthItemsQuery = useQuery({
    queryKey: ["transactions", "month", monthKey],
    queryFn: () => api.transactions({ month: monthKey, page: 1, pageSize: 200 }),
  })
  const categoriesQuery = useQuery({ queryKey: ["transaction-categories"], queryFn: api.transactionCategories })
  const accountsQuery = useQuery({ queryKey: ["ledger-accounts"], queryFn: api.ledgerAccounts })

  const refresh = () => Promise.all([
    queryClient.invalidateQueries({ queryKey: ["transactions"] }),
    queryClient.invalidateQueries({ queryKey: ["transaction-summary"] }),
    queryClient.invalidateQueries({ queryKey: ["transaction-categories"] }),
    queryClient.invalidateQueries({ queryKey: ["ledger-accounts"] }),
  ])

  const deleteMutation = useMutation({
    mutationFn: (transaction: LedgerTransaction) => api.deleteTransaction(transaction.id, transaction.version),
    onSuccess: async () => {
      await refresh()
      setDeleting(undefined)
      toast({ title: "记账已删除", type: "success" })
    },
    onError: async (cause) => {
      setDeleting(undefined)
      if (cause instanceof ApiClientError && cause.status === 409) await refresh()
      toast({ title: "删除失败", description: cause.message, type: "error" })
    },
  })

  const changeMonth = (next: Date) => {
    const start = startOfMonth(next)
    setMonth(start)
    setSelectedDay(monthKeyOf(start) === monthKeyOf(new Date()) ? formatDate(new Date()) : formatDate(start))
  }

  const buckets = useMemo(() => bucketByDate(summaryQuery.data?.days), [summaryQuery.data])
  const dayItems = useMemo(
    () => (monthItemsQuery.data?.items || []).filter((item) => item.occurredOn === selectedDay),
    [monthItemsQuery.data, selectedDay],
  )
  const accounts = accountsQuery.data || []
  const categories = categoriesQuery.data || []

  return (
    <main className="workspace transaction-workspace">
      <header className="page-header">
        <div><p className="eyebrow">收支流水</p><h1>记账</h1><p>点日历格子选中日期，再点一次或点 + 记一笔。</p></div>
        <Button aria-label="记一笔" onClick={() => setFormState({ date: selectedDay })} variant="primary"><PlusIcon /><span className="button-label">记一笔</span></Button>
      </header>
      <MetricStrip loading={summaryQuery.isLoading} summary={summaryQuery.data} />
      {summaryQuery.error ? <InlineNotice type="error">{summaryQuery.error.message}<Button onClick={() => summaryQuery.refetch()} size="sm">重试</Button></InlineNotice> : null}
      <TabsRoot className="workspace-tabs" defaultValue="calendar">
        <TabsList>
          <TabsTrigger value="calendar">日历</TabsTrigger>
          <TabsTrigger value="list">列表</TabsTrigger>
        </TabsList>
        <TabsContent className="tab-content" value="calendar">
          <div className="tx-layout">
            <TransactionCalendar
              buckets={buckets}
              month={month}
              onAdd={(date) => setFormState({ date })}
              onMonthChange={changeMonth}
              onSelect={setSelectedDay}
              selectedDay={selectedDay}
            />
            <aside className="tx-side">
              <DayDetailPanel
                day={selectedDay}
                items={dayItems}
                loading={monthItemsQuery.isLoading}
                onAdd={(date) => setFormState({ date })}
                onDelete={setDeleting}
                onEdit={(transaction) => setFormState({ transaction, date: transaction.occurredOn })}
              />
              <TrendChart days={summaryQuery.data?.days || []} month={month} />
              <CategoryShare summary={summaryQuery.data} />
            </aside>
          </div>
        </TabsContent>
        <TabsContent className="tab-content" value="list">
          <TransactionListTab
            accounts={accounts}
            categories={categories}
            monthKey={monthKey}
            onDelete={setDeleting}
            onEdit={(transaction) => setFormState({ transaction, date: transaction.occurredOn })}
          />
        </TabsContent>
      </TabsRoot>
      <TransactionFormModal
        accounts={accounts}
        categories={categories}
        defaultDate={formState?.date || selectedDay}
        onOpenChange={(open) => !open && setFormState(undefined)}
        onSaved={refresh}
        open={Boolean(formState)}
        transaction={formState?.transaction}
      />
      <ConfirmDialog
        confirmLabel="确认删除"
        description="删除后该笔记账将归档，不再计入统计与账户余额，可通过恢复找回。"
        onConfirm={() => deleting && deleteMutation.mutate(deleting)}
        onOpenChange={(open) => !open && setDeleting(undefined)}
        open={Boolean(deleting)}
        pending={deleteMutation.isPending}
        title={`删除“${deleting?.category || "未分类"} ${deleting ? yuan(deleting.amountCents) : ""}”？`}
      />
    </main>
  )
}
