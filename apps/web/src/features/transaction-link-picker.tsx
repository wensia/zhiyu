import { useQuery } from "@tanstack/react-query"
import { LinkIcon } from "lucide-react"
import { useState } from "react"

import { api } from "../api/client"
import type { TransactionLinkCandidate } from "../api/types"
import { Button, InlineNotice, Modal } from "../components/ui"
import { ledgerAccountDisplayLabel } from "./ledger-account"

const yuan = (cents: number) => new Intl.NumberFormat("zh-CN", { style: "currency", currency: "CNY" }).format(cents / 100)

export function TransactionLinkPicker({ debtId, amountCents, clearLabel = "取消选择", label = "从流水选取", selected, onSelect, onClear }: {
  debtId?: string
  amountCents?: number
  clearLabel?: string
  label?: string
  selected?: TransactionLinkCandidate | null
  onSelect: (transaction: TransactionLinkCandidate) => void
  onClear?: () => void
}) {
  const [open, setOpen] = useState(false)
  const query = useQuery({
    queryKey: ["debt-link-candidates", debtId || "new", amountCents],
    enabled: open,
    queryFn: async () => debtId
      ? api.transactionLinkCandidates(debtId, { amountCents })
      : (await api.transactions({ page: 1, pageSize: 200 })).items.filter((item) => !item.links.some((link) => link.pluginId === "debts")),
  })
  return <>
    <div className="transaction-link-control">
      {selected ? <div className="linked-transaction-proof"><span>已选流水 · {selected.occurredOn} · {selected.kind === "income" ? "+" : "-"}{yuan(selected.amountCents)}</span><span>{selected.note || "无备注"} · {ledgerAccountDisplayLabel(selected.account)}</span></div> : null}
      <div className="transaction-link-actions"><Button onClick={() => setOpen(true)} size="sm" variant="outline"><LinkIcon />{selected ? "更换流水" : label}</Button>{selected && onClear ? <Button onClick={onClear} size="sm">{clearLabel}</Button> : null}</div>
    </div>
    <Modal open={open} onOpenChange={setOpen} title="选择已入账流水" description="已入账且未关联的流水，联系人相关的排在前，其余按时间从新到旧。选择后金额、日期与账户以流水为准。">
      {query.isLoading ? <div className="table-state"><div className="spinner" /><p>正在查找候选流水…</p></div> : query.error ? <InlineNotice type="error">{query.error.message}</InlineNotice> : query.data?.length ? <div className="transaction-candidate-list">{query.data.map((item) => <button className="transaction-candidate" key={item.id} onClick={() => { onSelect(item); setOpen(false) }}>
        <span><time dateTime={item.occurredOn}>{item.occurredOn}</time><strong>{item.note || "无备注"}</strong></span>
        <span><small>{ledgerAccountDisplayLabel(item.account)}</small><b className={item.kind === "income" ? "tx-amount-income" : ""}>{item.kind === "income" ? "+" : "-"}{yuan(item.amountCents)}</b></span>
      </button>)}</div> : <div className="table-state"><p>没有可关联的流水</p></div>}
    </Modal>
  </>
}
