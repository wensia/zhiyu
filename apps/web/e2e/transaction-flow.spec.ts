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

// mobile 仿真下 position:fixed 底栏与 visual viewport 命中测试不稳定，保存按钮在移动项目用 force 点击
async function saveDialog(page: Page, dialog: ReturnType<Page["getByRole"]>, mobile: boolean) {
  await dialog.getByRole("button", { name: "保存" }).click(mobile ? { force: true } : undefined)
}

async function recordTransaction(page: Page, amount: string, category: string, mobile: boolean, kind: "支出" | "收入" = "支出") {
  await page.getByRole("button", { name: "记一笔" }).click()
  const dialog = page.getByRole("dialog", { name: "记一笔" })
  await expect(dialog).toBeVisible()
  if (kind === "收入") await dialog.getByRole("button", { name: "收入" }).click()
  await dialog.getByLabel("金额（元）").fill(amount)
  await dialog.getByLabel("分类").click()
  await dialog.getByLabel("分类").fill(category)
  await page.getByRole("option", { name: `新建"${category}"` }).click()
  await saveDialog(page, dialog, mobile)
  await expect(dialog).toBeHidden()
}

test("current transaction page: add, edit, delete and filter", async ({ page }, testInfo) => {
  const mobile = testInfo.project.name.includes("mobile")
  await registerVerifyLogin(page)
  await page.goto("/app/transactions")
  await expect(page.locator(".topbar")).toContainText("流水")
  await expect(page.locator(".tx-data-dock")).toContainText("当月暂无符合条件的账目")
  await expect(page.getByRole("button", { name: "导入账单" })).toBeVisible()

  await recordTransaction(page, "12.34", "餐饮", mobile)
  await expect(page.locator(".toast-viewport")).toContainText("已记一笔")
  const dock = page.locator(".tx-data-dock")
  await expect(dock).toContainText("餐饮")
  await expect(dock).toContainText("-¥12.34")

  await recordTransaction(page, "100", "工资", mobile, "收入")
  await expect(dock).toContainText("+¥100.00")

  await dock.getByRole("button", { name: "操作 餐饮 ¥12.34" }).click()
  await page.getByRole("menuitem", { name: "编辑" }).click()
  const editDialog = page.getByRole("dialog", { name: "编辑记账" })
  await expect(editDialog.getByLabel("金额（元）")).toHaveValue("12.34")
  await editDialog.getByLabel("金额（元）").fill("20")
  await saveDialog(page, editDialog, mobile)
  await expect(editDialog).toBeHidden()
  await expect(dock).toContainText("-¥20.00")

  await dock.getByRole("button", { name: "操作 餐饮 ¥20.00" }).click()
  await page.getByRole("menuitem", { name: "删除" }).click()
  await page.getByRole("button", { name: "确认删除" }).click()
  await expect(page.locator(".toast-viewport")).toContainText("记账已删除")
  await expect(dock).not.toContainText("餐饮")
  await expect(page.getByText("共 1 笔")).toBeVisible()
  await expect(dock).toContainText("工资")
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
  await page.getByRole("button", { name: "记一笔" }).click()
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
  const dock = page.locator(".tx-data-dock")
  await dock.getByRole("button", { name: "操作 红包 ¥50.00" }).click()
  await page.getByRole("menuitem", { name: "删除" }).click()
  await page.getByRole("button", { name: "确认删除" }).click()
  await expect(page.locator(".toast-viewport")).toContainText("记账已删除")

  await page.goto("/app/accounts")
  await expect(page.locator(".account-data-dock")).toContainText("¥100.00")
  await expect(page.locator(".account-data-dock")).not.toContainText("¥150.00")
})
