import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { render } from "@testing-library/react"
import { MemoryRouter, Route, Routes } from "react-router-dom"
import { describe, expect, it, vi } from "vitest"

vi.mock("./navigation", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./navigation")>()
  return {
    ...actual,
    navigationItems: [
      ...actual.navigationItems,
      {
        ...actual.navigationItems[0],
        path: "/app/reports",
        label: "报表",
        mobileLabel: "报表",
        group: "分析",
      },
    ],
  }
})

import { AppShell } from "./App"
import { AppToastProvider } from "./components/ui"

describe("AppShell navigation groups", () => {
  it("renders group labels when navigation has two or more groups", () => {
    const client = new QueryClient({ defaultOptions: { mutations: { retry: false } } })
    const { container } = render(
      <QueryClientProvider client={client}>
        <AppToastProvider>
          <MemoryRouter initialEntries={["/app/debts"]}>
            <Routes>
              <Route path="/app/*" element={<AppShell />} />
            </Routes>
          </MemoryRouter>
        </AppToastProvider>
      </QueryClientProvider>,
    )

    expect(Array.from(container.querySelectorAll(".nav-group-label"), (label) => label.textContent)).toEqual([
      "个人账本",
      "分析",
    ])
  })
})
