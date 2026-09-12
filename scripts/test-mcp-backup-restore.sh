#!/usr/bin/env sh
set -eu

# Tests for the MCP database backup/restore contract (slice 9):
#   - mcp_image_contains_sqlite_cli
#   - backup_snapshot_passes_integrity_check
#   - deployment_docs_name_mcp_backup_restore_and_verification_steps
#
# Usage: sh scripts/test-mcp-backup-restore.sh

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
work_dir=$(mktemp -d)
trap 'rm -rf "$work_dir"' EXIT

PASSED=0
FAILED=0

pass() {
  echo "PASS: $1"
  PASSED=$((PASSED + 1))
}

fail() {
  echo "FAIL: $1" >&2
  FAILED=$((FAILED + 1))
}

# --- mcp_image_contains_sqlite_cli -----------------------------------------
# The documented online backup command (sqlite3 ... VACUUM INTO) must be
# available in the production MCP image. CI does not build images, so assert
# the Dockerfile installs the CLI; the runtime image is debian:bookworm-slim
# with no other sqlite3 source.
if grep -Eq 'apt-get install -y --no-install-recommends[[:space:]].*\bsqlite3\b' \
  "$repo_root/Dockerfile.mcp"; then
  pass "mcp_image_contains_sqlite_cli"
else
  fail "mcp_image_contains_sqlite_cli: Dockerfile.mcp must install the sqlite3 CLI"
fi

# --- backup_snapshot_passes_integrity_check --------------------------------
# The documented backup procedure (VACUUM INTO on a live WAL database) must
# produce a consistent, readable SQLite file.
if ! command -v sqlite3 >/dev/null 2>&1; then
  echo "SKIP: backup_snapshot_passes_integrity_check (sqlite3 CLI not installed)"
else
  live_db="$work_dir/mcp-server.db"
  snapshot="$work_dir/mcp-server-backup.db"

  sqlite3 "$live_db" "PRAGMA journal_mode=WAL;" >/dev/null
  sqlite3 "$live_db" \
    "CREATE TABLE mcp_audit (timestamp INTEGER, request_id TEXT);
     INSERT INTO mcp_audit (timestamp, request_id) VALUES (1700000000, 'req-1');"

  # The documented online backup command, run while the database is live.
  sqlite3 "$live_db" "VACUUM INTO '$snapshot';"

  integrity=$(sqlite3 "$snapshot" "PRAGMA integrity_check;")
  rows=$(sqlite3 "$snapshot" "SELECT COUNT(*) FROM mcp_audit;")
  request=$(sqlite3 "$snapshot" "SELECT request_id FROM mcp_audit;")

  if [ "$integrity" = "ok" ] && [ "$rows" = "1" ] && [ "$request" = "req-1" ]; then
    pass "backup_snapshot_passes_integrity_check"
  else
    fail "backup_snapshot_passes_integrity_check: integrity=$integrity rows=$rows request=$request"
  fi
fi

# --- deployment_docs_name_mcp_backup_restore_and_verification_steps --------
# The recovery guidance must remain discoverable in docs/DEPLOYMENT.md and
# must match the deployed path and the single-replica constraint.
docs="$repo_root/docs/DEPLOYMENT.md"
missing=""

for needle in \
  '## MCP Database Backup and Restore' \
  '/app/data/mcp-server.db' \
  'commoncal-mcp-data' \
  'VACUUM INTO' \
  'PRAGMA integrity_check' \
  'kubectl scale deployment commoncal-mcp -n commoncal --replicas=0' \
  'exactly one replica'
do
  if ! grep -F -q -- "$needle" "$docs"; then
    missing="$missing $needle"
  fi
done

if [ -z "$missing" ]; then
  pass "deployment_docs_name_mcp_backup_restore_and_verification_steps"
else
  fail "deployment_docs_name_mcp_backup_restore_and_verification_steps: missing:$missing"
fi

echo ""
echo "Results: $PASSED passed, $FAILED failed"

if [ "$FAILED" -gt 0 ]; then
  exit 1
fi
