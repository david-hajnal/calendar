# CommonCal — Agent Guide

Multi-user calendar platform. Rust backend (Axum + SQLite), React 19 frontend (Vite 7 + TypeScript), MCP server (Rust + rmcp), CalDAV sync, and encrypted backup/restore.

## Repository Layout

| Path | Purpose |
|------|---------|
| `backend/` | Rust API server, CLI commands, CalDAV, MCP internal API |
| `backend/src/` | Domain services, HTTP handlers, CalDAV, MCP bridge |
| `backend/src/caldav/` | CalDAV protocol implementation (auth, http, ical, query, repository) |
| `backend/migrations/` | SQL migrations (0001–0031), rollbacks in `rollbacks/` |
| `backend/tests/` | Integration tests (Axum tower, SQLite temp files) |
| `frontend/` | React 19 SPA (Vite 7, TypeScript, Vitest, ESLint) |
| `frontend/src/` | Feature modules: `auth/`, `calendar/`, `notification/`, `public/`, `settings/`, `theme/` |
| `mcp-server/` | Standalone MCP endpoint (Axum + rmcp, OAuth bearer auth) |
| `mcp-server/src/tools/` | MCP tool handlers (calendar_list, event_*, availability_find, reminder_set) |
| `e2e/` | Playwright E2E tests (desktop Firefox + mobile iPhone 13) |
| `mcp-acceptance/` | MCP OAuth lifecycle, discovery, security-isolation acceptance tests |
| `slice1-lab/` | Cross-client MCP connector lab (auth-server, hydra, proof harness) |
| `deploy/` | Helm charts, Flux overlays, deployment scripts |
| `deploy/flux/overlays/` | `production/`, `auth-cutover/`, `auth-preparation/` |
| `scripts/` | Dev, build, deploy, validation, and test scripts |
| `docs/` | Product docs, plans, deployment guides |
| `docs/plans/` | Feature plans with task slices (apple-calendar, ics-import, mcp-server, etc.) |
| `.agent/` | Agent plans, task manifests, run artifacts |
| `.agents/` | Agent skills (local-mcp, mcp-add-events, orchestrator-factory) |
| `.opencode/agents/` | OpenCode agent definitions (qwen-operations, qwen-orchestrator, qwen-slice-worker) |

## Entry Points

| Entry | File | Description |
|-------|------|-------------|
| Backend HTTP | `backend/src/main.rs` | Axum server, CLI subcommands (backup, restore, bootstrap-superadmin, seed) |
| Backend lib | `backend/src/lib.rs` | Public module exports (40 modules) |
| HTTP routing | `backend/src/http.rs` | All `/api/v1/*` routes, middleware, error envelope |
| CalDAV DAV | `backend/src/caldav/http.rs` | PROPFIND/REPORT/PUT/DELETE `/.well-known/caldav`, `/dav/*` |
| CalDAV mgmt | `backend/src/caldav/auth.rs` | Connection management, per-device passwords |
| MCP internal | `backend/src/mcp_internal.rs` | Internal API for MCP server (API-key auth) |
| MCP consent | `backend/src/mcp_consent.rs` | Consent flow (bridge-backed) |
| MCP grants | `backend/src/mcp_grant_management.rs` | Grant list/revoke (session-bound) |
| Frontend | `frontend/src/main.tsx` | React root |
| Frontend app | `frontend/src/App.tsx` | Routing, auth flow, page composition |
| MCP server | `mcp-server/src/main.rs` | MCP endpoint (`/mcp`), OAuth middleware |
| MCP gateway | `mcp-server/src/gateway.rs` | Internal API client, rate limiting |

## Request Path (POST /api/v1/calendars/:id/events)

```
Axum router (http.rs)
  → session middleware (cookie + CSRF check)
  → rate-limit middleware (fixed window, per-user)
  → handler (http.rs)
    → EventService (event.rs)
      → authorization check (authorization.rs)
      → sqlx query (SQLite WAL, pool max 5)
      → notification replanner (spawned, fire-and-forget)
    → ApiError → ErrorEnvelope JSON { error: { code, message, current_version? } }
```

## Conventions

