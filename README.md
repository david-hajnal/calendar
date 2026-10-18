# CommonCal

Multi-user calendar platform with email-based authentication, shared composite views,
public calendar sharing, one-time editable ICS file import, external ICS feed ingestion,
Apple Calendar (CalDAV) account sync, and encrypted backup/restore.

Admins manage invitations, resends, account access, and email changes under
Settings → Users. Users can recover passwords and confirm email changes under
Settings → Account. See the [account management guide](docs/ACCOUNT-MANAGEMENT.md).

## Connect an AI assistant

CommonCal exposes a remote MCP server at `https://mcal.hajnal.space/mcp`.
The recipes below are under compatibility verification; see the version table
for current results. You need a CommonCal account with access to a calendar, an installed assistant
CLI, and a browser on the same machine to finish OAuth login. The deployment
must have its authorization service enabled. Use the browser login and consent
flow; **never paste access tokens, refresh tokens, client secrets, internal API
keys, or session cookies into an assistant, config file, or issue**.

Choose your client below. During consent, check the client and calendar names
and approve only the capabilities you need. Calendar visibility and event
titles/details are separate permissions; create, update, and delete require
their own permissions. Denying a new authorization creates no grant. Removing a connection
locally and revoking its CommonCal grant are separate actions.

### OpenCode

Add the server to your user configuration (available in every project):

<!-- mcp-snippet: opencode add -->
```sh
opencode mcp add commoncal --global --url https://mcal.hajnal.space/mcp
```

This writes a remote `mcp.servers.commoncal` entry to `~/.config/opencode/opencode.json`. If you already
have a server with that name, edit its URL instead of adding a duplicate. For
a deterministic configuration, merge this entry into your existing file:

<!-- mcp-config: opencode -->
```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "servers": {
      "commoncal": {
        "type": "remote",
        "url": "https://mcal.hajnal.space/mcp"
      }
    }
  }
}
```

Run `opencode mcp auth commoncal`, finish browser login, and review consent.
Then check the connection:

<!-- mcp-snippet: opencode list -->
```sh
opencode mcp list
```

It should report a connected server. In OpenCode, ask: “Use CommonCal's
`calendar_list` tool to list my calendars.” A successful tool result is the
connection check; a saved configuration alone does not prove calendar access.
LM Studio or another model provider supplies the model; OpenCode manages MCP.

To disconnect, clear the stored credentials:

<!-- mcp-snippet: opencode remove -->
```sh
opencode mcp logout commoncal
```

Then remove `mcp.servers.commoncal` from `~/.config/opencode/opencode.json` (or set `disabled` to `true`
to disable it temporarily). Restart OpenCode and confirm it is absent or disabled.

### Codex

<!-- mcp-snippet: codex add -->
```sh
codex mcp add commoncal --url https://mcal.hajnal.space/mcp
```

Complete browser authorization when prompted. If needed, run
`codex mcp login commoncal --oauth-client-registration dcr --scopes commoncal.calendar.metadata.read`
to retry with read-only calendar metadata access. Check the saved connection:

<!-- mcp-snippet: codex list -->
```sh
codex mcp list
```

Start a new Codex session, inspect `/mcp`, and ask it to call CommonCal's
`calendar_list`. To remove the connection:

<!-- mcp-snippet: codex remove -->
```sh
codex mcp remove commoncal
```

### Claude Code

Add an HTTP server in user scope:

<!-- mcp-snippet: claude add -->
```sh
claude mcp add --transport http --scope user commoncal https://mcal.hajnal.space/mcp
```

Start Claude Code, open `/mcp`, select CommonCal, and finish browser
authentication. Check the server:

<!-- mcp-snippet: claude list -->
```sh
claude mcp list
```

Ask Claude Code to call CommonCal's `calendar_list`. To remove it:

<!-- mcp-snippet: claude remove -->
```sh
claude mcp remove --scope user commoncal
```

### Revoke access in CommonCal

