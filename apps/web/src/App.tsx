import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { ArrowLeftIcon, LogOutIcon, PanelLeftIcon } from "lucide-react"
import { useEffect, useState } from "react"
import { Navigate, NavLink, Outlet, Route, Routes, useMatch, useNavigate } from "react-router-dom"

import { api } from "./api/client"
import { BrandMark } from "./components/brand-mark"
import { Button, useToast } from "./components/ui"
import { AccountWorkspace } from "./features/account-workspace"
import { DebtDetailPage, DebtWorkspace } from "./features/debt-workspace"
import { TransactionWorkspace } from "./features/transaction-workspace"
import { navigationItems, navigationKey, navigationShortcut } from "./navigation"
import {
  ForgotPasswordPage,
  LoginPage,
  RegisterPage,
  ResetPasswordPage,
  VerifyEmailPage,
} from "./features/auth-pages"

function savedSidebarState() {
  try {
    return window.localStorage?.getItem("zhiyu-sidebar-collapsed") === "true"
  } catch {
    return false
  }
}

const navigationGroups = Array.from(new Set(navigationItems.map((item) => item.group)))

function ProtectedLayout() {
  const me = useQuery({ queryKey: ["me"], queryFn: api.me, retry: false })
  if (me.isLoading) return <div className="app-loading"><span className="brand-mark"><BrandMark /></span><div className="spinner" /><p>正在打开知余…</p></div>
  if (me.isError) return <Navigate replace to="/login" />
  return <AppShell />
}

export function AppShell() {
  const navigate = useNavigate()
  const debtDetailRoute = useMatch("/app/debts/:id")
  const queryClient = useQueryClient()
  const toast = useToast()
  const [collapsed, setCollapsed] = useState(savedSidebarState)
  const [commandPressed, setCommandPressed] = useState(false)
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
      if (!(event.metaKey || event.ctrlKey) || event.key.toLowerCase() !== "b") return
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
  }, [navigate])
  const logout = useMutation({
    mutationFn: api.logout,
    onSuccess: () => { queryClient.clear(); navigate("/login", { replace: true }) },
    onError: (error) => toast({ title: "退出失败", description: error.message, type: "error" }),
  })
  return (
    <div className="app-shell" data-command-pressed={commandPressed || undefined} data-sidebar={collapsed ? "collapsed" : "expanded"}>
      <aside className="sidebar" id="app-sidebar">
        <div className="sidebar-header">
          <div className="brand-lockup"><span className="brand-mark"><BrandMark /></span><span className="brand-copy"><strong>知余</strong><small>个人账本</small></span></div>
          <Button
            aria-controls="app-sidebar"
            aria-expanded={!collapsed}
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
            <span className="nav-group-label">{group}</span>
            <nav>
              {navigationItems.map((item, index) => {
                if (item.group !== group) return null
                const shortcut = navigationShortcut(index)
                const Icon = item.icon
                return (
                  <NavLink
                    aria-keyshortcuts={shortcut ? `Meta+${shortcut}` : undefined}
                    aria-label={`${item.group}：${item.label}`}
                    key={item.path}
                    to={item.path}
                  >
                    <Icon aria-hidden="true" />
                    <span className="nav-copy">{item.label}</span>
                    {shortcut ? <kbd aria-hidden="true" className="nav-shortcut">⌘{shortcut}</kbd> : null}
                    <span aria-hidden="true" className="nav-tooltip"><strong>{item.label}</strong><small>{item.group}</small></span>
                  </NavLink>
                )
              })}
            </nav>
          </div>
        ))}
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
      </aside>
      <section className="app-frame">
        <header className="topbar">
          <div>
            {debtDetailRoute
              ? <Button aria-label="返回债务列表" onClick={() => navigate("/app/debts")} size="icon" title="返回债务列表" variant="ghost"><ArrowLeftIcon /></Button>
              : <span className="topbar-tick" />}
            <strong>个人账本</strong>
          </div>
          <span>知余工作台</span>
        </header>
        <div className="app-main"><Outlet /></div>
      </section>
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

export default function App() {
  return (
    <Routes>
      <Route path="/login" element={<LoginPage />} />
      <Route path="/register" element={<RegisterPage />} />
      <Route path="/verify-email" element={<VerifyEmailPage />} />
      <Route path="/forgot-password" element={<ForgotPasswordPage />} />
      <Route path="/reset-password" element={<ResetPasswordPage />} />
      <Route path="/app" element={<ProtectedLayout />}>
        <Route path="debts" element={<DebtWorkspace />} />
        <Route path="debts/:id" element={<DebtDetailPage />} />
        <Route path="accounts" element={<AccountWorkspace />} />
        <Route path="transactions" element={<TransactionWorkspace />} />
        <Route index element={<Navigate replace to="debts" />} />
      </Route>
      <Route path="*" element={<Navigate replace to="/app/debts" />} />
    </Routes>
  )
}
