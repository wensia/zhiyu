import { expect, test, type Page } from "@playwright/test"
import { readdir, readFile } from "node:fs/promises"
import { resolve } from "node:path"

const password = "YOUR_TEST_PASSWORD_HERE"
const fixture = resolve(process.cwd(), "../api/tests/fixtures/alipay_synthetic_gb18030.csv")
const wechatFixture = resolve(process.cwd(), "../api/tests/fixtures/wechat.xlsx")

async function verificationLink(email: string) {
  const directory = resolve(process.cwd(), "../../var/e2e-mail")
  await expect.poll(async () => {
    for (const file of await readdir(directory).catch(() => [])) {
      const content = await readFile(resolve(directory, file), "utf8")
      if (content.includes(`To: ${email}`)) return content.match(/https?:\/\/\S+token=\S+/)?.[0]
    }
  }).toBeTruthy()
  for (const file of await readdir(directory)) {
    const content = await readFile(resolve(directory, file), "utf8")
    if (content.includes(`To: ${email}`)) return content.match(/https?:\/\/\S+token=\S+/)![0]
  }
  throw new Error("verification email not found")
}

async function loginFresh(page: Page) {
  const email = `import-${Date.now()}-${Math.random().toString(16).slice(2)}@example.com`
  await page.goto("/register")
  await page.getByLabel("邮箱").fill(email)
  await page.getByLabel("密码", { exact: true }).fill(password)
  await page.getByRole("button", { name: "创建账户" }).click()
  await expect(page.getByText("验证邮件已生成")).toBeVisible()
  await page.goto(await verificationLink(email))
  await page.getByRole("link", { name: "前往登录" }).click()
  await expect(page).toHaveURL(/\/login$/)
  await page.getByLabel("邮箱").fill(email)
  await page.getByLabel("密码", { exact: true }).fill(password)
  await page.getByRole("button", { name: "登录" }).click()
  await expect(page).toHaveURL(/\/app\//)
}

async function upload(page: Page) {
  await page.goto("/app/transactions/imports")
  await page.getByRole("button", { name: "选择文件" }).click()
  await page.getByLabel("账单文件", { exact: true }).setInputFiles(fixture)
  await page.getByRole("button", { name: "上传并预览" }).click()
  await expect(page).toHaveURL(/\/app\/transactions\/imports\/[0-9a-z-]+/)
  await expect(page.getByRole("heading", { name: "alipay_synthetic_gb18030.csv" })).toBeVisible()
}

async function bindUnmappedPaymentMethods(page: Page, accountLabel: string) {
  const mappings = page.getByRole("region", { name: "支付方式映射" })
  await expect(mappings).toBeVisible()
  const initialUnbound = await mappings.getByText("未绑定", { exact: true }).count()
  for (let index = 0; index < initialUnbound; index += 1) {
    const select = mappings.getByRole("combobox").filter({ hasText: "请选择账户" }).first()
    await expect(select).toBeEnabled()
    await select.click()
    await page.getByRole("option", { name: accountLabel, exact: true }).click()
    await expect(page.locator(".toast-viewport")).toContainText("支付方式映射已保存")
  }
  await expect(mappings.getByText("已全部绑定", { exact: true })).toBeVisible()
}

test("bill import happy path includes zero amount, commit, duplicate and edited undo", async ({ page }) => {
  await loginFresh(page)
  const matchedResponse = await page.request.post("/api/v1/ledger-accounts", {
    headers: { "Idempotency-Key": `e2e-import-account-${Date.now()}`, Origin: new URL(page.url()).origin },
    data: { accountType: "alipay_balance", name: "身份命中账户", email: "fake@example.test", note: "", openingBalanceCents: 0 },
  })
  expect(matchedResponse.ok()).toBeTruthy()
  const matchedAccount = await matchedResponse.json()
  const alternateResponse = await page.request.post("/api/v1/ledger-accounts", {
    headers: { "Idempotency-Key": `e2e-import-alternate-${Date.now()}`, Origin: new URL(page.url()).origin },
    data: { accountType: "alipay_balance", name: "导入测试账户", email: "other@example.test", note: "", openingBalanceCents: 0 },
  })
  expect(alternateResponse.ok()).toBeTruthy()
  const account = await alternateResponse.json()
  await page.goto("/app/transactions")
  await page.getByRole("button", { name: "导入账单" }).click()
  await expect(page).toHaveURL(/\/app\/transactions\/imports$/)

  await upload(page)
  const attribution = page.getByRole("region", { name: "账户" })
  await expect(attribution).toBeVisible()
  await expect(attribution.getByRole("combobox", { name: "本批账单绑定账户" })).toContainText("身份命中账户")
  expect(matchedAccount.email).toBe("fake@example.test")
  await attribution.getByRole("combobox", { name: "本批账单绑定账户" }).click()
  await page.getByRole("option", { name: "支付宝余额 · 导入测试账户", exact: true }).click()
  await expect(page.getByText("成功但实付 0 元，保留在导入记录中，不写入正式收支账本。")).toBeVisible()
  const pendingCard = page.getByRole("button", { name: "按未完成筛选汇总记录" })
  await pendingCard.click()
  await expect(pendingCard).toHaveAttribute("aria-pressed", "true")
  await expect(page.getByRole("tab", { name: "未完成" })).toHaveAttribute("data-state", "active")
  const rows = page.locator(".desktop-table .import-record-table tbody tr")
  await expect.poll(async () => { const texts = await rows.allTextContents(); return texts.length > 0 && texts.every((text) => text.includes("未完成")) }).toBe(true)

  const expenseCard = page.getByRole("button", { name: "按导入支出筛选汇总记录" })
  await expenseCard.click()
  await expect(expenseCard).toHaveAttribute("aria-pressed", "true")
  await expect(page.getByRole("tab", { name: "待入账" })).toHaveAttribute("data-state", "active")
  const expenseRows = page.locator(".desktop-table .import-record-table tbody tr")
  await expect.poll(async () => { const texts = await expenseRows.allTextContents(); return texts.length > 0 && texts.every((text) => text.includes("待入账") && text.includes("−")) }).toBe(true)
  await expenseCard.click()
  await expect(expenseCard).toHaveAttribute("aria-pressed", "false")
  await expect(page.getByRole("tab", { name: "全部" })).toHaveAttribute("data-state", "active")
  await expect.poll(async () => { const texts = await rows.allTextContents(); return texts.some((text) => text.includes("未完成")) && texts.some((text) => text.includes("待入账")) }).toBe(true)

  await bindUnmappedPaymentMethods(page, "支付宝余额 · 导入测试账户")
  await page.getByRole("button", { name: "确认入账" }).click()
  const commitDialog = page.getByRole("alertdialog", { name: "确认将账单写入账本？" })
  await expect(commitDialog).toBeVisible()
  await commitDialog.getByRole("button", { name: "确认入账", exact: true }).click()
  await expect(page.locator(".toast-viewport")).toContainText("账单已确认入账")
  await expect(page.getByText("已入账", { exact: true })).toBeVisible()

  const committedUrl = page.url()
  // fixture 属于历史月份；把一条已导入且已归因的流水移到本月，再用真实列表筛选验证。
  const transactionList = await page.request.get("/api/v1/transactions?month=2026-01&page=1&pageSize=20")
  expect(transactionList.ok()).toBeTruthy()
  const imported = (await transactionList.json()).items.find((item: { account?: { id: string }; amountCents: number }) => item.account?.id === account.id && item.amountCents === 1234)
  expect(imported).toBeTruthy()
  const importedPayeeName = "虚构商户甲"
  const editedNote = "虚构 E2E 用户编辑"
  const today = new Date().toISOString().slice(0, 10)
  const edited = await page.request.patch(`/api/v1/transactions/${imported.id}`, {
    headers: { "Idempotency-Key": `e2e-edit-${Date.now()}`, Origin: new URL(page.url()).origin },
    data: { kind: imported.kind, amountCents: imported.amountCents, occurredOn: today, category: imported.category, accountId: account.id, note: editedNote, version: imported.version },
  })
  expect(edited.ok()).toBeTruthy()
  await page.goto("/app/transactions")
  await page.getByRole("combobox", { name: "账户" }).click()
  await page.getByRole("option", { name: "支付宝余额 · 导入测试账户", exact: true }).click()
  await expect(page.locator(".tx-data-dock")).toContainText(imported.category)
  await expect(page.locator(".tx-data-dock")).toContainText(importedPayeeName)
  await expect(page.locator(".tx-data-dock")).toContainText(editedNote)

  const transactionDock = page.locator(".tx-data-dock")
  await transactionDock.getByRole("button", { name: new RegExp(`操作 .* ¥${(imported.amountCents / 100).toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`) }).click()
  await page.getByRole("menuitem", { name: "创建债务" }).click()
  const debtDialog = page.getByRole("dialog", { name: "新增债务" })
  await expect(debtDialog).toBeVisible()
  await expect(debtDialog.getByRole("button", { name: "借出（别人欠我）" })).toBeDisabled()
  await expect(debtDialog.getByLabel("本金（元）")).toHaveValue(String(imported.amountCents / 100))
  await expect(debtDialog.getByLabel("本金（元）")).toBeDisabled()
  await expect(debtDialog.getByLabel("发生日期")).toContainText(today)
  await expect(debtDialog.getByLabel("发生日期")).toBeDisabled()
  await expect(debtDialog.getByRole("combobox", { name: "付款账户" })).toBeDisabled()
  await expect(debtDialog.getByLabel("联系人")).toHaveValue(importedPayeeName)
  await debtDialog.getByRole("button", { name: "保存" }).click()
  await expect(debtDialog).toBeHidden()
  await expect(page.locator(".toast-viewport")).toContainText("债务已新增")
  await expect(transactionDock).toContainText("债务往来")

  const debtsResponse = await page.request.get(`/api/v1/debts?page=1&pageSize=20&search=${encodeURIComponent(importedPayeeName)}`)
  expect(debtsResponse.ok()).toBeTruthy()
  const createdDebt = (await debtsResponse.json()).items[0]
  expect(createdDebt).toBeTruthy()
  await page.goto(`/app/debts/${createdDebt.id}`)
  await expect(page.getByRole("heading", { name: importedPayeeName })).toBeVisible()
  const initialRow = page.locator(".timeline-row").filter({ hasText: `¥${(imported.amountCents / 100).toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}` })
  await expect(initialRow.getByText("已关联流水", { exact: false })).toBeVisible()

  // 迁移 0024 把债务的现金往来并进了 ledger_transactions：现金变动**始终**对应一笔流水，
  // 「取消关联」这个状态不再存在。能做的是两件——退回系统自动建的那笔，或改绑到另一笔
  // 已入账流水。顺序不能反：候选池排除了已被债务占用的流水，所以要先退回自动流水，把
  // 导入的那笔释放回池子，才谈得上再把它挑回来。
  await initialRow.getByRole("button", { name: /^操作 / }).click()
  await page.getByRole("menuitem", { name: "管理流水" }).click()
  let linkDialog = page.getByRole("dialog", { name: "管理流水" })
  await linkDialog.getByRole("button", { name: "使用自动流水" }).click()
  await linkDialog.getByRole("button", { name: "保存" }).click()
  await expect(page.locator(".toast-viewport")).toContainText("已使用自动流水")
  await expect(initialRow.getByText("已关联流水", { exact: false })).toBeVisible()

  await initialRow.getByRole("button", { name: /^操作 / }).click()
  await page.getByRole("menuitem", { name: "管理流水" }).click()
  linkDialog = page.getByRole("dialog", { name: "管理流水" })
  await linkDialog.getByRole("button", { name: "更换流水" }).click()
  const candidateDialog = page.getByRole("dialog", { name: "选择已入账流水" })
  await candidateDialog.getByRole("button").filter({ hasText: editedNote }).click()
  await linkDialog.getByRole("button", { name: "保存" }).click()
  await expect(page.locator(".toast-viewport")).toContainText("流水已关联")
  await expect(initialRow.getByText("已关联流水", { exact: false })).toBeVisible()

  // 另一个渠道走完整入账，验证批次账户由入账结果推断出来。
  await page.goto("/app/transactions/imports")
  await page.getByRole("button", { name: "选择文件" }).click()
  await page.getByLabel("账单文件", { exact: true }).setInputFiles(wechatFixture)
  await page.getByRole("button", { name: "上传并预览" }).click()
  await expect(page.getByRole("combobox", { name: "本批账单绑定账户" })).toContainText("不绑定")
  await bindUnmappedPaymentMethods(page, "支付宝余额 · 导入测试账户")
  await page.getByRole("button", { name: "确认入账" }).click()
  await page.getByRole("alertdialog").getByRole("button", { name: "确认入账" }).click()
  // 批次账户不再需要手工补绑：入账后由结果推断——本批交易都落在同一个账户，详情页
  // 直接显示它。顶栏那颗「绑定账户」只留给账户不唯一、或整批都没有账户的批次。
  await expect(page.getByRole("region", { name: "账户" })).toContainText("导入测试账户")
  await expect(page.getByRole("button", { name: "绑定账户" })).toHaveCount(0)

  await upload(page)
  await page.getByRole("tab", { name: "重复交易" }).click()
  await expect(page.locator(".import-disposition-copy").getByText("同用户、同渠道、同交易单号已存在；已归档交易也视为存在。", { exact: true })).toBeVisible()
  await expect(page.locator(".import-record-dock")).toContainText("重复交易")

  await page.goto(committedUrl)
  await page.getByRole("button", { name: "撤销导入" }).click()
  await expect(page.getByRole("alertdialog", { name: "撤销这个导入批次？" })).toBeVisible()
  await page.getByRole("button", { name: "确认撤销" }).click()
  await expect(page.locator(".toast-viewport")).toContainText(/仍保留 1 条用户已编辑或归档的交易/)
})
