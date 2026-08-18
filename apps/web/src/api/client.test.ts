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
    expect((failure as ApiNetworkError).message).toContain("无法连接服务")
    expect((failure as ApiNetworkError).message).toContain("检查后端是否运行")
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

describe("请求 body headers", () => {
  it("JSON body 自动设置 application/json 并传递幂等键", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify({}), { status: 200 }))
    vi.stubGlobal("fetch", fetchMock)

    await api.createTransaction({} as never, { idempotencyKey: "json-key" })

    const init = fetchMock.mock.calls[0][1] as RequestInit
    expect(init.headers).toMatchObject({ "Content-Type": "application/json", "Idempotency-Key": "json-key" })
  })

  it("FormData 不设置 Content-Type，由浏览器生成 multipart boundary", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify({}), { status: 201 }))
    vi.stubGlobal("fetch", fetchMock)

    await api.uploadImport(
      { file: new File(["safe fixture"], "bill.csv"), channel: "alipay" },
      { idempotencyKey: "upload-key" },
    )

    const init = fetchMock.mock.calls[0][1] as RequestInit
    expect(init.body).toBeInstanceOf(FormData)
    expect(init.headers).toEqual({ "Idempotency-Key": "upload-key" })
    expect(new Headers(init.headers).has("Content-Type")).toBe(false)
  })

  it("面板整体替换使用 PUT、JSON body 与调用方幂等键", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify({}), { status: 200 }))
    vi.stubGlobal("fetch", fetchMock)

    await api.replaceDashboardWidgets(
      "dashboard-1",
      [
        {
          widgetType: "core:category-share",
          pluginId: null,
          x: 8,
          y: 0,
          w: 4,
          h: 4,
          config: {},
        },
      ],
      { idempotencyKey: "dashboard-widgets-key" },
    )

    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/dashboards/dashboard-1/widgets",
      expect.objectContaining({
        method: "PUT",
        headers: {
          "Content-Type": "application/json",
          "Idempotency-Key": "dashboard-widgets-key",
        },
      }),
    )
  })
})

describe("统计聚合 query", () => {
  it("序列化分组与可选筛选条件", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(JSON.stringify([]), { status: 200 }))
    vi.stubGlobal("fetch", fetchMock)

    await api.statisticsAggregate({
      from: "2026-01-01",
      to: "2026-02-01",
      groupBy: "category",
      accountId: "account-1",
      categoryId: "category-1",
      kind: "expense",
    })

    expect(fetchMock.mock.calls[0][0]).toBe(
      "/api/v1/statistics/aggregate?from=2026-01-01&to=2026-02-01&groupBy=category&accountId=account-1&categoryId=category-1&kind=expense",
    )
  })
})

describe("成功响应但 body 不是 JSON", () => {
  it("200 + text/html 必须抛错，不能静默当成空对象", async () => {
    // 桌面端 dev 把 /api/* 代理到线上；线上没有这个路由时，请求落到 SPA
    // 的 catch-all，回来的是 200 + index.html。旧实现 `.catch(() => ({}))`
    // 把它当成成功的空对象放行，调用方拿到 {} 而不是数组，直到渲染时
    // 才以 `categories.flatMap is not a function` 炸掉，整页白屏。
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(
        new Response("<!doctype html><html><body>SPA fallback</body></html>", {
          status: 200,
          headers: { "Content-Type": "text/html" },
        }),
      ),
    )

    const failure = await api.categories().catch((error: unknown) => error)

    expect(failure).toBeInstanceOf(ApiClientError)
    expect((failure as ApiClientError).code).toBe("invalid_response")
  })
})
