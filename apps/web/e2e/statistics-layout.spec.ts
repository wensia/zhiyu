import { expect, test, type Page } from "@playwright/test"
import { readdir, readFile } from "node:fs/promises"
import { resolve } from "node:path"

// 自由留白的验收：组件待在你放的位置。
// 这是唯一能真正证明「拖到哪停在哪 + 刷新一致 + 删了不上浮」的测试 —— 单测跑在 jsdom 里，
// 拖不动真实的 react-grid-layout，只能验到 onLayoutChange 的入参。

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
  const email = `layout-${Date.now()}-${Math.random().toString(16).slice(2)}@example.com`
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

/** 打开统计页并确保有一份默认布局（新账号首次进入是空状态引导）。 */
async function openDashboard(page: Page) {
  await page.goto("/app/statistics")
  const useDefault = page.getByRole("button", { name: "使用默认布局" })
  const firstWidget = page.getByRole("heading", { name: "收支趋势" })
  // isVisible() 不等待，页面刚打开时查询还在飞；先等两者之一落地
  await expect(useDefault.or(firstWidget).first()).toBeVisible()
  if (await useDefault.isVisible()) await useDefault.click()
  await expect(firstWidget).toBeVisible()
}

const card = (page: Page, title: string) => page.locator(".react-grid-item").filter({ hasText: title })

/** RGL 只认真实的指针序列，dragTo 的单跳位移不会被它当成拖拽。 */
async function dragBy(page: Page, title: string, dx: number, dy: number) {
  const handle = card(page, title).locator(".statistics-widget-header")
  const box = (await handle.boundingBox())!
  const fromX = box.x + box.width / 2
  const fromY = box.y + box.height / 2
  await page.mouse.move(fromX, fromY)
  await page.mouse.down()
  await page.mouse.move(fromX + dx, fromY + dy, { steps: 12 })
  await page.mouse.up()
  await page.waitForTimeout(300)
}

test("拖到下方的留白处，松手不回弹，刷新后还在那儿", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "布局只在桌面断点可编辑")
  await registerVerifyLogin(page)
  await openDashboard(page)
  await page.getByRole("button", { name: "编辑" }).click()

  const target = card(page, "账户余额")
  const before = (await target.boundingBox())!
  await dragBy(page, "账户余额", 0, 420)

  const after = (await target.boundingBox())!
  // 关掉 compactType 之前，这里会被自动上浮拽回原处
  expect(after.y).toBeGreaterThan(before.y + 200)

  await page.waitForTimeout(900) // 越过 queueWidgetSave 的 500ms 防抖
  await page.reload()
  await expect(page.getByRole("heading", { name: "收支趋势" })).toBeVisible()
  const reloaded = (await card(page, "账户余额").boundingBox())!
  expect(Math.abs(reloaded.y - after.y)).toBeLessThan(8)
})

test("删掉上面的组件，下面的不会自动上浮", async ({ page }, testInfo) => {
  test.skip(testInfo.project.name.includes("mobile"), "布局只在桌面断点可编辑")
  await registerVerifyLogin(page)
  await openDashboard(page)
  await page.getByRole("button", { name: "编辑" }).click()

  // 默认布局：trend(0,0,8,4) / category(8,0,4,4) / balances(0,4,4,3) / compare(4,4,8,3)
  const compare = card(page, "月度对比")
  const before = (await compare.boundingBox())!

  await card(page, "收支趋势").getByRole("button", { name: /^操作组件/ }).click()
  await page.getByRole("menuitem", { name: "删除" }).click()
  await expect(page.getByRole("heading", { name: "收支趋势" })).toHaveCount(0)
  await page.waitForTimeout(300)

  const after = (await compare.boundingBox())!
  // vertical compact 会把它拽到顶上；保持位置语义下它必须原地不动
  expect(Math.abs(after.y - before.y)).toBeLessThan(8)
})
