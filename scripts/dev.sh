#!/usr/bin/env bash
set -euo pipefail

# Local dev Docker orchestrator for CommonCal
# Usage: ./scripts/dev.sh {start|stop|rebuild|logs|status|clean}

COMPOSE_FILES="-f docker-compose.yml -f docker-compose.dev.yml"
COMPOSE="docker compose ${COMPOSE_FILES}"
COMPOSE_PROJECT="happening"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log()  { printf "${GREEN}[+]${NC} %s\n" "$1"; }
warn() { printf "${YELLOW}[!]${NC} %s\n" "$1"; }
err()  { printf "${RED}[-]${NC} %s\n" "$1" >&2; }
info() { printf "${BLUE}[i]${NC} %s\n" "$1"; }

die() { err "$1"; exit 1; }

check_prereqs() {
  command -v docker >/dev/null 2>&1 || die "docker not found"
  docker compose version >/dev/null 2>&1 || die "docker compose not found"
  if [[ "${1:-core}" != "mcp" && ! -f .env.local ]]; then
    die ".env.local not found — copy .env.example to .env.local first"
  fi
}

cmd_start() {
  check_prereqs

  # Check if already running
  if docker compose ${COMPOSE_FILES} ps --status running 2>/dev/null | grep -q app; then
    warn "app container is already running"
  fi

  log "Building images..."
  docker compose ${COMPOSE_FILES} build app || die "Build failed"

  log "Starting containers (dev mode)..."
  docker compose ${COMPOSE_FILES} up -d app frontend-dev || die "Failed to start"

  info "Waiting for app healthcheck..."
  local retries=20
  local i=0
  while [[ $i -lt $retries ]]; do
    if docker compose ${COMPOSE_FILES} ps --status healthy 2>/dev/null | grep -q app; then
      log "App is healthy!"
      info "Backend:  http://localhost:3000"
      info "Frontend: http://localhost:5173"
      return 0
    fi
    i=$((i + 1))
    sleep 2
  done

  warn "Healthcheck did not pass after ${retries} attempts"
  info "Check logs with: $0 logs -f"
  info "Backend:  http://localhost:3000"
  info "Frontend: http://localhost:5173"
}

cmd_stop() {
  log "Stopping containers..."
  docker compose ${COMPOSE_FILES} down || die "Failed to stop"
  log "Stopped"
}

cmd_rebuild() {
  check_prereqs

  log "Stopping containers and removing volumes..."
  docker compose ${COMPOSE_FILES} down -v || true

  log "Rebuilding images (no cache)..."
  docker compose ${COMPOSE_FILES} build --no-cache app || die "Build failed"

  log "Starting containers (dev mode)..."
  docker compose ${COMPOSE_FILES} up -d app frontend-dev || die "Failed to start"

  info "Waiting for app healthcheck..."
  local retries=20
  local i=0
  while [[ $i -lt $retries ]]; do
    if docker compose ${COMPOSE_FILES} ps --status healthy 2>/dev/null | grep -q app; then
      log "App is healthy!"
      info "Seeding database (first run)..."
      docker compose ${COMPOSE_FILES} exec -T app commoncal-backend seed || true
      info "Backend:  http://localhost:3000"
      info "Frontend: http://localhost:5173"
      return 0
    fi
    i=$((i + 1))
    sleep 2
  done

  warn "Healthcheck did not pass after ${retries} attempts"
  info "Check logs with: $0 logs -f"
  info "Backend:  http://localhost:3000"
  info "Frontend: http://localhost:5173"
}

cmd_reset() {
  check_prereqs

  warn "This will remove all database data"
  read -r -p "Continue? [y/N] " confirm
  if [[ ! "$confirm" =~ ^[Yy]$ ]]; then
    info "Aborted"
    return 0
  fi

  log "Stopping containers and removing volumes..."
  docker compose ${COMPOSE_FILES} down -v || true
  log "Pruning build cache..."
  docker builder prune -f || true

  log "Building fresh images..."
  docker compose ${COMPOSE_FILES} build --no-cache app || die "Build failed"

  log "Starting containers (dev mode)..."
  docker compose ${COMPOSE_FILES} up -d app frontend-dev || die "Failed to start"

  info "Waiting for app healthcheck..."
  local retries=20
  local i=0
  while [[ $i -lt $retries ]]; do
    if docker compose ${COMPOSE_FILES} ps --status healthy 2>/dev/null | grep -q app; then
      log "App is healthy!"
      info "Seeding database (fresh volume)..."
      docker compose ${COMPOSE_FILES} exec -T app commoncal-backend seed || true
      info "Backend:  http://localhost:3000"
      info "Frontend: http://localhost:5173"
      return 0
    fi
    i=$((i + 1))
    sleep 2
  done

  warn "Healthcheck did not pass after ${retries} attempts"
  info "Check logs with: $0 logs -f"
  info "Backend:  http://localhost:3000"
  info "Frontend: http://localhost:5173"
}

