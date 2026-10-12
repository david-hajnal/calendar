---
name: local-mcp
description: Start and verify CommonCal core, OAuth, and MCP services locally for MCP debugging, using captured email instead of live SMTP.
---

# Local MCP services

Work from the repository root. Start the real core, authorization server, and
MCP implementation with local storage and captured email. Leave services running
and report URLs, validation results, and the stop command.

## Launcher

Read `.agents/AGENTS.md`, `scripts/dev.sh`, and the development Compose files.
Use `scripts/dev.sh` for all local Docker operations, including builds, logs,
inspection, and shutdown.

The existing `start` command starts only core and Vite. Look for an existing
full-stack command first. If absent, implement focused `mcp-start`, `mcp-status`,
`mcp-logs`, and `mcp-stop` commands in that script, backed by a dedicated Compose
file/project. These names are proposed commands, not existing capabilities.
Preserve the core-only commands. Validate shell syntax and Compose configuration
through the launcher, then start the stack. Keep startup reproducible without
hand-run background processes. A local start request authorizes this wiring;
production deployment requires its own request.

## Development stack

Use standalone local configuration rather than production secrets or `.env.local`.
Publish ports on `127.0.0.1`, bind container listeners on `0.0.0.0`, and use
separate named volumes. Keep the OAuth private bridge on the container network.
Suggested public ports: core 3000, optional frontend 5173, OAuth 4000, MCP 3001.

- Core: set `APP_ENV=development`, writable `DATABASE_PATH`, `APP_ORIGIN`,
  `AUTH_BRIDGE_URL` pointing to OAuth's private port 4001,
  `AUTH_BRIDGE_SECRET`, and `MCP_INTERNAL_API_KEY`.
- Email: set core's `E2E_EMAIL_OUTBOX=/app/data/emails.jsonl` with writable storage.
  Development ignores SMTP settings. This JSONL contains private login links;
  report how to inspect it locally without dumping its contents into chat.
- OAuth: run the real `slice1-lab/auth-server/src/server.mjs` development
  entrypoint with PostgreSQL. `Dockerfile.auth` defaults to production: override
  its entrypoint for this path. Configure `DATABASE_URL`, `LAB_ISSUER`,
  `LAB_RESOURCE_URL`, `LAB_COMMONCAL_URL`, `LAB_BRIDGE_KEY`, and public/private
  binds and ports. The bridge key must match core's `AUTH_BRIDGE_SECRET`.
  Mount the auth-server directory's `test-jwks.json` for disposable signing;
  the production Dockerfile omits this fixture.
- MCP: build `Dockerfile.mcp`; set `APP_ENV=development`,
  `BIND_ADDRESS=0.0.0.0:3001`, writable `MCP_DATABASE_PATH`,
  `MCP_PUBLIC_RESOURCE_URL`, `MCP_OAUTH_ISSUER`, `MCP_INTERNAL_API_BASE`,
  and the same `MCP_INTERNAL_API_KEY` as core.

Use a consistent public hostname for browser sessions; mixing `localhost` and
`127.0.0.1` breaks consent cookies. Public issuer/resource URLs must resolve
from the containers too: container loopback is not the host. Provide shared
hostname resolution or a local proxy, and verify discovery inside the containers.
Use service names for private core/bridge traffic. The configured MCP resource
host must match incoming Host headers; `mcp-server/src/transport.rs` enforces it.

Read current configuration from:

- `backend/src/config.rs`, `backend/src/email.rs`, `docs/external/account-email.md`
- `slice1-lab/auth-server/src/server.mjs` and `src/production-config.mjs` in
  that auth-server directory, plus `Dockerfile.auth`
- `mcp-server/src/config.rs`, `mcp-server/src/transport.rs`, `Dockerfile.mcp`

For production-mode reproduction, use the production entrypoints and their
configuration requirements. OAuth production uses SQLite and requires HTTPS;
MCP production also requires HTTPS URLs. Provide local TLS and certificate
trust. Core production requires authenticated, verified TLS SMTP; use a
controlled fixture following `backend/src/smtp/tests.rs`. A plaintext mail
catcher does not satisfy this configuration. Preserve production checks.

## OpenCode client configuration

For OpenCode 2.0.20, entries under `mcp.servers` use snake_case OAuth fields:
`redirect_uri`, `callback_port`, and `auth_server_metadata_url`. CamelCase
`redirectUri`/`callbackPort` belongs to the legacy flat `mcp` entry format.
Using camelCase under `mcp.servers` silently falls back to a random-port
`/callback`, which the auth server correctly rejects. Configure an admitted
loopback URI such as `http://127.0.0.1:7778/mcp/oauth/callback` and reload the
client configuration before retrying sign-in. For other versions, inspect the
installed client's schema/runtime before changing redirect admission.

## Completion

Wait with a bounded timeout for core `/health/ready`, OAuth `/ready`, and MCP
`/health/ready`. Inspect failing logs through the launcher and report unresolved
startup failures with the affected service and cause.

Verify OAuth discovery/JWKS and MCP metadata at
`/.well-known/oauth-protected-resource`. An unauthenticated `/mcp` request
should return the OAuth challenge (401). Health and a 401 prove startup,
not an authenticated tool round trip.

When validation is requested, inspect `mcp-acceptance/README.md` and the actual
harness scripts; some documentation describes earlier server behavior.
Explicitly set local `MCP_URL` and `MCP_OAUTH_ISSUER`: defaults target production.
Exercise real OAuth consent/token exchange and authenticated MCP initialization
and a tool call. Use core's `/dev-login` for development login and the actual
seed commands/UI for calendar access. Report which flows passed and which
remain untested.

Stop through the launcher and retain volumes. Remove state only for a requested
reset. If Docker is unavailable, retain the runnable setup and report the
blocker; do not claim runtime validation passed.
