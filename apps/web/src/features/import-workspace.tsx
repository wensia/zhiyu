import { useQuery } from "@tanstack/react-query"
import { ArrowLeftIcon, FileSpreadsheetIcon, UploadIcon, XIcon } from "lucide-react"
import { useEffect, useRef, useState } from "react"
import { useNavigate, useParams } from "react-router-dom"

import { importQueries, useBindImportAccount, useCommitImport, useDiscardImport, useUploadImport, useUpsertImportAccountMapping } from "../api/imports"
import { api } from "../api/client"
import type { ImportBatch, ImportDetail, ImportPayMethod, ImportRecord, ImportSummary, ImportSummaryItem, LedgerAccount } from "../api/types"
import { Button, ConfirmDialog, InlineNotice, Modal, Select, TablePagination, TabsList, TabsRoot, TabsTrigger, useToast } from "../components/ui"
import { useTopbarSlots } from "../components/topbar-slots"
import { ledgerAccountDisplayLabel } from "./ledger-account"

const money = (cents: number) => new Intl.NumberFormat("zh-CN", { style: "currency", currency: "CNY" }).format(cents / 100)
const fileKindLabel = (name: string) => name.toLowerCase().endsWith(".xlsx") ? "XLSX" : name.toLowerCase().endsWith(".csv") ? "CSV" : "文件"
const formatBytes = (bytes: number) => bytes >= 1024 * 1024 ? `${(bytes / 1024 / 1024).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1024))} KB`
const channelLabel = (channel: string) => channel === "alipay" ? "支付宝" : channel === "wechat" ? "微信支付" : channel
const statusLabels: Record<string, string> = { preview: "待确认", blocked: "已阻止", committed: "已入账", discarded: "已撤销" }
const dispositionLabels: Record<string, string> = {
  import: "待入账",
  pending: "未完成",
  neutral: "中性资金移动",
  closed: "已关闭",
  zero_amount: "零金额",
  unknown: "未知状态",
  duplicate: "重复交易",
}
const dispositionCopy: Record<string, string> = {
  pending: "源账单尚未完成，本批次暂不计入收支；完成后重新导出上传，新批次重新判断。",
  neutral: "渠道标记为不计收支或中性资金移动，例如充值、提现、还款、账户间资金移动；本期不写入收支账本。",
  closed: "渠道已关闭或取消，不写入账本。",
  zero_amount: "成功但实付 0 元，保留在导入记录中，不写入正式收支账本。",
  unknown: "发现未支持的渠道状态，为防止错账，本批次已阻止确认。",
  duplicate: "同用户、同渠道、同交易单号已存在；已归档交易也视为存在。",
}
const summaryDefinitions: Array<[keyof ImportSummary, string, string]> = [
  ["importExpense", "导入支出", "import"], ["importIncome", "导入收入", "import"],
  ["pending", "未完成", "pending"], ["neutral", "中性资金移动", "neutral"],
  ["closed", "已关闭", "closed"], ["zeroAmount", "零金额", "zero_amount"],
  ["unknown", "未知状态", "unknown"], ["duplicate", "重复交易", "duplicate"],
]

function ImportStatus({ value }: { value: string }) {
  return <span className={`status import-status import-status-${value}`}>{statusLabels[value] || value}</span>
}

