import { useQuery, useQueryClient } from "@tanstack/react-query"
import {
  CalendarDaysIcon,
  HandCoinsIcon,
  ChevronLeftIcon,
  ChevronRightIcon,
  PencilIcon,
  PlusIcon,
  SearchIcon,
  UploadIcon,
  Trash2Icon,
  XIcon,
} from "lucide-react"
import { useEffect, useMemo, useRef, useState, type CSSProperties, type Ref } from "react"
import { Link, useNavigate } from "react-router-dom"

import { ApiClientError, api } from "../api/client"
import { useIdempotentMutation } from "../api/use-idempotent-mutation"
import type {
  Category,
  DuplicateSuspicion,
  LedgerAccount,
  LedgerTransaction,
  TransactionList,
  TransactionDaySummary,
  TransactionKind,
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
  Sheet,
  TablePagination,
  Textarea,
  useToast,
} from "../components/ui"
import { useTopbarSlots } from "../components/topbar-slots"
import { pluginRegistry } from "../plugins/registry"
import { usePluginEnabled, usePluginState } from "../plugins/context"
import { ledgerAccountDisplayLabel } from "./ledger-account"
import { DebtFormModal } from "./debt-workspace"
import { formatDate, monthDays, parseDate, startOfMonth } from "../components/date-utils"

const yuan = (cents: number) => new Intl.NumberFormat("zh-CN", { style: "currency", currency: "CNY" }).format(cents / 100)

/** 日历格子用的紧凑金额：¥0.40 / ¥999 / ¥1.2k / ¥3.4万，防止小格内溢出。
 * 一元以下保留两位小数——逐笔清单里几毛钱的支出取整会变成没有信息量的「¥0」。 */
