import { expect, test, type Locator, type Page } from "@playwright/test"
import { readdir, readFile } from "node:fs/promises"
import { resolve } from "node:path"

const password = "YOUR_TEST_PASSWORD_HERE"
const ledgerAccountTypeLabels = ["微信零钱", "支付宝余额", "银行卡", "现金", "数字人民币", "其他账户"] as const

async function chooseYear(dialog: Locator, targetYear: number) {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    const yearCell = dialog.getByRole("gridcell", { name: `${targetYear} 年`, exact: true })
    if (await yearCell.count()) {
      await yearCell.click()
      return
    }

    const rangeLabel = await dialog.locator(".date-picker-period-label").innerText()
    const range = /^(\d{4})–(\d{4}) 年$/.exec(rangeLabel)
    if (!range) throw new Error(`无法解析年份区间：${rangeLabel}`)
    const rangeStart = Number(range[1])
    const direction = targetYear < rangeStart ? "上一个十年" : "下一个十年"
    await dialog.getByRole("button", { name: direction, exact: true }).click()
  }

  throw new Error(`无法在年份选择器中定位 ${targetYear} 年`)
}

async function chooseDate(page: Page, trigger: Locator, value: string) {
  const [year, month] = value.split("-").map(Number)
  await trigger.click()
  const dialog = page.getByRole("dialog", { name: "选择日期" })
  await dialog.getByRole("button", { name: /^选择年月，当前/ }).click()
  await dialog.getByRole("button", { name: /^选择年份，当前/ }).click()
  await chooseYear(dialog, year)
  await dialog.getByRole("gridcell", { name: `${month} 月`, exact: true }).click()
  await dialog.getByRole("gridcell", { name: value, exact: true }).click()
}

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

async function registerVerifyLogin(page: Page, email: string) {
  await page.goto("/register")
  await page.getByLabel("邮箱").fill(email)
  await page.getByLabel("密码", { exact: true }).fill(password)
  await page.getByRole("button", { name: "创建账户" }).click()
  await expect(page.getByText("验证邮件已生成")).toBeVisible()
  await page.goto(await verificationLink(email))
  await expect(page.getByRole("heading", { name: "验证成功" })).toBeVisible()
  await page.getByRole("link", { name: "前往登录" }).click()
  await page.getByLabel("邮箱").fill(email)
  await page.getByLabel("密码", { exact: true }).fill(password)
  await page.getByRole("button", { name: "登录" }).click()
  await expect(page).toHaveURL(/\/app\/debts/)
  await expect(page.getByRole("heading", { name: "债务" })).toBeVisible()
}

async function configureLedgerAccount(page: Page, name: string, accountType: (typeof ledgerAccountTypeLabels)[number] = "微信零钱") {
  await page.getByRole("link", { name: /账户/ }).click()
  await expect(page).toHaveURL(/\/app\/accounts$/)
  await expect(page.getByRole("heading", { name: "账户" })).toBeVisible()
  await page.getByRole("button", { name: "新增账户" }).click()
  const dialog = page.getByRole("dialog", { name: "新增账户" })
  await dialog.getByRole("combobox", { name: "账户类型" }).click()
  for (const label of ledgerAccountTypeLabels) {
    await expect(page.getByRole("option", { name: label, exact: true })).toBeVisible()
  }
  await page.getByRole("option", { name: accountType, exact: true }).click()
  await expect(dialog.getByRole("combobox", { name: "账户类型" })).toHaveText(accountType)
  const nameLabel = accountType === "银行卡" || accountType === "微信零钱" || accountType === "支付宝余额" ? "自定义名称（可选）" : "账户名称"
  await dialog.getByLabel(nameLabel).fill(name)
  await dialog.getByRole("button", { name: "保存" }).click()
  await expect(dialog).toBeHidden()
  const accountRow = page.getByRole("row", { name: new RegExp(name) })
  await expect(accountRow.getByText(accountType, { exact: true })).toBeVisible()
  await expect(accountRow.getByText(name, { exact: true })).toBeVisible()
  await page.getByRole("link", { name: /债务/ }).click()
  await expect(page).toHaveURL(/\/app\/debts$/)
}

