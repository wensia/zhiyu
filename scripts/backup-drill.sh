#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
drill_root="$(mktemp -d "${TMPDIR:-/tmp}/zhiyu-backup-drill.XXXXXX")"
trap 'rm -rf "$drill_root"' EXIT

echo "备份恢复演练目录：$drill_root"
cargo run --quiet --manifest-path "$repo_root/Cargo.toml" -p zhiyu-api --bin backup_drill -- "$drill_root/run"
