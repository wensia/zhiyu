import { expect, test, type Page } from "@playwright/test"
import { readdir, readFile } from "node:fs/promises"
import { resolve } from "node:path"

const password = "YOUR_TEST_PASSWORD_HERE"

async function verificationLink(email: string) {
  const directory = resolve(process.cwd(), "../../var/e2e-mail")
  await expect.poll(async () => {
    const files = await readdir(directory).catch(() => [])
    for (const file of files) {
      const content = await readFile(resolve(directory, file), "utf8")
      if (content.includes(`To: ${email}`)) return content.match(/https?:\/\/\S+token=\S+/)?.[0]
    }
    return undefined
  }).toBeTruthy()
  const files = await readdir(directory)
  for (const file of files) {
    const content = await readFile(resolve(directory, file), "utf8")
    if (content.includes(`To: ${email}`)) return content.match(/https?:\/\/\S+token=\S+/)![0]
  }
  throw new Error("verification email not found")
}

async function registerVerifyLogin(page: Page) {
  const email = `transaction-${Date.now()}-${Math.random().toString(16).slice(2)}@example.com`
  await page.goto("/register")
  await page.getByLabel("邮箱").fill(email)
  await page.getByLabel("密码", { exact: true }).fill(password)
  await page.getByRole("button", { name: "创建账户" }).click()
  await expect(page.getByText("验证邮件已生成")).toBeVisible()
  await page.goto(await verificationLink(email))
  await page.getByRole("link", { name: "前往登录" }).click()
  await page.getByLabel("邮箱").fill(email)
  await page.getByLabel("密码", { exact: true }).fill(password)
  await page.getByRole("button", { name: "登录" }).click()
  await expect(page).toHaveURL(/\/app\/debts/)
}

const todayString = () => {
  const now = new Date()
  const month = String(now.getMonth() + 1).padStart(2, "0")
  const day = String(now.getDate()).padStart(2, "0")
  return `${now.getFullYear()}-${month}-${day}`
}

// mobile 仿真下 position:fixed 底栏与 visual viewport 命中测试不稳定，保存按钮在移动项目用 force 点击
async function saveDialog(page: Page, dialog: ReturnType<Page["getByRole"]>, mobile: boolean) {
  await dialog.getByRole("button", { name: "保存" }).click(mobile ? { force: true } : undefined)
}

async function recordExpenseFromCalendar(page: Page, amount: string, category: string, mobile: boolean) {
  const today = todayString()
  const cell = page.getByRole("button", { name: new RegExp(`^${today}，`) })
  // 今天默认处于选中态，点一次即弹快速记账；否则第一次点击只是选中
  await cell.click()
  if (!(await page.getByRole("dialog", { name: "记一笔" }).isVisible().catch(() => false))) await cell.click()
  const dialog = page.getByRole("dialog", { name: "记一笔" })
  await expect(dialog).toBeVisible()
  await expect(dialog.getByText(today)).toBeVisible()
  await dialog.getByLabel("金额（元）").fill(amount)
  await dialog.getByLabel("分类").click()
  await dialog.getByLabel("分类").fill(category)
  await page.getByRole("option", { name: `新建"${category}"` }).click()
  await saveDialog(page, dialog, mobile)
  await expect(dialog).toBeHidden()
}