function compactYuan(cents: number) {
  const sign = cents < 0 ? "-" : ""
  const abs = Math.abs(cents)
  if (abs >= 1_000_000) return `${sign}¥${(abs / 1_000_000).toFixed(1)}万`
  if (abs >= 100_000) return `${sign}¥${(abs / 100_000).toFixed(1)}k`
  if (abs < 100) return `${sign}¥${(abs / 100).toFixed(2)}`
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
const KIND_LABEL: Record<TransactionKind, string> = { income: "收入", expense: "支出", transfer: "转账" }
type DayBucket = { income: number; expense: number }

type FlatCategory = Category & { label: string }

// 调用方给了 `|| []`，但递归这层拿的是 category.children——后端少返回一个
// children 字段，这里就会对 undefined 调 flatMap，整棵树连同页面一起炸掉。
// 分类树是展示用的，缺一层子节点不该让流水页打不开。
function flattenCategories(categories: Category[] | undefined, parentLabel = ""): FlatCategory[] {
  if (!Array.isArray(categories)) return []
  return categories.flatMap((category) => {
    const label = parentLabel ? `${parentLabel} / ${category.name}` : category.name
    return [{ ...category, label }, ...flattenCategories(category.children, label)]
  })
}

function transactionCategoryLabel(transaction: LedgerTransaction, categoriesById: Map<string, FlatCategory>) {
  return (transaction.categoryId ? categoriesById.get(transaction.categoryId)?.label : "") || transaction.category || "未分类"
}

function transactionUpdateInput(transaction: LedgerTransaction, categoryId: string) {
  return {
    version: transaction.version,
    kind: transaction.kind,
    amountCents: transaction.amountCents,
    occurredOn: transaction.occurredOn,
    category: transaction.category,
    categoryId,
    accountId: transaction.account?.id || null,
    transferFromAccountId: transaction.transferFromAccount?.id || null,
    transferToAccountId: transaction.transferToAccount?.id || null,
    note: transaction.note,
  }
}

const EMPTY_ITEMS: LedgerTransaction[] = []

/** 日历格子的清单区高度随窗口变化（六行网格分摊剩余高度），条数和行高都实测后算：
 * 先按最小可读行高求出塞得下几行，再把行高摊回可用高度填满，避免出现半截被裁的行。 */
const ENTRY_GAP = 2
const MIN_ENTRY_HEIGHT = 17
const MAX_ENTRY_HEIGHT = 26
const MAX_ENTRY_ROWS = 8
/** 拿不到布局时（首帧、jsdom）退回固定行数，行高交给 CSS 默认值。 */
const DEFAULT_ENTRY_LAYOUT: EntryLayout = { rows: 4, height: 0 }

type EntryLayout = { rows: number; height: number }

function computeEntryLayout(available: number): EntryLayout {
  if (!available) return DEFAULT_ENTRY_LAYOUT
  if (available < MIN_ENTRY_HEIGHT) return { rows: 0, height: 0 }
  const rows = Math.min(MAX_ENTRY_ROWS, Math.floor((available + ENTRY_GAP) / (MIN_ENTRY_HEIGHT + ENTRY_GAP)))
  return { rows, height: Math.min(MAX_ENTRY_HEIGHT, (available - ENTRY_GAP * (rows - 1)) / rows) }
}

function bucketByDate(days: TransactionDaySummary[] | undefined) {
  const map = new Map<string, DayBucket>()
  for (const day of days || []) map.set(day.date, { income: day.incomeCents, expense: day.expenseCents })
  return map
}

/** 日历与当日抽屉都要按天列出每一笔，服务端单页上限是 200 条，导入过账单的月份远不止这些。
 * 先取第一页拿到 total，剩下的页并发补齐；封顶 10 页，避免异常 total 把浏览器拖垮。 */
const MONTH_PAGE_SIZE = 200
const MONTH_PAGE_CAP = 10

async function fetchMonthItems(monthKey: string) {
  const first = await api.transactions({ month: monthKey, page: 1, pageSize: MONTH_PAGE_SIZE })
  const pages = Math.min(Math.ceil(first.total / MONTH_PAGE_SIZE), MONTH_PAGE_CAP)
  if (pages <= 1) return first
  const rest = await Promise.all(
    Array.from({ length: pages - 1 }, (_, index) => api.transactions({ month: monthKey, page: index + 2, pageSize: MONTH_PAGE_SIZE })),
  )
  return { ...first, items: rest.reduce((items, chunk) => items.concat(chunk.items), first.items) }
}

function itemsByDate(items: LedgerTransaction[] | undefined) {
  const map = new Map<string, LedgerTransaction[]>()
  for (const item of items || []) {
    const bucket = map.get(item.occurredOn)
    if (bucket) bucket.push(item)
    else map.set(item.occurredOn, [item])
  }
  return map
}


/** 表格/卡片行的账户标签：行数据里的 account 是精简对象（无银行名/卡号），
 * 优先从已加载的账户列表取完整对象，保证与筛选下拉、绑定下拉的展示完全一致。 */
function rowAccountLabel(account: LedgerTransaction["account"], byId: Map<string, LedgerAccount>) {
  if (!account) return ""
  return ledgerAccountDisplayLabel(byId.get(account.id) ?? account)
}

function transactionTitle(item: LedgerTransaction) {
  const title = item.payeeName?.trim() || item.category || "未分类"
  return {
    title,
    description: item.description?.trim() || "",
    note: item.note?.trim() || "",
    category: item.category && item.category !== title ? item.category : "",
  }
}

function transactionAccountLabel(item: LedgerTransaction, byId: Map<string, LedgerAccount>) {
  if (item.kind !== "transfer") return rowAccountLabel(item.account, byId)
  const from = rowAccountLabel(item.transferFromAccount ?? null, byId) || "未知账户"
  const to = rowAccountLabel(item.transferToAccount ?? null, byId) || "未知账户"
  return `${from} → ${to}`
}

const DUPLICATE_PAGE_SIZE = 200
const DUPLICATE_PAGE_CAP = 10

async function fetchOpenDuplicateSuspicions() {
  const first = await api.duplicateSuspicions({ page: 1, pageSize: DUPLICATE_PAGE_SIZE })
  const pages = Math.min(Math.ceil(first.total / DUPLICATE_PAGE_SIZE), DUPLICATE_PAGE_CAP)
  if (pages <= 1) return first.items
  const rest = await Promise.all(Array.from({ length: pages - 1 }, (_, index) => api.duplicateSuspicions({ page: index + 2, pageSize: DUPLICATE_PAGE_SIZE })))
  return rest.reduce((items, chunk) => items.concat(chunk.items), first.items)
}

function CalendarDayCell({
  date,
  bucket,
  items,
  rows,
  entriesRef,
  currentMonth,
  selected,
  today,
  onSelect,
}: {
  date: Date
  bucket: DayBucket | undefined
  items: LedgerTransaction[]
  rows: number
  entriesRef?: Ref<HTMLSpanElement>
  currentMonth: boolean
  selected: boolean
  today: boolean
  onSelect: (date: string) => void
}) {
  const dateValue = formatDate(date)
  const classNames = [
    "tx-day",
    currentMonth ? "" : "tx-day-other-month",
    selected ? "tx-day-selected" : "",
  ].filter(Boolean).join(" ")
  // 一天里几分钱的分账手续费能有十几笔，按时间序排格子会被这类零头占满，
  // 几百块的那笔反而挤进「还有 N 笔」。只有几行的地方先给金额最大的，明细面板仍按时间序。
  const ranked = useMemo(() => (rows > 0 ? [...items].sort((a, b) => b.amountCents - a.amountCents) : EMPTY_ITEMS), [items, rows])
  // 溢出提示自己也占一行，所以放不下时让出一条明细的位置，总行数始终不超过实测的 rows。
  const overflow = rows > 0 && items.length > rows ? items.length - rows + 1 : 0
  const visible = rows > 0 ? ranked.slice(0, overflow ? rows - 1 : rows) : EMPTY_ITEMS
  return (
    <button
      aria-current={today ? "date" : undefined}
      aria-label={`${dateValue}${bucket ? `，收入 ${yuan(bucket.income)}，支出 ${yuan(bucket.expense)}` : "，无记录"}${items.length ? `，共 ${items.length} 笔` : ""}`}
      aria-pressed={selected}
      className={classNames}
      onClick={() => onSelect(dateValue)}
      type="button"
    >
      <span className="tx-day-head">
        <span className={`tx-day-number ${today ? "tx-day-today" : ""}`} data-center-content>
          <span data-center-ink>{date.getDate()}</span>
        </span>
        {bucket ? (
          <span className="tx-day-amounts">
            {bucket.income > 0 ? <span className="tx-amount-income">+{compactYuan(bucket.income)}</span> : null}
            {bucket.expense > 0 ? <span className="tx-amount-expense">-{compactYuan(bucket.expense)}</span> : null}
          </span>
        ) : null}
      </span>
      {/* 清单容器常驻：它就是量高度的探针，空日子也要占住网格第二行。 */}
      <span className="tx-day-entries" ref={entriesRef}>
        {visible.map((item) => (
          <span className={`tx-day-entry tx-day-entry-${item.kind}`} key={item.id}>
            <span className="tx-day-entry-name">{transactionTitle(item).title}</span>
            <span className="tx-day-entry-amount">{item.kind === "income" ? "+" : item.kind === "expense" ? "-" : ""}{compactYuan(item.amountCents)}</span>
          </span>
        ))}
        {overflow > 0 ? <span className="tx-day-more">{visible.length ? `还有 ${overflow} 笔` : `共 ${items.length} 笔`}</span> : null}
      </span>
    </button>
  )
}

function TransactionCalendar({
  month,
  selectedDay,
  buckets,
  itemsByDate,
  onSelect,
}: {
  month: Date
  selectedDay: string
  buckets: Map<string, DayBucket>
  itemsByDate: Map<string, LedgerTransaction[]>
  onSelect: (date: string) => void
}) {
  const days = useMemo(() => monthDays(month), [month])
  const today = formatDate(new Date())
  // 所有格子等高，量第一个格子的清单区就够；它的高度只由网格行高决定，与里面列了几笔无关，
  // 所以观测不会自激。ResizeObserver 缺席（jsdom）时保持默认布局。
  const probeRef = useRef<HTMLSpanElement>(null)
  const [layout, setLayout] = useState(DEFAULT_ENTRY_LAYOUT)
  useEffect(() => {
    const node = probeRef.current
    if (!node || typeof ResizeObserver === "undefined") return
    const observer = new ResizeObserver((entries) => {
      const next = computeEntryLayout(entries[0]?.contentRect.height ?? 0)
      setLayout((current) => (current.rows === next.rows && current.height === next.height ? current : next))
    })
    observer.observe(node)
    return () => observer.disconnect()
  }, [])
  const gridStyle = { "--tx-entry-gap": `${ENTRY_GAP}px`, ...(layout.height ? { "--tx-entry-height": `${layout.height}px` } : {}) } as CSSProperties
  return (
    <section aria-label="记账日历" className="tx-calendar">
      <div className="tx-calendar-weekdays">{WEEKDAYS.map((day) => <span key={day}>{day}</span>)}</div>
      <div className="tx-calendar-grid" role="grid" style={gridStyle}>
        {days.map((date, index) => (
          <CalendarDayCell
            bucket={buckets.get(formatDate(date))}
            currentMonth={date.getMonth() === month.getMonth()}
            date={date}
            entriesRef={index === 0 ? probeRef : undefined}
            items={itemsByDate.get(formatDate(date)) || EMPTY_ITEMS}
            key={formatDate(date)}
            onSelect={onSelect}
            rows={layout.rows}
            selected={formatDate(date) === selectedDay}
            today={formatDate(date) === today}
          />
        ))}
      </div>
    </section>
  )
}

function MonthControls({ month, onMonthChange }: { month: Date; onMonthChange: (month: Date) => void }) {
  return (
    <div aria-label={`${month.getFullYear()} 年 ${month.getMonth() + 1} 月`} className="topbar-month-controls" role="group">
      <Button aria-label="上一月" onClick={() => onMonthChange(new Date(month.getFullYear(), month.getMonth() - 1, 1))} size="icon" title="上一月" variant="outline"><ChevronLeftIcon /></Button>
      <strong>{month.getFullYear()} 年 {month.getMonth() + 1} 月</strong>
      <Button aria-label="下一月" onClick={() => onMonthChange(new Date(month.getFullYear(), month.getMonth() + 1, 1))} size="icon" title="下一月" variant="outline"><ChevronRightIcon /></Button>
    </div>
  )
}

function TransactionAmount({ transaction }: { transaction: LedgerTransaction }) {
  const sign = transaction.kind === "income" ? "+" : transaction.kind === "expense" ? "-" : ""
  return (
    <span className={`tx-row-amount tx-amount-${transaction.kind}`}>
      {sign}{yuan(transaction.amountCents)}
    </span>
  )
}

function TransactionLinkBadges({ transaction }: { transaction: LedgerTransaction }) {
  const { plugins } = usePluginState()
  return transaction.links.map((link) => {
    const plugin = pluginRegistry.find((candidate) => candidate.id === link.pluginId)
    const enabled = plugins?.find((candidate) => candidate.id === link.pluginId)?.enabled ?? true
    const descriptor = { kind: link.kind, refId: link.refId }
    const text = plugin?.linkLabel?.(descriptor) ?? link.label
    const href = plugin?.linkHref?.(descriptor)
    const title = enabled ? link.label && link.label !== text ? `${link.label} · ${text}` : text : "插件已关闭"
    return href && enabled
      ? <Link className="debt-link-badge" key={`${link.pluginId}:${link.kind}:${link.refId}`} title={title} to={href}>{text}</Link>
      : <span className={`debt-link-badge${enabled ? "" : " debt-link-badge-disabled"}`} key={`${link.pluginId}:${link.kind}:${link.refId}`} title={title}>{text}</span>
  })
}

function transactionDeletionWarning(transaction: LedgerTransaction | undefined) {
  if (!transaction) return ""
  const creatorId = transaction.createdBy.startsWith("plugin:") ? transaction.createdBy.slice(7) : ""
  const pluginIds = [...new Set([
    ...(creatorId ? [creatorId] : []),
    ...transaction.links.map((link) => link.pluginId),
  ])]
  if (!pluginIds.length) return ""
  const pluginNames = pluginIds.map((pluginId) => {
    const plugin = pluginRegistry.find((candidate) => candidate.id === pluginId)
    return `${plugin?.name ?? pluginId}插件`
  })
  const labels = [...new Set(transaction.links.map((link) => link.label).filter(Boolean))]
  const relationship = creatorId
    ? transaction.links.length ? "创建/关联" : "创建"
    : "关联"
  return `这笔由${pluginNames.join("、")}${relationship}${labels.length ? `（${labels.join("、")}）` : ""}，删除会解除关联。`
}

function pluginOwnedTransactionMessage(transaction: LedgerTransaction | undefined) {
  if (!transaction?.createdBy.startsWith("plugin:")) return ""
  const pluginId = transaction.createdBy.slice(7)
  const plugin = pluginRegistry.find((candidate) => candidate.id === pluginId && candidate.ownsTransactions)
  return plugin ? `这笔由${plugin.name}创建，请在${plugin.name}里删除对应记录` : ""
}

const hasDebtsLink = (transaction: LedgerTransaction) => transaction.links.some((link) => link.pluginId === "debts")

function PnlScopeBadge({ transaction }: { transaction: LedgerTransaction }) {
  return transaction.pnlScope === "excluded" ? <span className="pnl-scope-badge">不计入收支</span> : null
}

const CATEGORY_SOURCE_LABEL: Record<string, string> = {
  user: "手动归类",
  rule: "规则自动归类",
  import: "账单自带",
  none: "未归类",
}

/** 不可编辑、但值得看见的那些字段：账单自带的说明、分类是谁定的、精确到秒的时间、
 *  债务关联、版本号。它们不进表单控件（用户改不了），而是作为表单末尾的补充说明，
 *  查看和编辑时都在——不另开一套只读 UI。 */
function TransactionMeta({
  transaction,
  accountsById,
  categoriesById,
}: {
  transaction: LedgerTransaction
  accountsById: Map<string, LedgerAccount>
  categoriesById: Map<string, FlatCategory>
}) {
  // 后端只在有秒级精度时才给 occurredAt；日精度的账单（招商 PDF 只有日期）
  // 不要假装我们知道几点几分。
  const time = transaction.occurredAtPrecision === "second" && transaction.occurredAt
    ? transaction.occurredAt.replace("T", " ").replace(/Z$/, "")
    : `${transaction.occurredOn}（仅精确到日）`
  const rows: Array<[string, string]> = [["发生时间", time]]
  if (transaction.description) rows.push(["账单说明", transaction.description])
  const categorySource = transaction.categorySource === "rule" && transaction.categoryRuleName
    ? `规则：${transaction.categoryRuleName}`
    : CATEGORY_SOURCE_LABEL[transaction.categorySource ?? ""] || transaction.categorySource || "未归类"
  rows.push(["分类来源", categorySource])
  if (transaction.payeeName) rows.push(["交易对象", transaction.payeeName])
  if (transaction.kind === "transfer") {
    rows.push(["转账路径", transactionAccountLabel(transaction, accountsById) || "—"])
  }
  if (transaction.currency && transaction.currency !== "CNY") rows.push(["币种", transaction.currency])
  transaction.links.forEach((link) => {
    const plugin = pluginRegistry.find((candidate) => candidate.id === link.pluginId)
    rows.push([plugin?.linkLabel?.({ kind: link.kind, refId: link.refId }) ?? "关联", link.label])
  })
  if (transaction.pnlScope === "excluded") rows.push(["收支统计", "不计入收支"])
  if (transaction.archived) rows.push(["状态", "已归档"])
  rows.push(["记录时间", `${transaction.createdAt.replace("T", " ").slice(0, 19)}\u3000·\u3000第 ${transaction.version} 版`])
  void categoriesById

  return (
    <dl className="tx-meta selectable">
      {rows.map(([label, value], index) => (
        <div key={`${label}:${value}:${index}`}>
          <dt>{label}</dt>
          <dd>{value}</dd>
        </div>
      ))}
    </dl>
  )
}

function DuplicateSuspicionBadge({ suspicion, onOpen }: { suspicion?: DuplicateSuspicion; onOpen: (suspicion: DuplicateSuspicion) => void }) {
  if (!suspicion || suspicion.status !== "open") return null
  return <button className="duplicate-suspicion-badge" onClick={() => onOpen(suspicion)} type="button">疑似重复</button>
}

function DuplicateSuspicionModal({ suspicion, pending, onOpenChange, onResolve }: { suspicion?: DuplicateSuspicion; pending: boolean; onOpenChange: (open: boolean) => void; onResolve: (status: "confirmed" | "dismissed") => void }) {
  const pair = suspicion ? [suspicion.transactionA, suspicion.transactionB] : []
  return <Modal
    description={suspicion?.reason || "对比两笔流水后选择处理结果；此操作不会合并或删除交易。"}
    footer={<><Button disabled={pending} onClick={() => onResolve("dismissed")} variant="outline">忽略提示</Button><Button disabled={pending} onClick={() => onResolve("confirmed")} variant="default">确认重复</Button></>}
    onOpenChange={onOpenChange}
    open={Boolean(suspicion)}
    title="处理疑似重复"
  >
    <div className="duplicate-comparison">
      {pair.map((item, index) => <section aria-label={`候选流水 ${index + 1}`} className="duplicate-comparison-row" key={item.id}>
        <strong>{item.occurredOn} · {KIND_LABEL[item.kind as TransactionKind] || item.kind}</strong>
        <span>{yuan(item.amountCents)} · {channelLabel(item.sourceChannel)}</span>
      </section>)}
      <InlineNotice>仅记录核对结果，不会自动合并或删除任何流水。</InlineNotice>
    </div>
  </Modal>
}

function channelLabel(channel: string) {
  return channel === "alipay" ? "支付宝" : channel === "wechat" ? "微信支付" : channel || "手工记账"
}

function DayDetailPanel({
  day,
  items,
  accounts,
  loading,
  onEdit,
  onDelete,
  onAdd,
  duplicateByTransaction,
  onOpenDuplicate,
}: {
  day: string
  items: LedgerTransaction[]
  accounts: LedgerAccount[]
  loading: boolean
  onEdit: (transaction: LedgerTransaction) => void
  onDelete: (transaction: LedgerTransaction) => void
  onAdd: (date: string) => void
  duplicateByTransaction: Map<string, DuplicateSuspicion>
  onOpenDuplicate: (suspicion: DuplicateSuspicion) => void
  // WIP：日历面板的「查看」尚未接线，先收下回调让类型闭合
  onView?: (transaction: LedgerTransaction) => void
}) {
  const accountsById = new Map(accounts.map((account) => [account.id, account]))
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
          {items.map((item) => {
            const { title, description, note, category } = transactionTitle(item)
            return (
              <div className="tx-row" key={item.id}>
                <div className="tx-row-copy">
                  <span className="tx-category-with-badge"><strong>{title}</strong><TransactionLinkBadges transaction={item} /><PnlScopeBadge transaction={item} /><DuplicateSuspicionBadge onOpen={onOpenDuplicate} suspicion={duplicateByTransaction.get(item.id)} /></span>
                  <span>{[description, note, transactionAccountLabel(item, accountsById), category].filter(Boolean).join(" · ") || "—"}</span>
                </div>
                <TransactionAmount transaction={item} />
                <ActionMenu
                  items={[
                    { label: "编辑", icon: <PencilIcon />, onSelect: () => onEdit(item) },
                    ...(pluginOwnedTransactionMessage(item) ? [] : [{ label: "删除", icon: <Trash2Icon />, onSelect: () => onDelete(item), destructive: true }]),
                  ]}
                  label={`操作 ${title} ${yuan(item.amountCents)}`}
                  quiet
                />
              </div>
            )
          })}
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

type CategoryScope = "single" | "merchant"

function CategoryAssignmentModal({
  transaction,
  categories,
  filters,
  onOpenChange,
  onSaved,
}: {
  transaction: LedgerTransaction
  categories: FlatCategory[]
  filters: { month: string; kind: string; category: string; accountId: string }
  onOpenChange: (open: boolean) => void
  onSaved: (result: { category: Category; scope: CategoryScope; updated?: LedgerTransaction }) => void
}) {
  const autoCategorizeEnabled = usePluginEnabled("auto-categorize")
  const matchingCategories = categories.filter((category) => !category.archived && (transaction.kind === "transfer" || category.kind === transaction.kind))
  const currentCategory = matchingCategories.find((category) => category.id === transaction.categoryId)
    || matchingCategories.find((category) => category.name === transaction.category)
  const [categoryValue, setCategoryValue] = useState(currentCategory?.id || "")
  const [categoryText, setCategoryText] = useState("")
  const [scope, setScope] = useState<CategoryScope>("single")
  const [error, setError] = useState("")
  const options = matchingCategories.map((category) => ({ value: category.id, label: category.label }))
  const impactQuery = useQuery({
    queryKey: ["transactions", "category-impact", { ...filters, payeeKey: transaction.payeeKey }],
    queryFn: () => api.transactions({ ...filters, q: transaction.payeeKey, page: 1, pageSize: 1 }),
    enabled: Boolean(transaction.payeeKey),
  })

  const mutation = useIdempotentMutation({
    mutationFn: async (_variables: void, write) => {
      setError("")
      const typedName = categoryText.trim()
      let selected = matchingCategories.find((category) => category.id === categoryValue)
        || matchingCategories.find((category) => category.label.trim().toLowerCase() === typedName.toLowerCase())
        || matchingCategories.find((category) => category.name.trim().toLowerCase() === typedName.toLowerCase())
      if (!selected && typedName) {
        if (transaction.kind === "transfer") throw new Error("转账流水只能选择已有分类")
        selected = await api.createCategory({ parentId: null, name: typedName, kind: transaction.kind, sortOrder: 0 }, write) as FlatCategory
      }
      if (!selected) throw new Error("请选择或新建分类")
      if (scope === "single") {
        const updated = await api.updateTransaction(transaction.id, transactionUpdateInput(transaction, selected.id), write)
        return { category: selected, scope, updated }
      }
      if (!transaction.payeeKey) throw new Error("这笔流水没有可用于规则匹配的商户键")
      await api.createCategoryRule({
        priority: 100,
        enabled: true,
        sourceChannel: "",
        categoryId: selected.id,
        note: "",
        conditions: [{ matchField: "payee_key", matchKind: "exact", matchValue: transaction.payeeKey }],
      }, write)
      await api.recategorize(write)
      return { category: selected, scope }
    },
    onSuccess: onSaved,
    onError: (cause) => setError(cause instanceof Error ? cause.message : "归类失败"),
  })

  const merchantUnavailable = !transaction.payeeKey || impactQuery.isLoading || impactQuery.isError
  const canSubmit = Boolean(categoryValue || categoryText.trim()) && !mutation.isPending && (scope === "single" || !merchantUnavailable)
  return (
    <Modal
      description={transaction.payeeName || transaction.description || "这笔流水"}
      footer={<><Button disabled={mutation.isPending} onClick={() => onOpenChange(false)}>取消</Button><Button disabled={!canSubmit} onClick={() => mutation.mutate()} variant="default">{mutation.isPending ? "正在归类…" : "确认归类"}</Button></>}
      onOpenChange={onOpenChange}
      open
      title="归类流水"
    >
      <div className="form-stack category-assignment-form">
        <Field label="分类（可搜索或输入新建）">
          <CreatableSelect
            ariaLabel="分类"
            emptyHint="暂无可用分类，直接输入新建"
            onSelect={setCategoryValue}
            onTextChange={setCategoryText}
            options={options}
            placeholder="搜索已有分类或输入新分类"
            text={categoryText}
            value={categoryValue}
          />
        </Field>
        <div className="field">
          <span className="field-label">归类范围</span>
          <div aria-label="归类范围" className="category-scope-options" role="group">
            <button aria-label="只归这一笔" aria-pressed={scope === "single"} onClick={() => setScope("single")} type="button">
              <strong>只归这一笔</strong>
              <span>仅修改当前流水，保留其他记录原样。</span>
            </button>
            {autoCategorizeEnabled ? <button aria-label="以后这个商户都归它" aria-pressed={scope === "merchant"} disabled={!transaction.payeeKey} onClick={() => setScope("merchant")} type="button">
              <strong>以后这个商户都归它</strong>
              <span>{transaction.payeeKey ? `按归一化商户键“${transaction.payeeKey}”建立规则。` : "这笔流水没有可用于规则匹配的商户键。"}</span>
            </button> : null}
          </div>
        </div>
        {scope === "merchant" ? (
          impactQuery.isLoading ? <InlineNotice>正在计算当前筛选下的影响笔数…</InlineNotice>
            : impactQuery.isError ? <InlineNotice type="error">影响笔数计算失败，请重试后再建立规则。</InlineNotice>
              : <InlineNotice>当前筛选下预计影响 {impactQuery.data?.total ?? 0} 笔；规则会回填所有可由规则归类的流水，用户手动归类不会被覆盖。</InlineNotice>
        ) : null}
        {error ? <InlineNotice type="error">{error}</InlineNotice> : null}
      </div>
    </Modal>
  )
}

function TransactionListTab({
  monthKey,
  accounts,
  categories,
  onEdit,
  onView,
  onDelete,
  onCreateDebt,
  duplicateByTransaction,
  onOpenDuplicate,
}: {
  monthKey: string
  accounts: LedgerAccount[]
  categories: string[]
  onEdit: (transaction: LedgerTransaction) => void
  onView: (transaction: LedgerTransaction) => void
  onDelete: (transaction: LedgerTransaction) => void
  onCreateDebt: (transaction: LedgerTransaction) => void
  duplicateByTransaction: Map<string, DuplicateSuspicion>
  onOpenDuplicate: (suspicion: DuplicateSuspicion) => void
}) {
  const debtsEnabled = usePluginEnabled("debts")
  const queryClient = useQueryClient()
  const toast = useToast()
  const [kind, setKind] = useState("")
  const [category, setCategory] = useState("")
  const [accountId, setAccountId] = useState("")
  const [searchInput, setSearchInput] = useState("")
  const [q, setQ] = useState("")
  const searchQueryRef = useRef("")
  const [page, setPage] = useState(1)
  const [pageSize, setPageSize] = useState(20)
  const [categoryTransaction, setCategoryTransaction] = useState<LedgerTransaction>()
  useEffect(() => {
    const timer = window.setTimeout(() => {
      const next = searchInput.trim()
      if (searchQueryRef.current === next) return
      searchQueryRef.current = next
      setQ(next)
      setPage(1)
    }, 300)
    return () => window.clearTimeout(timer)
  }, [searchInput])
  const listQueryKey = ["transactions", "list", { month: monthKey, kind, category, accountId, q, page, pageSize }] as const
  const query = useQuery({
    queryKey: listQueryKey,
    queryFn: () => api.transactions({ month: monthKey, kind, category, accountId, ...(q ? { q } : {}), page, pageSize }),
  })
  const categoryTreeQuery = useQuery({ queryKey: ["categories"], queryFn: api.categories })
  const changeFilter = (setter: (value: string) => void) => (value: string) => { setter(value); setPage(1) }
  const clearSearch = () => { searchQueryRef.current = ""; setSearchInput(""); setQ(""); setPage(1) }
  const items = query.data?.items || []
  const empty = !query.isLoading && items.length === 0
  const emptyCopy = q
    ? { title: "没有匹配的流水", description: "试试其他商户名或搜索词。" }
    : kind || category || accountId
      ? { title: "本月没有符合筛选条件的流水", description: "调整筛选条件，或回到日历点格子记一笔。" }
      : { title: "本月没有流水", description: "回到日历点格子，开始记一笔。" }
  const emptyState = () => <div className="table-state"><CalendarDaysIcon /><h2>{emptyCopy.title}</h2><p>{emptyCopy.description}</p></div>
  const kindOptions = [{ value: "expense", label: "支出" }, { value: "income", label: "收入" }, { value: "transfer", label: "转账" }]
  const categoryOptions = categories.map((value) => ({ value, label: value }))
  const flatCategories = useMemo(() => flattenCategories(categoryTreeQuery.data || []), [categoryTreeQuery.data])
  const categoriesById = useMemo(() => new Map(flatCategories.map((item) => [item.id, item])), [flatCategories])
  const accountsById = new Map(accounts.map((account) => [account.id, account]))
  const accountOptions = accounts.map((account) => ({ value: account.id, label: ledgerAccountDisplayLabel(account) }))
  const categoryEntry = (item: LedgerTransaction, title: string) => {
    const label = transactionCategoryLabel(item, categoriesById)
    return <button aria-label={`归类 ${title}`} className="tx-category-entry" data-empty={label === "未分类" || undefined} onClick={() => setCategoryTransaction(item)} type="button">{label}</button>
  }
  const categorySaved = ({ category: savedCategory, scope, updated }: { category: Category; scope: CategoryScope; updated?: LedgerTransaction }) => {
    queryClient.setQueriesData<TransactionList>({ queryKey: ["transactions"] }, (current) => current ? {
      ...current,
      items: current.items.map((item) => {
        if (scope === "single") return item.id === updated?.id ? updated : item
        const ruleEligible = !item.categorySource || item.categorySource === "none" || item.categorySource === "rule"
        return item.payeeKey === categoryTransaction?.payeeKey && ruleEligible
          ? { ...item, categoryId: savedCategory.id, categorySource: "rule" }
          : item
      }),
    } : current)
    queryClient.setQueryData<Category[]>(["categories"], (current) => current?.some((item) => item.id === savedCategory.id) ? current : [...(current || []), savedCategory])
    queryClient.setQueryData<string[]>(["transaction-categories"], (current) => current?.includes(savedCategory.name) ? current : [...(current || []), savedCategory.name])
    void queryClient.invalidateQueries({ queryKey: ["transactions"], refetchType: "none" })
    void queryClient.invalidateQueries({ queryKey: ["transaction-summary"], refetchType: "none" })
    setCategoryTransaction(undefined)
    toast({ title: scope === "single" ? "已归类这笔流水" : "商户归类规则已生效", type: "success" })
  }
  return (
    <div className="tx-list">
      <section aria-label="账目筛选" className="toolbar tx-toolbar">
        <FilterSelect ariaLabel="收支类型" onValueChange={changeFilter(setKind)} options={kindOptions} placeholder="全部类型" value={kind} />
        <FilterSelect ariaLabel="分类" onValueChange={changeFilter(setCategory)} options={categoryOptions} placeholder="全部分类" value={category} />
        <FilterSelect ariaLabel="账户" onValueChange={changeFilter(setAccountId)} options={accountOptions} placeholder="全部账户" value={accountId} />
        <label className="sr-only" htmlFor="transaction-search">搜索流水</label>
        <div className="tx-search">
          <SearchIcon aria-hidden="true" className="tx-search-icon" />
          <Input id="transaction-search" maxLength={100} onChange={(event) => setSearchInput(event.target.value)} placeholder="搜索商户、说明或备注" type="search" value={searchInput} />
          {searchInput ? <button aria-label="清除搜索" className="tx-search-clear" onClick={clearSearch} type="button"><XIcon /></button> : null}
        </div>
      </section>
      {query.error ? <InlineNotice type="error">{query.error.message}<Button onClick={() => query.refetch()} size="sm">重试</Button></InlineNotice> : null}
      <div aria-busy={query.isLoading} className="data-dock tx-data-dock">
        <div className="desktop-table tx-table-frame">
          <table className="tx-table" data-slot="table">
            <thead><tr><th>日期</th><th>交易对象 / 分类</th><th>账户</th><th>说明 / 备注</th><th>金额</th><th data-sticky-cell="right"><span className="sr-only">操作</span></th></tr></thead>
            <tbody>
              {query.isLoading ? Array.from({ length: 6 }, (_, index) => (
                <tr aria-hidden="true" className="skeleton-row" key={`tx-skeleton-${index}`}>
                  {Array.from({ length: 6 }, (__, cellIndex) => <td key={cellIndex}><span className="skeleton-line" /></td>)}
                </tr>
              )) : empty ? (
                <tr className="table-state-row"><td colSpan={6}>{emptyState()}</td></tr>
              ) : items.map((item) => {
                const { title, description, note } = transactionTitle(item)
                return <tr
                  data-slot="table-row"
                  key={item.id}
                  onClick={(event) => {
                    // 行里还坐着「归类」「疑似重复」「操作」三个按钮，各有各的动作，
                    // 点它们不该顺带弹详情。
                    if ((event.target as HTMLElement).closest("button, a, [role='menuitem']")) return
                    // 单元格是允许选中复制的（styles.css 放开了 .desktop-table td），
                    // 拖选订单号后松手不该被当成一次点击。
                    if (window.getSelection()?.toString()) return
                    onView(item)
                  }}
                  onKeyDown={(event) => {
                    if (event.target !== event.currentTarget) return
                    if (event.key !== "Enter" && event.key !== " ") return
                    event.preventDefault()
                    onView(item)
                  }}
                  tabIndex={0}
                >
                  <td>{item.occurredOn}</td>
                  <td><span className="tx-category-with-badge"><strong>{title}</strong><TransactionLinkBadges transaction={item} /><PnlScopeBadge transaction={item} /><DuplicateSuspicionBadge onOpen={onOpenDuplicate} suspicion={duplicateByTransaction.get(item.id)} /></span>{categoryEntry(item, title)}</td>
                  <td>{transactionAccountLabel(item, accountsById) || "—"}</td>
                  <td><span className="tx-note">{[description, note].filter(Boolean).join(" · ") || "—"}</span></td>
                  <td className="tx-amount-cell"><TransactionAmount transaction={item} /></td>
                  <td data-sticky-cell="right">
                    <ActionMenu
                      items={[
                        ...(debtsEnabled && item.kind !== "transfer" && !hasDebtsLink(item) ? [{ label: "创建债务", icon: <HandCoinsIcon />, onSelect: () => onCreateDebt(item) }] : []),
                        { label: "编辑", icon: <PencilIcon />, onSelect: () => onEdit(item) },
                        ...(pluginOwnedTransactionMessage(item) ? [] : [{ label: "删除", icon: <Trash2Icon />, onSelect: () => onDelete(item), destructive: true }]),
                      ]}
                      label={`操作 ${title} ${yuan(item.amountCents)}`}
                      quiet
                    />
                  </td>
                </tr>
              })}
            </tbody>
          </table>
        </div>
        <div className="tx-list-cards">
          {query.isLoading ? Array.from({ length: 3 }, (_, index) => <div aria-hidden="true" className="tx-list-card" key={index}><span className="skeleton-line" /><span className="skeleton-line" /></div>) : empty ? (
            emptyState()
          ) : items.map((item) => {
            const { title, description, note } = transactionTitle(item)
            return <article
              className="tx-list-card"
              key={item.id}
              onClick={(event) => {
                if ((event.target as HTMLElement).closest("button, a, [role='menuitem']")) return
                if (window.getSelection()?.toString()) return
                onView(item)
              }}
            >
              <div><span className="tx-category-with-badge"><strong>{title}</strong><TransactionLinkBadges transaction={item} /><PnlScopeBadge transaction={item} /><DuplicateSuspicionBadge onOpen={onOpenDuplicate} suspicion={duplicateByTransaction.get(item.id)} /></span><TransactionAmount transaction={item} /></div>
              <span>{item.occurredOn} · {[description, note, transactionAccountLabel(item, accountsById)].filter(Boolean).join(" · ") || KIND_LABEL[item.kind]}</span>
              <div className="tx-list-card-category"><span>分类</span>{categoryEntry(item, title)}</div>
              <div className="tx-list-card-footer">
                <span>{KIND_LABEL[item.kind]}</span>
                <ActionMenu
                  items={[
                    ...(debtsEnabled && item.kind !== "transfer" && !hasDebtsLink(item) ? [{ label: "创建债务", icon: <HandCoinsIcon />, onSelect: () => onCreateDebt(item) }] : []),
                    { label: "编辑", icon: <PencilIcon />, onSelect: () => onEdit(item) },
                    ...(pluginOwnedTransactionMessage(item) ? [] : [{ label: "删除", icon: <Trash2Icon />, onSelect: () => onDelete(item), destructive: true }]),
                  ]}
                  label={`操作 ${title} ${yuan(item.amountCents)}`}
                  quiet
                />
              </div>
            </article>
          })}
        </div>
      </div>
      <TablePagination
        disabled={query.isLoading}
        onPageChange={setPage}
        onPageSizeChange={(size) => { setPageSize(size); setPage(1) }}
        page={page}
        pageSize={pageSize}
        total={query.data?.total ?? 0}
        unit="笔"
      />
      {categoryTransaction ? <CategoryAssignmentModal
        categories={flatCategories}
        filters={{ month: monthKey, kind, category, accountId }}
        onOpenChange={(open) => !open && setCategoryTransaction(undefined)}
        onSaved={categorySaved}
        transaction={categoryTransaction}
      /> : null}
    </div>
  )
}

/** 查看与编辑是同一张表单的两个模式，不是两套 UI。
 *  从列表点进来是 view：字段照常渲染但禁用，底部给「删除 / 编辑」；点「编辑」原地
 *  切成 edit，底部换「取消 / 保存」。这样字段顺序、标签、格式只有一处定义——
 *  曾经短暂存在过一个独立的只读详情弹窗，同样的字段被描述了两遍，样式随即分叉。 */
function TransactionFormModal({
  transaction,
  defaultDate,
  accounts,
  categories,
  categoriesById,
  accountsById,
  open,
  initialMode = "edit",
  onOpenChange,
  onSaved,
  onDelete,
}: {
  transaction?: LedgerTransaction
  defaultDate: string
  accounts: LedgerAccount[]
  categories: string[]
  categoriesById: Map<string, FlatCategory>
  accountsById: Map<string, LedgerAccount>
  open: boolean
  initialMode?: "view" | "edit"
  onOpenChange: (open: boolean) => void
  onSaved: () => Promise<unknown>
  onDelete?: (transaction: LedgerTransaction) => void
}) {
  const toast = useToast()
  const [mode, setMode] = useState<"view" | "edit">(initialMode)
  const readOnly = mode === "view" && Boolean(transaction)
  const [kind, setKind] = useState<TransactionKind>("expense")
  const [amount, setAmount] = useState("")
  const [occurredOn, setOccurredOn] = useState(defaultDate)
  const [categoryValue, setCategoryValue] = useState("")
  const [categoryText, setCategoryText] = useState("")
  const [accountId, setAccountId] = useState("")
  const [transferFromAccountId, setTransferFromAccountId] = useState("")
  const [transferToAccountId, setTransferToAccountId] = useState("")
  const [note, setNote] = useState("")
  const [error, setError] = useState("")

  useEffect(() => {
    if (!open) return
    setMode(initialMode)
    setKind(transaction?.kind || "expense")
    setAmount(transaction ? String(transaction.amountCents / 100) : "")
    setOccurredOn(transaction?.occurredOn || defaultDate)
    setCategoryValue(transaction?.category || "")
    setCategoryText("")
    setAccountId(transaction?.account?.id || "")
    setTransferFromAccountId(transaction?.transferFromAccount?.id || "")
    setTransferToAccountId(transaction?.transferToAccount?.id || "")
    setNote(transaction?.note || "")
    setError("")
  }, [open, transaction, defaultDate, initialMode])

  const categoryOptions = categories.map((value) => ({ value, label: value }))
  const accountOptions = accounts.filter((account) => !account.archived).map((account) => ({ value: account.id, label: `${ledgerAccountDisplayLabel(account)} · 余额 ${yuan(account.balanceCents)}` }))

  const mutation = useIdempotentMutation({
    mutationFn: async (_variables: void, write) => {
      const amountCents = toCents(amount)
      if (!occurredOn) throw new Error("请选择发生日期")
      const isTransfer = kind === "transfer"
      if (isTransfer && !transferFromAccountId && !transferToAccountId) {
        throw new Error("转账至少需要一个转出或转入账户")
      }
      if (isTransfer && transferFromAccountId && transferFromAccountId === transferToAccountId) {
        throw new Error("转出账户和转入账户不能相同")
      }
      const category = (categoryValue || categoryText).trim()
      const input = {
        kind,
        amountCents,
        occurredOn,
        category,
        accountId: isTransfer ? null : accountId || null,
        transferFromAccountId: isTransfer ? transferFromAccountId || null : null,
        transferToAccountId: isTransfer ? transferToAccountId || null : null,
        note: note.trim(),
      }
      if (transaction) return api.updateTransaction(transaction.id, { ...input, version: transaction.version }, write)
      return api.createTransaction(input, write)
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
      footer={readOnly && transaction ? (
        <>
          {onDelete && !pluginOwnedTransactionMessage(transaction) ? <Button onClick={() => { onOpenChange(false); onDelete(transaction) }} variant="ghost">删除</Button> : null}
          <Button onClick={() => setMode("edit")} variant="default">编辑</Button>
        </>
      ) : (
        <>
          <Button disabled={mutation.isPending} onClick={() => onOpenChange(false)}>取消</Button>
          <Button disabled={mutation.isPending} onClick={() => mutation.mutate()} variant="default">{mutation.isPending ? "正在保存…" : "保存"}</Button>
        </>
      )}
      onOpenChange={onOpenChange}
      open={open}
      title={readOnly ? "流水详情" : transaction ? "编辑记账" : "记一笔"}
    >
      <div className="form-stack">
        <div aria-label="收支类型" className="segmented" role="group">
          {(["expense", "income", "transfer"] as const).map((value) => (
            <button aria-pressed={kind === value} disabled={readOnly} key={value} onClick={() => setKind(value)} type="button">{KIND_LABEL[value]}</button>
          ))}
        </div>
        <div className="form-grid">
          <Field label="金额（元）">
            <Input
              aria-label="金额（元）"
              autoFocus={!readOnly}
              disabled={readOnly}
              inputMode="decimal"
              onChange={(event) => setAmount(event.target.value)}
              placeholder="0.00"
              value={amount}
            />
          </Field>
          <Field label="发生日期">
            <DatePicker ariaLabel="发生日期" disabled={readOnly} onValueChange={setOccurredOn} value={occurredOn} />
          </Field>
        </div>
        <Field label="分类（可输入新建）">
          <CreatableSelect
            ariaLabel="分类"
            disabled={readOnly}
            emptyHint="暂无历史分类，直接输入新建"
            onSelect={setCategoryValue}
            onTextChange={setCategoryText}
            options={categoryOptions}
            placeholder="例如：餐饮、交通、工资"
            text={categoryText}
            value={categoryValue}
          />
        </Field>
        {kind === "transfer" ? (
          <div className="form-grid">
            <Field label="转出账户">
              <Select
                ariaLabel="转出账户"
                clearLabel="清空转出账户"
                clearable
                disabled={readOnly}
                onValueChange={setTransferFromAccountId}
                options={accountOptions}
                placeholder="不指定转出"
                value={transferFromAccountId}
              />
            </Field>
            <Field label="转入账户">
              <Select
                ariaLabel="转入账户"
                clearLabel="清空转入账户"
                clearable
                disabled={readOnly}
                onValueChange={setTransferToAccountId}
                options={accountOptions}
                placeholder="不指定转入"
                value={transferToAccountId}
              />
            </Field>
          </div>
        ) : (
          <Field label="账户（可选）">
            <Select
              ariaLabel="账户"
              clearLabel="清空账户选择"
              clearable
              disabled={readOnly}
              onValueChange={setAccountId}
              options={accountOptions}
              placeholder="不关联账户"
              value={accountId}
            />
          </Field>
        )}
        <Field label="备注（可选）">
          <Textarea disabled={readOnly} maxLength={2000} onChange={(event) => setNote(event.target.value)} placeholder="补充用途或说明" rows={2} value={note} />
        </Field>
        {transaction ? <TransactionMeta accountsById={accountsById} categoriesById={categoriesById} transaction={transaction} /> : null}
        {readOnly && pluginOwnedTransactionMessage(transaction) ? <InlineNotice>{pluginOwnedTransactionMessage(transaction)}</InlineNotice> : null}
        {error ? <InlineNotice type="error">{error}</InlineNotice> : null}
      </div>
    </Modal>
  )
}

function TransactionWorkspaceBase({ page }: { page: "calendar" | "transactions" }) {
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const toast = useToast()
  const setTopbarSlots = useTopbarSlots()
  const billImportsEnabled = usePluginEnabled("bill-imports")
  const debtsEnabled = usePluginEnabled("debts")
  const today = formatDate(new Date())
  const [month, setMonth] = useState(() => startOfMonth(new Date()))
  const [detailDay, setDetailDay] = useState<string | undefined>()
  const [formState, setFormState] = useState<{ transaction?: LedgerTransaction; date: string; mode?: "view" | "edit" } | undefined>()
  const [deleting, setDeleting] = useState<LedgerTransaction | undefined>()
  const [debtTransaction, setDebtTransaction] = useState<LedgerTransaction | undefined>()
  const [duplicateSuspicion, setDuplicateSuspicion] = useState<DuplicateSuspicion | undefined>()
  const monthKey = monthKeyOf(month)

  const summaryQuery = useQuery({
    queryKey: ["transaction-summary", monthKey],
    queryFn: () => api.transactionSummary(monthKey),
  })
  const monthItemsQuery = useQuery({
    queryKey: ["transactions", "month", monthKey],
    queryFn: () => fetchMonthItems(monthKey),
  })
  const categoriesQuery = useQuery({ queryKey: ["transaction-categories"], queryFn: api.transactionCategories })
  const accountsQuery = useQuery({ queryKey: ["ledger-accounts"], queryFn: api.ledgerAccounts })
  const counterpartiesQuery = useQuery({ queryKey: ["counterparties"], queryFn: api.counterparties, enabled: Boolean(debtTransaction) })
  const duplicateQuery = useQuery({ queryKey: ["duplicate-suspicions", "open"], queryFn: fetchOpenDuplicateSuspicions })
  const duplicateByTransaction = useMemo(() => {
    const map = new Map<string, DuplicateSuspicion>()
    for (const suspicion of duplicateQuery.data || []) {
      if (suspicion.status !== "open") continue
      map.set(suspicion.transactionA.id, suspicion)
      map.set(suspicion.transactionB.id, suspicion)
    }
    return map
  }, [duplicateQuery.data])

  const refresh = () => Promise.all([
    queryClient.invalidateQueries({ queryKey: ["transactions"] }),
    queryClient.invalidateQueries({ queryKey: ["transaction-summary"] }),
    queryClient.invalidateQueries({ queryKey: ["transaction-categories"] }),
    queryClient.invalidateQueries({ queryKey: ["ledger-accounts"] }),
    queryClient.invalidateQueries({ queryKey: ["counterparties"] }),
  ])

  const restoreMutation = useIdempotentMutation({
    mutationFn: (transaction: Pick<LedgerTransaction, "id" | "version">, write) =>
      api.restoreTransaction(transaction.id, transaction.version, write),
    onSuccess: async () => {
      await refresh()
      toast({ title: "已撤销删除", type: "success" })
    },
    onError: async (cause) => {
      await refresh()
      toast({ title: "撤销失败", description: cause.message, type: "error" })
    },
  })

  const deleteMutation = useIdempotentMutation({
    mutationFn: (transaction: LedgerTransaction, write) => api.deleteTransaction(transaction.id, transaction.version, write),
    onSuccess: async (_result, transaction) => {
      await refresh()
      setDeleting(undefined)
      toast({
        title: "记账已删除",
        type: "success",
        // Deleting bumps the optimistic-lock version, so restoring targets version + 1.
        action: { label: "撤销", onSelect: () => restoreMutation.mutate({ id: transaction.id, version: transaction.version + 1 }) },
      })
    },
    onError: async (cause) => {
      setDeleting(undefined)
      if (cause instanceof ApiClientError && cause.status === 409) await refresh()
      toast({ title: "删除失败", description: cause.message, type: "error" })
    },
  })

  const duplicateMutation = useIdempotentMutation({
    mutationFn: ({ id, status }: { id: string; status: "confirmed" | "dismissed" }, write) => api.updateDuplicateSuspicion(id, { status }, write),
    onSuccess: async (_result, variables) => {
      await queryClient.invalidateQueries({ queryKey: ["duplicate-suspicions"] })
      setDuplicateSuspicion(undefined)
      toast({ title: variables.status === "confirmed" ? "已确认重复" : "已忽略疑似重复", type: "success" })
    },
    onError: (cause) => toast({ title: "处理失败", description: cause.message, type: "error" }),
  })

  const changeMonth = (next: Date) => {
    const start = startOfMonth(next)
    setMonth(start)
    setDetailDay(undefined)
  }

  const buckets = useMemo(() => bucketByDate(summaryQuery.data?.days), [summaryQuery.data])
  const monthItemsByDate = useMemo(() => itemsByDate(monthItemsQuery.data?.items), [monthItemsQuery.data])
  const dayItems = useMemo(
    () => (monthItemsQuery.data?.items || []).filter((item) => item.occurredOn === detailDay),
    [monthItemsQuery.data, detailDay],
  )
  const accounts = useMemo(() => accountsQuery.data || [], [accountsQuery.data])
  const categories = categoriesQuery.data || []
  // 表单末尾的只读元数据要把 accountId / categoryId 翻成人看得懂的名字
  const accountsById = useMemo(() => new Map(accounts.map((account) => [account.id, account])), [accounts])
  const categoryTree = useQuery({ queryKey: ["categories"], queryFn: api.categories })
  const categoriesById = useMemo(
    () => new Map(flattenCategories(categoryTree.data || []).map((item) => [item.id, item])),
    [categoryTree.data],
  )
  const openForm = (state: { transaction?: LedgerTransaction; date: string; mode?: "view" | "edit" }) => {
    setDetailDay(undefined)
    setFormState(state)
  }
  useEffect(() => {
    setTopbarSlots({
      edge: page === "calendar",
      title: page === "calendar" ? "日历" : "流水",
      actions: page === "transactions" ? <>{billImportsEnabled ? <Button onClick={() => navigate("/app/transactions/imports")} variant="outline"><UploadIcon /><span className="button-label">导入账单</span></Button> : null}<Button aria-label="记一笔" onClick={() => openForm({ date: today })} variant="primary"><PlusIcon /><span className="button-label">记一笔</span></Button></> : <>
        <MonthControls month={month} onMonthChange={changeMonth} />
        <Button onClick={() => changeMonth(startOfMonth(new Date()))} variant="outline">今天</Button>
        {page === "calendar" ? <Button aria-label="记一笔" onClick={() => openForm({ date: detailDay || today })} variant="primary"><PlusIcon /><span className="button-label">记一笔</span></Button> : null}
      </>,
    })
    return () => setTopbarSlots(undefined)
  }, [billImportsEnabled, month, page, detailDay, navigate, setTopbarSlots, today])

  return (
    <main className={`workspace transaction-workspace transaction-workspace-${page}`}>
      {page === "calendar" ? <>
        {summaryQuery.error ? <div className="calendar-state"><InlineNotice type="error">{summaryQuery.error.message}<Button onClick={() => summaryQuery.refetch()} size="sm">重试</Button></InlineNotice></div> : null}
        <TransactionCalendar buckets={buckets} itemsByDate={monthItemsByDate} month={month} onSelect={setDetailDay} selectedDay={detailDay || ""} />
        <Sheet onOpenChange={() => setDetailDay(undefined)} open={Boolean(detailDay)} title={detailDay ? `${detailDay} 当日流水` : "当日流水"}>
          <DayDetailPanel accounts={accountsQuery.data || []} day={detailDay || today} duplicateByTransaction={duplicateByTransaction} items={dayItems} loading={monthItemsQuery.isLoading} onAdd={(date) => openForm({ date })} onDelete={(transaction) => { setDeleting(transaction); setDetailDay(undefined) }} onEdit={(transaction) => openForm({ transaction, date: transaction.occurredOn })} onOpenDuplicate={setDuplicateSuspicion} onView={(transaction) => openForm({ transaction, date: transaction.occurredOn, mode: "view" })} />
        </Sheet>
      </> : null}
      {page === "transactions" ? <TransactionListTab accounts={accounts} categories={categories} duplicateByTransaction={duplicateByTransaction} monthKey={monthKeyOf(new Date())} onCreateDebt={setDebtTransaction} onDelete={setDeleting} onEdit={(transaction) => openForm({ transaction, date: transaction.occurredOn })} onOpenDuplicate={setDuplicateSuspicion} onView={(transaction) => openForm({ transaction, date: transaction.occurredOn, mode: "view" })} /> : null}
      <TransactionFormModal
        accounts={accounts}
        accountsById={accountsById}
        categories={categories}
        categoriesById={categoriesById}
        defaultDate={formState?.date || detailDay || today}
        initialMode={formState?.mode || "edit"}
        onDelete={setDeleting}
        onOpenChange={(open) => !open && setFormState(undefined)}
        onSaved={refresh}
        open={Boolean(formState)}
        transaction={formState?.transaction}
      />
      <DebtFormModal
        accounts={accounts}
        accountsLoading={accountsQuery.isLoading}
        counterparties={counterpartiesQuery.data || []}
        initialTransaction={debtTransaction}
        onManageAccounts={() => navigate("/app/accounts")}
        onOpenChange={(open) => !open && setDebtTransaction(undefined)}
        onSaved={refresh}
        open={debtsEnabled && Boolean(debtTransaction) && !counterpartiesQuery.isLoading}
      />
      <ConfirmDialog
        confirmLabel="确认删除"
        description={
          <>
            <span>删除后不再计入统计与账户余额。删除完成后可在提示中撤销。</span>
            {transactionDeletionWarning(deleting) ? <><br /><span>{transactionDeletionWarning(deleting)}</span></> : null}
          </>
        }
        onConfirm={() => deleting && deleteMutation.mutate(deleting)}
        onOpenChange={(open) => !open && setDeleting(undefined)}
        open={Boolean(deleting)}
        pending={deleteMutation.isPending}
        title={`删除“${deleting?.category || "未分类"} ${deleting ? yuan(deleting.amountCents) : ""}”？`}
      />
      <DuplicateSuspicionModal
        onOpenChange={(open) => !open && setDuplicateSuspicion(undefined)}
        onResolve={(status) => duplicateSuspicion && duplicateMutation.mutate({ id: duplicateSuspicion.id, status })}
        pending={duplicateMutation.isPending}
        suspicion={duplicateSuspicion}
      />
    </main>
  )
}

export function CalendarWorkspace() { return <TransactionWorkspaceBase page="calendar" /> }
export function TransactionWorkspace() { return <TransactionWorkspaceBase page="transactions" /> }
export const TransactionListWorkspace = TransactionWorkspace
export { StatisticsDashboardWorkspace as StatisticsWorkspace, StatisticsDashboardWorkspace as TransactionStatisticsWorkspace } from "./statistics"