cmd_logs() {
  local follow=""
  if [[ "${1:-}" == "-f" ]]; then
    follow="-f"
  fi
  docker compose ${COMPOSE_FILES} logs ${follow} --tail 100 app frontend-dev
}

cmd_status() {
  log "Containers:"
  docker compose ${COMPOSE_FILES} ps
  echo ""
  info "Volumes:"
  docker volume ls 2>/dev/null | grep "${COMPOSE_PROJECT}" || info "  (none)"
}

cmd_seed() {
  check_prereqs

  log "Waiting for app healthcheck..."
  local retries=20
  local i=0
  while [[ $i -lt $retries ]]; do
    if docker compose ${COMPOSE_FILES} ps --status healthy 2>/dev/null | grep -q app; then
      break
    fi
    i=$((i + 1))
    sleep 2
  done

  info "Seeding database..."
  docker compose ${COMPOSE_FILES} exec -T app commoncal-backend seed || die "Seed failed"
  log "Done"
  info "Run: $0 logs"
}

cmd_clean() {
  warn "This will stop containers, remove volumes, and prune build cache"
  read -r -p "Continue? [y/N] " confirm
  if [[ ! "$confirm" =~ ^[Yy]$ ]]; then
    info "Aborted"
    return 0
  fi

  docker compose ${COMPOSE_FILES} down -v || true
  log "Pruning build cache..."
  docker builder prune -f || true
  log "Cleaned"
}

# ---------------------------------------------------------------------------
# MCP development stack
# ---------------------------------------------------------------------------

MCP_COMPOSE_FILES="-f docker-compose.mcp.yml"
MCP_COMPOSE="docker compose ${MCP_COMPOSE_FILES}"

cmd_mcp_start() {
  check_prereqs mcp

  log "Building MCP stack images..."
  docker compose ${MCP_COMPOSE_FILES} build app auth mcp || die "Build failed"

  log "Starting MCP stack (Core + OAuth + MCP server + PostgreSQL)..."
  docker compose ${MCP_COMPOSE_FILES} up -d || die "Failed to start"

  local endpoint service ready attempt
  for service in core auth mcp; do
    case "$service" in
      core) endpoint="http://127.0.0.1:3000/health/ready" ;;
      auth) endpoint="http://127.0.0.1:4000/ready" ;;
      mcp) endpoint="http://127.0.0.1:3001/health/ready" ;;
    esac
    info "Waiting for $service readiness..."
    ready=0
    for ((attempt=0; attempt<30; attempt++)); do
      if curl --max-time 2 -sf "$endpoint" >/dev/null 2>&1; then
        ready=1
        break
      fi
      sleep 1
    done
    if [[ "$ready" != 1 ]]; then
      docker compose ${MCP_COMPOSE_FILES} logs --tail 50
      die "$service did not become ready — inspect with: $0 mcp-logs"
    fi
    log "$service is ready!"
  done

  # Validate OAuth discovery + JWKS
  info "Validating OAuth discovery..."
  if curl --max-time 5 -sf http://127.0.0.1:4000/.well-known/openid-configuration >/dev/null 2>&1; then
    log "OAuth discovery OK"
  else
    warn "OAuth discovery not reachable — check logs with: $0 mcp-logs"
  fi

  info "Validating MCP metadata..."
  if curl --max-time 5 -sf http://127.0.0.1:3001/.well-known/oauth-protected-resource >/dev/null 2>&1; then
    log "MCP metadata OK"
  else
    warn "MCP metadata not reachable — check logs with: $0 mcp-logs"
  fi

  # Verify unauthenticated MCP returns 401
  local http_code
  http_code=$(curl --max-time 5 -s -o /dev/null -w "%{http_code}" http://127.0.0.1:3001/mcp 2>/dev/null || echo "000")
  if [[ "$http_code" == "401" ]]; then
    log "MCP auth challenge OK (401 on unauthenticated /mcp)"
  else
    warn "MCP /mcp returned HTTP $http_code (expected 401)"
  fi

  info "OAuth:    http://127.0.0.1:4000"
  info "MCP:      http://127.0.0.1:3001"
  info "Postgres: 127.0.0.1:55433"
  info "Inspect emails: cat $(docker compose ${MCP_COMPOSE_FILES} exec -T app sh -c 'echo $E2E_EMAIL_OUTBOX' 2>/dev/null || echo 'mcp-server/data/emails.jsonl')"
}

