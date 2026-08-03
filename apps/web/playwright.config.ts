import { defineConfig, devices } from "@playwright/test"

export default defineConfig({
  testDir: "./e2e",
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: "list",
  use: {
    baseURL: "http://127.0.0.1:25173",
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  projects: [
    { name: "desktop-chromium", use: { ...devices["Desktop Chrome"], viewport: { width: 1440, height: 900 } } },
    { name: "mobile-chromium", use: { browserName: "chromium", viewport: { width: 390, height: 844 }, deviceScaleFactor: 3, isMobile: true, hasTouch: true } },
  ],
  webServer: [
    {
      command: "BIND_ADDR=127.0.0.1:28787 PUBLIC_BASE_URL=http://127.0.0.1:25173 DATABASE_URL=file:../../var/e2e.db DEV_MAIL_DIR=../../var/e2e-mail WEB_DIST_DIR=./dist cargo run --manifest-path ../../Cargo.toml -p zhiyu-api --bin zhiyu-api",
      url: "http://127.0.0.1:28787/health/ready",
      reuseExistingServer: true,
      timeout: 120_000,
    },
    {
      command: "WEB_PORT=25173 API_PROXY=http://127.0.0.1:28787 pnpm dev",
      url: "http://127.0.0.1:25173/login",
      reuseExistingServer: true,
      timeout: 120_000,
    },
  ],
})