test("calendar bookkeeping flow: add, edit, delete with linked stats", async ({ page }, testInfo) => {
  const mobile = testInfo.project.name.includes("mobile")
  await registerVerifyLogin(page)
  await page.goto("/app/transactions")
  await expect(page.getByRole("heading", { name: "记账" })).toBeVisible()
  await expect(page.getByText("本月暂无收支数据")).toBeVisible()

  await recordExpenseFromCalendar(page, "12.34", "餐饮", mobile)
  await expect(page.locator(".toast-viewport")).toContainText("已记一笔")
  // 日历格子、metrics、当日明细联动
  await expect(page.locator(".tx-day-selected")).toContainText("¥12")
  const metrics = page.locator(".metrics")
  await expect(metrics).toContainText("¥12.34")
  const detail = page.getByLabel("当日明细")
  await expect(detail).toContainText("餐饮")
  await expect(detail).toContainText("-¥12.34")

  // 再记一笔收入，结余与趋势图联动
  await page.locator(".page-header").getByRole("button", { name: "记一笔" }).click()
  const incomeDialog = page.getByRole("dialog", { name: "记一笔" })
  await incomeDialog.getByRole("button", { name: "收入" }).click()
  await incomeDialog.getByLabel("金额（元）").fill("100")
  await incomeDialog.getByLabel("分类").click()
  await incomeDialog.getByLabel("分类").fill("工资")
  await page.getByRole("option", { name: '新建"工资"' }).click()
  await saveDialog(page, incomeDialog, mobile)
  await expect(incomeDialog).toBeHidden()
  await expect(metrics).toContainText("+¥87.66")
  await expect(page.locator(".tx-trend-svg rect").first()).toBeVisible()
  await expect(page.getByLabel("分类占比")).toContainText("餐饮")

  // 编辑：金额 12.34 → 20，数字联动更新
  await detail.getByRole("button", { name: "操作 餐饮 ¥12.34" }).click()
  await page.getByRole("menuitem", { name: "编辑" }).click()
  const editDialog = page.getByRole("dialog", { name: "编辑记账" })
  await expect(editDialog.getByLabel("金额（元）")).toHaveValue("12.34")
  await editDialog.getByLabel("金额（元）").fill("20")
  await saveDialog(page, editDialog, mobile)
  await expect(editDialog).toBeHidden()
  await expect(metrics).toContainText("+¥80.00")
  await expect(detail).toContainText("-¥20.00")

  // 删除：格子数字与统计回落
  await detail.getByRole("button", { name: "操作 餐饮 ¥20.00" }).click()
  await page.getByRole("menuitem", { name: "删除" }).click()
  await page.getByRole("button", { name: "确认删除" }).click()
  await expect(page.locator(".toast-viewport")).toContainText("记账已删除")
  await expect(metrics).toContainText("+¥100.00")
  await expect(detail).not.toContainText("餐饮")

  // 列表 Tab 展示同一批数据
  await page.getByRole("tab", { name: "列表" }).click()
  await expect(page.getByText("共 1 笔")).toBeVisible()
  await expect(page.locator(".tx-data-dock")).toContainText("工资")
})

test("account balance tracks transactions and rolls back on delete", async ({ page }, testInfo) => {
  const mobile = testInfo.project.name.includes("mobile")
  await registerVerifyLogin(page)
  await page.goto("/app/accounts")
  await page.getByRole("button", { name: "新增账户" }).click()
  const createDialog = page.getByRole("dialog", { name: "新增账户" })
  await createDialog.getByRole("combobox", { name: "账户类型" }).click()
  await page.getByRole("option", { name: "现金", exact: true }).click()
  await createDialog.getByLabel("账户名称").fill("随身现金")
  await createDialog.getByLabel("初始余额（可选，元）").fill("100")
  await createDialog.getByRole("button", { name: "保存" }).click()
  await expect(createDialog).toBeHidden()
  await expect(page.locator(".account-data-dock")).toContainText("¥100.00")

  await page.goto("/app/transactions")
  await page.locator(".page-header").getByRole("button", { name: "记一笔" }).click()
  const dialog = page.getByRole("dialog", { name: "记一笔" })
  await dialog.getByRole("button", { name: "收入" }).click()
  await dialog.getByLabel("金额（元）").fill("50")
  await dialog.getByLabel("分类").click()
  await dialog.getByLabel("分类").fill("红包")
  await page.getByRole("option", { name: '新建"红包"' }).click()
  await dialog.getByRole("combobox", { name: "账户" }).click()
  await page.getByRole("option", { name: /随身现金/ }).click()
  await saveDialog(page, dialog, mobile)
  await expect(dialog).toBeHidden()

  await page.goto("/app/accounts")
  await expect(page.locator(".account-data-dock")).toContainText("¥150.00")

  await page.goto("/app/transactions")
  const detail = page.getByLabel("当日明细")
  await detail.getByRole("button", { name: "操作 红包 ¥50.00" }).click()
  await page.getByRole("menuitem", { name: "删除" }).click()
  await page.getByRole("button", { name: "确认删除" }).click()
  await expect(page.locator(".toast-viewport")).toContainText("记账已删除")

  await page.goto("/app/accounts")
  await expect(page.locator(".account-data-dock")).toContainText("¥100.00")
  await expect(page.locator(".account-data-dock")).not.toContainText("¥150.00")
})
