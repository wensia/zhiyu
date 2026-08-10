import { afterEach, describe, expect, it, vi } from "vitest"

import { ApiClientError, ApiNetworkError, api } from "./client"

afterEach(() => vi.unstubAllGlobals())

describe("请求失败的错误映射", () => {
  it("把 fetch 的原生英文网络错误换成中文，并保留原始 cause", async () => {
    // WKWebView 在连接被拒时抛的就是这个：TypeError("Load failed")。
    const cause = new TypeError("Load failed")
    vi.stubGlobal("fetch", vi.fn().mockRejectedValue(cause))

    const failure = await api.ledgerAccounts().catch((error: unknown) => error)

    expect(failure).toBeInstanceOf(ApiNetworkError)
    expect((failure as ApiNetworkError).message).toContain("无法连接服务器")
    expect((failure as ApiNetworkError).message).not.toContain("Load failed")
    expect((failure as ApiNetworkError).status).toBe(0)
    expect((failure as ApiNetworkError).code).toBe("network_unreachable")
    expect((failure as ApiNetworkError).cause).toBe(cause)
  })

  it("服务器返回的错误响应仍走 ApiClientError，不被网络分支吞掉", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        new Response(JSON.stringify({ code: "version_conflict", message: "记录已在其他设备更新" }), {
          status: 409,
          headers: { "Content-Type": "application/json" },
        }),
      ),
    )

    const failure = await api.ledgerAccounts().catch((error: unknown) => error)

    expect(failure).toBeInstanceOf(ApiClientError)
    expect(failure).not.toBeInstanceOf(ApiNetworkError)
    expect((failure as ApiClientError).status).toBe(409)
    expect((failure as ApiClientError).message).toBe("记录已在其他设备更新")
  })
})
