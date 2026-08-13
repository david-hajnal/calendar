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
  if [[ ! -f .env.local ]]; then
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
  clean     Stop, remove volumes, prune build cache (interactive)

${YELLOW}Ports:${NC}
  3000 — backend API + served frontend
  5173 — Vite dev server (dev mode)

EOF
}

case "${1:-}" in
  start)    cmd_start ;;
  stop)     cmd_stop ;;
  rebuild)  cmd_rebuild ;;
  reset)    cmd_reset ;;
  seed)     cmd_seed ;;
  logs)     cmd_logs "${2:-}" ;;
  status)   cmd_status ;;
  clean)    cmd_clean ;;
  *)        usage ;;
esac
