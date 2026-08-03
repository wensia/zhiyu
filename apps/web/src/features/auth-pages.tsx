import { zodResolver } from "@hookform/resolvers/zod"
import { useMutation } from "@tanstack/react-query"
import { ArrowLeftIcon, CheckCircle2Icon, LandmarkIcon } from "lucide-react"
import { useEffect } from "react"
import { useForm } from "react-hook-form"
import { Link, useNavigate, useSearchParams } from "react-router-dom"
import { z } from "zod"

import { api } from "../api/client"
import { Button, Field, InlineNotice, Input, PasswordInput } from "../components/ui"

const credentialsSchema = z.object({
  email: z.string().email("请输入正确的邮箱"),
  password: z.string().min(12, "密码至少 12 个字符").max(128, "密码不能超过 128 个字符"),
})

type Credentials = z.infer<typeof credentialsSchema>

function AuthFrame({ title, subtitle, children }: { title: string; subtitle: string; children: React.ReactNode }) {
  return (
    <main className="auth-page">
      <section className="auth-panel">
        <div className="brand-lockup"><span className="brand-mark"><LandmarkIcon /></span><span>知余</span></div>
        <div className="auth-heading"><h1>{title}</h1><p>{subtitle}</p></div>
        {children}
      </section>
      <aside className="auth-context" aria-hidden="true">
        <div className="auth-context-inner">
          <span className="eyebrow">个人债务管理</span>
          <h2>每一笔往来，<br />都清楚有据。</h2>
          <div className="auth-sample">
            <div><span>待收</span><strong>¥ 12,800.00</strong></div>
            <div><span>待还</span><strong>¥ 3,200.00</strong></div>
            <p><CheckCircle2Icon /> 余额来自本金与还款记录，不手工覆盖</p>
          </div>
        </div>
      </aside>
    </main>
  )
}

export function LoginPage() {
  const navigate = useNavigate()
  const mutation = useMutation({ mutationFn: ({ email, password }: Credentials) => api.login(email, password), onSuccess: () => navigate("/app/debts", { replace: true }) })
  const { register, handleSubmit, formState: { errors } } = useForm<Credentials>({ resolver: zodResolver(credentialsSchema) })
  return (
    <AuthFrame title="登录知余" subtitle="继续管理你的借入、借出与还款记录。">
      <form className="auth-form" noValidate onSubmit={handleSubmit((values) => mutation.mutate(values))}>
        <Field label="邮箱" error={errors.email?.message}><Input aria-invalid={Boolean(errors.email)} autoComplete="email" type="email" {...register("email")} /></Field>
        <Field label="密码" error={errors.password?.message}><PasswordInput aria-invalid={Boolean(errors.password)} aria-label="密码" autoComplete="current-password" {...register("password")} /></Field>
        {mutation.error ? <InlineNotice type="error">{mutation.error.message}</InlineNotice> : null}
        <Button disabled={mutation.isPending} type="submit" variant="default">{mutation.isPending ? "正在登录…" : "登录"}</Button>
        <div className="auth-links"><Link to="/forgot-password">忘记密码</Link><Link to="/register">创建账户</Link></div>
      </form>
    </AuthFrame>
  )
}

export function RegisterPage() {
  const mutation = useMutation({ mutationFn: (values: Credentials) => api.register({ ...values, timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || "Asia/Shanghai" }) })
  const { register, handleSubmit, formState: { errors } } = useForm<Credentials>({ resolver: zodResolver(credentialsSchema) })
  return (
    <AuthFrame title="创建账户" subtitle="邮箱验证后即可开始记录个人债务。">
      {mutation.isSuccess ? (
        <div className="auth-success"><CheckCircle2Icon /><h2>验证邮件已生成</h2><p>{mutation.data.message}</p><Link className="button button-default button-default" to="/login">返回登录</Link></div>
      ) : (
        <form className="auth-form" noValidate onSubmit={handleSubmit((values) => mutation.mutate(values))}>
          <Field label="邮箱" error={errors.email?.message}><Input aria-invalid={Boolean(errors.email)} autoComplete="email" type="email" {...register("email")} /></Field>
          <Field label="密码" error={errors.password?.message}><PasswordInput aria-invalid={Boolean(errors.password)} aria-label="密码" autoComplete="new-password" {...register("password")} /></Field>
          <p className="field-help">至少 12 个字符。注册后需通过邮件链接验证。</p>
          {mutation.error ? <InlineNotice type="error">{mutation.error.message}</InlineNotice> : null}
          <Button disabled={mutation.isPending} type="submit" variant="default">{mutation.isPending ? "正在创建…" : "创建账户"}</Button>
          <div className="auth-links single"><Link to="/login"><ArrowLeftIcon /> 返回登录</Link></div>
        </form>
      )}
    </AuthFrame>
  )
}