async function chooseLedgerAccount(
  page: Page,
  dialog: Locator,
  label: "付款账户" | "收款账户",
  name: string,
  accountType: (typeof ledgerAccountTypeLabels)[number] = "微信零钱",
) {
  const displayName = `${accountType} · ${name}`
  const select = dialog.getByRole("combobox", { name: label })
  await select.click()
  const option = page.getByRole("option", { name: displayName, exact: true })
  await expect(option).toBeVisible()
  await option.click()
  await expect(select).toHaveText(displayName)
}

async function chooseMovementAction(page: Page, dialog: Locator, label: "追加借入" | "登记还款") {
  const select = dialog.getByRole("combobox", { name: "动作" })
  await select.click()
  await page.getByRole("option", { name: label, exact: true }).click()
}

test("registration, ledger events, reversal and account isolation", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "full write flow runs once on desktop")
  const suffix = `${Date.now()}-${Math.random().toString(16).slice(2)}`
  await registerVerifyLogin(page, `first-${suffix}@example.com`)
  await configureLedgerAccount(page, "微信支付-测试号")

  await page.getByRole("button", { name: "新增债务" }).click()
  const borrowDialog = page.getByRole("dialog", { name: "新增债务" })
  await borrowDialog.getByLabel("联系人").fill("阿青")
  await borrowDialog.getByLabel("本金（元）").fill("1000")
  await chooseLedgerAccount(page, borrowDialog, "收款账户", "微信支付-测试号")
  await chooseDate(page, borrowDialog.getByLabel("发生日期"), "2026-08-02")
  await chooseDate(page, borrowDialog.getByLabel("到期日（可选）"), "2026-08-09")
  await borrowDialog.getByLabel("备注").fill("E2E 借款")
  await borrowDialog.getByRole("button", { name: "保存" }).click()
  await expect(page.getByRole("button", { name: "阿青", exact: true })).toBeVisible()
  await expect(page.getByRole("columnheader", { name: "债务概况" })).toBeVisible()
  const borrowRow = page.getByRole("row", { name: /阿青 借入/ })
  await expect(borrowRow.getByText("2026-08-02", { exact: true })).toBeVisible()
  await expect(borrowRow.getByText("本金", { exact: true })).toBeVisible()
  await expect(borrowRow.getByText("已还", { exact: true })).toBeVisible()
  await expect(borrowRow.getByText("剩余", { exact: true })).toBeVisible()

  await page.getByRole("button", { name: "新增债务" }).click()
  const lendDialog = page.getByRole("dialog", { name: "新增债务" })
  await lendDialog.getByRole("button", { name: "借出（别人欠我）" }).click()
  await lendDialog.getByLabel("联系人").fill("阿岚")
  await lendDialog.getByLabel("本金（元）").fill("2000")
  await chooseLedgerAccount(page, lendDialog, "付款账户", "微信支付-测试号")
  await chooseDate(page, lendDialog.getByLabel("发生日期"), "2026-08-02")
  await lendDialog.getByRole("button", { name: "保存" }).click()
  await expect(page.getByRole("button", { name: "阿岚", exact: true })).toBeVisible()

  await page.getByRole("button", { name: "阿青", exact: true }).click()
  await expect(page).toHaveURL(/\/app\/debts\/[^/]+$/)
  const detail = page.locator(".debt-detail-page")
  await expect(detail.getByRole("heading", { name: "债务详情" })).toBeAttached()
  const topbar = page.locator(".topbar")
  const backToDebtList = topbar.getByRole("button", { name: "返回债务列表" })
  await expect(backToDebtList).toBeVisible()
  await expect(detail.locator(".detail-page-header")).toHaveCount(0)
  await expect(detail.getByRole("button", { name: "返回债务列表" })).toHaveCount(0)
  await expect(detail.getByText("个人往来", { exact: true })).toHaveCount(0)
  const history = detail.getByRole("region", { name: "债务往来记录" })
  await expect(history.getByText("初始借入 ¥1,000.00", { exact: true })).toBeVisible()
  await expect(history.getByText("1 条", { exact: true })).toBeVisible()
  await detail.getByRole("button", { name: "登记往来" }).click()
  const paymentDialog = page.getByRole("dialog", { name: "登记往来" })
  await chooseMovementAction(page, paymentDialog, "登记还款")
  await paymentDialog.getByLabel("还款金额（元）").fill("400")
  await chooseLedgerAccount(page, paymentDialog, "付款账户", "微信支付-测试号")
  await chooseDate(page, paymentDialog.getByLabel("还款日期"), "2026-08-03")
  await paymentDialog.getByRole("button", { name: "确认登记" }).click()
  await expect(detail.locator(".detail-overview-card").getByText("¥600.00", { exact: true })).toBeVisible()
  await expect(detail.locator(".timeline-row").filter({ hasText: "还款 ¥400.00" }).getByText("微信零钱 · 微信支付-测试号", { exact: true })).toBeVisible()

  const paymentRow = detail.locator(".timeline-row").filter({ hasText: "还款 ¥400.00" })
  await paymentRow.getByRole("button", { name: /^操作 / }).click()
  await page.getByRole("menuitem", { name: "编辑记录" }).click()
  const editPaymentDialog = page.getByRole("dialog", { name: "编辑还款记录" })
  await editPaymentDialog.getByLabel("还款金额（元）").fill("350")
  await editPaymentDialog.getByLabel("备注").fill("修正还款")
  await editPaymentDialog.getByRole("button", { name: "保存修改" }).click()
  await expect(detail.locator(".detail-overview-card").getByText("¥650.00", { exact: true })).toBeVisible()
  await expect(detail.getByText("还款 ¥350.00", { exact: true })).toBeVisible()

  await detail.getByRole("button", { name: "登记往来" }).click()
  const additionDialog = page.getByRole("dialog", { name: "登记往来" })
  await additionDialog.getByLabel("追加借入金额（元）").fill("500")
  await chooseLedgerAccount(page, additionDialog, "收款账户", "微信支付-测试号")
  await chooseDate(page, additionDialog.getByLabel("追加日期"), "2026-08-04")
  await additionDialog.getByLabel("备注").fill("追加周转")
  await additionDialog.getByRole("button", { name: "确认登记" }).click()
  await expect(detail.locator(".detail-overview-card").getByText("¥1,500.00", { exact: true })).toBeVisible()
  await expect(detail.locator(".detail-overview-card").getByText("¥1,150.00", { exact: true })).toBeVisible()
  await expect(detail.getByText("追加借入 ¥500.00", { exact: true })).toBeVisible()
  await expect(history.getByText("初始借入 ¥1,000.00", { exact: true })).toBeVisible()
  await expect(history.getByText("3 条", { exact: true })).toBeVisible()

  const additionRow = detail.locator(".timeline-row").filter({ hasText: "追加借入 ¥500.00" })
  await additionRow.getByRole("button", { name: /^操作 / }).click()
  await page.getByRole("menuitem", { name: "编辑记录" }).click()
  const editAdditionDialog = page.getByRole("dialog", { name: "编辑追加借入" })
  await editAdditionDialog.getByLabel("追加借入金额（元）").fill("450")
  await editAdditionDialog.getByLabel("备注").fill("修正追加")
  await editAdditionDialog.getByRole("button", { name: "保存修改" }).click()
  await expect(detail.locator(".detail-overview-card").getByText("¥1,450.00", { exact: true })).toBeVisible()
  await expect(detail.locator(".detail-overview-card").getByText("¥1,100.00", { exact: true })).toBeVisible()
  await expect(detail.getByText("追加借入 ¥450.00", { exact: true })).toBeVisible()
  await expect(history.getByText("初始借入 ¥1,000.00", { exact: true })).toBeVisible()

  await detail.getByRole("button", { name: "登记往来" }).click()
  const overpayDialog = page.getByRole("dialog", { name: "登记往来" })
  await chooseMovementAction(page, overpayDialog, "登记还款")
  await overpayDialog.getByLabel("还款金额（元）").fill("1200")
  await chooseLedgerAccount(page, overpayDialog, "付款账户", "微信支付-测试号")
  await overpayDialog.getByRole("button", { name: "确认登记" }).click()
  await expect(overpayDialog.getByText("还款金额不能超过剩余金额")).toBeVisible()
  await overpayDialog.getByRole("button", { name: "关闭" }).click()

  await detail.getByRole("button", { name: "登记往来" }).click()
  const settleDialog = page.getByRole("dialog", { name: "登记往来" })
  await chooseMovementAction(page, settleDialog, "登记还款")
  await settleDialog.getByLabel("还款金额（元）").fill("1100")
  await chooseLedgerAccount(page, settleDialog, "付款账户", "微信支付-测试号")
  await chooseDate(page, settleDialog.getByLabel("还款日期"), "2026-08-05")
  await settleDialog.getByRole("button", { name: "确认登记" }).click()
  await expect(detail.locator(".detail-overview-card").getByText("¥0.00", { exact: true })).toBeVisible()
  await expect(detail.getByText("已结清")).toBeVisible()

  const settlementRow = detail.locator(".timeline-row").filter({ hasText: "还款 ¥1,100.00" })
  await settlementRow.getByRole("button", { name: /^操作 / }).click()
  await page.getByRole("menuitem", { name: "撤销还款" }).click()
  await page.getByRole("button", { name: "确认撤销" }).click()
  await expect(detail.locator(".detail-overview-card").getByText("¥1,100.00", { exact: true }).first()).toBeVisible()
  await expect(page.getByText("撤销还款 ¥1,100.00")).toBeVisible()

  await detail.getByRole("button", { name: "更多债务操作" }).click()
  await page.getByRole("menuitem", { name: "归档债务" }).click()
  await page.getByRole("button", { name: "确认归档" }).click()
  await expect(detail.getByText("已归档")).toBeVisible()
  await backToDebtList.click()
  await expect(page).toHaveURL(/\/app\/debts$/)
  await expect(backToDebtList).toHaveCount(0)
  await page.getByLabel("退出登录").click()

  await registerVerifyLogin(page, `second-${suffix}@example.com`)
  await expect(page.getByRole("heading", { name: "暂无符合条件的债务", exact: true })).toBeVisible()
})

