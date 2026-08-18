import { expect, test, type Locator, type Page } from "@playwright/test"
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
  const email = `account-${Date.now()}-${Math.random().toString(16).slice(2)}@example.com`
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
  await page.goto("/app/accounts")
  // 空状态标题「暂无符合条件的账户」也匹配「账户」，不加 exact 会撞上 strict mode。
  await expect(page.getByRole("heading", { name: "账户", exact: true })).toBeVisible()
}

async function chooseAccountType(page: Page, dialog: Locator, label: string) {
  await dialog.getByRole("combobox", { name: "账户类型" }).click()
  await page.getByRole("option", { name: label, exact: true }).click()
}

test("modal overlay covers the sticky account operation header", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "Desktop table is hidden on mobile")
  await registerVerifyLogin(page)
  await page.getByRole("button", { name: "新增账户" }).click()
  await expect(page.getByRole("dialog", { name: "新增账户" })).toBeVisible()

  const coveredByOverlay = await page.locator('.fund-account-table th[data-sticky-cell="right"]').evaluate((header) => {
    const frame = header.getBoundingClientRect()
    const hit = document.elementFromPoint(frame.left + frame.width / 2, frame.top + frame.height / 2)
    return hit?.classList.contains("overlay") ?? false
  })
  expect(coveredByOverlay).toBe(true)
})

test("structured account details create, display, search and edit", async ({ page }, testInfo) => {
  await registerVerifyLogin(page)
  await page.getByRole("button", { name: "新增账户" }).click()
  const createDialog = page.getByRole("dialog", { name: "新增账户" })
  await chooseAccountType(page, createDialog, "银行卡")
  await expect(createDialog.getByLabel("自定义名称（可选）")).toHaveValue("")
  await createDialog.getByRole("combobox", { name: "银行（可选）" }).click()
  await expect(page.getByRole("option")).toHaveCount(9)
  await page.getByRole("option", { name: "其他银行", exact: true }).click()
  await createDialog.getByLabel("银行名称（可选）").fill("浦发银行")
  await createDialog.getByLabel("开户行（可选）").fill("上海陆家嘴支行")
  await createDialog.getByLabel("银行卡号（可选）").fill("6222 0000 0000 1234")
  await createDialog.getByRole("button", { name: "保存" }).click()
  await expect(createDialog).toBeHidden()

  const list = testInfo.project.name.includes("mobile")
    ? page.locator(".fund-account-mobile-list")
    : page.locator(".fund-account-table")
  await expect(list.getByText("6222000000001234", { exact: true })).toBeVisible()
  await expect(list.getByText("浦发银行", { exact: true })).toBeVisible()
  await expect(list.getByText("上海陆家嘴支行", { exact: true })).toBeVisible()
  await page.getByPlaceholder("搜索账户类型、账户、账户信息或备注").fill("陆家嘴支行")
  await expect(list.getByText("浦发银行", { exact: true })).toBeVisible()
  await page.getByPlaceholder("搜索账户类型、账户、账户信息或备注").fill("")

  await list.getByRole("button", { name: "操作 浦发银行 ····1234" }).click()
  await page.getByRole("menuitem", { name: "编辑账户" }).click()
  const editDialog = page.getByRole("dialog", { name: "编辑账户" })
  await expect(editDialog.getByRole("combobox", { name: "银行（可选）" })).toHaveText("其他银行")
  await expect(editDialog.getByLabel("银行名称（可选）")).toHaveValue("浦发银行")
  await expect(editDialog.getByLabel("开户行（可选）")).toHaveValue("上海陆家嘴支行")
  await expect(editDialog.getByLabel("银行卡号（可选）")).toHaveValue("6222000000001234")
  await expect(editDialog.getByLabel("自定义名称（可选）")).toHaveValue("")
  await chooseAccountType(page, editDialog, "支付宝余额")
  await expect(editDialog.getByLabel("昵称（可选）")).toHaveValue("")
  await expect(editDialog.getByLabel("手机号（可选）")).toHaveValue("")
  await expect(editDialog.getByLabel("邮箱（可选）")).toHaveValue("")
  await expect(editDialog.getByLabel("自定义名称（可选）")).toHaveValue("")
  await expect(editDialog.getByRole("combobox", { name: "银行（可选）" })).toHaveCount(0)
  await editDialog.getByLabel("昵称（可选）").fill("支付宝小余")
  await editDialog.getByLabel("手机号（可选）").fill("+86 138-0013-8000")
  await editDialog.getByLabel("邮箱（可选）").fill("USER@Example.com")
  await editDialog.getByRole("button", { name: "保存" }).click()
  await expect(editDialog).toBeHidden()

  await page.reload()
  await expect(list.getByText("支付宝小余", { exact: true })).toBeVisible()
  await expect(list.getByText("+86 138-0013-8000", { exact: true })).toBeVisible()
  await expect(list.getByText("user@example.com", { exact: true })).toBeVisible()
  await expect(list.getByText("浦发银行", { exact: true })).toHaveCount(0)
  await expect(list.getByText("上海陆家嘴支行", { exact: true })).toHaveCount(0)

  await page.getByPlaceholder("搜索账户类型、账户、账户信息或备注").fill("user@example.com")
  await expect(list.getByText("支付宝小余", { exact: true })).toBeVisible()
  await page.getByPlaceholder("搜索账户类型、账户、账户信息或备注").fill("")
  await list.getByRole("button", { name: "操作 支付宝余额 · 支付宝小余" }).click()
  await page.getByRole("menuitem", { name: "编辑账户" }).click()
  const persistedDialog = page.getByRole("dialog", { name: "编辑账户" })
  await expect(persistedDialog.getByLabel("昵称（可选）")).toHaveValue("支付宝小余")
  await expect(persistedDialog.getByLabel("手机号（可选）")).toHaveValue("+86 138-0013-8000")
  await expect(persistedDialog.getByLabel("邮箱（可选）")).toHaveValue("user@example.com")
  await expect(persistedDialog.getByLabel("自定义名称（可选）")).toHaveValue("")
  await persistedDialog.getByRole("button", { name: "取消" }).click()

  await page.getByRole("button", { name: "新增账户" }).click()
  const wechatDialog = page.getByRole("dialog", { name: "新增账户" })
  await chooseAccountType(page, wechatDialog, "微信零钱")
  await wechatDialog.getByLabel("昵称（可选）").fill("微信小余")
  await expect(wechatDialog.getByLabel("自定义名称（可选）")).toHaveValue("")
  await wechatDialog.getByRole("button", { name: "保存" }).click()
  await expect(wechatDialog).toBeHidden()
  await expect(list.getByText("微信小余", { exact: true })).toBeVisible()

  await page.getByRole("button", { name: "新增账户" }).click()
  const cashDialog = page.getByRole("dialog", { name: "新增账户" })
  await chooseAccountType(page, cashDialog, "现金")
  await expect(cashDialog.getByLabel("账户名称")).toHaveValue("")
  await cashDialog.getByRole("button", { name: "保存" }).click()
  await expect(cashDialog.getByRole("alert")).toContainText("请输入账户名称")
})