### Errors
- Backend: `ApiError` struct (status, code, message, current_version) → `ErrorEnvelope` JSON. Service errors are typed enums (e.g. `CalendarServiceError`, `EventServiceError`) with `Display` impls.
- Frontend: `ApiError` class wrapping `Response`; API client in `frontend/src/auth/api.ts` adds CSRF header for unsafe methods.
- MCP server: `mcp-server/src/error.rs` with typed error variants.

### Config
- Backend: `AppConfig::from_env()` reads `APP_ENV`, `BIND_ADDRESS`, `DATABASE_PATH`, `SESSION_SECRET`, `APP_ORIGIN`, `CALDAV_PUBLIC_ORIGIN`, `PASSWORD_LOGIN_ENABLED`, `AUTH_BRIDGE_URL/SECRET/TIMEOUT_SECS`.
- Production requires: `SESSION_SECRET`, `PASSWORD_LOGIN_ENABLED=true`, `CALDAV_PUBLIC_ORIGIN` (https), `MCP_INTERNAL_API_KEY`.
- MCP server: `mcp-server/src/config.rs` reads `MCP_OAUTH_ISSUER`, `MCP_INTERNAL_API_BASE`, `MCP_INTERNAL_API_KEY`, `MCP_SESSION_SECRET`, `MCP_DATABASE_PATH`, `MCP_DOMAIN`, `MCP_PUBLIC_RESOURCE_URL`, `BIND_ADDRESS`, `DPOP_KEY_PATH`, `MCP_RATE_LIMIT_ENABLED/MAX/WINDOW_SECS`.

### Tests
- Backend: Rust integration tests in `backend/tests/` (44 test files). Use `tower::ServiceExt::oneshot`, temp SQLite files, `AppConfig::with_database_path`. Table-driven where appropriate.
- Frontend: Vitest + Testing Library. Co-located `*.test.tsx` / `*.test.ts` files.
- E2E: Playwright in `e2e/tests/` (5 spec files). Desktop Firefox + mobile iPhone 13.
- MCP: Integration tests in `mcp-server/tests/` (e2e.rs, integration.rs, transport_host_regression.rs).
- Acceptance: `mcp-acceptance/` (discovery, oauth-lifecycle, security-isolation).

### Migrations
- SQL files in `backend/migrations/` (0001–0031). Run at startup via `sqlx::migrate!`.
- Rollbacks in `backend/migrations/rollbacks/`.
- Foreign keys disabled during migration, re-enabled after.

### Rate Limiting
- Fixed-window limiters. Per-user (not per-calendar).
- Tiers: Critical (10/60s), Standard (30/60s), Permissive (60/60s).
- Disabled in development. Superadmin bypasses.
- Public endpoints: 100/60s. Admin invitations: 5/60s (production only).

### Sessions & Auth
- Cookie: `__Host-commoncal_session` (HTTPS) / `commoncal_session` (HTTP).
- CSRF: Token in `x-csrf-token` header for POST/PUT/PATCH/DELETE.
- Login: Password + email-link. Dev: `/dev-login` page (any email).
- CalDAV: Basic auth with per-device connection passwords (isolated from browser sessions).
- MCP: OAuth bearer token (DPoP optional). Internal API: API key.

### Frontend
- No router library. Custom `navigate()` / `useLocation()` in `frontend/src/navigation.ts`.
- API client: `createApiClient()` in `frontend/src/auth/api.ts` (fetch wrapper, CSRF, credentials).
- Feature modules: `auth/`, `calendar/`, `notification/`, `public/`, `settings/`, `theme/`.
- Styling: Plain CSS files co-located with components.
- Lint: ESLint 9 + typescript-eslint. Typecheck: `tsc -b`.

### Docker
- Multi-stage: `node:22-bookworm-slim` (frontend) → `rust:1-bookworm` (backend) → `debian:bookworm-slim` (runtime).
- Non-root user `commoncal`. Health check on `/health/ready`.
- MCP: `Dockerfile.mcp`. Auth: `Dockerfile.auth`.
- Use `./scripts/dev.sh` for local Docker operations. Never run raw `docker compose`.

## Key Commands

