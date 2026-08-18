import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { MemoryRouter, Outlet, Route, Routes, useLocation } from "react-router-dom"
import { useEffect } from "react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { AppShell } from "./App"
import { AppToastProvider } from "./components/ui"
import { useTopbarSlots } from "./components/topbar-slots"
import { navigationItems, navigationItemsForPlugins, navigationShortcut } from "./navigation"

function LocationProbe() {
  const location = useLocation()
  return <output aria-label="当前位置">{location.pathname}</output>
}

function Page() {
  return <><LocationProbe /><input aria-label="测试输入框" /><Outlet /></>
}

function EdgePage() {
  const setTopbarSlots = useTopbarSlots()
  useEffect(() => { setTopbarSlots({ title: "日历", edge: true }); return () => setTopbarSlots(undefined) }, [setTopbarSlots])
  return <LocationProbe />
}

function renderShell(initialEntry = "/app/debts") {
  const client = new QueryClient({ defaultOptions: { mutations: { retry: false } } })
  return render(
    <QueryClientProvider client={client}>
      <AppToastProvider>
        <MemoryRouter initialEntries={[initialEntry]}>
          <Routes>
            <Route path="/app" element={<AppShell />}>
              <Route path="debts" element={<Page />} />
              <Route path="calendar" element={<Page />} />
              <Route path="transactions" element={<Page />} />
              <Route path="statistics" element={<Page />} />
              <Route path="calendar-edge" element={<EdgePage />} />
              <Route path="accounts" element={<Page />} />
              <Route path="settings/plugins" element={<Page />} />
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
  window.localStorage?.setItem("zhiyu-sidebar-collapsed", "true")
  Reflect.deleteProperty(window, "__TAURI_INTERNALS__")
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
    expect(screen.getByText("⌘5")).toBeInTheDocument()
    expect(screen.getByText("⌘6")).toBeInTheDocument()

    fireEvent.keyUp(window, { key: "Meta" })
    expect(shell).not.toHaveAttribute("data-command-pressed")
  })

  it("navigates with Command plus 1 through 6 in navigation order", () => {
    renderShell()

    fireEvent.keyDown(window, { code: "Digit2", key: "¡", metaKey: true })
    expect(screen.getByLabelText("当前位置")).toHaveTextContent("/app/calendar")
    fireEvent.keyDown(window, { key: "3", metaKey: true })
    expect(screen.getByLabelText("当前位置")).toHaveTextContent("/app/transactions")
    fireEvent.keyDown(window, { key: "4", metaKey: true })
    expect(screen.getByLabelText("当前位置")).toHaveTextContent("/app/statistics")
    fireEvent.keyDown(window, { key: "5", metaKey: true })
    expect(screen.getByLabelText("当前位置")).toHaveTextContent("/app/accounts")
    fireEvent.keyDown(window, { key: "1", metaKey: true })
    expect(screen.getByLabelText("当前位置")).toHaveTextContent("/app/debts")
    fireEvent.keyDown(window, { key: "6", metaKey: true })
    expect(screen.getByLabelText("当前位置")).toHaveTextContent("/app/settings/plugins")
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

    expect(itemsWithNewPage.map((_, index) => navigationShortcut(index))).toEqual(["1", "2", "3", "4", "5", "6", "7"])
  })

  it("does not assign shortcuts after Command plus 9", () => {
    expect(Array.from({ length: 10 }, (_, index) => navigationShortcut(index))).toEqual([
      "1", "2", "3", "4", "5", "6", "7", "8", "9", undefined,
    ])
  })
})