test("cashless debt skips the money account end to end", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "full write flow runs once on desktop")
  const suffix = `${Date.now()}-${Math.random().toString(16).slice(2)}`
  await registerVerifyLogin(page, `cashless-${suffix}@example.com`)
  await configureLedgerAccount(page, "微信支付-测试号")

  await page.getByRole("button", { name: "新增债务" }).click()
  const createDialog = page.getByRole("dialog", { name: "新增债务" })
  await createDialog.getByRole("button", { name: "赊账·无资金进出" }).click()
  await expect(createDialog.getByRole("combobox", { name: "收款账户" })).toHaveCount(0)
  await createDialog.getByLabel("联系人").fill("代办记账")
  await createDialog.getByLabel("本金（元）").fill("1500")
  await createDialog.getByLabel("备注").fill("代办执照+代记账尾款")
  await createDialog.getByRole("button", { name: "保存" }).click()

  const cashlessRow = page.getByRole("row", { name: /代办记账 借入/ })
  await expect(cashlessRow.getByText("无资金进出", { exact: true })).toBeVisible()
  await page.getByRole("button", { name: "代办记账", exact: true }).click()
  const detail = page.locator(".debt-detail-page")
  const history = detail.getByRole("region", { name: "债务往来记录" })
  await expect(history.getByText("确认应付 ¥1,500.00", { exact: true })).toBeVisible()
  await expect(history.getByText("无资金进出", { exact: true })).toBeVisible()

  await detail.getByRole("button", { name: "登记往来" }).click()
  const paymentDialog = page.getByRole("dialog", { name: "登记往来" })
  await chooseMovementAction(page, paymentDialog, "登记还款")
  await paymentDialog.getByLabel("还款金额（元）").fill("500")
  await chooseLedgerAccount(page, paymentDialog, "付款账户", "微信支付-测试号")
  await paymentDialog.getByRole("button", { name: "确认登记" }).click()
  await expect(detail.locator(".detail-overview-card").getByText("¥1,000.00", { exact: true })).toBeVisible()
  await expect(history.getByText("确认应付 ¥1,500.00", { exact: true })).toBeVisible()
})

