#!/usr/bin/env bash
# 知余 dev 环境启停脚本
# 用法:
#   scripts/dev.sh start    启动 API (8790) + Web (5173)
#   scripts/dev.sh stop     停止所有 dev 进程
#   scripts/dev.sh restart  先停再启
#   scripts/dev.sh status   查看运行状态

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
API_PORT=8790
WEB_PORT=5173
LAUNCHD_LABEL="com.zhiyu.dev"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
NC='\033[0m'

info()  { echo -e "${BLUE}[知余]${NC} $*"; }
ok()    { echo -e "${GREEN}[知余]${NC} $*"; }
warn()  { echo -e "${YELLOW}[知余]${NC} $*"; }
err()   { echo -e "${RED}[知余]${NC} $*"; }

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

port_pid() {
  lsof -ti ":$1" 2>/dev/null || true
}

port_owner() {
  lsof -i ":$1" 2>/dev/null | tail -n +2 | awk '{print $1, $2}' || true
}

is_running() {
  local port=$1
  [[ -n "$(port_pid "$port")" ]]
}

# Kill all zhiyu dev process trees, including orphaned ones respawned by
# concurrently / pnpm dev, and any launchd supervisor.
kill_all() {
  local killed=0

  # 1. Stop launchd supervisor if present (it respawns everything).
  if launchctl list 2>/dev/null | grep -q "$LAUNCHD_LABEL"; then
    info "停止 launchd 服务 $LAUNCHD_LABEL"
    launchctl bootout "gui/$(id -u)/$LAUNCHD_LABEL" 2>/dev/null || true
    killed=1
  fi

  # 2. Kill all zhiyu-related dev root processes (pnpm dev, concurrently).
  # Skip this script and its caller: a shell invoked as `dev.sh stop; pnpm dev`
  # carries "pnpm dev" in its own command line and would otherwise kill itself.
  local pids
  pids=$(ps aux | grep -E "pnpm dev|concurrently.*zhiyu" | grep -v grep | awk '{print $2}' \
    | grep -vxE "$$|$PPID" || true)
  if [[ -n "$pids" ]]; then
    echo "$pids" | xargs kill -9 2>/dev/null || true
    killed=1
  fi

  # 3. Kill whatever is on the dev ports (catches stragglers).
  local port_pids
  port_pids=$(lsof -ti ":$API_PORT" -ti ":$WEB_PORT" 2>/dev/null || true)
  if [[ -n "$port_pids" ]]; then
    echo "$port_pids" | xargs kill -9 2>/dev/null || true
    killed=1
  fi

  # 4. Kill zhiyu-api binary.
  pkill -f "zhiyu-api" 2>/dev/null && killed=1 || true

  if [[ "$killed" -eq 1 ]]; then
    sleep 2
  fi
}

wait_for_port() {
  local port=$1
  local name=$2
  local max_wait=${3:-30}
  local waited=0

  while [[ $waited -lt $max_wait ]]; do
    if is_running "$port"; then
      ok "$name 已就绪 (port $port)"
      return 0
    fi
    sleep 1
    waited=$((waited + 1))
  done

  err "$name 启动超时 (port $port, ${max_wait}s)"
  return 1
}

# ---------------------------------------------------------------------------
# Commands
# ---------------------------------------------------------------------------

cmd_stop() {
  info "停止知余 dev 环境..."

  if ! is_running "$API_PORT" && ! is_running "$WEB_PORT"; then
    # Check for orphaned processes even if ports are free
    local orphans
    orphans=$(ps aux | grep -E "pnpm dev|concurrently.*zhiyu" | grep -v grep | awk '{print $2}' || true)
    if [[ -z "$orphans" ]] && ! launchctl list 2>/dev/null | grep -q "$LAUNCHD_LABEL"; then
      ok "没有运行中的知余 dev 进程"
      return 0
    fi
  fi

  kill_all

  # Verify
  if is_running "$API_PORT" || is_running "$WEB_PORT"; then
    err "端口仍被占用，请手动检查:"
    lsof -i ":$API_PORT" -i ":$WEB_PORT" 2>/dev/null || true
    return 1
  fi

  ok "已停止"
}

cmd_start() {
  info "启动知余 dev 环境..."

  # Clean slate first
  if is_running "$API_PORT" || is_running "$WEB_PORT"; then
    warn "检测到已有进程占用端口，先清理..."
    kill_all
  fi

  # Double-check ports are free
  if is_running "$API_PORT" || is_running "$WEB_PORT"; then
    err "端口仍被占用:"
    lsof -i ":$API_PORT" -i ":$WEB_PORT" 2>/dev/null || true
    return 1
  fi

  cd "$PROJECT_ROOT"
  info "启动 API (port $API_PORT) + Web (port $WEB_PORT)..."
  pnpm dev &

  wait_for_port "$WEB_PORT" "Web" 30
  wait_for_port "$API_PORT" "API" 60

  echo ""
  ok "知余 dev 环境已启动"
  info "  Web:  http://127.0.0.1:$WEB_PORT"
  info "  API:  http://127.0.0.1:$API_PORT"
}

cmd_status() {
  local api_ok=0 web_ok=0

  if is_running "$WEB_PORT"; then
    ok "Web  http://127.0.0.1:$WEB_PORT  $(port_owner "$WEB_PORT" | head -1)"
    web_ok=1
  else
    warn "Web  (port $WEB_PORT) 未运行"
  fi

  if is_running "$API_PORT"; then
    ok "API  http://127.0.0.1:$API_PORT  $(port_owner "$API_PORT" | head -1)"
    api_ok=1
  else
    warn "API  (port $API_PORT) 未运行"
  fi

  if launchctl list 2>/dev/null | grep -q "$LAUNCHD_LABEL"; then
    warn "launchd 服务 $LAUNCHD_LABEL 存在（可能会自动重启进程）"
  fi

  if [[ "$api_ok" -eq 1 && "$web_ok" -eq 1 ]]; then
    return 0
  else
    return 1
  fi
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

case "${1:-}" in
  start)   cmd_start ;;
  stop)    cmd_stop ;;
  restart) cmd_stop && cmd_start ;;
  status)  cmd_status ;;
  *)
    echo "用法: $0 {start|stop|restart|status}"
    exit 1
    ;;
esac
