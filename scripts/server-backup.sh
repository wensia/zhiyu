#!/usr/bin/env bash
set -euo pipefail

readonly PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
readonly DATABASE_PATH="${ZHIYU_DATABASE_PATH:-/data/preview.db}"
readonly SNAPSHOT_DIR="/data/backups"
readonly SNAPSHOT_PATH="${SNAPSHOT_DIR}/ledger.sqlite3"
readonly MANIFEST_PATH="${SNAPSHOT_DIR}/manifest.json"
readonly RESTIC_BACKUP_PATH="/opt/zhiyu/data/backups"
readonly STARTED_AT="$(date -u +'%Y-%m-%dT%H:%M:%SZ')"
readonly TEMP_SUFFIX="${STARTED_AT//[:\-]/}-$$"
readonly SNAPSHOT_TEMP="${SNAPSHOT_DIR}/.ledger.sqlite3.${TEMP_SUFFIX}.tmp"
readonly MANIFEST_TEMP="${SNAPSHOT_DIR}/.manifest.json.${TEMP_SUFFIX}.tmp"

log() {
  printf '%s [zhiyu-backup] %s\n' "$(date -u +'%Y-%m-%dT%H:%M:%SZ')" "$*" >&2
}

fail() {
  log "ERROR: $*"
  exit 1
}

cleanup() {
  rm -f -- "${SNAPSHOT_TEMP}" "${MANIFEST_TEMP}"
}

on_error() {
  local exit_code=$?
  log "ERROR: backup failed at line ${BASH_LINENO[0]} (exit ${exit_code})"
  exit "${exit_code}"
}

trap cleanup EXIT
trap on_error ERR

for command in sqlite3 restic sha256sum stat; do
  command -v "${command}" >/dev/null 2>&1 || fail "required command not found: ${command}"
done
[[ -f "${DATABASE_PATH}" ]] || fail "database not found: ${DATABASE_PATH}"
[[ -n "${RESTIC_REPOSITORY:-}" ]] || fail "RESTIC_REPOSITORY is required (use the Tencent COS S3 endpoint)"
[[ -n "${AWS_ACCESS_KEY_ID:-}" ]] || fail "AWS_ACCESS_KEY_ID is required"
[[ -n "${AWS_SECRET_ACCESS_KEY:-}" ]] || fail "AWS_SECRET_ACCESS_KEY is required"
if [[ -z "${RESTIC_PASSWORD:-}" && -z "${RESTIC_PASSWORD_FILE:-}" && -z "${RESTIC_PASSWORD_COMMAND:-}" ]]; then
  fail "RESTIC_PASSWORD, RESTIC_PASSWORD_FILE, or RESTIC_PASSWORD_COMMAND is required"
fi

log "creating SQLite snapshot from ${DATABASE_PATH}"
mkdir -p -- "${SNAPSHOT_DIR}"
sqlite3 -batch -bail "${DATABASE_PATH}" "VACUUM INTO '${SNAPSHOT_TEMP}';"

log "running SQLite integrity checks"
integrity_result="$(sqlite3 -batch -bail "${SNAPSHOT_TEMP}" 'PRAGMA integrity_check;')"
[[ "${integrity_result}" == "ok" ]] || fail "integrity_check failed: ${integrity_result}"
foreign_key_result="$(sqlite3 -batch -bail "${SNAPSHOT_TEMP}" 'PRAGMA foreign_key_check;')"
[[ -z "${foreign_key_result}" ]] || fail "foreign_key_check failed: ${foreign_key_result}"

snapshot_sha256="$(sha256sum "${SNAPSHOT_TEMP}" | awk '{print $1}')"
snapshot_bytes="$(stat -c '%s' "${SNAPSHOT_TEMP}")"
printf '{\n  "created_at": "%s",\n  "source": "%s",\n  "snapshot": "%s",\n  "sha256": "%s",\n  "bytes": %s,\n  "integrity_check": "ok",\n  "foreign_key_check": "ok"\n}\n' \
  "${STARTED_AT}" "${DATABASE_PATH}" "${SNAPSHOT_PATH}" "${snapshot_sha256}" "${snapshot_bytes}" >"${MANIFEST_TEMP}"
mv -f -- "${SNAPSHOT_TEMP}" "${SNAPSHOT_PATH}"
mv -f -- "${MANIFEST_TEMP}" "${MANIFEST_PATH}"

[[ -r "${RESTIC_BACKUP_PATH}/ledger.sqlite3" && -r "${RESTIC_BACKUP_PATH}/manifest.json" ]] || \
  fail "${RESTIC_BACKUP_PATH} must expose the snapshots written under ${SNAPSHOT_DIR}"

log "uploading ${RESTIC_BACKUP_PATH} to restic repository"
restic backup --no-scan "${RESTIC_BACKUP_PATH}"
log "applying restic retention policy"
restic forget --keep-daily 7 --keep-weekly 4 --keep-monthly 6 --prune
log "backup completed successfully (sha256=${snapshot_sha256}, bytes=${snapshot_bytes})"
