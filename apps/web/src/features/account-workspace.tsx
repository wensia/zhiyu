import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { ArchiveIcon, PencilIcon, PlusIcon, RotateCcwIcon, SearchIcon, WalletCardsIcon } from "lucide-react"
import { useEffect, useMemo, useState } from "react"

import { ApiClientError, api } from "../api/client"
import type { LedgerAccount, LedgerAccountType } from "../api/types"
import {
  ActionMenu,
  Button,
  CheckboxControl,
  ConfirmDialog,
  Field,
  InlineNotice,
  Input,
  Modal,
  Select,
  Textarea,
  useToast,
} from "../components/ui"
import {
  BANK_NAME_OPTIONS,
  LEDGER_ACCOUNT_TYPE_OPTIONS,
  OTHER_BANK_VALUE,
  ledgerAccountDetailItems,
  ledgerAccountDisplayLabel,
  ledgerAccountSearchText,
  ledgerAccountTypeLabel,
  type LedgerAccountDetails,
} from "./ledger-account"

function normalizeOptional(value: string) {
  const normalized = value.trim()
  return normalized || null
}

function normalizePhone(value: string) {
  const normalized = normalizeOptional(value)
  if (!normalized) return null
  const validCharacters = [...normalized].every((character, index) => /[0-9 -]/.test(character) || (index === 0 && character === "+"))
  const digitCount = [...normalized].filter((character) => /[0-9]/.test(character)).length
  if ([...normalized].length > 64 || !validCharacters || digitCount < 7 || digitCount > 20) throw new Error("手机号须包含 7–20 位数字、总长不超过 64 个字符，仅可使用开头的 +、空格或短横线")
  return normalized
}

function normalizeEmail(value: string) {
  const normalized = normalizeOptional(value)?.toLocaleLowerCase("en-US") || null
  if (normalized && (new TextEncoder().encode(normalized).length > 254 || !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(normalized))) throw new Error("邮箱格式不正确")
  return normalized
}

function normalizeCardNumber(value: string) {
  const normalized = normalizeOptional(value)
  if (!normalized) return null
  if (![...normalized].every((character) => /[0-9 -]/.test(character))) throw new Error("银行卡号仅可使用数字、空格或短横线")
  const digits = [...normalized].filter((character) => /[0-9]/.test(character)).join("")
  if (digits.length < 12 || digits.length > 23) throw new Error("银行卡号须包含 12–23 位数字")
  return digits
}

const yuan = (cents: number) => new Intl.NumberFormat("zh-CN", { style: "currency", currency: "CNY" }).format(cents / 100)

/** 初始余额允许负值（如信用卡透支），最多两位小数。 */
function parseOpeningBalance(value: string) {
  const normalized = value.trim().replace(/[¥,，\s]/g, "")
  if (!normalized) return 0
  if (!/^-?\d+(\.\d{1,2})?$/.test(normalized)) throw new Error("初始余额格式不正确，最多两位小数")
  const cents = Math.round(Number.parseFloat(normalized) * 100)
  if (!Number.isSafeInteger(cents)) throw new Error("初始余额超出安全范围")
  return cents
}

function AccountBalance({ cents }: { cents: number }) {
  return <span className={`fund-account-balance ${cents > 0 ? "positive" : cents < 0 ? "negative" : ""}`}>{yuan(cents)}</span>
}

function AccountDetails({ account }: { account: LedgerAccountDetails }) {
  const items = ledgerAccountDetailItems(account)
  if (!items.length) return <span className="fund-account-details-empty">—</span>
  return (
    <span className="fund-account-details">
      {items.map(({ label, value }) => <span key={label}><small>{label}</small><span>{value}</span></span>)}
    </span>
  )
}

function AccountStatus({ archived }: { archived: boolean }) {
  return <span className={`status ${archived ? "status-archived" : "account-status-active"}`}>{archived ? "已归档" : "可用"}</span>
}