test("account dialog keeps its header and footer visible in narrow-height viewports", async ({ page }, testInfo) => {
  await registerVerifyLogin(page)
  await page.setViewportSize(testInfo.project.name.includes("mobile") ? { width: 390, height: 568 } : { width: 900, height: 500 })
  await page.getByRole("button", { name: "新增账户" }).click()
  const dialog = page.getByRole("dialog", { name: "新增账户" })
  await chooseAccountType(page, dialog, "支付宝余额")

  const geometry = await dialog.evaluate((element) => {
    const frame = element.getBoundingClientRect()
    const header = element.querySelector(".dialog-header")!.getBoundingClientRect()
    const body = element.querySelector(".dialog-body")!
    const footer = element.querySelector(".dialog-footer")!.getBoundingClientRect()
    return {
      frameTop: frame.top,
      frameBottom: frame.bottom,
      headerTop: header.top,
      footerBottom: footer.bottom,
      bodyClientHeight: body.clientHeight,
      bodyScrollHeight: body.scrollHeight,
    }
  })
  expect(geometry.frameTop).toBeGreaterThanOrEqual(0)
  expect(geometry.frameBottom).toBeLessThanOrEqual(testInfo.project.name.includes("mobile") ? 568 : 500)
  expect(Math.abs(geometry.headerTop - geometry.frameTop)).toBeLessThan(1)
  expect(Math.abs(geometry.footerBottom - geometry.frameBottom)).toBeLessThan(1)
  expect(geometry.bodyScrollHeight).toBeGreaterThan(geometry.bodyClientHeight)

  await dialog.getByRole("combobox", { name: "账户类型" }).click()
  await page.getByRole("option", { name: "银行卡", exact: true }).click()
  await dialog.getByRole("combobox", { name: "银行（可选）" }).click()
  const dropdownGeometry = await page.locator(".select-content").evaluate((element) => {
    const frame = element.getBoundingClientRect()
    const viewport = element.querySelector(".select-viewport")!
    return { top: frame.top, bottom: frame.bottom, clientHeight: viewport.clientHeight, scrollHeight: viewport.scrollHeight }
  })
  expect(dropdownGeometry.top).toBeGreaterThanOrEqual(0)
  expect(dropdownGeometry.bottom).toBeLessThanOrEqual(testInfo.project.name.includes("mobile") ? 568 : 500)
  expect(dropdownGeometry.scrollHeight).toBeGreaterThanOrEqual(dropdownGeometry.clientHeight)
})
