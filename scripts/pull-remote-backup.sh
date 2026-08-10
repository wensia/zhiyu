#!/usr/bin/env bash
#
# 把服务端已生成并校验过的数据库快照拉到本机，作为独立于桌面应用的异地副本。
#
# 为什么需要它：服务端每日快照只落在容器数据卷里，机器整体故障就一起没了；
# 桌面端虽然也会拉取，但那依赖应用正在运行。本脚本由 launchd 定时触发，
# 不需要打开任何应用。
#
# 与桌面端拉取相互独立、互不干扰：两者都只做只读下载，各自维护保留策略。
set -euo pipefail

SERVER_URL="${ZHIYU_SERVER_URL:?ZHIYU_SERVER_URL 未设置}"
API_KEY="${ZHIYU_API_KEY:?ZHIYU_API_KEY 未设置}"
DEST="${ZHIYU_BACKUP_DIR:-$HOME/zhiyu-backups/remote}"
RETENTION_DAYS="${ZHIYU_RETENTION_DAYS:-30}"

log() { printf '%s [zhiyu-pull] %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*"; }
fail() { log "ERROR: $*"; exit 1; }

for cmd in curl python3 shasum sqlite3; do
  command -v "$cmd" >/dev/null 2>&1 || fail "缺少命令：$cmd"
done

mkdir -p "$DEST"

listing="$(curl -fsS --max-time 30 -H "Authorization: Bearer $API_KEY" \
  "$SERVER_URL/api/v1/backups")" || fail "获取备份列表失败（服务器不可达或密钥无效）"

# 不用 mapfile：macOS 自带 bash 仍是 3.2，没有该内建命令，脚本会直接报
# "mapfile: command not found"。while read 在 3.2 与 5.x 上行为一致。
manifest="$(printf '%s' "$listing" | python3 -c '
import json, sys
for item in json.load(sys.stdin):
    print(item["id"], item["sha256"], item["size"])
')"

[ -n "$manifest" ] || fail "服务器没有可用快照"

downloaded=0
while read -r id sha size; do
  [ -z "$id" ] && continue
  # 文件名里的冒号在 macOS Finder 中会被显示成斜杠，换成安全形式
  safe="${id//:/-}"
  target="$DEST/zhiyu-$safe.db"
  [ -f "$target" ] && continue

  tmp="$target.partial"
  curl -fsS --max-time 300 -H "Authorization: Bearer $API_KEY" \
    "$SERVER_URL/api/v1/backups/$id" -o "$tmp" || { rm -f "$tmp"; fail "下载 $id 失败"; }

  actual_size="$(wc -c < "$tmp" | tr -d ' ')"
  [ "$actual_size" = "$size" ] || { rm -f "$tmp"; fail "$id 长度不符：期望 $size 实际 $actual_size"; }

  actual_sha="$(shasum -a 256 "$tmp" | cut -d' ' -f1)"
  [ "$actual_sha" = "$sha" ] || { rm -f "$tmp"; fail "$id 校验和不符"; }

  # 校验和相同也可能是一份内部损坏的库被完整传输过来，再过一次 SQLite 自检
  integrity="$(sqlite3 "$tmp" 'PRAGMA integrity_check;' 2>&1)"
  [ "$integrity" = "ok" ] || { rm -f "$tmp"; fail "$id 完整性检查失败：$integrity"; }

  mv -f "$tmp" "$target"
  downloaded=$((downloaded + 1))
  log "已拉取 ${id}（$actual_size 字节）"
done <<EOF
$manifest
EOF

# 保留策略：删除超期副本，但无论如何保留最新一份——磁盘写满或长期未运行时
# 不能把唯一的本地副本删光。
kept_latest="$(ls -t "$DEST"/zhiyu-*.db 2>/dev/null | head -1 || true)"
while IFS= read -r old; do
  [ -z "$old" ] && continue
  [ "$old" = "$kept_latest" ] && continue
  rm -f "$old"
  log "已清理超过 ${RETENTION_DAYS} 天的副本：$(basename "$old")"
done < <(find "$DEST" -name 'zhiyu-*.db' -type f -mtime "+${RETENTION_DAYS}" 2>/dev/null)

total="$(ls "$DEST"/zhiyu-*.db 2>/dev/null | wc -l | tr -d ' ')"
log "完成：本次新增 ${downloaded} 份，本地共 ${total} 份，目录 $DEST"
