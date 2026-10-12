#!/usr/bin/env bash
set -euo pipefail

# Local integration test runner against the Docker stack started by
# scripts/dev.sh start (i.e. the "prod-local-up" Makefile target).
#
# Services started by dev.sh start:
#   - app          : backend API + served frontend (port 3000)
#   - frontend-dev : Vite dev server (port 5173)
#
# Services NOT started by dev.sh start (documented here so the test
# suite can skip them gracefully instead of failing silently):
#   - auth server   (port 4000) — not started
#   - MCP server    (port 8081) — not started
#   - mail capture  (port 1025)  — not started
#   - ingress proxy — not started
#
# Prerequisites:
#   - Docker daemon running
#   - Container stack up-to-date (run 'make prod-local-up' first)
#   - CALDAV_ORIGIN / CALDAV_USERNAME / CALDAV_PASSWORD
#     (optional) — if set, CalDAV smoke tests are exercised

FAILED=0
FLOW_COUNT=0
FLOW_NAMES=()

# Helper: record which flows were exercised
record_flow() {
  FLOW_NAMES+=("$1")
  FLOW_COUNT=$((FLOW_COUNT + 1))
}

# ── Prerequisite checks ───────────────────────────────────────────────

prereq_failures=0

# 1. Docker daemon
if ! docker info >/dev/null 2>&1; then
  echo "FAIL: Docker daemon is not available" >&2
  prereq_failures=$((prereq_failures + 1))
fi

# 2. Containers running
if ! docker compose -f docker-compose.yml -f docker-compose.dev.yml ps \
    --status running 2>/dev/null | grep -q app; then
  echo "FAIL: 'app' container is not running" >&2
  prereq_failures=$((prereq_failures + 1))
fi

# 3. Backend health endpoint
BACKEND_HEALTH="http://localhost:3000/health/ready"
if ! curl -sf --max-time 5 "$BACKEND_HEALTH" >/dev/null 2>&1; then
  echo "FAIL: Backend health endpoint unreachable at $BACKEND_HEALTH" >&2
  prereq_failures=$((prereq_failures + 1))
fi

# If any prerequisite failed, report and exit
if ((prereq_failures > 0)); then
  echo ""
  echo "Integration tests aborted: $prereq_failures prerequisite(s) missing." >&2
  echo "Run 'make prod-local-up' to start the local stack, then retry." >&2
  exit 1
fi

# ── Integration flows ─────────────────────────────────────────────────

# Flow 1: Backend health endpoint assertion
echo "==> Flow 1: Backend health endpoint"
HEALTH_STATUS=$(curl -sf --max-time 5 "$BACKEND_HEALTH" || true)
if [ -n "$HEALTH_STATUS" ]; then
  echo "  PASS: Backend health endpoint responded"
  record_flow "backend-health"
else
  echo "  FAIL: Backend health endpoint returned no response" >&2
  FAILED=$((FAILED + 1))
fi

# Flow 2: Frontend dev server connectivity
echo "==> Flow 2: Frontend dev server connectivity"
if curl -sf --max-time 5 http://localhost:5173 >/dev/null 2>&1; then
  echo "  PASS: Frontend dev server (port 5173) responded"
  record_flow "frontend-connectivity"
else
  echo "  SKIP: Frontend dev server not reachable (may not be started)"
fi

# Flow 3: CalDAV smoke test (optional — requires env vars)
if [ -n "${CALDAV_ORIGIN:-}" ] && \
   [ -n "${CALDAV_USERNAME:-}" ] && \
   [ -n "${CALDAV_PASSWORD:-}" ]; then
  echo "==> Flow 3: CalDAV smoke test (via caldav-smoke.py)"
  if command -v python3 >/dev/null 2>&1; then
    if python3 scripts/caldav-smoke.py 2>&1; then
      record_flow "caldav-smoke"
    else
      echo "  CalDAV smoke test failed (protocol assertion error)" >&2
      FAILED=$((FAILED + 1))
      record_flow "caldav-smoke"
    fi
  else
    echo "  SKIP: python3 not available"
  fi
else
  echo "==> Flow 3: CalDAV smoke test — SKIPPED (CALDAV_ORIGIN, CALDAV_USERNAME, CALDAV_PASSWORD not set)"
fi

# ── Report ────────────────────────────────────────────────────────────

echo ""
echo "Integration test summary:"
echo "  Flows exercised: $FLOW_COUNT"
for flow in "${FLOW_NAMES[@]}"; do
  echo "    - $flow"
done

if ((FAILED > 0)); then
  echo ""
  echo "Result: $FAILED flow(s) failed" >&2
  exit 1
fi

echo "Result: All exercised flows passed"
exit 0