export function VerifyEmailPage() {
  const [params] = useSearchParams()
  const token = params.get("token") || ""
  const mutation = useMutation({ mutationFn: () => api.verifyEmail(token) })
  useEffect(() => { if (token && mutation.isIdle) mutation.mutate() }, [token, mutation])
  return (
    <AuthFrame title="验证邮箱" subtitle="正在确认你的账户邮箱。">
      <div className="auth-success">
        {mutation.isPending ? <div className="spinner" /> : <CheckCircle2Icon />}
        <h2>{!token ? "链接不完整" : mutation.isPending ? "正在验证…" : mutation.isSuccess ? "验证成功" : "验证失败"}</h2>
        <p>{mutation.isSuccess ? mutation.data.message : mutation.error?.message || (!token ? "缺少验证 token" : "请稍候")}</p>
        {!mutation.isPending ? <Link className="button button-default button-default" to="/login">前往登录</Link> : null}
      </div>
    </AuthFrame>
  )
}

const emailSchema = z.object({ email: z.string().email("请输入正确的邮箱") })

export function ForgotPasswordPage() {
  const mutation = useMutation({ mutationFn: (email: string) => api.forgotPassword(email) })
  const { register, handleSubmit, formState: { errors } } = useForm<{ email: string }>({ resolver: zodResolver(emailSchema) })
  return (
    <AuthFrame title="找回密码" subtitle="我们会生成一封密码重置邮件。">
      <form className="auth-form" noValidate onSubmit={handleSubmit(({ email }) => mutation.mutate(email))}>
        <Field label="邮箱" error={errors.email?.message}><Input aria-invalid={Boolean(errors.email)} type="email" {...register("email")} /></Field>
        {mutation.isSuccess ? <InlineNotice type="success">{mutation.data.message}</InlineNotice> : null}
        {mutation.error ? <InlineNotice type="error">{mutation.error.message}</InlineNotice> : null}
        <Button disabled={mutation.isPending} type="submit" variant="default">发送重置邮件</Button>
        <div className="auth-links single"><Link to="/login"><ArrowLeftIcon /> 返回登录</Link></div>
      </form>
    </AuthFrame>
  )
}

export function ResetPasswordPage() {
  const [params] = useSearchParams()
  const token = params.get("token") || ""
  const schema = z.object({ password: z.string().min(12, "密码至少 12 个字符").max(128) })
  const mutation = useMutation({ mutationFn: (password: string) => api.resetPassword(token, password) })
  const { register, handleSubmit, formState: { errors } } = useForm<{ password: string }>({ resolver: zodResolver(schema) })
  return (
    <AuthFrame title="设置新密码" subtitle="重置后，其他设备上的旧会话会失效。">
      <form className="auth-form" noValidate onSubmit={handleSubmit(({ password }) => mutation.mutate(password))}>
        <Field label="新密码" error={errors.password?.message}><PasswordInput aria-invalid={Boolean(errors.password)} aria-label="新密码" autoComplete="new-password" {...register("password")} /></Field>
        {!token ? <InlineNotice type="error">重置链接不完整</InlineNotice> : null}
        {mutation.isSuccess ? <InlineNotice type="success">{mutation.data.message}</InlineNotice> : null}
        {mutation.error ? <InlineNotice type="error">{mutation.error.message}</InlineNotice> : null}
        <Button disabled={!token || mutation.isPending} type="submit" variant="default">保存新密码</Button>
        <div className="auth-links single"><Link to="/login"><ArrowLeftIcon /> 返回登录</Link></div>
      </form>
    </AuthFrame>
  )
}
