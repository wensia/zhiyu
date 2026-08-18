#!/usr/bin/env node

import { spawn } from "node:child_process"
import { readFile } from "node:fs/promises"
import { createConnection, createServer } from "node:net"
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
const localApiUrl = "http://127.0.0.1:8790"
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

function useRemoteServer() {
  const value = process.env.ZHIYU_DESKTOP_REMOTE?.trim().toLowerCase()
  return value === "1" || value === "true"
}

function canConnect(port) {
  return new Promise((resolveConnected) => {
    const socket = createConnection({ host: "127.0.0.1", port })
    let settled = false
    const finish = (connected) => {
      if (settled) return
      settled = true
      socket.destroy()
      resolveConnected(connected)
    }
    socket.setTimeout(1_000)
    socket.once("connect", () => finish(true))
    socket.once("error", () => finish(false))
    socket.once("timeout", () => finish(false))
  })
}

async function loadLocalApiKeyFile() {
  const configuredPath = process.env.ZHIYU_DESKTOP_API_KEY_FILE
  if (!configuredPath) {
    throw new Error(
      "本地模式需要 ZHIYU_DESKTOP_API_KEY_FILE。先执行：\n" +
        '  mkdir -p "$HOME/.config/zhiyu"\n' +
        '  DATABASE_URL=file:./var/preview.db cargo run -p zhiyu-api --bin zhiyu-api-key -- machine-user@example.com > "$HOME/.config/zhiyu/local-api-key"\n' +
        '  chmod 600 "$HOME/.config/zhiyu/local-api-key"\n' +
        '再执行：ZHIYU_DESKTOP_API_KEY_FILE="$HOME/.config/zhiyu/local-api-key" pnpm desktop:dev',
    )
  }

  const keyPath = resolve(process.cwd(), configuredPath)
  let apiKey
  try {
    apiKey = await readFile(keyPath, "utf8")
  } catch (error) {
    if (error?.code === "ENOENT") {
      throw new Error(
        `ZHIYU_DESKTOP_API_KEY_FILE 指向的文件不存在：${keyPath}\n` +
          "请先用本地库签发：\n" +
          '  mkdir -p "$HOME/.config/zhiyu"\n' +
          '  DATABASE_URL=file:./var/preview.db cargo run -p zhiyu-api --bin zhiyu-api-key -- machine-user@example.com > "$HOME/.config/zhiyu/local-api-key"\n' +
          '  chmod 600 "$HOME/.config/zhiyu/local-api-key"\n' +
          '然后设置：ZHIYU_DESKTOP_API_KEY_FILE="$HOME/.config/zhiyu/local-api-key"',
      )
    }
    throw new Error(`无法读取 ZHIYU_DESKTOP_API_KEY_FILE：${keyPath}（${error.message}）`)
  }
  if (!apiKey.trim()) {
    throw new Error(`ZHIYU_DESKTOP_API_KEY_FILE 不能为空：${keyPath}`)
  }
  return keyPath
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
    // ESRCH 是进程组已经没了；EPERM 是进程组还在但已不归我们管（外部 SIGTERM 收走
    // 整棵树时实测会命中）。两种都已经没有可杀的东西，却都会从这里抛出去——而
    // stopProcess 是在 shutdown 的循环里调用的，一抛就跳过后面的 SIGKILL 兜底和
    // 子进程回收，反而留下孤儿 cargo tauri dev，正是 ee67a78 要根除的症状。
    if (error?.code !== "ESRCH" && error?.code !== "EPERM") throw error
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
  const remote = useRemoteServer()
  let serverUrl
  let localApiKeyFile
  if (remote) {
    serverUrl = await loadSavedServerUrl()
    console.warn("\n!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!")
    console.warn("[知余] 警告：桌面 dev 正在连接线上 API！")
    console.warn("[知余] 前端的新端点如果服务端尚未部署会失败。")
    console.warn("!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!\n")
  } else {
    serverUrl = localApiUrl
    if (!(await canConnect(8790))) {
      throw new Error("本地 API 未运行（127.0.0.1:8790），请先在另一个终端运行 `pnpm dev`")
    }
    localApiKeyFile = await loadLocalApiKeyFile()
  }

  // 本地模式复用 `pnpm dev` 已经起好的那个 Vite，不再起第二个。
  //
  // 起第二个的代价不是多一个进程，是端口对不上：5173 被占后它顺延到 5174/5175，
  // 而本地 API 的 PUBLIC_BASE_URL 写死 5173，csrf_guard 比对 Origin 时端口不等
  // （origins_match 对 host 宽松、对端口严格），任何写操作都会 403
  // 「请求来源与服务端配置的不一致」。同源就没有这个问题。
  const reuseDevServer = !remote && (await canConnect(5173))
  const webPort = reuseDevServer ? 5173 : await findAvailablePort()
  const localUrl = `http://127.0.0.1:${webPort}`
  const desktopEnv = {
    ...process.env,
    ZHIYU_DESKTOP_URL: localUrl,
  }
  if (localApiKeyFile) {
    desktopEnv.ZHIYU_DESKTOP_API_KEY_FILE = localApiKeyFile
  } else {
    delete desktopEnv.ZHIYU_DESKTOP_API_KEY_FILE
  }

  console.log(`[知余] 本地界面：${localUrl}`)
  console.log(`[知余] API 代理：${serverUrl}`)

  let webExit = new Promise(() => {})
  if (reuseDevServer) {
    console.log("[知余] 复用 pnpm dev 的前端，未另起 Vite")
  } else {
    const web = startProcess(viteExecutable, ["--host", "127.0.0.1"], {
      cwd: webDirectory,
      env: {
        ...process.env,
        API_PROXY: serverUrl,
        WEB_PORT: String(webPort),
      },
    })
    children.push(web)
    webExit = childExit(web, "Vite")
  }
  await waitForHttp(localUrl, webExit)

  const desktop = startProcess(
    "cargo",
    ["tauri", "dev", "--config", "src-tauri/tauri.conf.json"],
    {
      cwd: desktopDirectory,
      env: desktopEnv,
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
