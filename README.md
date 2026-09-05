# CommonCal

Multi-user calendar platform with email-based authentication, shared composite views,
public calendar sharing, external ICS feed ingestion, and encrypted backup/restore.

## Tech stack

- **Backend**: Rust (Axum, SQLite, sqlx, tokio)
- **Frontend**: React 19, Vite 7, TypeScript
- **Infra**: Docker multi-stage build, Helm charts
- **Auth**: Magic-link email auth, session cookies (`__Host-commoncal_session`)
- **Storage**: SQLite (single-node, file-backed)

## Prerequisites

- Rust stable
- Node.js 22
- pnpm 9.12.3

Yarn is not supported. Do not create a `yarn.lock`.

## Setup

Install frontend dependencies from the repository root:

```sh
pnpm install --frozen-lockfile
```

Run all checks:

```sh
make check
```

All checks must pass before committing. Run `make check` locally first.

Focused commands:

```sh
make backend-test
make frontend-test
make lint
```

Start the frontend development server:

```sh
pnpm --dir frontend dev
```

Start the backend development server:

```sh
APP_ORIGIN=http://localhost:5173 cargo run --manifest-path backend/Cargo.toml
```

The backend defaults to `APP_ENV=development`,
`BIND_ADDRESS=127.0.0.1:3000`, and `APP_ORIGIN=http://127.0.0.1:3000`.
When developing through Vite, set `APP_ORIGIN` to Vite's browser-visible origin
(`http://localhost:5173` by default), as shown above.
Production startup requires `APP_ENV=production` and a non-empty
`SESSION_SECRET`. Set `APP_ORIGIN` to the browser-visible application origin;
authenticated unsafe requests must match it.

## Features

- **Calendar management** — create, update, archive, restore, delete calendars with ACL
- **Event management** — CRUD events with recurring event support (update single/occurrence/this-and-following)
- **Composite views** — combine multiple calendars into named views with per-calendar color and position
- **Public sharing** — publish composite views via tokenized public URLs with ICS feed endpoints
- **External ICS feeds** — subscribe to remote calendars with configurable refresh intervals
- **Notifications** — in-app notification surface with event-based notification replanning
- **Admin API** — user management (list, suspend, reactivate, promote, demote), invitation management
- **Backup & restore** — encrypted backup (AES-256-GCM) with SHA-256 integrity verification
- **Multi-session** — inspect and revoke active sessions
- **Rate limiting** — per-user write rate limiting on authenticated endpoints (enabled in staging/production)

## API overview

| Area | Key endpoints |
|------|---------------|
| Auth | `POST /api/v1/auth/login-links`, `POST /api/v1/auth/login-links/consume`, `GET/DELETE /api/v1/auth/session`, `DELETE /api/v1/auth/sessions` |
| Admin | `GET /api/v1/admin/users`, `POST/DELETE /api/v1/admin/invitations/*`, user suspend/reactivate/promote/demote, `POST /api/v1/admin/users/:id/revoke-sessions` |
| Calendars | `GET/POST /api/v1/calendars`, `GET/PATCH/DELETE /api/v1/calendars/:id`, `POST /api/v1/calendars/:id/archive|restore`, ACL, ownership transfer |
| Events | `GET/POST /api/v1/calendars/:id/events`, `GET/PATCH/DELETE /api/v1/calendars/:id/events/:id`, occurrence overrides |
| Views | `GET/POST /api/v1/views`, `GET/PATCH/DELETE /api/v1/views/:id`, calendar composition, publication management |
| Feeds | `GET/POST /api/v1/calendars/:id/external-feeds`, `DELETE/POST /api/v1/external-feeds/:id/disable|refresh` |
| Notifications | `GET /api/v1/notifications` |
| Public | `GET /api/v1/public/views/:token`, `GET /api/v1/public/views/:token/events` |
| Backup | `cargo run -- backup`, `cargo run -- restore` (CLI commands) |

## Rate limiting

Write endpoints on authenticated routes are rate-limited per-user (not per-calendar). Rate limiting is active
when `APP_ENV=staging` or `APP_ENV=production`; it is disabled in development.

| Tier | Limit | Examples |
|------|-------|----------|
| Critical | 10 req / 60s | ACL changes, calendar ownership transfer |
| Standard | 30 req / 60s | Event CRUD, occurrence updates, external feed operations |
| Permissive | 60 req / 60s | Calendar CRUD, archive/restore, view management |

When rate limited, the API returns `429 Too Many Requests` with an `X-Retry-After` header. Superadmin users
bypass all write rate limits.

## Local Docker build

Build and push to GitHub Container Registry (GHCR):

```sh
# Build only (also tags as commoncal:local)
make docker-build

# Push to GHCR (requires GHCR_TOKEN)
make docker-push

# Build and push
make docker-build-push
```

Override the image tag:

```sh
IMAGE_TAG=v1.2.3 make docker-build-push
```

Set up GHCR authentication:

```sh
# Option 1: export the token directly
export GHCR_TOKEN=ghp_xxx

# Option 2: use the GitHub CLI (token fetched automatically)
gh auth login
```

The script `scripts/docker-build-push.sh` supports `--dry-run` and `--build-only` flags.
Env vars: `IMAGE_TAG`, `DOCKER_REGISTRY`, `IMAGE_NAME`, `DOCKERFILE`, `LOCAL_TAG`, `GHCR_TOKEN`, `DRY_RUN`, `PLATFORMS`.

`make check` runs CI-parity checks on top of the build targets: `docker-check`
builds the core and MCP images locally, and `trivy-check` scans both for
CRITICAL/HIGH vulnerabilities. `trivy-check` requires the `trivy` CLI; set
`SKIP_TRIVY=1` to skip the scan.

### Multi-platform builds

Production nodes are x86_64 (amd64). Building on Apple Silicon (ARM64) without
explicit platform targeting produces an image that **cannot run on amd64 nodes**
(`no match for platform in manifest: not found`).

The build script targets both `linux/amd64` and `linux/arm64` by default.
Override with the `PLATFORMS` env var if needed:

```sh
# amd64 only (faster, smaller)
PLATFORMS=linux/amd64 make docker-build-push

# arm64 only (for ARM nodes)
PLATFORMS=linux/arm64 make docker-build-push
```

Requires `docker buildx` (included with Docker Desktop). Verify with:

```sh
docker buildx version
```

## Production container

Build the production image using `scripts/docker-build-push.sh --build-only`,
then run the bounded runtime acceptance checks (non-root execution, read-only
filesystem, configuration failure, health, frontend, and image contents):

```sh
docker build --tag commoncal:local .
scripts/verify-production-image.sh commoncal:local
```

Run with Docker Compose:

```sh
docker compose up
```

Bootstrap a superadmin for first-run:

```sh
APP_ENV=development cargo run --manifest-path backend/Cargo.toml -- bootstrap-superadmin <email> [display-name]
```
