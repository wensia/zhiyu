import type { TransactionLinkCandidate } from "../api/types"

export function transactionSelectionValues(transaction: TransactionLinkCandidate) {
  return {
    transactionId: transaction.id,
    amount: String(transaction.amountCents / 100),
    occurredOn: transaction.occurredOn,
    accountId: transaction.account?.id || "",
  }
}