function AccountFormModal({
  account,
  open,
  onOpenChange,
  onSaved,
}: {
  account?: LedgerAccount
  open: boolean
  onOpenChange: (open: boolean) => void
  onSaved: () => Promise<unknown>
}) {
  const toast = useToast()
  const [accountType, setAccountType] = useState<LedgerAccountType | "">("")
  const [name, setName] = useState("")
  const [note, setNote] = useState("")
  const [bankSelection, setBankSelection] = useState("")
  const [customBankName, setCustomBankName] = useState("")
  const [branchName, setBranchName] = useState("")
  const [cardNumber, setCardNumber] = useState("")
  const [nickname, setNickname] = useState("")
  const [phone, setPhone] = useState("")
  const [email, setEmail] = useState("")
  const [openingBalance, setOpeningBalance] = useState("")
  const [error, setError] = useState("")
  const bankAccount = accountType === "bank_card"
  const walletAccount = accountType === "wechat_balance" || accountType === "alipay_balance"
  const structuredAccount = bankAccount || walletAccount

  useEffect(() => {
    if (!open) return
    const details = account as (LedgerAccount & LedgerAccountDetails) | undefined
    const initialBankName = details?.bankName || ""
    const knownBank = initialBankName !== OTHER_BANK_VALUE && BANK_NAME_OPTIONS.some((option) => option.value === initialBankName)
    setAccountType(account?.accountType || "")
    setName(details?.nameSource === "derived" ? "" : account?.name || "")
    setNote(account?.note || "")
    setBankSelection(initialBankName ? (knownBank ? initialBankName : OTHER_BANK_VALUE) : "")
    setCustomBankName(initialBankName && !knownBank ? initialBankName : "")
    setBranchName(details?.branchName || "")
    setCardNumber(details?.cardNumber || "")
    setNickname(details?.nickname || "")
    setPhone(details?.phone || "")
    setEmail(details?.email || "")
    setOpeningBalance(account?.openingBalanceCents ? String(account.openingBalanceCents / 100) : "")
    setError("")
  }, [account, open])

  const changeAccountType = (value: LedgerAccountType) => {
    const changed = value !== accountType
    const replacingExistingType = accountType !== ""
    setAccountType(value)
    setError("")
    if (!changed || !replacingExistingType) return
    setName("")
    setBankSelection("")
    setCustomBankName("")
    setBranchName("")
    setCardNumber("")
    setNickname("")
    setPhone("")
    setEmail("")
  }

  const mutation = useMutation({
    mutationFn: async () => {
      if (!accountType) throw new Error("请选择账户类型")
      const normalizedName = name.trim()
      if (!structuredAccount && !normalizedName) throw new Error("请输入账户名称")
      const normalizedBankName = accountType === "bank_card"
        ? normalizeOptional(bankSelection === OTHER_BANK_VALUE ? customBankName : bankSelection)
        : null
      const details = {
        bankName: normalizedBankName,
        branchName: accountType === "bank_card" ? normalizeOptional(branchName) : null,
        cardNumber: accountType === "bank_card" ? normalizeCardNumber(cardNumber) : null,
        nickname: accountType === "wechat_balance" || accountType === "alipay_balance" ? normalizeOptional(nickname) : null,
        phone: accountType === "wechat_balance" || accountType === "alipay_balance" ? normalizePhone(phone) : null,
        email: accountType === "alipay_balance" ? normalizeEmail(email) : null,
      }
      const input = { accountType, name: normalizedName, note: note.trim(), openingBalanceCents: parseOpeningBalance(openingBalance), ...details }
      if (account) return api.updateLedgerAccount(account.id, { ...input, version: account.version })
      return api.createLedgerAccount(input)
    },
    onSuccess: async () => {
      await onSaved()
      onOpenChange(false)
      toast({ title: account ? "账户已更新" : "账户已新增", type: "success" })
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
      title={account ? "编辑账户" : "新增账户"}
      wide
    >
      <div className="account-form">
        <section aria-labelledby="account-form-basic-heading" className="account-form-section">
          <div className="account-form-section-heading">
            <h3 id="account-form-basic-heading">基本信息</h3>
          </div>
          <div className="account-form-row">
            <Field label="账户类型">
              <Select ariaLabel="账户类型" onValueChange={(value) => changeAccountType(value as LedgerAccountType)} options={LEDGER_ACCOUNT_TYPE_OPTIONS} placeholder="请选择账户类型" value={accountType} />
            </Field>
            <Field
              description={structuredAccount ? "留空时按账户详情识别；仅在需要区分同类账户时填写。" : undefined}
              label={structuredAccount ? "自定义名称（可选）" : "账户名称"}
            >
              <Input
                aria-label={structuredAccount ? "自定义名称（可选）" : "账户名称"}
                maxLength={80}
                onChange={(event) => setName(event.target.value)}
                placeholder={structuredAccount ? "例如：工资卡、日常号" : "例如：随身现金、数字人民币钱包"}
                value={name}
              />
            </Field>
          </div>
          <div className="account-form-row">
            <Field description="余额 = 初始余额 + 记账流水 ± 债务现金流水。" label="初始余额（可选，元）">
              <Input inputMode="decimal" onChange={(event) => setOpeningBalance(event.target.value)} placeholder="0.00，可为负数" value={openingBalance} />
            </Field>
            <span aria-hidden="true" className="account-form-empty-cell" />
          </div>
        </section>
        {bankAccount ? (
          <section aria-labelledby="account-form-details-heading" className="account-form-section">
            <div className="account-form-section-heading">
              <h3 id="account-form-details-heading">银行卡信息</h3>
            </div>
            <div className="account-form-row">
              <Field label="银行（可选）">
                <Select ariaLabel="银行（可选）" onValueChange={(value) => { setBankSelection(value); if (value !== OTHER_BANK_VALUE) setCustomBankName("") }} options={BANK_NAME_OPTIONS} placeholder="请选择银行" value={bankSelection} />
              </Field>
              {bankSelection === OTHER_BANK_VALUE ? (
                <Field label="银行名称（可选）">
                  <Input maxLength={80} onChange={(event) => setCustomBankName(event.target.value)} placeholder="请输入银行名称" value={customBankName} />
                </Field>
              ) : <span aria-hidden="true" className="account-form-empty-cell" />}
            </div>
            <div className="account-form-row">
              <Field label="开户行（可选）">
                <Input maxLength={120} onChange={(event) => setBranchName(event.target.value)} placeholder="例如：北京中关村支行" value={branchName} />
              </Field>
              <Field label="银行卡号（可选）">
                <Input autoComplete="cc-number" inputMode="numeric" maxLength={31} onChange={(event) => setCardNumber(event.target.value)} placeholder="例如：6222 0000 0000 1234" value={cardNumber} />
              </Field>
            </div>
          </section>
        ) : null}
        {walletAccount ? (
          <section aria-labelledby="account-form-details-heading" className="account-form-section">
            <div className="account-form-section-heading">
              <h3 id="account-form-details-heading">{accountType === "wechat_balance" ? "微信信息" : "支付宝信息"}</h3>
            </div>
            <div className="account-form-row">
              <Field label="昵称（可选）">
                <Input autoComplete="nickname" maxLength={80} onChange={(event) => setNickname(event.target.value)} placeholder={accountType === "wechat_balance" ? "微信昵称" : "支付宝昵称"} value={nickname} />
              </Field>
              <Field label="手机号（可选）">
                <Input autoComplete="tel" inputMode="tel" maxLength={64} onChange={(event) => setPhone(event.target.value)} placeholder="例如：138 0013 8000" type="tel" value={phone} />
              </Field>
            </div>
            {accountType === "alipay_balance" ? (
              <div className="account-form-row">
                <Field label="邮箱（可选）">
                  <Input autoComplete="email" maxLength={254} onChange={(event) => setEmail(event.target.value)} placeholder="例如：name@example.com" type="email" value={email} />
                </Field>
                <span aria-hidden="true" className="account-form-empty-cell" />
              </div>
            ) : null}
          </section>
        ) : null}
        <section aria-labelledby="account-form-note-heading" className="account-form-section">
          <div className="account-form-section-heading">
            <h3 id="account-form-note-heading">补充信息</h3>
          </div>
          <Field className="account-form-note" label="备注（可选）">
            <Textarea maxLength={500} onChange={(event) => setNote(event.target.value)} placeholder="补充持有人、用途或其他识别信息" rows={3} value={note} />
          </Field>
        </section>
        {error ? <InlineNotice type="error">{error}</InlineNotice> : null}
      </div>
    </Modal>
  )
}

function AccountTable({
  accounts,
  loading,
  onEdit,
  onToggleArchive,
}: {
  accounts: LedgerAccount[]
  loading: boolean
  onEdit: (account: LedgerAccount) => void
  onToggleArchive: (account: LedgerAccount) => void
}) {
  const empty = !loading && accounts.length === 0
  const rows = loading ? Array.from({ length: 6 }, (_, index) => (
    <tr aria-hidden="true" className="skeleton-row" key={`account-skeleton-${index}`}>
      {Array.from({ length: 8 }, (__, cellIndex) => <td key={cellIndex}><span className="skeleton-line" /></td>)}
    </tr>
  )) : accounts.map((account) => (
    <tr data-slot="table-row" key={account.id}>
      <td><span className="fund-account-type">{ledgerAccountTypeLabel(account.accountType)}</span></td>
      <td><strong className="fund-account-name">{account.name}</strong></td>
      <td><AccountDetails account={account} /></td>
      <td><span className="fund-account-note">{account.note || "—"}</span></td>
      <td>{account.usageCount} 笔</td>
      <td><AccountBalance cents={account.balanceCents} /></td>
      <td><AccountStatus archived={account.archived} /></td>
      <td data-sticky-cell="right">
        <ActionMenu
          items={[
            { label: "编辑账户", icon: <PencilIcon />, onSelect: () => onEdit(account) },
            { label: account.archived ? "恢复账户" : "归档账户", icon: account.archived ? <RotateCcwIcon /> : <ArchiveIcon />, onSelect: () => onToggleArchive(account) },
          ]}
          label={`操作 ${ledgerAccountDisplayLabel(account)}`}
          quiet
        />
      </td>
    </tr>
  ))

  return (
    <div aria-busy={loading} className="data-dock account-data-dock">
      <div className="desktop-table fund-account-table-frame">
        <table className="fund-account-table" data-slot="table">
          <thead><tr><th>账户类型</th><th>账户</th><th>账户信息</th><th>备注</th><th>关联流水</th><th>余额</th><th>状态</th><th data-sticky-cell="right"><span className="sr-only">操作</span></th></tr></thead>
          <tbody>
            {empty ? <tr className="table-state-row"><td colSpan={8}><div className="table-state"><WalletCardsIcon /><h2>暂无符合条件的账户</h2><p>调整搜索或归档筛选。</p></div></td></tr> : rows}
          </tbody>
        </table>
      </div>
      <div className="fund-account-mobile-list">
        {loading ? Array.from({ length: 3 }, (_, index) => <div aria-hidden="true" className="fund-account-card fund-account-card-skeleton" key={index}><span className="skeleton-line" /><span className="skeleton-line" /></div>) : empty ? <div className="table-state"><WalletCardsIcon /><h2>暂无符合条件的账户</h2><p>调整搜索或归档筛选。</p></div> : accounts.map((account) => (
          <article className="fund-account-card" key={account.id}>
            <div><strong>{account.name}</strong><AccountStatus archived={account.archived} /></div>
            <span className="fund-account-type">{ledgerAccountTypeLabel(account.accountType)}</span>
            <AccountDetails account={account} />
            {account.note ? <p>{account.note}</p> : null}
            <div className="fund-account-card-footer"><span>关联 {account.usageCount} 笔流水 · 余额 <AccountBalance cents={account.balanceCents} /></span><ActionMenu items={[{ label: "编辑账户", icon: <PencilIcon />, onSelect: () => onEdit(account) }, { label: account.archived ? "恢复账户" : "归档账户", icon: account.archived ? <RotateCcwIcon /> : <ArchiveIcon />, onSelect: () => onToggleArchive(account) }]} label={`操作 ${ledgerAccountDisplayLabel(account)}`} quiet /></div>
          </article>
        ))}
      </div>
    </div>
  )
}

export function AccountWorkspace() {
  const queryClient = useQueryClient()
  const toast = useToast()
  const [search, setSearch] = useState("")
  const [showArchived, setShowArchived] = useState(false)
  const [createOpen, setCreateOpen] = useState(false)
  const [editing, setEditing] = useState<LedgerAccount | undefined>()
  const [confirming, setConfirming] = useState<LedgerAccount | undefined>()
  const query = useQuery({ queryKey: ["ledger-accounts"], queryFn: api.ledgerAccounts })
  const refresh = () => queryClient.invalidateQueries({ queryKey: ["ledger-accounts"] })
  const accounts = useMemo(() => {
    const needle = search.trim().toLocaleLowerCase("zh-CN")
    return (query.data || []).filter((account) => (showArchived || !account.archived) && (!needle || ledgerAccountSearchText(account).toLocaleLowerCase("zh-CN").includes(needle)))
  }, [query.data, search, showArchived])
  const archiveMutation = useMutation({
    mutationFn: async (account: LedgerAccount) => account.archived
      ? api.restoreLedgerAccount(account.id, account.version)
      : api.archiveLedgerAccount(account.id, account.version),
    onSuccess: async (_, account) => {
      await refresh()
      setConfirming(undefined)
      toast({ title: account.archived ? "账户已恢复" : "账户已归档", type: "success" })
    },
    onError: (error) => toast({ title: "操作失败", description: error.message, type: "error" }),
  })

  return (
    <main className="workspace account-workspace">
      <header className="page-header">
        <div><p className="eyebrow">资金溯源</p><h1>账户管理</h1><p>配置付款与收款账户，让每一笔资金往来都有明确去向。</p></div>
        <Button aria-label="新增账户" onClick={() => setCreateOpen(true)} variant="primary"><PlusIcon /><span className="button-label">新增账户</span></Button>
      </header>
      <section aria-label="账户筛选" className="toolbar account-toolbar">
        <label className="search-box"><SearchIcon /><span className="sr-only">搜索账户</span><input onChange={(event) => setSearch(event.target.value)} placeholder="搜索账户类型、账户、账户信息或备注" value={search} /></label>
        <CheckboxControl checked={showArchived} label="查看归档" onChange={(event) => setShowArchived(event.target.checked)} />
      </section>
      {query.error ? <InlineNotice type="error">{query.error.message}<Button onClick={() => query.refetch()} size="sm">重试</Button></InlineNotice> : null}
      <AccountTable accounts={accounts} loading={query.isLoading} onEdit={setEditing} onToggleArchive={setConfirming} />
      <p className="tx-balance-hint">余额 = 初始余额 + 记账流水 ± 债务现金流水；同一笔钱请勿重复记录；历史（未确认现金流）债务的还款不计入余额。</p>
      <AccountFormModal onOpenChange={setCreateOpen} onSaved={refresh} open={createOpen} />
      <AccountFormModal account={editing} onOpenChange={(open) => !open && setEditing(undefined)} onSaved={refresh} open={Boolean(editing)} />
      <ConfirmDialog
        confirmLabel={confirming?.archived ? "确认恢复" : "确认归档"}
        description={confirming?.archived ? "恢复后，该账户可以重新用于新的付款和收款。" : "归档后，该账户不能用于新的付款或收款；历史流水仍会保留并显示账户名称。可随时恢复。"}
        destructive={false}
        onConfirm={() => confirming && archiveMutation.mutate(confirming)}
        onOpenChange={(open) => !open && setConfirming(undefined)}
        open={Boolean(confirming)}
        pending={archiveMutation.isPending}
        title={`${confirming?.archived ? "恢复" : "归档"}“${confirming ? ledgerAccountDisplayLabel(confirming) : "该账户"}”？`}
      />
    </main>
  )
}
