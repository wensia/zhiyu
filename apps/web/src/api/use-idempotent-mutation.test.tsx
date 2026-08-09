import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { act, renderHook, waitFor } from "@testing-library/react"
import type { ReactNode } from "react"
import { describe, expect, it } from "vitest"

import { ApiClientError } from "./client"
import { useIdempotentMutation } from "./use-idempotent-mutation"

const wrapper = ({ children }: { children: ReactNode }) => {
  const client = new QueryClient({ defaultOptions: { mutations: { retry: false } } })
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>
}

describe("useIdempotentMutation", () => {
  it("失败后重试沿用同一把幂等键，服务端才认得出是同一笔", async () => {
    const keys: string[] = []
    let shouldFail = true
    const { result } = renderHook(
      () =>
        useIdempotentMutation({
          mutationFn: async (_v: void, write) => {
            keys.push(write.idempotencyKey!)
            if (shouldFail) throw new ApiClientError(503, { code: "unavailable" })
            return "ok"
          },
        }),
      { wrapper },
    )

    await act(async () => {
      result.current.mutate()
    })
    await waitFor(() => expect(result.current.isError).toBe(true))

    shouldFail = false
    await act(async () => {
      result.current.mutate()
    })
    await waitFor(() => expect(result.current.isSuccess).toBe(true))

    expect(keys).toHaveLength(2)
    expect(keys[0]).toBe(keys[1])
  })

  it("成功后清空键，下一次提交是新意图", async () => {
    const keys: string[] = []
    const { result } = renderHook(
      () =>
        useIdempotentMutation({
          mutationFn: async (_v: void, write) => {
            keys.push(write.idempotencyKey!)
            return "ok"
          },
        }),
      { wrapper },
    )

    await act(async () => {
      result.current.mutate()
    })
    await waitFor(() => expect(result.current.isSuccess).toBe(true))
    await act(async () => {
      result.current.mutate()
    })
    await waitFor(() => expect(keys).toHaveLength(2))

    expect(keys[0]).not.toBe(keys[1])
  })

  it("服务端报 idempotency_mismatch 时换新键重试一次", async () => {
    const keys: string[] = []
    const { result } = renderHook(
      () =>
        useIdempotentMutation({
          mutationFn: async (_v: void, write) => {
            keys.push(write.idempotencyKey!)
            // 首次调用用的键被判定为「已用于不同请求」，模拟用户改了表单内容后重新提交。
            if (keys.length === 1) throw new ApiClientError(409, { code: "idempotency_mismatch" })
            return "ok"
          },
        }),
      { wrapper },
    )

    await act(async () => {
      result.current.mutate()
    })
    await waitFor(() => expect(result.current.isSuccess).toBe(true))

    expect(keys).toHaveLength(2)
    expect(keys[0]).not.toBe(keys[1])
  })

  it("非幂等冲突的错误照常抛出，不吞掉也不重试", async () => {
    const keys: string[] = []
    const { result } = renderHook(
      () =>
        useIdempotentMutation({
          mutationFn: async (_v: void, write) => {
            keys.push(write.idempotencyKey!)
            throw new ApiClientError(409, { code: "version_conflict" })
          },
        }),
      { wrapper },
    )

    await act(async () => {
      result.current.mutate()
    })
    await waitFor(() => expect(result.current.isError).toBe(true))

    expect(keys).toHaveLength(1)
    expect((result.current.error as ApiClientError).code).toBe("version_conflict")
  })
})
