#!/usr/bin/env bash
set -euo pipefail

readonly DATABASE_PATH="${ZHIYU_DATABASE_PATH:-/opt/zhiyu/data/preview.db}"
# 该脚本由宿主机 ubuntu 用户执行。这里不用 /opt/zhiyu/data：该目录随容器挂载归
# nobody:nogroup 所有，ubuntu 无法可靠写入；用户主目录无需 sudo 且适合交给 restic。
readonly SNAPSHOT_DIR="${ZHIYU_BACKUP_DIR:-/home/ubuntu/zhiyu-backups}"
readonly SNAPSHOT_PATH="${SNAPSHOT_DIR}/ledger.sqlite3"
readonly MANIFEST_PATH="${SNAPSHOT_DIR}/manifest.json"
readonly STARTED_AT="$(date -u +'%Y-%m-%dT%H:%M:%SZ')"
readonly TEMP_SUFFIX="${STARTED_AT//[:\-]/}-$$"
readonly SNAPSHOT_TEMP="${SNAPSHOT_DIR}/.ledger.sqlite3.${TEMP_SUFFIX}.tmp"
readonly MANIFEST_TEMP="${SNAPSHOT_DIR}/.manifest.json.${TEMP_SUFFIX}.tmp"
readonly SQLITE_IMAGE="${ZHIYU_SQLITE_IMAGE:-alpine:latest}"
readonly SQLITE_CONTAINER_TIMEOUT_SECONDS=420

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

for command in docker restic sha256sum stat timeout; do
  command -v "${command}" >/dev/null 2>&1 || fail "required command not found: ${command}"
done
[[ -f "${DATABASE_PATH}" ]] || fail "database not found: ${DATABASE_PATH}"
[[ -r "${DATABASE_PATH}" ]] || fail "database is not readable: ${DATABASE_PATH}"
docker info >/dev/null 2>&1 || fail "Docker is unavailable or the current user cannot access it"
[[ -n "${RESTIC_REPOSITORY:-}" ]] || fail "RESTIC_REPOSITORY is required (use the Tencent COS S3 endpoint)"
[[ -n "${AWS_ACCESS_KEY_ID:-}" ]] || fail "AWS_ACCESS_KEY_ID is required"
[[ -n "${AWS_SECRET_ACCESS_KEY:-}" ]] || fail "AWS_SECRET_ACCESS_KEY is required"
if [[ -z "${RESTIC_PASSWORD:-}" && -z "${RESTIC_PASSWORD_FILE:-}" && -z "${RESTIC_PASSWORD_COMMAND:-}" ]]; then
  fail "RESTIC_PASSWORD, RESTIC_PASSWORD_FILE, or RESTIC_PASSWORD_COMMAND is required"
fi

mkdir -p -- "${SNAPSHOT_DIR}"
[[ -d "${SNAPSHOT_DIR}" && -w "${SNAPSHOT_DIR}" ]] || \
  fail "backup directory is not writable by $(id -un): ${SNAPSHOT_DIR}"

readonly DATABASE_DIR="$(dirname -- "${DATABASE_PATH}")"
readonly DATABASE_FILE="$(basename -- "${DATABASE_PATH}")"
readonly SNAPSHOT_TEMP_FILE="$(basename -- "${SNAPSHOT_TEMP}")"

log "creating SQLite snapshot from ${DATABASE_PATH} with ${SQLITE_IMAGE}"
if ! timeout "${SQLITE_CONTAINER_TIMEOUT_SECONDS}" docker run --rm \
  -e "DATABASE_FILE=${DATABASE_FILE}" \
  -e "SNAPSHOT_FILE=${SNAPSHOT_TEMP_FILE}" \
  -v "${DATABASE_DIR}:/src:ro" \
  -v "${SNAPSHOT_DIR}:/out" \
  "${SQLITE_IMAGE}" sh -eu -c '
    attempt=1
    while [ "$attempt" -le 3 ]; do
      echo "[zhiyu-backup] installing sqlite (attempt ${attempt}/3)" >&2
      if timeout 120 apk add --no-cache sqlite; then
        exec sqlite3 -readonly "/src/${DATABASE_FILE}" \
          "VACUUM INTO '\''/out/${SNAPSHOT_FILE}'\'';"
      fi
      if [ "$attempt" -lt 3 ]; then
        sleep $((attempt * 5))
      fi
      attempt=$((attempt + 1))
    done
    echo "[zhiyu-backup] ERROR: failed to install sqlite after 3 attempts" >&2
    exit 1
  '