describe("AppShell sidebar toggle", () => {
  it("starts collapsed when no sidebar preference has been saved", () => {
    window.localStorage?.removeItem("zhiyu-sidebar-collapsed")
    const { container } = renderShell()

    expect(container.querySelector(".app-shell")).toHaveAttribute("data-sidebar", "collapsed")
    expect(screen.getByRole("button", { name: "展开侧边栏" })).toBeInTheDocument()
  })

  it("toggles the sidebar with Command plus B", () => {
    const { container } = renderShell()
    const shell = container.querySelector(".app-shell")
    // 无偏好时默认折叠，所以此刻按钮的可访问名是「展开侧边栏」。
    const toggle = screen.getByRole("button", { name: "展开侧边栏" })

    expect(toggle).toHaveAttribute("aria-keyshortcuts", "Meta+B")
    expect(fireEvent.keyDown(window, { key: "b", metaKey: true })).toBe(false)
    expect(shell).toHaveAttribute("data-sidebar", "expanded")

    fireEvent.keyDown(window, { key: "B", metaKey: true })
    expect(shell).toHaveAttribute("data-sidebar", "collapsed")
  })

  it("does not toggle the sidebar while editing text", () => {
    const { container } = renderShell()
    const input = screen.getByLabelText("测试输入框")

    fireEvent.keyDown(input, { key: "b", metaKey: true })
    expect(container.querySelector(".app-shell")).toHaveAttribute("data-sidebar", "collapsed")
  })

  it("restores and centers the desktop window from the sidebar", async () => {
    const invoke = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(window, "__TAURI_INTERNALS__", { configurable: true, value: { invoke } })
    renderShell()

    const resetButton = screen.getByRole("button", { name: "还原窗口大小并居中" })
    expect(resetButton).not.toHaveTextContent("还原窗口")
    fireEvent.click(resetButton)

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("reset_main_window"))
  })

  it("marks the desktop chrome as window drag regions", () => {
    const { container } = renderShell()

    expect(container.querySelector(".topbar")).toHaveAttribute("data-tauri-drag-region")
    expect(container.querySelector(".sidebar-header")).toHaveAttribute("data-tauri-drag-region")
    expect(screen.getByRole("button", { name: "展开侧边栏" })).not.toHaveAttribute("data-tauri-drag-region")
  })

  it("keeps the brand mark in the fixed toggle when the sidebar is collapsed", () => {
    const { container } = renderShell()
    const shell = container.querySelector(".app-shell")
    // 默认即折叠：品牌标记此刻就该在切换按钮里，展开后仍不能丢。
    const collapsedToggle = screen.getByRole("button", { name: "展开侧边栏" })

    expect(shell).toHaveAttribute("data-sidebar", "collapsed")
    expect(collapsedToggle.querySelector(".sidebar-toggle-brand .brand-symbol")).toBeInTheDocument()

    fireEvent.click(collapsedToggle)

    expect(shell).toHaveAttribute("data-sidebar", "expanded")
    expect(screen.getByRole("button", { name: "折叠侧边栏" })).toContainElement(
      container.querySelector(".sidebar-toggle-brand .brand-symbol"),
    )
  })

  it("keeps collapsed navigation labels below square icon containers without repeating them in hover labels", () => {
    const { container } = renderShell()
    // 默认即折叠，不需要先点一次切换按钮。

    const debtNavItem = screen.getByRole("link", { name: "个人账本：债务" })
    expect(container.querySelector(".app-shell")).toHaveAttribute("data-sidebar", "collapsed")
    expect(debtNavItem.querySelector(".nav-icon svg")).toBeInTheDocument()
    expect(debtNavItem.querySelector(".nav-copy")).toHaveTextContent("债务")
    expect(container.querySelector(".nav-tooltip")).not.toBeInTheDocument()
  })
})

describe("AppShell navigation groups", () => {
  it("does not render a group label when navigation has only one group", () => {
    const { container } = renderShell()

    expect(container.querySelector(".nav-group-label")).not.toBeInTheDocument()
  })
})

describe("plugin navigation", () => {
  it("keeps the original order while filtering disabled plugin contributions", () => {
    expect(navigationItemsForPlugins(new Set(["bill-imports", "auto-categorize"])).map((item) => item.label)).toEqual([
      "日历", "流水", "统计", "账户", "设置",
    ])
    expect(navigationItemsForPlugins(new Set(["debts", "bill-imports", "auto-categorize"])).map((item) => item.label)).toEqual([
      "债务", "日历", "流水", "统计", "账户", "设置",
    ])
  })
})

describe("AppShell topbar slots", () => {
  it("makes the canonical calendar route edge-to-edge before page slots mount", () => {
    const calendar = renderShell("/app/calendar")

    expect(calendar.container.querySelector(".app-main")).toHaveClass("app-main-edge")
    expect(calendar.container.querySelector(".topbar")).toHaveAttribute("data-calendar-topbar")
  })

  it("uses edge content only when a route explicitly requests it", async () => {
    const standard = renderShell()
    expect(standard.container.querySelector(".app-main")).not.toHaveClass("app-main-edge")
    standard.unmount()
    const edge = renderShell("/app/calendar-edge")
    await waitFor(() => expect(edge.container.querySelector(".app-main")).toHaveClass("app-main-edge"))
    expect(edge.container.querySelector(".topbar")).toHaveAttribute("data-calendar-topbar")
  })
})
