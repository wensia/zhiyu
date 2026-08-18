import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { ArrowLeftIcon, LogOutIcon, PanelLeftIcon, RotateCcwIcon } from "lucide-react"
import { useEffect, useMemo, useState } from "react"
import { Navigate, NavLink, Outlet, Route, Routes, useMatch, useNavigate } from "react-router-dom"

import { api } from "./api/client"
import { BrandMark } from "./components/brand-mark"
import { Button, useToast } from "./components/ui"
import { TopbarSlotContext, type TopbarSlots } from "./components/topbar-slots"
import { AccountWorkspace } from "./features/account-workspace"
import { DebtDetailPage, DebtWorkspace } from "./features/debt-workspace"
import { ImportDetailWorkspace, ImportListWorkspace } from "./features/import-workspace"
import { PluginRoute, PluginSettingsPage } from "./features/plugin-settings"
import { CalendarWorkspace, StatisticsWorkspace, TransactionWorkspace } from "./features/transaction-workspace"
import { navigationItemsForPlugins, navigationKey, navigationShortcut } from "./navigation"
import { usePluginEnabled } from "./plugins/context"
import { PluginStateProvider } from "./plugins/state"
import {
  ForgotPasswordPage,
  LoginPage,
  RegisterPage,
  ResetPasswordPage,
  VerifyEmailPage,
} from "./features/auth-pages"

function savedSidebarState() {
  try {
    const saved = window.localStorage?.getItem("zhiyu-sidebar-collapsed")
    return saved === null || saved === undefined ? true : saved === "true"
  } catch {
    return true
  }
}

type TauriInternals = {
  invoke<T>(command: string): Promise<T>
}

function tauriInternals() {
  return (window as Window & { __TAURI_INTERNALS__?: TauriInternals }).__TAURI_INTERNALS__
}

function ProtectedLayout() {
  const me = useQuery({ queryKey: ["me"], queryFn: api.me, retry: false })
  if (me.isLoading) return <div className="app-loading"><span className="brand-mark"><BrandMark /></span><div className="spinner" /><p>正在打开知余…</p></div>
  if (me.isError) return <Navigate replace to="/login" />
  return <AppShell />
}