Local logout or removal does not substitute for server-side revocation. While
signed into CommonCal, use the authenticated grant-management API:
`GET /api/v1/mcp-grants` lists your active grants with `grant_id` and
`oauth_client_id`; `DELETE /api/v1/mcp-grants/{grant_id}` revokes the selected
grant. Send the delete from the same CommonCal origin using the existing
session and CSRF protection; do not copy session credentials into a CLI.
There is currently no grant-management settings screen. See the
[connection guide](docs/MCP-CONNECTION.md#server-side-revocation) for a browser
procedure. After revocation, another `calendar_list` call must fail for that
connection. Reconnect through browser consent to grant access again.

### Versions and troubleshooting

| Client | Local command verification | Full live support proof |
| --- | --- | --- |
| OpenCode | v2.0.20: add, JSON and DCR callback checked; isolated list/logout checks fail | Browser/calendar/refresh/revoke matrix still to be recorded |
| Codex | 0.163.0-alpha.10.2 (bundled preview) | Released minimum version still to be established |
| Claude Code | Official CLI syntax; CLI unavailable in this verification environment | Minimum version and live matrix still to be recorded |

These are setup instructions, not a claim that all three clients have passed
the release matrix. The [evidence and verification guide](docs/MCP-CONNECTION.md)
records the remaining checks. Use a current client with remote HTTP MCP and
OAuth support; a minimum supported release is published only after live proof.

- **OpenCode v2 lists no servers after adding:** check `opencode debug config`
  and the selected configuration scope. This occurs in the isolated v2.0.20
  checks; do not treat a successful add as proof of connection.
- **OpenCode registration rejects `/callback`:** the operator must deploy the
  updated auth callback policy before retrying; do not supply a token instead.
- **Needs authentication / 401:** retry the client's browser login. After a
  revoked grant, re-approve consent rather than reusing old credentials.
- **Browser cannot return to the CLI:** keep the CLI running and use a browser
  on the same machine. Remote shells need an explicitly supported callback
  setup; do not add an arbitrary redirect URL or disable callback checks.
- **Consent denied or expired:** rerun login for a fresh authorization request.
- **Connected but missing calendars:** check account, selected calendars,
  current membership, and consent permissions. Ask for `calendar_list` first.
- **Discovery, registration, or schema errors:** check the URL and client
  version; the operator should follow [authorization operations](docs/AUTH-PRODUCTION.md).
  Report the error and version with credentials and authorization URLs removed.

Official references: [OpenCode MCP](https://opencode.ai/v2/docs/mcp-servers),
[Codex MCP](https://developers.openai.com/codex/mcp/), and
[Claude Code MCP](https://code.claude.com/docs/en/mcp).

## Tech stack

- **Backend**: Rust (Axum, SQLite, sqlx, tokio)
- **Frontend**: React 19, Vite 7, TypeScript
- **Infra**: Docker multi-stage build, Helm charts
- **Auth**: Password and email-link login, invitation setup, password recovery,
  verified email changes, and session cookies (`__Host-commoncal_session` on HTTPS)
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
- **ICS file import** — upload a local `.ics` file into an existing writable calendar; imported events are native and editable, with no ongoing synchronization
- **External ICS feeds** — subscribe to remote calendars with configurable refresh intervals
- **Notifications** — in-app notification surface with event-based notification replanning
- **Admin API** — user management (list, suspend, reactivate, promote, demote), invitation management
- **Backup & restore** — encrypted backup (AES-256-GCM) with SHA-256 integrity verification
- **Multi-session** — inspect and revoke active sessions
- **Rate limiting** — per-user write rate limiting on authenticated endpoints (enabled in staging/production)
- **Apple Calendar sync** — two-way CalDAV account connection for macOS/iOS Calendar with revocable per-device passwords, incremental sync, and full CRUD

## API overview

| Area | Key endpoints |
|------|---------------|
| Auth | `POST /api/v1/auth/login-links`, `POST /api/v1/auth/login-links/consume`, `GET/DELETE /api/v1/auth/session`, `DELETE /api/v1/auth/sessions` |
| Admin | `GET /api/v1/admin/users`, `POST/DELETE /api/v1/admin/invitations/*`, user suspend/reactivate/promote/demote, `POST /api/v1/admin/users/:id/revoke-sessions` |
| Calendars | `GET/POST /api/v1/calendars`, `GET/PATCH/DELETE /api/v1/calendars/:id`, `POST /api/v1/calendars/:id/archive|restore`, ACL, ownership transfer |
| ICS import | `POST /api/v1/calendars/:id/import-ics` accepts a one-time `text/calendar` upload and creates editable native events |
| Events | `GET/POST /api/v1/calendars/:id/events`, `GET/PATCH/DELETE /api/v1/calendars/:id/events/:id`, occurrence overrides |
| Views | `GET/POST /api/v1/views`, `GET/PATCH/DELETE /api/v1/views/:id`, calendar composition, publication management |
| Feeds | `GET/POST /api/v1/calendars/:id/external-feeds`, `DELETE/POST /api/v1/external-feeds/:id/disable|refresh` |
| Notifications | `GET /api/v1/notifications` |
| Public | `GET /api/v1/public/views/:token`, `GET /api/v1/public/views/:token/events` |
| Apple Calendar | `GET/DELETE /api/v1/calendar-connections/apple`, `POST /api/v1/calendar-connections/apple/passwords`, `DELETE /api/v1/calendar-connections/apple/passwords/:id`, `GET /.well-known/caldav`, `PROPFIND/REPORT/PUT/DELETE /dav/*` |
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

Production invitations and account recovery require authenticated TLS SMTP and
`PASSWORD_LOGIN_ENABLED=true`. See [account email setup](docs/external/account-email.md)
for required environment variables, Helm secret mappings, and delivery checks.
Development captures mail locally.

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
