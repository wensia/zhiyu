import { describe, expect, it } from "vitest"

import { BANK_NAME_OPTIONS, LEDGER_ACCOUNT_TYPE_LABELS, OTHER_BANK_VALUE, ledgerAccountDetailItems, ledgerAccountDisplayLabel, ledgerAccountSearchText } from "./ledger-account"

describe("ledger account labels", () => {
  it("keeps every API account type mapped to a stable Chinese label", () => {
    expect(LEDGER_ACCOUNT_TYPE_LABELS).toEqual({
      wechat_balance: "微信零钱",
      alipay_balance: "支付宝余额",
      bank_card: "银行卡",
      cash: "现金",
      digital_cny: "数字人民币",
      other: "其他账户",
    })
  })

  it("does not repeat the type when the account name is already the type label", () => {
    expect(ledgerAccountDisplayLabel({ accountType: "wechat_balance", name: "微信零钱" })).toBe("微信零钱")
    expect(ledgerAccountDisplayLabel({ accountType: "wechat_balance", name: "日常零钱" })).toBe("微信零钱 · 日常零钱")
    expect(ledgerAccountDisplayLabel(null)).toBe("历史未指定")
  })

  it("makes the Chinese type and its code searchable", () => {
    const text = ledgerAccountSearchText({ accountType: "alipay_balance", name: "生活号", note: "买菜", nickname: "小余", phone: "13800138000", email: "YU@example.com" })
    expect(text).toContain("支付宝余额")
    expect(text).toContain("alipay_balance")
    expect(text).toContain("小余")
    expect(text).toContain("13800138000")
    expect(text).toContain("YU@example.com")
  })

  it("keeps the bank dropdown compact and renders only details that belong to the account type", () => {
    expect(BANK_NAME_OPTIONS).toHaveLength(9)
    expect(BANK_NAME_OPTIONS.at(-1)).toEqual({ value: OTHER_BANK_VALUE, label: "其他银行" })
    expect(ledgerAccountDetailItems({
      accountType: "bank_card",
      bankName: "招商银行",
      branchName: "北京中关村支行",
      cardNumber: "6222000000001234",
      nickname: "不应显示",
    })).toEqual([
      { label: "银行卡号", value: "6222000000001234" },
      { label: "银行", value: "招商银行" },
      { label: "开户行", value: "北京中关村支行" },
    ])
  })

  it("omits only the detail used as a derived resolved name", () => {
    expect(ledgerAccountDetailItems({
      accountType: "bank_card",
      name: "6222000000001234",
      nameSource: "derived",
      cardNumber: "6222000000001234",
      bankName: "招商银行",
      branchName: "北京中关村支行",
    })).toEqual([
      { label: "银行", value: "招商银行" },
      { label: "开户行", value: "北京中关村支行" },
    ])

    expect(ledgerAccountDetailItems({
      accountType: "alipay_balance",
      name: "小余",
      nameSource: "derived",
      nickname: "小余",
      phone: "13800138000",
      email: "yu@example.com",
    })).toEqual([
      { label: "手机号", value: "13800138000" },
      { label: "邮箱", value: "yu@example.com" },
    ])

    expect(ledgerAccountDetailItems({
      accountType: "alipay_balance",
      name: "小余",
      nameSource: "custom",
      nickname: "小余",
    })).toEqual([{ label: "昵称", value: "小余" }])

    expect(ledgerAccountSearchText({
      accountType: "alipay_balance",
      name: "小余",
      nameSource: "derived",
      note: "",
      nickname: "小余",
    })).toContain("昵称 小余")

    expect(ledgerAccountSearchText({
      accountType: "bank_card",
      name: "6222000000001234",
      nameSource: "derived",
      note: "",
      cardNumber: "6222000000001234",
    })).toContain("银行卡号 6222000000001234")
  })
})

describe("ledgerAccountDisplayLabel 银行卡增强", () => {
  it("银行卡显示银行名与尾号", () => {
    expect(ledgerAccountDisplayLabel({ accountType: "bank_card", name: "工资卡", bankName: "招商银行", cardNumber: "6222000000001234" })).toBe("招商银行 ····1234 · 工资卡")
  })
  it("name 就是卡号时不重复展示", () => {
    expect(ledgerAccountDisplayLabel({ accountType: "bank_card", name: "6222000000001234", bankName: "招商银行", cardNumber: "6222000000001234" })).toBe("招商银行 ····1234")
  })
  it("无银行名无卡号退回类型标签", () => {
    expect(ledgerAccountDisplayLabel({ accountType: "bank_card", name: "备用卡" })).toBe("银行卡 · 备用卡")
  })
  it("哨兵银行名不展示", () => {
    expect(ledgerAccountDisplayLabel({ accountType: "bank_card", name: "特殊", bankName: "__other_bank__", cardNumber: "1234567890123456" })).toBe("银行卡 ····3456 · 特殊")
  })
  it("非银行卡类型不受影响", () => {
    expect(ledgerAccountDisplayLabel({ accountType: "wechat_balance", name: "航子" })).toBe("微信零钱 · 航子")
  })
})
