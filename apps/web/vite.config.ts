import { defineConfig } from "vitest/config"
import react from "@vitejs/plugin-react"
import type { ProxyOptions } from "vite"

import { BRAND_ASSETS } from "./src/brand-assets"

const apiProxyTarget = process.env.API_PROXY || "http://127.0.0.1:8787"
const apiProxyOrigin = new URL(apiProxyTarget).origin

function createApiProxy(): ProxyOptions {
  return {
    target: apiProxyTarget,
    changeOrigin: true,
    configure(proxy) {
      if (!apiProxyTarget.startsWith("https://")) return

      // Production uses a Secure __Host- cookie. The desktop dev page is served from
      // loopback HTTP, so keep a loopback cookie in WKWebView and restore the production
      // cookie name only while proxying requests upstream. Unsafe API requests must also
      // present the upstream origin because the API deliberately rejects the loopback
      // page origin as a CSRF defense.
      proxy.on("proxyReq", (proxyRequest, request) => {
        if (request.headers.origin) proxyRequest.setHeader("origin", apiProxyOrigin)
        const cookie = request.headers.cookie?.replace(
          /(^|;\s*)zhiyu_session=/g,
          "$1__Host-zhiyu_session=",
        )
        if (cookie) proxyRequest.setHeader("cookie", cookie)
      })
      proxy.on("proxyRes", (proxyResponse) => {
        const setCookie = proxyResponse.headers["set-cookie"]
        if (!setCookie) return
        proxyResponse.headers["set-cookie"] = setCookie.map((cookie) =>
          cookie
            .replace(/^__Host-zhiyu_session=/, "zhiyu_session=")
            .replace(/;\s*Secure/gi, ""),
        )
      })
    },
  }
}

export default defineConfig({
  plugins: [
    react(),
    {
      name: "zhiyu-brand-assets",
      transformIndexHtml: (html) => html.replaceAll("%BRAND_FAVICON_URL%", BRAND_ASSETS.faviconUrl),
    },
  ],
  server: {
    port: Number(process.env.WEB_PORT || 5173),
    strictPort: true,
    proxy: {
      "/api": createApiProxy(),
      "/health": createApiProxy(),
      "/desktop/handoff": createApiProxy(),
    },
  },
  test: {
    environment: "jsdom",
    setupFiles: "./src/test/setup.ts",
    exclude: ["e2e/**", "node_modules/**", "dist/**"],
  },
})
