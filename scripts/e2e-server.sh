#!/bin/sh
set -eu

e2e_state_dir="${E2E_STATE_DIR:-.e2e}"
mkdir -p "$e2e_state_dir"
: > "$e2e_state_dir/outbox.ndjson"
rm -f "$e2e_state_dir/commoncal.sqlite" "$e2e_state_dir/commoncal.sqlite-shm" "$e2e_state_dir/commoncal.sqlite-wal"

pnpm --dir frontend build
APP_ENV=development \
SESSION_SECRET="${E2E_SESSION_SECRET:-commoncal-e2e-session-secret}" \
BIND_ADDRESS="${E2E_BIND_ADDRESS:-127.0.0.1:3100}" \
APP_ORIGIN="${E2E_BASE_URL:-http://127.0.0.1:3100}" \
DATABASE_PATH="$e2e_state_dir/commoncal.sqlite" \
FRONTEND_DIR=frontend/dist \
E2E_EMAIL_OUTBOX="$e2e_state_dir/outbox.ndjson" \
E2E_ICS_FIXTURE=e2e/support/controlled.ics \
cargo run --quiet --manifest-path backend/Cargo.toml