export function AppShell() {
  const navigate = useNavigate()
  const debtDetailRoute = useMatch("/app/debts/:id")
  const calendarRoute = useMatch("/app/calendar")
  const queryClient = useQueryClient()
  const toast = useToast()
  const pluginsQuery = useQuery({ queryKey: ["plugins"], queryFn: api.plugins })
  const enabledPluginIds = useMemo(() => pluginsQuery.data
    ? new Set(pluginsQuery.data.filter((plugin) => plugin.enabled).map((plugin) => plugin.id))
    : undefined, [pluginsQuery.data])
  const navigationItems = useMemo(() => navigationItemsForPlugins(enabledPluginIds), [enabledPluginIds])
  const settingsNavigation = navigationItems.find((item) => item.path === "/app/settings/plugins")!
  const SettingsNavigationIcon = settingsNavigation.icon
  const primaryNavigationItems = navigationItems.filter((item) => item !== settingsNavigation)
  const navigationGroups = Array.from(new Set(primaryNavigationItems.map((item) => item.group)))
  const [collapsed, setCollapsed] = useState(savedSidebarState)
  const [commandPressed, setCommandPressed] = useState(false)
  const [resettingWindow, setResettingWindow] = useState(false)
  const [topbarSlots, setTopbarSlots] = useState<TopbarSlots>()
  const edgeContent = Boolean(calendarRoute || topbarSlots?.edge)
  useEffect(() => {
    try {
      window.localStorage?.setItem("zhiyu-sidebar-collapsed", String(collapsed))
    } catch {
      // Storage can be unavailable in private or embedded contexts.
    }
  }, [collapsed])
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Meta") setCommandPressed(true)
      const target = event.target
      if (target instanceof Element && target.closest("input, textarea, select, [contenteditable='true']")) return
      const shortcut = event.metaKey ? navigationKey(event) : undefined
      const destination = shortcut ? navigationItems[Number(shortcut) - 1]?.path : undefined
      if (destination) {
        event.preventDefault()
        navigate(destination)
        return
      }
      if (!event.metaKey || event.key.toLowerCase() !== "b") return
      event.preventDefault()
      setCollapsed((value) => !value)
    }
    const onKeyUp = (event: KeyboardEvent) => {
      if (event.key === "Meta") setCommandPressed(false)
    }
    const onBlur = () => setCommandPressed(false)
    window.addEventListener("keydown", onKeyDown)
    window.addEventListener("keyup", onKeyUp)
    window.addEventListener("blur", onBlur)
    return () => {
      window.removeEventListener("keydown", onKeyDown)
      window.removeEventListener("keyup", onKeyUp)
      window.removeEventListener("blur", onBlur)
    }
  }, [navigate, navigationItems])
  const logout = useMutation({
    mutationFn: api.logout,
    onSuccess: () => { queryClient.clear(); navigate("/login", { replace: true }) },
    onError: (error) => toast({ title: "退出失败", description: error.message, type: "error" }),
  })
  const resetWindow = async () => {
    const internals = tauriInternals()
    if (!internals) return
    setResettingWindow(true)
    try {
      await internals.invoke("reset_main_window")
    } catch (error) {
      toast({ title: "还原窗口失败", description: error instanceof Error ? error.message : String(error), type: "error" })
    } finally {
      setResettingWindow(false)
    }
  }
  return (
    <div className="app-shell" data-command-pressed={commandPressed || undefined} data-sidebar={collapsed ? "collapsed" : "expanded"}>
      {/* 顶栏只盖内容列，侧边栏通高（见 styles.css 的 .app-shell 栅格）。桌面端窗口
          用的是 macOS 的 Overlay 标题栏，交通灯浮在网页内容上；窗口左上角因此是侧边栏
          的首格，交通灯归它收留，顶栏不必再为它让出一条左边距。 */}
      <header className="topbar" data-calendar-topbar={edgeContent || undefined} data-tauri-drag-region title="拖动以移动窗口">
        {/* 顶栏的标题槽就是路由的 h1（kiln：Title Authority）。之前它是个加粗 span，
            于是日历、流水、统计三页整页没有任何 h1——读屏软件落进一个从不自报门户的
            页面，而债务、账户两页反过来在正文里各自又立了一个标题。
            没有路由认领这个槽时（详情路由的标题是那条记录本身，由正文的记录卡持有），
            这里退回产品常量，且必须是普通文本：无条件渲染成标题就会在详情页凑出两个
            h1，正是这条规范要防的那种重复。 */}
        <div className="topbar-leading">
          {topbarSlots?.leading ?? (debtDetailRoute
            ? <Button aria-label="返回债务列表" onClick={() => navigate("/app/debts")} size="icon" title="返回债务列表" variant="ghost"><ArrowLeftIcon /></Button>
            : null)}
          {topbarSlots?.title
            ? <h1 className="topbar-title">{topbarSlots.title}</h1>
            : <span className="topbar-title">个人账本</span>}
        </div>
        <div className="topbar-actions" data-tauri-drag-region="false">{topbarSlots?.actions ?? <span>知余工作台</span>}</div>
      </header>
      <aside className="sidebar" id="app-sidebar">
        <div className="sidebar-header" data-tauri-drag-region title="拖动以移动窗口">
          <div className="brand-lockup"><span className="brand-mark"><BrandMark /></span><span className="brand-copy"><strong>知余</strong><small>个人账本</small></span></div>
          <Button
            aria-controls="app-sidebar"
            aria-expanded={!collapsed}
            aria-keyshortcuts="Meta+B"
            aria-label={collapsed ? "展开侧边栏" : "折叠侧边栏"}
            className="sidebar-toggle"
            onClick={() => setCollapsed((value) => !value)}
            size="icon-sm"
            title={collapsed ? "展开侧边栏（⌘B）" : "折叠侧边栏（⌘B）"}
            variant="ghost"
          >
            <span className="brand-mark sidebar-toggle-brand"><BrandMark /></span>
            <PanelLeftIcon aria-hidden="true" className="sidebar-toggle-icon" />
          </Button>
        </div>
        {navigationGroups.map((group) => (
          <div className="nav-group" key={group}>
            {navigationGroups.length >= 2 ? <span className="nav-group-label">{group}</span> : null}
            <nav>
              {primaryNavigationItems.map((item) => {
                if (item.group !== group) return null
                const shortcut = navigationShortcut(navigationItems.indexOf(item))
                const Icon = item.icon
                return (
                  <NavLink
                    aria-keyshortcuts={shortcut ? `Meta+${shortcut}` : undefined}
                    aria-label={`${item.group}：${item.label}`}
                    key={item.path}
                    to={item.path}
                  >
                    <span className="nav-icon"><Icon aria-hidden="true" /></span>
                    <span className="nav-copy">{item.label}</span>
                    {shortcut ? <kbd aria-hidden="true" className="nav-shortcut">⌘{shortcut}</kbd> : null}
                  </NavLink>
                )
              })}
            </nav>
          </div>
        ))}
        <div className="sidebar-bottom-actions">
          {tauriInternals() ? (
            <button
              aria-label="还原窗口大小并居中"
              className="sidebar-window-reset"
              disabled={resettingWindow}
              onClick={resetWindow}
              title="还原窗口大小并居中"
              type="button"
            >
              <RotateCcwIcon aria-hidden="true" />
            </button>
          ) : null}
          <NavLink
            aria-keyshortcuts={`Meta+${navigationShortcut(navigationItems.indexOf(settingsNavigation))}`}
            aria-label="设置：插件"
            className="sidebar-settings"
            to={settingsNavigation.path}
          >
            <span className="nav-icon"><SettingsNavigationIcon aria-hidden="true" /></span>
            <span className="nav-copy">{settingsNavigation.label}</span>
            <kbd aria-hidden="true" className="nav-shortcut">⌘{navigationShortcut(navigationItems.indexOf(settingsNavigation))}</kbd>
          </NavLink>
          <button
            aria-label="退出登录"
            className="sidebar-logout"
            disabled={logout.isPending}
            onClick={() => logout.mutate()}
            type="button"
          >
            <LogOutIcon aria-hidden="true" />
            <span className="nav-copy">退出登录</span>
          </button>
        </div>
      </aside>
      <TopbarSlotContext.Provider value={setTopbarSlots}>
        <PluginStateProvider value={{ plugins: pluginsQuery.data, isLoading: pluginsQuery.isLoading }}>
          <div className={`app-main${edgeContent ? " app-main-edge" : ""}`}><Outlet /></div>
        </PluginStateProvider>
      </TopbarSlotContext.Provider>
      <nav className="mobile-nav">
        {navigationItems.map((item) => {
          const Icon = item.icon
          return <NavLink key={item.path} to={item.path}><Icon /><span>{item.mobileLabel}</span></NavLink>
        })}
        <button disabled={logout.isPending} onClick={() => logout.mutate()} type="button"><LogOutIcon /><span>退出</span></button>
      </nav>
    </div>
  )
}