test("long debt history scrolls independently from the debt summary", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "desktop history uses an independent scroll region")
  const suffix = `${Date.now()}-${Math.random().toString(16).slice(2)}`
  await registerVerifyLogin(page, `scroll-${suffix}@example.com`)
  await configureLedgerAccount(page, "微信支付-测试号")

  await page.getByRole("button", { name: "新增债务" }).click()
  const createDialog = page.getByRole("dialog", { name: "新增债务" })
  await createDialog.getByLabel("联系人").fill("滚动验收")
  await createDialog.getByLabel("本金（元）").fill("1000")
  await chooseLedgerAccount(page, createDialog, "收款账户", "微信支付-测试号")
  await createDialog.getByRole("button", { name: "保存" }).click()
  await page.getByRole("button", { name: "滚动验收", exact: true }).click()

  for (let index = 0; index < 12; index += 1) {
    await page.getByRole("button", { name: "登记往来", exact: true }).click()
    const dialog = page.getByRole("dialog", { name: "登记往来" })
    await dialog.getByLabel("追加借入金额（元）").fill("1")
    await chooseLedgerAccount(page, dialog, "收款账户", "微信支付-测试号")
    await dialog.getByRole("button", { name: "确认登记" }).click()
  }

  const history = page.locator(".detail-history")
  expect(await history.evaluate((element) => element.scrollHeight > element.clientHeight)).toBe(true)
  await history.hover()
  await page.mouse.wheel(0, 480)
  await expect.poll(() => history.evaluate((element) => element.scrollTop)).toBeGreaterThan(0)
})