cmd_mcp_status() {
  log "MCP stack containers:"
  docker compose ${MCP_COMPOSE_FILES} ps
  echo ""
  info "MCP stack volumes:"
  docker volume ls 2>/dev/null | grep "happening_mcp" || info "  (none)"
}

cmd_mcp_logs() {
  local follow=""
  if [[ "${1:-}" == "-f" ]]; then
    follow="-f"
  fi
  docker compose ${MCP_COMPOSE_FILES} logs ${follow} --tail 100
}

cmd_mcp_stop() {
  log "Stopping MCP stack..."
  docker compose ${MCP_COMPOSE_FILES} down || die "Failed to stop"
  log "Stopped (volumes retained)"
}

# Production auth checks use a disposable SQLite directory, without a database server.
cmd_auth_check() (
  cd slice1-lab/auth-server
  node tests/dcr-scopes-integration.mjs
  node tests/production-integration.mjs
)

# Optional verification for an already-running legacy PostgreSQL issuer.
ensure_auth_docker() {
  if ! docker info >/dev/null 2>&1; then
    if [[ "$(uname -s)" == Darwin ]]; then
      open -a Docker
      for _ in {1..30}; do
        docker info >/dev/null 2>&1 && break
        sleep 2
      done
    fi
    docker info >/dev/null 2>&1 || die "Docker daemon is not available"
  fi
}

