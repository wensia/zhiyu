import type { Counterparty, LedgerTransaction } from "../api/types"

export function debtDraftCounterparty(transaction: Pick<LedgerTransaction, "note" | "payeeName" | "description" | "category">, counterparties: Counterparty[]) {
  const searchable = [transaction.payeeName, transaction.description, transaction.note].filter(Boolean).join(" ")
  const match = counterparties
    .filter((item) => !item.archived && searchable.includes(item.displayName))
    .sort((left, right) => right.displayName.length - left.displayName.length)[0]
  return match
    ? { counterpartyId: match.id, counterpartyName: "" }
    : { counterpartyId: "", counterpartyName: transaction.payeeName?.trim() || transaction.category || "" }
}

export const transactionDebtDirection = (kind: LedgerTransaction["kind"]): "lend_out" | "borrow_in" => kind === "expense" ? "lend_out" : "borrow_in"
