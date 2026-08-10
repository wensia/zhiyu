import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { fireEvent, render, screen } from "@testing-library/react"
import { MemoryRouter, Outlet, Route, Routes, useLocation } from "react-router-dom"
import { beforeEach, describe, expect, it } from "vitest"

import { AppShell } from "./App"
import { AppToastProvider } from "./components/ui"
import { navigationItems, navigationShortcut } from "./navigation"

function LocationProbe() {
  const location = useLocation()
  return <output aria-label="当前位置">{location.pathname}</output>
}

function Page() {
  return <><LocationProbe /><input aria-label="测试输入框" /><Outlet /></>
}

function renderShell() {
  const client = new QueryClient({ defaultOptions: { mutations: { retry: false } } })
  return render(
    <QueryClientProvider client={client}>
      <AppToastProvider>
        <MemoryRouter initialEntries={["/app/debts"]}>
          <Routes>
            <Route path="/app" element={<AppShell />}>
              <Route path="debts" element={<Page />} />
              <Route path="transactions" element={<Page />} />
              <Route path="accounts" element={<Page />} />
            </Route>
          </Routes>
        </MemoryRouter>
      </AppToastProvider>
    </QueryClientProvider>,
  )
}

beforeEach(() => {
  // 与 App.tsx 读写侧边栏折叠状态时的写法保持一致：Node 26 下 localStorage 是
  // 需要显式开启的实验特性，jsdom 里可能拿不到，不能假定它一定存在。
  window.localStorage?.clear()
})

describe("AppShell navigation shortcuts", () => {
  it("shows shortcut hints only while Command is held", () => {
    const { container } = renderShell()
    const shell = container.querySelector(".app-shell")

    expect(shell).not.toHaveAttribute("data-command-pressed")
    fireEvent.keyDown(window, { key: "Meta", metaKey: true })
    expect(shell).toHaveAttribute("data-command-pressed", "true")
    expect(screen.getByText("⌘1")).toBeInTheDocument()
    expect(screen.getByText("⌘2")).toBeInTheDocument()
    expect(screen.getByText("⌘3")).toBeInTheDocument()

    fireEvent.keyUp(window, { key: "Meta" })
    expect(shell).not.toHaveAttribute("data-command-pressed")
  })

  it("navigates with Command plus 1, 2, or 3", () => {
    renderShell()

    fireEvent.keyDown(window, { code: "Digit2", key: "¡", metaKey: true })
    expect(screen.getByLabelText("当前位置")).toHaveTextContent("/app/transactions")
    fireEvent.keyDown(window, { key: "3", metaKey: true })
    expect(screen.getByLabelText("当前位置")).toHaveTextContent("/app/accounts")
    fireEvent.keyDown(window, { key: "1", metaKey: true })
    expect(screen.getByLabelText("当前位置")).toHaveTextContent("/app/debts")
  })

  it("does not navigate while editing text", () => {
    renderShell()
    const input = screen.getByLabelText("测试输入框")

    input.focus()
    fireEvent.keyDown(input, { key: "2", metaKey: true })
    expect(screen.getByLabelText("当前位置")).toHaveTextContent("/app/debts")
  })

  it("assigns the next shortcut when a navigation item is appended", () => {
    const itemsWithNewPage = [
      ...navigationItems,
      { ...navigationItems[0], path: "/app/reports", label: "报表", mobileLabel: "报表" },
    ]

    expect(itemsWithNewPage.map((_, index) => navigationShortcut(index))).toEqual(["1", "2", "3", "4"])
  })

  it("does not assign shortcuts after Command plus 9", () => {
    expect(Array.from({ length: 10 }, (_, index) => navigationShortcut(index))).toEqual([
      "1", "2", "3", "4", "5", "6", "7", "8", "9", undefined,
    ])
  })
})

describe("AppShell sidebar toggle", () => {
  it("keeps the brand mark in the fixed toggle when the sidebar is collapsed", () => {
    const { container } = renderShell()
    const shell = container.querySelector(".app-shell")
    const toggle = screen.getByRole("button", { name: "折叠侧边栏" })

    expect(toggle.querySelector(".sidebar-toggle-brand .brand-symbol")).toBeInTheDocument()
    fireEvent.click(toggle)

    expect(shell).toHaveAttribute("data-sidebar", "collapsed")
    expect(screen.getByRole("button", { name: "展开侧边栏" })).toContainElement(
      container.querySelector(".sidebar-toggle-brand .brand-symbol"),
    )
  })
})

describe("AppShell navigation groups", () => {
  it("does not render a group label when navigation has only one group", () => {
    const { container } = renderShell()

    expect(container.querySelector(".nav-group-label")).not.toBeInTheDocument()
  })
})
