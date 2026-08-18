import { expect, test, type Page } from "@playwright/test"
import { readdir, readFile } from "node:fs/promises"
import { resolve } from "node:path"

// @ts-expect-error kiln 的契约是 .mjs，没有类型声明；它的入参就是 Playwright 的 Page。
import { auditPage, createReport, printReport } from "kiln/contract/runtime"

// kiln 的渲染契约。这个文件里**没有规则**——断言全部来自 kiln/contract/runtime，
// 这里只负责三件宿主才知道的事：跑哪些路由、怎么登录、每条路由算哪一档。
// 规则写两份必然漂移，而漂移是这套系统坏掉的唯一方式：上游 kiln 3.11 之前，本仓库
// 自己手写过一份断言，把分段轨道钉在 40px（kiln 的规格是控件高 36px），两份规则各自
// 「通过」了半年。升级 kiln 就升级规则，不需要回来改这个文件。
//
// 覆盖面就是契约的上限：断言没走到的路由，等于那条路由没有规范。

const password = "YOUR_TEST_PASSWORD_HERE"

// 登录后的工作台路由。新增页面时加进来，否则新页面等于没有契约。
const WORKBENCH_ROUTES: Array<[string, string]> = [
  ["债务", "/app/debts"],
  ["账户", "/app/accounts"],
  ["日历", "/app/calendar"],
  ["流水", "/app/transactions"],
  ["统计", "/app/statistics"],
  ["账单导入", "/app/transactions/imports"],
]

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
  const email = `contract-${Date.now()}-${Math.random().toString(16).slice(2)}@example.com`
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
  await expect(page).toHaveURL(/\/app\//)
}

test("kiln 渲染契约覆盖每条路由", async ({ page }, testInfo) => {
  const report = createReport()

  // 登录页属于纸面层（paper）：它不承载重复工作，字号上限按 landing 档放开，
  // 但字号下限、锚点数量、阴影色这些照样要过。
  await page.goto("/login")
  await page.waitForTimeout(700)
  await auditPage(page, `登录页(${testInfo.project.name})`, { kind: "landing", report })

  await registerVerifyLogin(page)
  for (const [label, path] of WORKBENCH_ROUTES) {
    await page.goto(path)
    if (path === "/app/statistics") {
      const useDefault = page.getByRole("button", { name: "使用默认布局" })
      if (await useDefault.isVisible()) await useDefault.click()
      await expect(page.getByRole("heading", { name: "收支趋势" })).toBeVisible()
      await expect(page.getByRole("heading", { name: "分类占比" })).toBeVisible()
      await expect(page.getByRole("heading", { name: "账户余额" })).toBeVisible()
      await expect(page.getByRole("heading", { name: "月度对比" })).toBeVisible()
    }
    await page.waitForTimeout(700)
    await auditPage(page, `${label}(${testInfo.project.name})`, { kind: "admin", report })
  }

  printReport(report, { title: `kiln 渲染契约 · ${testInfo.project.name}` })
  expect(report.failures.join("\n")).toBe("")
})