cmd_auth_import_check() (
  ensure_auth_docker
  trap 'docker compose -p happening-auth-import-check -f docker-compose.auth-test.yml down --volumes >/dev/null 2>&1' EXIT
  docker compose -p happening-auth-import-check -f docker-compose.auth-test.yml up -d --wait postgres
  (cd slice1-lab/auth-server && AUTH_TEST_DATABASE_URL=postgresql://auth_test:disposable-auth-check-password@127.0.0.1:55433/auth_test node tests/postgres-import-proof.mjs)
)

# Actual Node 22 auth image, age encryption, non-root filesystem and recovery.
cmd_auth_storage_check() (
  ensure_auth_docker
  trap 'docker compose -p happening-auth-sqlite-check -f docker-compose.auth-sqlite-test.yml down --volumes >/dev/null 2>&1' EXIT
  docker compose -p happening-auth-sqlite-check -f docker-compose.auth-sqlite-test.yml build proof
  docker compose -p happening-auth-sqlite-check -f docker-compose.auth-sqlite-test.yml run --rm proof
)

# Match the publication gate against the actual production auth image.
cmd_auth_image_scan() (
  command -v trivy >/dev/null 2>&1 || die "Trivy is required for auth-image-scan"
  ensure_auth_docker
  local auth_image_id
  auth_image_id="$(docker image inspect --format '{{.Id}}' commoncal-auth-sqlite-proof:local 2>/dev/null)" || die "Run auth-storage-check before auth-image-scan"
  [[ -n "$auth_image_id" ]] || die "Run auth-storage-check before auth-image-scan"
  trivy image --severity CRITICAL,HIGH --exit-code 1 --ignore-unfixed "$auth_image_id"
)

usage() {
  cat <<EOF
${BLUE}happening local dev Docker orchestrator${NC}

${YELLOW}Usage:${NC}  $0 <command>

${YELLOW}Commands:${NC}
  start     Start containers in dev mode (build + up, wait for health)
  stop      Stop containers (keeps volumes)
  rebuild   Stop, remove volumes, rebuild images (no cache), and start
  reset     Full reset: remove volumes, prune cache, rebuild, and start
  seed      Run db seed command against running app container
  logs      Show recent logs (add -f to follow)
  status    Show container and volume status
  auth-storage-check Verify SQLite backup/recovery in the non-root Node 22 auth image
  auth-image-scan Scan the production auth image using the publication security gate
  auth-restore-check Verify private restored OAuth state with Docker networking disabled
  auth-import-check Verify optional legacy PostgreSQL-to-SQLite import
  auth-check Run production auth proofs with disposable SQLite storage
  mcp-start Start core + OAuth + MCP dev stack (build + up, wait for health)
  mcp-update-core Rebuild core and apply its local OAuth bridge configuration
  mcp-update-network Apply OAuth/MCP network configuration without rebuilding
  mcp-update-server Rebuild and replace the MCP server, retaining its data
  mcp-restart-core Restart core to clear development in-memory state
  dev-user  Create/reset dev@example.com (DEV_USER_PASSWORD overrides default)
  mcp-status Show MCP stack container + volume status
  mcp-logs   Show MCP stack logs (add -f to follow)
  mcp-stop   Stop MCP stack (keeps volumes)
  clean     Stop, remove volumes, prune build cache (interactive)

${YELLOW}Ports:${NC}
  3000 — backend API + served frontend
  3001 — MCP server
  4000 — OAuth authorization server
  5173 — Vite dev server (dev mode)

EOF
}

cmd_dev_user() {
  check_prereqs
  python3 - <<'PY' | docker compose ${COMPOSE_FILES} exec -T app sh -c 'test "$APP_ENV" = development || exit 1; exec sqlite3 "$DATABASE_PATH"'
import bcrypt
import os
password = os.environ.get('DEV_USER_PASSWORD', 'LocalDev2026!')
hashed = bcrypt.hashpw(password.encode(), bcrypt.gensalt(rounds=12)).decode()
print(".timeout 5000")
print("INSERT INTO users (normalized_email, display_name, status, created_at, password_hash) VALUES ('dev@example.com', 'Dev User', CASE WHEN instr((SELECT sql FROM sqlite_master WHERE name='users'), '''registered''') > 0 THEN 'registered' ELSE 'active' END, unixepoch(), '" + hashed + "') ON CONFLICT(normalized_email) DO UPDATE SET password_hash=excluded.password_hash, status=excluded.status;")
PY
  log "Local password account ready: dev@example.com"
}

case "${1:-}" in
  mcp-update-server) docker compose ${MCP_COMPOSE_FILES} up -d --build --no-deps mcp ;;
  mcp-update-network) docker compose ${MCP_COMPOSE_FILES} up -d --no-deps auth mcp ;;
  mcp-update-core) docker compose ${MCP_COMPOSE_FILES} up -d --build --no-deps app ;;
  mcp-restart-core) docker compose ${MCP_COMPOSE_FILES} restart app ;;
  dev-user) cmd_dev_user ;;
  start)    cmd_start ;;
  stop)     cmd_stop ;;
  rebuild)  cmd_rebuild ;;
  reset)    cmd_reset ;;
  seed)     cmd_seed ;;
  logs)     cmd_logs "${2:-}" ;;
  status)   cmd_status ;;
  clean)    cmd_clean ;;
  auth-check) cmd_auth_check ;;
  auth-storage-check) cmd_auth_storage_check ;;
  auth-import-check) cmd_auth_import_check ;;
  auth-image-scan) cmd_auth_image_scan ;;
  auth-restore-check)
    : "${AUTH_RESTORE_INPUT_DIR:?Set AUTH_RESTORE_INPUT_DIR to private restored inputs}"
    export AUTH_RESTORE_INPUT_DIR
    export AUTH_RESTORE_UID="$(id -u)" AUTH_RESTORE_GID="$(id -g)"
    ensure_auth_docker
    docker compose -p happening-auth-restore-proof -f docker-compose.auth-restore-proof.yml run --build --rm proof ;;
  mcp-start)    cmd_mcp_start ;;
  mcp-status)   cmd_mcp_status ;;
  mcp-logs)     cmd_mcp_logs "${2:-}" ;;
  mcp-stop)     cmd_mcp_stop ;;
  *)            usage ;;
esac