function ImportUploadModal({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const navigate = useNavigate()
  const upload = useUploadImport()
  const [file, setFile] = useState<File>()
  const [dragging, setDragging] = useState(false)
  const fileInputRef = useRef<HTMLInputElement>(null)
  const submit = () => file && upload.mutate({ file }, { onSuccess: (detail) => { onOpenChange(false); navigate(`/app/transactions/imports/${detail.id}`) } })
  return <Modal open={open} onOpenChange={onOpenChange} title="选择账单文件" description="支持支付宝 CSV 与微信支付 XLSX；文件仅用于本次导入解析。" footer={<><Button disabled={upload.isPending} onClick={() => onOpenChange(false)}>取消</Button><Button disabled={!file || upload.isPending} onClick={submit} variant="default">{upload.isPending ? "正在上传…" : "上传并预览"}</Button></>}>
    <div className="form-stack">
      <div className="field">
        <label className="field-label" htmlFor="bill-import-file">账单文件</label>
        {/* 未选态是投放区，选中态收缩成文件条：两种状态承担的职责不同，形态也不同。
            文件条用 grid「1fr auto」而不是 flex 单行——文件名列吃剩余宽度并省略，
            移除按钮的列宽先分配，结构上不可能被长文件名挤出去。 */}
        {file ? (
          <div className="file-chip">
            <div className="file-chip-text">
              <span className="file-chip-name">{file.name}</span>
              <span className="file-chip-meta">{fileKindLabel(file.name)} · {formatBytes(file.size)}</span>
            </div>
            <Button aria-label="移除已选文件" disabled={upload.isPending} onClick={() => { setFile(undefined); upload.reset(); if (fileInputRef.current) fileInputRef.current.value = "" }} size="icon-sm" type="button" variant="ghost"><XIcon /></Button>
          </div>
        ) : (
          <button
            aria-label="添加账单文件，可拖放到此处"
            className="file-dropzone"
            data-dragging={dragging ? "true" : undefined}
            onClick={() => fileInputRef.current?.click()}
            onDragLeave={(event) => { event.preventDefault(); setDragging(false) }}
            onDragOver={(event) => { event.preventDefault(); setDragging(true) }}
            onDrop={(event) => {
              event.preventDefault()
              setDragging(false)
              const dropped = event.dataTransfer.files?.[0]
              if (dropped) { setFile(dropped); upload.reset() }
            }}
            type="button"
          >
            <span className="file-dropzone-main">把账单文件拖到这里，或点击选择</span>
            <span className="file-dropzone-hint">支付宝 .csv / 微信支付 .xlsx</span>
          </button>
        )}
        <input
          accept=".csv,.xlsx"
          aria-describedby="bill-import-file-description"
          className="file-picker-native"
          id="bill-import-file"
          onChange={(event) => { setFile(event.target.files?.[0]); upload.reset() }}
          ref={fileInputRef}
          tabIndex={-1}
          type="file"
        />
        <span className="field-description" id="bill-import-file-description">请选择从支付渠道导出的原始账单文件。</span>
      </div>
      {upload.error ? <InlineNotice type="error">{upload.error.message}</InlineNotice> : null}
    </div>
  </Modal>
}

function ImportBatchCards({ items, loading }: { items: ImportBatch[]; loading: boolean }) {
  const navigate = useNavigate()
  if (loading) return <div className="import-batch-grid" aria-busy="true">{Array.from({ length: 3 }, (_, i) => <div className="import-batch-card" key={i}><span className="skeleton-line" /><span className="skeleton-line" /></div>)}</div>
  if (!items.length) return <div className="table-state import-empty"><FileSpreadsheetIcon /><h2>还没有导入批次</h2><p>选择一份脱敏或渠道导出的账单开始预览。</p></div>
  return <div className="import-batch-grid">{items.map((item) => <button className="import-batch-card" key={item.id} onClick={() => navigate(`/app/transactions/imports/${item.id}`)} type="button">
    <span className="import-batch-card-head"><strong>{item.fileName}</strong><ImportStatus value={item.status} /></span>
    <span>{channelLabel(item.channel)} · {item.periodStart} 至 {item.periodEnd}</span>
    <span>{item.totalCount} 条记录 · 上传于 {new Date(item.createdAt).toLocaleString("zh-CN")}</span>
  </button>)}</div>
}

export function ImportListWorkspace() {
  const setTopbarSlots = useTopbarSlots()
  const navigate = useNavigate()
  const [page, setPage] = useState(1)
  const [uploadOpen, setUploadOpen] = useState(false)
  const query = useQuery(importQueries.list({ page, pageSize: 20 }))
  useEffect(() => {
    // 本页是从流水页钻取进来的子路由，返回动作归顶栏的 leading 槽，且可访问名要点出目的地。
    // 侧边栏折叠成图标时，它是唯一显式的回程入口。
    setTopbarSlots({
      leading: <Button aria-label="返回流水" onClick={() => navigate("/app/transactions")} size="icon" variant="ghost"><ArrowLeftIcon /></Button>,
      title: "账单导入",
      actions: <Button aria-label="选择文件" onClick={() => setUploadOpen(true)} variant="primary"><UploadIcon /><span className="button-label">选择文件</span></Button>,
    })
    return () => setTopbarSlots(undefined)
  }, [navigate, setTopbarSlots])
  return <main className="workspace import-workspace">
    {/* 顶栏已经是这条路由的标题（h1「账单导入」），正文再写一次就是第二个标题权威。
        留下的这句说的是后果——确认前不写账本——它改变用户的判断，不是填充副标题。 */}
    <section className="import-intro"><div><p>上传后先分组预览，确认前不会写入账本。</p></div></section>
    <InlineNotice>绑定账户用于分类统计；账户余额会包含导入流水，仅供参考。</InlineNotice>
    {query.error ? <InlineNotice type="error">{query.error.message}<Button onClick={() => query.refetch()} size="sm">重试</Button></InlineNotice> : null}
    <ImportBatchCards items={query.data?.items || []} loading={query.isLoading} />
    <TablePagination disabled={query.isLoading} onPageChange={setPage} page={page} pageSize={20} total={query.data?.total || 0} />
    <ImportUploadModal open={uploadOpen} onOpenChange={setUploadOpen} />
  </main>
}

function AccountAttribution({ detail, accounts, selection, onChange }: { detail: ImportDetail; accounts: LedgerAccount[]; selection: string; onChange: (accountId: string) => void }) {
  const active = accounts.filter((account) => !account.archived)
  const options = [{ value: "__none__", label: "不绑定" }, ...active.map((account) => ({ value: account.id, label: ledgerAccountDisplayLabel(account) }))]
  const accountNames = new Map(accounts.map((account) => [account.id, ledgerAccountDisplayLabel(account)]))
  return <section aria-label="账户" className="import-account-attribution"><div className="import-account-attribution-row"><strong>本批账单绑定到</strong>{detail.status === "preview" ? <Select ariaLabel="本批账单绑定账户" onValueChange={onChange} options={options} value={selection || "__none__"} /> : <span>{detail.accountId ? accountNames.get(detail.accountId) || "账户已删除" : "未绑定"}</span>}</div></section>
}

function PaymentMethodMappings({ channel, items, accounts }: { channel: string; items: ImportPayMethod[]; accounts: LedgerAccount[] }) {
  const toast = useToast()
  const mapping = useUpsertImportAccountMapping()
  const options = accounts.filter((account) => !account.archived).map((account) => ({ value: account.id, label: ledgerAccountDisplayLabel(account) }))
  const unboundCount = items.filter((item) => !item.accountId).length
  return <section aria-label="支付方式映射" className="import-payment-mappings">
    <header className="import-payment-mappings-header">
      <div><strong>本批支付方式</strong><span>逐项绑定到资金账户；中性资金移动缺少映射时，整批确认会失败并回滚。</span></div>
      {unboundCount ? <span className="status import-mapping-unbound">{unboundCount} 项未绑定</span> : <span className="status import-mapping-bound">已全部绑定</span>}
    </header>
    {items.length ? <div className="import-payment-mapping-list">{items.map((item) => <div className="import-payment-mapping-row" key={item.payMethod}>
      <div><strong>{item.payMethod || "未提供支付方式"}</strong><span>{item.count} 行</span></div>
      {!item.accountId ? <span className="status import-mapping-unbound">未绑定</span> : null}
      <Select
        ariaLabel={`支付方式 ${item.payMethod || "未提供"} 绑定账户`}
        disabled={mapping.isPending}
        onValueChange={(accountId) => mapping.mutate({ sourceChannel: channel, payMethod: item.payMethod, accountId }, {
          onSuccess: () => toast({ title: "支付方式映射已保存", description: `${item.payMethod || "未提供支付方式"} 已绑定到账户。`, type: "success" }),
          onError: (error) => toast({ title: "映射保存失败", description: error.message, type: "error" }),
        })}
        options={options}
        placeholder="请选择账户"
        value={item.accountId || ""}
      />
    </div>)}</div> : <div className="empty-compact">本批未提供支付方式。</div>}
  </section>
}

function SummaryCard({ label, item, disposition, direction, selected, onSelect }: { label: string; item: ImportSummaryItem; disposition: string; direction?: string; selected: boolean; onSelect: (disposition: string, direction?: string) => void }) {
  return <button aria-label={`按${label}筛选汇总记录`} aria-pressed={selected} className="import-summary-card" onClick={() => onSelect(disposition, direction)} type="button"><span>{label}</span><strong>{item.count} 条</strong><small>{money(item.amountCents)}</small>{dispositionCopy[disposition] ? <p>{dispositionCopy[disposition]}</p> : <p>确认后写入正式收支账本。</p>}</button>
}

function RecordRows({ detail, loading }: { detail?: ImportDetail; loading: boolean }) {
  const rows = detail?.records || []
  const empty = !loading && rows.length === 0
  return <div aria-busy={loading} className="data-dock import-record-dock">
    <div className="desktop-table"><table className="import-record-table"><thead><tr><th>日期</th><th>交易对象 / 商品</th><th>渠道状态</th><th>处理结果</th><th>支付方式</th><th>金额</th></tr></thead><tbody>
      {loading ? Array.from({ length: 6 }, (_, i) => <tr className="skeleton-row" key={i}>{Array.from({ length: 6 }, (__, j) => <td key={j}><span className="skeleton-line" /></td>)}</tr>) : empty ? <tr className="table-state-row"><td colSpan={6}><div className="table-state"><FileSpreadsheetIcon /><h2>该分组暂无记录</h2><p>切换上方分组查看其他处理结果。</p></div></td></tr> : rows.map((row) => <RecordTableRow key={row.id} row={row} />)}
    </tbody></table></div>
    <div className="import-record-cards">{loading ? Array.from({ length: 3 }, (_, i) => <div className="import-record-card" key={i}><span className="skeleton-line" /><span className="skeleton-line" /></div>) : empty ? <div className="table-state"><FileSpreadsheetIcon /><h2>该分组暂无记录</h2></div> : rows.map((row) => <RecordCard key={row.id} row={row} />)}</div>
  </div>
}

function RecordTableRow({ row }: { row: ImportRecord }) {
  return <tr><td>{row.occurredOn}</td><td><strong>{row.counterparty || "未知交易对象"}</strong><span className="import-record-sub">{row.product || row.externalId}</span></td><td>{row.channelStatus || "—"}</td><td><span className={`status import-disposition-${row.disposition}`}>{dispositionLabels[row.disposition] || row.disposition}</span></td><td>{row.payMethod || "未提供"}</td><td className="import-amount">{row.direction === "income" ? "+" : row.direction === "expense" ? "−" : ""}{money(row.amountCents)}</td></tr>
}

function RecordCard({ row }: { row: ImportRecord }) {
  return <article className="import-record-card"><div><strong>{row.counterparty || "未知交易对象"}</strong><span className="import-amount">{row.direction === "income" ? "+" : row.direction === "expense" ? "−" : ""}{money(row.amountCents)}</span></div><span>{row.occurredOn} · {row.product || "未提供商品说明"}</span><div><span className={`status import-disposition-${row.disposition}`}>{dispositionLabels[row.disposition] || row.disposition}</span><span>{row.channelStatus || "—"}</span></div></article>
}

export function ImportDetailWorkspace() {
  const { id = "" } = useParams()
  const navigate = useNavigate()
  const toast = useToast()
  const setTopbarSlots = useTopbarSlots()
  const [disposition, setDisposition] = useState("")
  const [direction, setDirection] = useState("")
  const [page, setPage] = useState(1)
  const [accountSelection, setAccountSelection] = useState("__none__")
  const [bindOpen, setBindOpen] = useState(false)
  const [bindAccountId, setBindAccountId] = useState("")
  const [commitError, setCommitError] = useState("")
  const initializedBatch = useRef("")
  const accountsQuery = useQuery({ queryKey: ["ledger-accounts"], queryFn: api.ledgerAccounts })
  const [confirm, setConfirm] = useState<"commit" | "discard" | null>(null)
  const query = useQuery(importQueries.detail(id, { disposition: disposition || undefined, direction: direction || undefined, page, pageSize: 20 }))
  const detail = query.data
  const status = detail?.status
  const hasUnboundPayMethods = detail?.payMethods.some((item) => !item.accountId) ?? false
  const commit = useCommitImport()
  const bind = useBindImportAccount()
  const discard = useDiscardImport()
  const pending = commit.isPending || discard.isPending || bind.isPending
  useEffect(() => {
    if (!detail || initializedBatch.current === detail.id) return
    initializedBatch.current = detail.id
    setAccountSelection(detail.accountId || "__none__")
  }, [detail])
  const selectSummary = (nextDisposition: string, nextDirection?: string) => {
    const isSelected = disposition === nextDisposition && direction === (nextDirection || "")
    setDisposition(isSelected ? "" : nextDisposition)
    setDirection(isSelected ? "" : nextDirection || "")
    setPage(1)
  }
  const doConfirm = () => {
    if (!detail) return
    if (confirm === "commit") {
      setCommitError("")
      commit.mutate({ id, input: { accountId: accountSelection !== "__none__" ? accountSelection : null } }, { onSuccess: (result) => { setConfirm(null); toast({ title: "账单已确认入账", description: `写入 ${result.importedCount} 条，重复 ${result.duplicateCount} 条。`, type: "success" }) }, onError: (error) => { setConfirm(null); setCommitError(error.message); toast({ title: "确认失败", description: error.message, type: "error" }) } })
    }
    if (confirm === "discard") discard.mutate(id, { onSuccess: (result) => { setConfirm(null); toast({ title: detail?.status === "committed" ? "已撤销导入" : "已放弃批次", description: result.retainedModifiedCount > 0 ? `仍保留 ${result.retainedModifiedCount} 条用户已编辑或归档的交易。` : `已删除 ${result.deletedCount} 条导入交易。`, type: "success" }) }, onError: (error) => { setConfirm(null); toast({ title: detail?.status === "committed" ? "撤销失败" : "放弃失败", description: error.message, type: "error" }) } })
  }
  useEffect(() => {
    const actions = status ? <>{status === "preview" ? <><Button onClick={() => setConfirm("discard")}>放弃</Button><Button disabled={hasUnboundPayMethods} onClick={() => setConfirm("commit")} title={hasUnboundPayMethods ? "请先绑定本批全部支付方式" : undefined} variant="primary">确认入账</Button></> : status === "blocked" ? <Button onClick={() => setConfirm("discard")} variant="destructive">放弃</Button> : status === "committed" ? <>{!detail?.accountId ? <Button onClick={() => setBindOpen(true)}>绑定账户</Button> : null}<Button onClick={() => setConfirm("discard")} variant="destructive">撤销导入</Button></> : null}</> : null
    // 记录路由的标题是那份文件本身，由下面的 import-detail-header 持有 h1；顶栏原来
    // 挂的是「导入详情」——它命名的是模板不是记录，规范明确不要这种 generic 标题。
    setTopbarSlots({ leading: <Button aria-label="返回导入批次" onClick={() => navigate("/app/transactions/imports")} size="icon" variant="ghost"><ArrowLeftIcon /></Button>, actions })
    return () => setTopbarSlots(undefined)
  }, [status, detail?.accountId, hasUnboundPayMethods, navigate, setTopbarSlots])
  if (query.error && !detail) return <main className="workspace import-workspace"><InlineNotice type="error">{query.error.message}<Button onClick={() => query.refetch()} size="sm">重试</Button></InlineNotice></main>
  if (!detail) return <main className="workspace import-workspace"><div className="table-state" aria-busy="true"><div className="spinner" /><p>正在加载导入批次…</p></div></main>
  return <main className="workspace import-workspace import-detail-workspace">
    <header className="import-detail-header"><div><span>{channelLabel(detail.channel)} · {detail.periodStart} 至 {detail.periodEnd}</span><h1>{detail.fileName}</h1></div><ImportStatus value={detail.status} /></header>
    <InlineNotice>绑定账户用于分类统计；账户余额会包含导入流水，仅供参考。</InlineNotice>
    {detail.previousCommittedBatchId ? <InlineNotice>发现历史同 hash 的已提交批次（{new Date(detail.previousCommittedAt || "").toLocaleString("zh-CN")}），仅作提示，不阻止继续确认。</InlineNotice> : null}
    {detail.status === "blocked" ? <InlineNotice type="error">发现未支持的渠道状态，为防止错账，本批次已阻止确认。请查看“未知状态”分组。</InlineNotice> : null}
    {commitError ? <InlineNotice type="error">确认入账失败：{commitError}</InlineNotice> : null}
    <AccountAttribution accounts={accountsQuery.data || []} detail={detail} onChange={setAccountSelection} selection={accountSelection} />
    <PaymentMethodMappings accounts={accountsQuery.data || []} channel={detail.channel} items={detail.payMethods} />
    <section aria-label="批次汇总" className="import-summary-grid">{summaryDefinitions.map(([key, label, value]) => {
      const summaryDirection = key === "importExpense" ? "expense" : key === "importIncome" ? "income" : undefined
      return <SummaryCard direction={summaryDirection} disposition={value} item={detail.summary[key]} key={key} label={label} onSelect={selectSummary} selected={disposition === value && direction === (summaryDirection || "")} />
    })}</section>
    <TabsRoot className="workspace-tabs import-record-tabs" onValueChange={(value) => { setDisposition(value); setDirection(""); setPage(1) }} value={disposition}>
      <TabsList aria-label="处理结果筛选"><TabsTrigger value="">全部</TabsTrigger>{summaryDefinitions.slice(2).map(([, label, value]) => <TabsTrigger key={value} value={value}>{label}</TabsTrigger>)}<TabsTrigger value="import">待入账</TabsTrigger></TabsList>
      <div className="import-disposition-copy">{dispositionCopy[disposition] || "按服务端处理结果分页展示本批次全部记录。"}</div>
      {query.error ? <InlineNotice type="error">{query.error.message}<Button onClick={() => query.refetch()} size="sm">重试</Button></InlineNotice> : null}
      <RecordRows detail={detail} loading={query.isFetching} />
      <TablePagination disabled={query.isFetching} onPageChange={setPage} page={page} pageSize={detail.pageSize} total={detail.filteredCount} />
    </TabsRoot>
    <ConfirmDialog destructive={confirm === "discard"} open={Boolean(confirm)} onOpenChange={(open) => !open && setConfirm(null)} title={confirm === "commit" ? "确认将账单写入账本？" : detail.status === "committed" ? "撤销这个导入批次？" : "放弃这个导入批次？"} description={confirm === "commit" ? "待入账收支会写入账本；中性资金移动会按支付方式映射记录为转账。任一必要映射缺失都会使整批失败并回滚。" : detail.status === "committed" ? "未被用户编辑或归档的导入交易会删除；已编辑或归档的交易将保留。此操作不会提供 toast 撤销。" : "批次将标记为已放弃，且无法重新确认。此操作不会提供 toast 撤销。"} confirmLabel={confirm === "commit" ? "确认入账" : detail.status === "committed" ? "确认撤销" : "确认放弃"} onConfirm={doConfirm} pending={pending} />
    <Modal open={bindOpen} onOpenChange={setBindOpen} title="绑定账户" description="只会为本批次尚未绑定账户的导入交易补上账户。" footer={<><Button onClick={() => setBindOpen(false)}>取消</Button><Button disabled={!bindAccountId || bind.isPending} onClick={() => bind.mutate({ id, input: { accountId: bindAccountId } }, { onSuccess: (result) => { setBindOpen(false); toast({ title: "账户绑定完成", description: `已为 ${result.updatedCount} 条导入流水绑定账户。`, type: "success" }) }, onError: (error) => toast({ title: "绑定失败", description: error.message, type: "error" }) })} variant="primary">确认绑定</Button></>}><div className="field"><label className="field-label">账户</label><Select ariaLabel="补绑账户" onValueChange={setBindAccountId} options={(accountsQuery.data || []).filter((account) => !account.archived).map((account) => ({ value: account.id, label: ledgerAccountDisplayLabel(account) }))} placeholder="请选择账户" value={bindAccountId} /></div></Modal>
  </main>
}
