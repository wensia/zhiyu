#!/usr/bin/env node

import { spawn } from "node:child_process"
import { readFile } from "node:fs/promises"
import { createServer } from "node:net"
import { homedir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const command = process.argv[2]

if (command !== "dev") {
  console.error("用法: pnpm tauri dev")
  process.exit(1)
}

const projectRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const webDirectory = join(projectRoot, "apps", "web")
const desktopDirectory = join(projectRoot, "apps", "desktop")
const viteExecutable = join(
  webDirectory,
  "node_modules",
  ".bin",
  process.platform === "win32" ? "vite.cmd" : "vite",
)
const connectionPath = join(
  homedir(),
  "Library",
  "Application Support",
  "app.zhiyu.desktop",
  "backup-client.json",
)

function parseServerUrl(value) {
  if (typeof value !== "string" || value.length === 0) {
    throw new Error("连接配置缺少 serverUrl")
  }

  const url = new URL(value)
  const isLoopbackHttp = url.protocol === "http:" && url.hostname === "127.0.0.1"
  if (url.protocol !== "https:" && !isLoopbackHttp) {
    throw new Error("服务器必须使用 HTTPS；本地开发只允许 http://127.0.0.1")
  }
  if (url.username || url.password || url.search || url.hash) {
    throw new Error("服务器地址不能包含凭据、查询参数或片段")
  }

  return url.toString().replace(/\/$/, "")
}

async function loadSavedServerUrl() {
  let contents
  try {
    contents = await readFile(connectionPath, "utf8")
  } catch (error) {
    if (error?.code === "ENOENT") {
      throw new Error("尚未保存桌面连接信息，请先在连接设置中保存服务器地址和 api-key")
    }
    throw error
  }

  let connection
  try {
    connection = JSON.parse(contents)
  } catch {
    throw new Error(`连接配置不是有效 JSON：${connectionPath}`)
  }

  return parseServerUrl(connection.serverUrl)
}

function canListen(port) {
  return new Promise((resolveAvailable) => {
    const server = createServer()
    server.unref()
    server.once("error", () => resolveAvailable(false))
    server.listen({ host: "127.0.0.1", port }, () => {
      server.close(() => resolveAvailable(true))
    })
  })
}

async function findAvailablePort() {
  for (let port = 5173; port <= 5193; port += 1) {
    if (await canListen(port)) return port
  }
  throw new Error("5173-5193 端口均被占用，无法启动本地前端")
}

function startProcess(commandName, args, options) {
  return spawn(commandName, args, {
    ...options,
    detached: process.platform !== "win32",
    stdio: "inherit",
  })
}

function childExit(child, name) {
  if (child.exitCode !== null || child.signalCode !== null) {
    return Promise.resolve({ name, code: child.exitCode, signal: child.signalCode })
  }
  return new Promise((resolveExit, rejectExit) => {
    child.once("error", (error) => rejectExit(new Error(`${name} 启动失败：${error.message}`)))
    child.once("exit", (code, signal) => resolveExit({ name, code, signal }))
  })
}

async function waitForHttp(url, webExit) {
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    const result = await Promise.race([
      fetch(url)
        .then((response) => ({ type: "response", ok: response.ok }))
        .catch(() => ({ type: "response", ok: false })),
      webExit.then((exit) => ({ type: "exit", exit })),
    ])

    if (result.type === "exit") {
      throw new Error(`Vite 在就绪前退出（code=${result.exit.code}, signal=${result.exit.signal}）`)
    }
    if (result.ok) return
    await new Promise((resolveWait) => setTimeout(resolveWait, 250))
  }
  throw new Error(`等待本地前端超时：${url}`)
}

function stopProcess(child, signal = "SIGTERM") {
  if (
    !child ||
    !Number.isInteger(child.pid) ||
    child.exitCode !== null ||
    child.signalCode !== null
  ) {
    return
  }
  try {
    if (process.platform === "win32") child.kill(signal)
    else process.kill(-child.pid, signal)
  } catch (error) {
    if (error?.code !== "ESRCH") throw error
  }
}

const children = []
let shuttingDown = false

async function shutdown(exitCode, signal = "SIGTERM") {
  if (shuttingDown) return
  shuttingDown = true
  for (const child of children) stopProcess(child, signal)

  const forceTimer = setTimeout(() => {
    for (const child of children) stopProcess(child, "SIGKILL")
  }, 3_000)
  forceTimer.unref()

  await Promise.allSettled(children.map((child) => childExit(child, "子进程")))
  process.exit(exitCode)
}

process.once("SIGINT", () => void shutdown(0, "SIGTERM"))
process.once("SIGTERM", () => void shutdown(143, "SIGTERM"))
// 关掉终端窗口发来的是 SIGHUP，node 默认直接退出、不跑上面的清理。而子进程是 detached
// 的（独立进程组，这样才能连子树一起杀），于是 cargo tauri 活了下来变成孤儿：窗口还开着，
// Vite 已经没了，界面上每个请求都连不上。接住它，走正常关停。
process.once("SIGHUP", () => void shutdown(129, "SIGTERM"))

try {
  const serverUrl = await loadSavedServerUrl()
  const webPort = await findAvailablePort()
  const localUrl = `http://127.0.0.1:${webPort}`

  console.log(`[知余] 本地界面：${localUrl}`)
  console.log(`[知余] API 代理：${serverUrl}`)

  const web = startProcess(viteExecutable, ["--host", "127.0.0.1"], {
    cwd: webDirectory,
    env: {
      ...process.env,
      API_PROXY: serverUrl,
      WEB_PORT: String(webPort),
    },
  })
  children.push(web)
  const webExit = childExit(web, "Vite")
  await waitForHttp(localUrl, webExit)

  const desktop = startProcess(
    "cargo",
    ["tauri", "dev", "--config", "src-tauri/tauri.conf.json"],
    {
      cwd: desktopDirectory,
      env: {
        ...process.env,
        ZHIYU_DESKTOP_URL: localUrl,
      },
    },
  )
  children.push(desktop)

  const exit = await Promise.race([webExit, childExit(desktop, "Tauri")])
  if (!shuttingDown) {
    const code = exit.code ?? 1
    console.error(`[知余] ${exit.name} 已退出（code=${exit.code}, signal=${exit.signal}）`)
    await shutdown(code)
  }
} catch (error) {
  console.error(`[知余] 启动失败：${error.message}`)
  await shutdown(1)
}
