import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { StrictMode } from "react"
import { createRoot } from "react-dom/client"
import { BrowserRouter } from "react-router-dom"

import "kiln/tokens"
import "kiln/tokens/fonts.css"
import "react-grid-layout/css/styles.css"
import "./styles.css"
import App from "./App"
import { ApiClientError } from "./api/client"
import { AppToastProvider } from "./components/ui"

/**
 * 只重试「再试一次可能会好」的失败。
 *
 * React Query 默认无差别重试 3 次，退避 1s + 2s + 4s。对确定性失败这 7 秒纯属白等：
 * 期间 `isLoading` 一直为 true，页面卡在骨架屏上，用户看不到任何原因——桌面端连着
 * 缺少某个路由的服务端时就是这个症状，等满 7 秒才浮出「该端点不存在」。
 *
 * 不重试：契约错误（`invalid_response`，服务端根本没返回 JSON）、以及除 408/429
 * 之外的 4xx（请求本身有问题，重发不会变对）。
 * 重试：5xx 与网络中断（`ApiNetworkError` 的 status 是 0），这些确实可能是暂时的。
 */
function shouldRetry(failureCount: number, error: unknown): boolean {
  if (error instanceof ApiClientError) {
    if (error.code === "invalid_response") return false
    if (error.status >= 400 && error.status < 500 && error.status !== 408 && error.status !== 429) {
      return false
    }
  }
  return failureCount < 3
}

const queryClient = new QueryClient({
  defaultOptions: {
    queries: { staleTime: 30_000, refetchOnWindowFocus: false, retry: shouldRetry },
    mutations: { retry: false },
  },
})

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <AppToastProvider>
        <BrowserRouter>
          <App />
        </BrowserRouter>
      </AppToastProvider>
    </QueryClientProvider>
  </StrictMode>,
)
