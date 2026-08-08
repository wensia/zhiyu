import { defineConfig } from "vitest/config"
import react from "@vitejs/plugin-react"

import { BRAND_ASSETS } from "./src/brand-assets"

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
      "/api": process.env.API_PROXY || "http://127.0.0.1:8787",
      "/health": process.env.API_PROXY || "http://127.0.0.1:8787",
    },
  },
  test: {
    environment: "jsdom",
    setupFiles: "./src/test/setup.ts",
    exclude: ["e2e/**", "node_modules/**", "dist/**"],
  },
})
