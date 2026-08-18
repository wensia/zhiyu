import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { act, renderHook, waitFor } from "@testing-library/react"
import type { ReactNode } from "react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { importKeys, useCommitImport, useDiscardImport, useUploadImport } from "./imports"

afterEach(() => vi.unstubAllGlobals())

function setup() {
  const client = new QueryClient({ defaultOptions: { mutations: { retry: false } } })
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  )
  return { client, wrapper }
}

describe("imports mutations", () => {
  it("mismatch 换 key 重试时重新构造可发送的 FormData", async () => {
    const bodies: FormData[] = []
    const keys: string[] = []
    const fetchMock = vi.fn().mockImplementation(async (_url: string, init: RequestInit) => {
      bodies.push(init.body as FormData)
      keys.push(new Headers(init.headers).get("Idempotency-Key")!)
      if (bodies.length === 1) {
        return new Response(JSON.stringify({ code: "idempotency_mismatch" }), { status: 409 })
      }
      return new Response(JSON.stringify({ id: "batch-1" }), { status: 201 })
    })
    vi.stubGlobal("fetch", fetchMock)
    const { wrapper } = setup()
    const { result } = renderHook(() => useUploadImport(), { wrapper })
    const file = new File(["safe fixture"], "bill.csv")

    await act(async () => result.current.mutate({ file, channel: "alipay" }))
    await waitFor(() => expect(result.current.isSuccess).toBe(true))

    expect(bodies).toHaveLength(2)
    expect(bodies[0]).not.toBe(bodies[1])
    expect(bodies.map((body) => body.get("file"))).toEqual([file, file])
    expect(keys[0]).not.toBe(keys[1])
  })

  it.each([
    [useCommitImport, { id: "batch-1", input: { accountId: null } }],
    [useDiscardImport, "batch-1"],
  ] as const)("commit/discard 成功刷新 imports 与账本缓存", async (useImportWrite, input) => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(JSON.stringify({}), { status: 200 })))
    const { client, wrapper } = setup()
    const invalidate = vi.spyOn(client, "invalidateQueries")
    const { result } = renderHook(() => useImportWrite(), { wrapper })

    await act(async () => result.current.mutate(input as never))
    await waitFor(() => expect(result.current.isSuccess).toBe(true))

    for (const queryKey of [
      importKeys.all,
      ["transactions"],
      ["transaction-summary"],
      ["transaction-categories"],
      ["ledger-accounts"],
    ]) {
      expect(invalidate).toHaveBeenCalledWith({ queryKey })
    }
  })
})
