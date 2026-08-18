import { describe, expect, it } from "vitest"

import { transactionSelectionValues } from "./transaction-link-selection"

describe("transactionSelectionValues", () => {
  it("takes immutable money, date, account, and id from the selected transaction", () => {
    expect(transactionSelectionValues({
      id: "tx-1",
      kind: "income",
      amountCents: 100_001,
      occurredOn: "2026-08-07",
      note: "黄英，(__old yellow，)",
      account: { id: "account-1", name: "微信零钱", accountType: "wechat_balance", archived: false },
    })).toEqual({ transactionId: "tx-1", amount: "1000.01", occurredOn: "2026-08-07", accountId: "account-1" })
  })

  it("preserves the valid null-account case", () => {
    expect(transactionSelectionValues({ id: "tx-2", kind: "expense", amountCents: 1, occurredOn: "2026-08-08", note: "", account: null }).accountId).toBe("")
  })
})