function DefaultAppRoute() {
  return <Navigate replace to={usePluginEnabled("debts") ? "debts" : "calendar"} />
}

export default function App() {
  // pointer-first 焦点策略的另一半（kiln：Interaction Is Quiet 的 focus policy）。
  // 这个产品早就在 :root 关掉了焦点环（--ring-focus / --shadow-primary-focus: none），
  // 却一直没拦住 Tab —— 而两半只做一半是最坏的组合：焦点仍在页面上一格一格地走，
  // 用户却看不见它走到了哪里，键盘用户得到的是一个隐形的光标。
  //
  // 挂在 document 的**捕获阶段**，是为了抢在浏览器默认行为和组件库之前截住：Radix 的
  // Dialog / Select 自己装了 focus trap，冒泡阶段再拦已经晚了。只拦 Tab 一个键 ——
  // 点击聚焦、输入、选中文本照旧；日期选择器的方向键 / PageUp / PageDown、表格行的
  // Enter / Space、菜单的高亮协议都不经过这里。
  useEffect(() => {
    const interceptTab = (event: KeyboardEvent) => {
      if (event.key !== "Tab") return
      event.preventDefault()
      event.stopPropagation()
    }
    document.addEventListener("keydown", interceptTab, true)
    return () => document.removeEventListener("keydown", interceptTab, true)
  }, [])
  return (
    <Routes>
      <Route path="/login" element={<LoginPage />} />
      <Route path="/register" element={<RegisterPage />} />
      <Route path="/verify-email" element={<VerifyEmailPage />} />
      <Route path="/forgot-password" element={<ForgotPasswordPage />} />
      <Route path="/reset-password" element={<ResetPasswordPage />} />
      <Route path="/app" element={<ProtectedLayout />}>
        <Route path="debts" element={<PluginRoute pluginId="debts"><DebtWorkspace /></PluginRoute>} />
        <Route path="debts/:id" element={<PluginRoute pluginId="debts"><DebtDetailPage /></PluginRoute>} />
        <Route path="accounts" element={<AccountWorkspace />} />
        <Route path="calendar" element={<CalendarWorkspace />} />
        <Route path="transactions" element={<TransactionWorkspace />} />
        <Route path="transactions/imports" element={<PluginRoute pluginId="bill-imports"><ImportListWorkspace /></PluginRoute>} />
        <Route path="transactions/imports/:id" element={<PluginRoute pluginId="bill-imports"><ImportDetailWorkspace /></PluginRoute>} />
        <Route path="statistics" element={<StatisticsWorkspace />} />
        <Route path="settings/plugins" element={<PluginSettingsPage />} />
        <Route index element={<DefaultAppRoute />} />
      </Route>
      <Route path="*" element={<Navigate replace to="/app/debts" />} />
    </Routes>
  )
}