test("date picker keeps a stable height and selects a distant leap day", async ({ page }) => {
  const suffix = `${Date.now()}-${Math.random().toString(16).slice(2)}`
  await registerVerifyLogin(page, `date-picker-${suffix}@example.com`)
  await page.getByRole("button", { name: "新增债务" }).click()

  const trigger = page.getByLabel("发生日期")
  await trigger.click()
  const dialog = page.getByRole("dialog", { name: "选择日期" })
  await expect(dialog).toBeVisible()

  const heights = { days: await dialog.evaluate((element) => element.getBoundingClientRect().height), months: 0, years: 0 }
  await dialog.getByRole("button", { name: /^选择年月，当前/ }).click()
  heights.months = await dialog.evaluate((element) => element.getBoundingClientRect().height)
  await dialog.getByRole("button", { name: /^选择年份，当前/ }).click()
  heights.years = await dialog.evaluate((element) => element.getBoundingClientRect().height)

  expect(Math.max(...Object.values(heights)) - Math.min(...Object.values(heights))).toBeLessThanOrEqual(1)

  await chooseYear(dialog, 2016)
  await dialog.getByRole("gridcell", { name: "2 月", exact: true }).click()
  await dialog.getByRole("gridcell", { name: "2016-02-29", exact: true }).click()
  await expect(trigger).toHaveText("2016-02-29")
  await expect(dialog).toBeHidden()
})

