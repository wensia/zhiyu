import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { ArrowLeftIcon, CircleUserRoundIcon, HandCoinsIcon, LandmarkIcon, LogOutIcon, PanelLeftIcon, SettingsIcon, WalletCardsIcon } from "lucide-react"
import { useEffect, useState } from "react"
import { Navigate, NavLink, Outlet, Route, Routes, useMatch, useNavigate } from "react-router-dom"

import { api } from "./api/client"
import { Button, useToast } from "./components/ui"
import { AccountWorkspace } from "./features/account-workspace"
import { DebtDetailPage, DebtWorkspace } from "./features/debt-workspace"
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

function ProtectedLayout() {
  const me = useQuery({ queryKey: ["me"], queryFn: api.me, retry: false })
  if (me.isLoading) return <div className="app-loading"><span className="brand-mark"><LandmarkIcon /></span><div className="spinner" /><p>正在打开知余…</p></div>
  if (me.isError) return <Navigate replace to="/login" />
  return <AppShell email={me.data!.email} />
}

export function AppShell({ email }: { email: string }) {
  const navigate = useNavigate()
  const debtDetailRoute = useMatch("/app/debts/:id")
  const queryClient = useQueryClient()
  const toast = useToast()
  const [collapsed, setCollapsed] = useState(savedSidebarState)
  useEffect(() => {
    try {
      window.localStorage?.setItem("zhiyu-sidebar-collapsed", String(collapsed))
    } catch {
      // Storage can be unavailable in private or embedded contexts.
    }
  }, [collapsed])
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "b") {
        event.preventDefault()
        setCollapsed((value) => !value)
      }
    }
    window.addEventListener("keydown", onKeyDown)
    return () => window.removeEventListener("keydown", onKeyDown)
  }, [])
  const logout = useMutation({
    mutationFn: api.logout,
    onSuccess: () => { queryClient.clear(); navigate("/login", { replace: true }) },
    onError: (error) => toast({ title: "退出失败", description: error.message, type: "error" }),
  })
  return (
    <div className="app-shell" data-sidebar={collapsed ? "collapsed" : "expanded"}>
      <aside className="sidebar">
        <div className="sidebar-header">
          <div className="brand-lockup"><span className="brand-mark"><LandmarkIcon aria-hidden="true" /></span><span className="brand-copy"><strong>知余</strong><small>个人账本</small></span></div>
          <Button
            aria-label={collapsed ? "展开侧边栏" : "折叠侧边栏"}
            onClick={() => setCollapsed((value) => !value)}
            size="icon-sm"
            title={collapsed ? "展开侧边栏（⌘B）" : "折叠侧边栏（⌘B）"}
            variant="ghost"
          ><PanelLeftIcon /></Button>
        </div>
        <div className="nav-group">
          <span className="nav-group-label">个人账本</span>
          <nav>
            <NavLink aria-label="个人账本：债务管理" to="/app/debts">
              <HandCoinsIcon aria-hidden="true" />
              <span className="nav-copy">债务管理</span>
              <span aria-hidden="true" className="nav-tooltip"><strong>债务管理</strong><small>个人账本</small></span>
            </NavLink>
            <NavLink aria-label="个人账本：账户管理" to="/app/accounts">
              <WalletCardsIcon aria-hidden="true" />
              <span className="nav-copy">账户管理</span>
              <span aria-hidden="true" className="nav-tooltip"><strong>账户管理</strong><small>个人账本</small></span>
            </NavLink>
            <span aria-disabled="true" className="nav-disabled">
              <SettingsIcon aria-hidden="true" />
              <span className="nav-copy">更多账本功能</span>
              <small className="nav-soon">即将推出</small>
              <span aria-hidden="true" className="nav-tooltip"><strong>更多账本功能</strong><small>个人账本</small></span>
            </span>
          </nav>
        </div>
        <div className="account-block">
          <span className="account-avatar"><CircleUserRoundIcon aria-hidden="true" /></span>
          <div><strong>{email}</strong><span>个人账户</span></div>
          <Button aria-label="退出登录" disabled={logout.isPending} onClick={() => logout.mutate()} size="icon" variant="ghost"><LogOutIcon /></Button>
        </div>
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
        <NavLink to="/app/debts"><HandCoinsIcon /><span>债务</span></NavLink>
        <NavLink to="/app/accounts"><WalletCardsIcon /><span>账户</span></NavLink>
        <button disabled={logout.isPending} onClick={() => logout.mutate()} type="button"><LogOutIcon /><span>退出</span></button>
      </nav>
    </div>
  )
}

export default function App() {
  useEffect(() => {
    const preventTabFocus = (event: KeyboardEvent) => {
      if (event.key !== "Tab") return
      event.preventDefault()
      event.stopPropagation()
    }
    document.addEventListener("keydown", preventTabFocus, true)
    return () => document.removeEventListener("keydown", preventTabFocus, true)
  }, [])

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
        <Route index element={<Navigate replace to="debts" />} />
      </Route>
      <Route path="*" element={<Navigate replace to="/app/debts" />} />
    </Routes>
  )
}