then
  fail "SQLite snapshot container failed or timed out after ${SQLITE_CONTAINER_TIMEOUT_SECONDS}s"
fi
[[ -s "${SNAPSHOT_TEMP}" ]] || fail "SQLite snapshot was not created: ${SNAPSHOT_TEMP}"

log "running SQLite integrity checks"
if ! check_output="$(timeout "${SQLITE_CONTAINER_TIMEOUT_SECONDS}" docker run --rm \
  -e "SNAPSHOT_FILE=${SNAPSHOT_TEMP_FILE}" \
  -v "${SNAPSHOT_DIR}:/out:ro" \
  "${SQLITE_IMAGE}" sh -eu -c '
    attempt=1
    while [ "$attempt" -le 3 ]; do
      echo "[zhiyu-backup] installing sqlite for verification (attempt ${attempt}/3)" >&2
      if timeout 120 apk add --no-cache sqlite; then
        exec sqlite3 -readonly "/out/${SNAPSHOT_FILE}" \
          "PRAGMA integrity_check; PRAGMA foreign_key_check;"
      fi
      if [ "$attempt" -lt 3 ]; then
        sleep $((attempt * 5))
      fi
      attempt=$((attempt + 1))
    done
    echo "[zhiyu-backup] ERROR: failed to install sqlite after 3 attempts" >&2
    exit 1
  ')"
then
  fail "SQLite verification container failed or timed out after ${SQLITE_CONTAINER_TIMEOUT_SECONDS}s"
fi
integrity_result="$(printf '%s\n' "${check_output}" | sed -n '1p')"
[[ "${integrity_result}" == "ok" ]] || fail "integrity_check failed: ${integrity_result}"
foreign_key_result="$(printf '%s\n' "${check_output}" | sed '1d')"
[[ -z "${foreign_key_result}" ]] || fail "foreign_key_check failed: ${foreign_key_result}"

snapshot_sha256="$(sha256sum "${SNAPSHOT_TEMP}" | awk '{print $1}')"
snapshot_bytes="$(stat -c '%s' "${SNAPSHOT_TEMP}")"
printf '{\n  "created_at": "%s",\n  "source": "%s",\n  "snapshot": "%s",\n  "sha256": "%s",\n  "bytes": %s,\n  "integrity_check": "ok",\n  "foreign_key_check": "ok"\n}\n' \
  "${STARTED_AT}" "${DATABASE_PATH}" "${SNAPSHOT_PATH}" "${snapshot_sha256}" "${snapshot_bytes}" >"${MANIFEST_TEMP}"
mv -f -- "${SNAPSHOT_TEMP}" "${SNAPSHOT_PATH}"
mv -f -- "${MANIFEST_TEMP}" "${MANIFEST_PATH}"

[[ -r "${SNAPSHOT_PATH}" && -r "${MANIFEST_PATH}" ]] || \
  fail "snapshot or manifest is not readable under ${SNAPSHOT_DIR}"

log "uploading ${SNAPSHOT_DIR} to restic repository"
restic backup --no-scan "${SNAPSHOT_DIR}"
log "applying restic retention policy"
restic forget --keep-daily 7 --keep-weekly 4 --keep-monthly 6 --prune
log "backup completed successfully (sha256=${snapshot_sha256}, bytes=${snapshot_bytes})"
