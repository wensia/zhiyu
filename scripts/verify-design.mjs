import { readFile, readdir } from "node:fs/promises"
import { createRequire } from "node:module"
import { dirname, join } from "node:path"

const webRequire = createRequire(new URL("../apps/web/package.json", import.meta.url))
const kilnTokensEntry = webRequire.resolve("kiln/tokens")
const kilnRoot = dirname(dirname(kilnTokensEntry))
const sourceRoot = new URL("../apps/web/src/", import.meta.url)
const failures = []

const contract = JSON.parse(await readFile(join(kilnRoot, "contract/tokens.json"), "utf8"))
const externalRuntimeTokens = ["--radix-select-trigger-width", "--radix-select-content-available-height"]
const allowed = new Set([...contract.tokens, ...Object.keys(contract.knownDeviations || {}), ...externalRuntimeTokens])
const tokenFiles = (await readdir(join(kilnRoot, "tokens")))
  .filter((name) => name.endsWith(".css") && !["index.css", "fonts.css"].includes(name))
const tokenCss = (await Promise.all(tokenFiles.map((name) => readFile(join(kilnRoot, "tokens", name), "utf8")))).join("\n")
const defined = new Set([...tokenCss.matchAll(/(--[a-z0-9\\.-]+)\s*:/gi)].map((match) => match[1].replaceAll("\\", "")))

for (const token of contract.tokens) {
  if (!defined.has(token)) failures.push(`Kiln contract token is missing from the pinned package: ${token}`)
}

const paletteClass = /\b(?:bg|text|border|ring|from|via|to)-(?:red|green|blue|amber|yellow|slate|gray|zinc|stone|purple)-\d{2,3}\b/g
const rawHex = /#[0-9a-fA-F]{3,8}\b/g

async function walk(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) {
      await walk(path)
      continue
    }
    if (!/\.(?:css|tsx?|html)$/.test(entry.name)) continue
    const contents = await readFile(path, "utf8")
    for (const match of contents.matchAll(paletteClass)) failures.push(`${path}: Tailwind palette ${match[0]}`)
    if (!entry.name.includes("test")) {
      for (const match of contents.matchAll(rawHex)) failures.push(`${path}: raw hex ${match[0]}`)
      for (const match of contents.matchAll(/<select\b/g)) failures.push(`${path}: native select ${match[0]} (use the shared kiln Select)`)
      for (const match of contents.matchAll(/<Input\b[^>]*\btype=["']date["']/g)) failures.push(`${path}: native date input ${match[0]} (use the shared Kiln DatePicker)`)
      if (!path.endsWith(join("components", "ui.tsx")) && /@radix-ui\/react-(?:select|dropdown-menu|tabs|dialog|alert-dialog)/.test(contents)) {
        failures.push(`${path}: Radix primitive imported outside the shared UI layer`)
      }
    }
    if (entry.name.endsWith(".css")) {
      for (const match of contents.matchAll(/box-shadow\s*:\s*([^;]+)/g)) {
        if (!match[1].trim().startsWith("var(")) failures.push(`${path}: raw box-shadow ${match[0]}`)
      }
    }
    for (const match of contents.matchAll(/var\((--[a-z0-9.-]+)/gi)) {
      if (!allowed.has(match[1])) failures.push(`${path}: unknown design token ${match[1]}`)
    }
  }
}

await walk(sourceRoot.pathname)
const mainSource = await readFile(new URL("../apps/web/src/main.tsx", import.meta.url), "utf8")
if (!mainSource.includes('"kiln/tokens"') || !mainSource.includes('"kiln/tokens/fonts.css"')) {
  failures.push("apps/web/src/main.tsx must import the pinned Kiln tokens and Noto Sans SC webfont")
}

if (failures.length) {
  console.error(failures.join("\n"))
  process.exit(1)
}

console.log(`Kiln token contract: OK (${contract.tokens.length} pinned tokens, no local design-value drift)`)