```sh
# All checks (CI parity)
make check

# Focused
make backend-test          # cargo test --locked
make frontend-test         # pnpm --dir frontend test
make lint                  # cargo fmt/clippy + eslint + tsc
make e2e                   # Playwright E2E
make mcp-test              # MCP server tests
make authorization-regression  # Auth regression suite

# Dev servers
pnpm --dir frontend dev    # Vite dev server (port 5173)
APP_ORIGIN=http://localhost:5173 cargo run --manifest-path backend/Cargo.toml  # Backend (port 3000)

# Docker dev
./scripts/dev.sh start     # Build + up, wait for healthcheck
./scripts/dev.sh stop      # Stop (keeps volumes)
./scripts/dev.sh rebuild   # Stop, rebuild (no cache), start
./scripts/dev.sh logs -f   # Follow logs
./scripts/dev.sh status    # Container + volume status
./scripts/dev.sh clean     # Stop, remove volumes, prune cache

# CLI commands
cargo run --manifest-path backend/Cargo.toml -- bootstrap-superadmin <email> [name]
cargo run --manifest-path backend/Cargo.toml -- backup
cargo run --manifest-path backend/Cargo.toml -- restore

# Docker build/push
make docker-build          # Build only
make docker-build-push     # Build + push to GHCR
PLATFORMS=linux/amd64 make docker-build-push  # amd64 only

# Production
make prod-render           # Render effective production manifests (offline)
make prod-check            # Validate production config invariants
make prod-doctor           # Diagnose configuration state
make deploy                # Deploy to production (requires SESSION_SECRET, BACKUP_ENCRYPTION_KEY_HEX, IMAGE_TAG)
```

## Ports

| Port | Service |
|------|---------|
| 3000 | Backend API + served frontend |
| 5173 | Vite dev server (frontend) |
| 4000 | Auth server (Docker) |
| 8081 | MCP server (Docker) |
| 1025 | Mail capture (Docker) |
| 3100 | E2E test server |

## Risk Areas

| Area | Why |
|------|-----|
| `backend/src/caldav/http.rs` (5,689 lines) | Largest file. CalDAV protocol complexity, recurrence, sync tokens, ETags. |
| `backend/src/http.rs` (4,062 lines) | All API routes, middleware, error handling. High churn (13 commits). |
| `backend/src/event.rs` (3,017 lines) | Event CRUD, recurrence, occurrence overrides, notification replanning. |
| `backend/src/mcp_internal.rs` (1,675 lines) | MCP internal API boundary. Security-critical. |
| `backend/src/calendar.rs` (1,226 lines) | Calendar CRUD, ACL, DAV metadata, version conflicts. |
| `deploy/flux/overlays/production/` | Immutable tags enforced. 3 HelmRelease manifests. High churn. |
| `mcp-server/src/gateway.rs` | MCP gateway, internal API client, rate limiting. |

## Absences & Gaps

- `scratch_axum.rs` and `scratch_ics.rs` in `backend/tests/` — likely WIP, not part of CI.
- No test coverage for `backend/src/mcp_bridge.rs` (bridge client).
- `docs/memory/` is empty.
- `slice1-lab/` is a lab environment, not part of the main build.

## Security Notes

- **Never** store or request Apple Account/iCloud credentials.
- **Never** paste access tokens, refresh tokens, client secrets, internal API keys, or session cookies into assistant configs or issues.
- Production requires: `SESSION_SECRET`, `PASSWORD_LOGIN_ENABLED=true`, `CALDAV_PUBLIC_ORIGIN` (https), `MCP_INTERNAL_API_KEY`, `BACKUP_ENCRYPTION_KEY_HEX`.
- Rate limiting is disabled in development.
- CSRF protection on all unsafe methods.
- CalDAV auth is isolated from browser sessions (Basic auth, no cookies).
- MCP internal API is API-key protected. Required in production.
- Backup/restore uses AES-256-GCM with SHA-256 integrity verification.

## Working Rules

- Read the linked plan packet before editing (`.agent/PLAN.md`, `docs/plans/`).
- Preserve unrelated worktree changes.
- Route event mutations through existing domain services, not direct table writes.
- Add a failing test for the packet behavior, make the smallest vertical change, then run the packet verification plus affected suites.
- Do not present public ICS/WebCal export as account-level CalDAV synchronization.
- All checks must pass before committing. Run `make check` locally first.
- Yarn is not supported. Do not create a `yarn.lock`.