test("Kiln rendered-style contract holds on desktop and mobile", async ({ page }) => {
  const suffix = `${Date.now()}-${Math.random().toString(16).slice(2)}`
  await registerVerifyLogin(page, `visual-${suffix}@example.com`)
  const primaryButtons = page.locator(".button-primary:visible")
  await expect(primaryButtons).toHaveCount(1)
  const styles = await page.evaluate(() => {
    const control = document.querySelector(".button-primary")!
    const metric = document.querySelector(".metric")!
    const dock = document.querySelector(".data-dock")!
    const panelProbe = document.createElement("div")
    panelProbe.style.borderRadius = "var(--radius-panel)"
    document.body.append(panelProbe)
    const controlStyle = getComputedStyle(control)
    const metricStyle = getComputedStyle(metric)
    const dockStyle = getComputedStyle(dock)
    const filter = document.querySelector<HTMLElement>(".toolbar .select-trigger")!
    const tabList = document.querySelector<HTMLElement>(".workspace-tabs [role=tablist]")!
    const activeTab = document.querySelector<HTMLElement>(".workspace-tabs [role=tab][data-state=active]")!
    const pagination = document.querySelector<HTMLElement>(".pagination")!
    const mobileNav = document.querySelector<HTMLElement>(".mobile-nav")!
    const result = {
      controlRadius: controlStyle.borderRadius,
      controlHeight: control.getBoundingClientRect().height,
      controlBackground: controlStyle.backgroundColor,
      metricRadius: metricStyle.borderRadius,
      metricShadow: metricStyle.boxShadow,
      panelRadius: getComputedStyle(panelProbe).borderRadius,
      filterHeight: filter.getBoundingClientRect().height,
      filterRadius: getComputedStyle(filter).borderRadius,
      tabListHeight: tabList.getBoundingClientRect().height,
      activeTabHeight: activeTab.getBoundingClientRect().height,
      activeTabBackground: getComputedStyle(activeTab).backgroundColor,
      fontFamily: getComputedStyle(document.body).fontFamily,
      notoDeclared: [...document.fonts].some((font) => /Noto Sans SC/i.test(font.family)),
      overflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
      dockHeight: dock.getBoundingClientRect().height,
      dockOverflow: dockStyle.overflow,
      dockBackground: dockStyle.backgroundColor,
      dockShadow: dockStyle.boxShadow,
      paginationBottom: pagination.getBoundingClientRect().bottom,
      mobileNavTop: mobileNav.getBoundingClientRect().top,
      mobile: innerWidth <= 760,
    }
    panelProbe.remove()
    return result
  })
  expect(styles.controlRadius).toBe("4px")
  expect(styles.controlHeight).toBe(styles.mobile ? 40 : 36)
  expect(styles.controlBackground).toBe("rgb(182, 83, 60)")
  expect(styles.metricRadius).toBe("6px")
  expect(styles.metricShadow).not.toBe("none")
  expect(styles.panelRadius).toBe("8px")
  expect(styles.filterHeight).toBe(36)
  expect(styles.filterRadius).toBe("4px")
  expect(styles.tabListHeight).toBe(40)
  expect(styles.activeTabHeight).toBe(28)
  expect(styles.activeTabBackground).toBe("rgb(255, 255, 255)")
  expect(styles.dockBackground).toBe("rgba(0, 0, 0, 0)")
  expect(styles.dockShadow).toBe("none")
  expect(styles.fontFamily.startsWith('"Noto Sans SC"')).toBeTruthy()
  expect(styles.notoDeclared).toBeTruthy()
  expect(styles.overflow).toBeLessThanOrEqual(1)
  if (styles.mobile) {
    expect(styles.dockOverflow).toBe("visible")
    expect(styles.paginationBottom).toBeLessThan(styles.mobileNavTop)
  } else {
    expect(styles.dockHeight).toBeGreaterThan(120)
    expect(styles.dockOverflow).toBe("hidden")
  }
  const focus = await primaryButtons.evaluate((element) => {
    element.focus()
    return { boxShadow: getComputedStyle(element).boxShadow, outlineStyle: getComputedStyle(element).outlineStyle }
  })
  expect(focus.boxShadow).toBe("none")
  expect(focus.outlineStyle).toBe("none")
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur())
  expect(await page.evaluate(() => document.activeElement === document.body)).toBeTruthy()
  await page.keyboard.press("Tab")
  expect(await page.evaluate(() => document.activeElement === document.body)).toBeTruthy()
  await page.keyboard.press("Shift+Tab")
  expect(await page.evaluate(() => document.activeElement === document.body)).toBeTruthy()

  await primaryButtons.click()
  const createDialog = page.getByRole("dialog", { name: "新增债务" })
  await expect(createDialog).toBeVisible()
  await page.waitForTimeout(250)
  const dialogStyles = await createDialog.evaluate((element) => {
    const dialogRect = element.getBoundingClientRect()
    const dialogStyle = getComputedStyle(element)
    const select = element.querySelector<HTMLElement>(".select-trigger")!
    return {
      width: dialogRect.width,
      left: dialogRect.left,
      right: innerWidth - dialogRect.right,
      top: dialogRect.top,
      bottom: innerHeight - dialogRect.bottom,
      radius: dialogStyle.borderRadius,
      shadow: dialogStyle.boxShadow,
      selectHeight: select.getBoundingClientRect().height,
      selectRadius: getComputedStyle(select).borderRadius,
    }
  })
  expect(dialogStyles.shadow).not.toBe("none")
  expect(dialogStyles.selectHeight).toBe(36)
  expect(dialogStyles.selectRadius).toBe("4px")
  if (styles.mobile) {
    expect(dialogStyles.left).toBe(0)
    expect(dialogStyles.right).toBe(0)
    expect(dialogStyles.bottom).toBe(0)
  } else {
    expect(dialogStyles.left).toBeGreaterThanOrEqual(24)
    expect(dialogStyles.right).toBeGreaterThanOrEqual(24)
    expect(dialogStyles.top).toBeGreaterThanOrEqual(24)
    expect(dialogStyles.bottom).toBeGreaterThanOrEqual(24)
    expect(dialogStyles.radius).toBe("6px")
  }
  const contactSelect = createDialog.getByRole("combobox", { name: "联系人" })
  await contactSelect.click()
  const selectContent = page.getByRole("listbox")
  await expect(selectContent).toBeVisible()
  await page.waitForTimeout(180)
  const popperStyles = await page.evaluate(() => {
    const trigger = document.querySelector<HTMLElement>(".dialog .select-trigger")!
    const content = document.querySelector<HTMLElement>(".select-content")!
    const triggerRect = trigger.getBoundingClientRect()
    const contentRect = content.getBoundingClientRect()
    return {
      leftDelta: Math.abs(contentRect.left - triggerRect.left),
      triggerWidth: triggerRect.width,
      contentWidth: contentRect.width,
      radius: getComputedStyle(content).borderRadius,
      shadow: getComputedStyle(content).boxShadow,
    }
  })
  expect(popperStyles.leftDelta).toBeLessThanOrEqual(1)
  expect(popperStyles.contentWidth).toBeGreaterThanOrEqual(popperStyles.triggerWidth - 1)
  expect(popperStyles.radius).toBe("6px")
  expect(popperStyles.shadow).not.toBe("none")
  await page.keyboard.press("Escape")
  await createDialog.getByRole("button", { name: "关闭" }).click()

  await page.goto("/login")
  const authStyles = await page.evaluate(() => {
    const input = document.querySelector<HTMLElement>(".auth-form .input")!
    const submit = document.querySelector<HTMLElement>(".auth-form .button-default")!
    const title = document.querySelector<HTMLElement>(".auth-heading h1")!
    return {
      inputHeight: input.getBoundingClientRect().height,
      inputRadius: getComputedStyle(input).borderRadius,
      submitBackground: getComputedStyle(submit).backgroundColor,
      submitRadius: getComputedStyle(submit).borderRadius,
      titleSize: getComputedStyle(title).fontSize,
      overflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
    }
  })
  expect(authStyles.inputHeight).toBe(36)
  expect(authStyles.inputRadius).toBe("4px")
  expect(authStyles.submitBackground).toBe("rgb(47, 47, 47)")
  expect(authStyles.submitRadius).toBe("4px")
  expect(authStyles.titleSize).toBe("20px")
  expect(authStyles.overflow).toBeLessThanOrEqual(1)
})
