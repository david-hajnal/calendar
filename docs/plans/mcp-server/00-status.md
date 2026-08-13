# Status: MCP Server

- Gate 1 — Product: APPROVED 2026-08-11
- Gate 2 — Architecture: APPROVED 2026-08-11
- Gate 3 — Program Design: APPROVED 2026-08-11
- Gate 4 — Slice plan: APPROVED 2026-08-11

## Slices
- [x] Slice 1 — tracer bullet: stub MCP server with tools/list returning empty tool catalog
- [x] Slice 2 — internal API client: InternalClient struct, token exchange (RFC 8693), 12 internal endpoint methods, 14 tests
- [x] Slice 3 — OAuth validation: JWT signature via JWKS, issuer/audience/expiry validation, TokenContext type, 23 tests
- [x] Slice 4 — McpGrant model + enforcement: real DB lookup (query_as + FromRow), check_calendar_access, check_tool_permission, revoke_grant, GrantError::Db, 32 tests
- [x] Slice 5 — calendar_list tool: full authorization pipeline (token extraction → validation → McpGrant check → internal API → filter), calendar_list.rs handler, gateway.rs bearer token + calendar_list route, 8 tests
- [x] Slice 6 — availability_find tool: time range validation (max 31 days), ISO 8601 + Unix epoch parsing, access level control, availability_find.rs handler, 15 tests
- [x] Slice 7 — event_get tool: access level control (full vs basic), grant calendar check, event_fetch via internal API, EventDescription untrusted tag, 6 tests
- [x] Slice 8 — event_search tool: time range validation, max 100 events, pagination token, access level control, 5 tests
- [x] Slice 9 — event_create tool: input validation (empty title, 256 char limit, 10000 char desc limit), create permission check, calendar access, 7 tests
- [x] Slice 10 — event_update tool: input validation (title, desc, location limits), version conflict detection (409→Conflict), calendar access, 8 tests
- [x] Slice 11 — event_delete_prepare + event_delete_commit: two-phase delete, intent creation (24h expiry), intent expiry check, already-committed conflict, 6 tests
- [x] Slice 12 — reminder_set tool: input validation (0 < minutes <= 10080), delete permission required, calendar access, 6 tests
- [x] Slice 13 — audit logging: real DB writes (mcp_audit INSERT), log_invocation + log_deletion with sqlx, 3 tests
- [x] Slice 14 — deletion confirmation URL elicitation: confirmation_url field in DeletePrepareOutput, InternalClient::api_base() accessor, 0 new tests (existing test updated)
- [x] Slice 15 — DPoP validation: proof header typ check (dpop+jwt), jwk presence check, base64url decoding, 4 tests
- [x] Slice 16 — anomaly detection hooks: check_anomalies (brute force, off-hours, rate limit), record_anomaly, classify_risk, check_auth_strength, 15 tests
- [x] Slice 17 — k3s manifests + container hardening: Dockerfile (multi-stage, non-root user), namespace/secret/deployment/service/PVC k8s manifests, liveness/readiness/startup probes, resource limits, rolling update strategy

- [x] Slice 18 — integration tests: wiremock mock JWKS server, rate limiter disabled mode, McpGrant serialization, output schema serialization, error display, risk classification, auth strength checks, anomaly detection, config/grant current_time validation, 11 tests
## Backend Integration (post-slice work)
- [x] `0020_mcp_server.sql` migration — mcp_grant, delete_intent, idempotency_key, mcp_audit tables
- [x] `backend/src/mcp_internal.rs` — 14 internal API handlers (token exchange, user status, calendars, events, delete intents, grants, idempotency, reminders)
- [x] `backend/src/mcp_grant_management.rs` — 5 McpGrant CRUD routes (list, create, update, revoke, resend)
- [x] `backend/src/lib.rs` — added mcp_internal + mcp_grant_management modules
- [x] `backend/src/main.rs` — wired MCP routes into router with SqlitePool state
- [x] `backend/src/http.rs` — added patch to routing imports

## E2E Integration Tests
- [x] `tests/e2e.rs` — 12 tests covering all tool output schemas, mock internal API flows, security module integration, error type integration

## Production Hardening
- [x] `k8s/network-policy.yaml` — ingress from ingress-nginx/traefik only, egress to DNS + OAuth issuer (443) + CommonCal backend (8080), no direct internet
- [x] `.github/workflows/ci-cd.yaml` — test → clippy → build → push to GHCR → deploy to k3s, GHA cache, rollout verification

## Notes for a fresh session
- Full MCP security architecture doc: docs/plan/mcp.md (1517 lines)
- Existing backend: Rust/axum + SQLite (commoncal-backend)
- MCP is a **separate** Rust service, not a module in the existing backend
- Core security rule: MCP client authority < user authority
- Transport: MCP Streamable HTTP over HTTPS only
- OAuth 2.1 + PKCE + DPoP for client auth
- MCP service acts as OAuth resource server, not auth server
- Internal API via OAuth Token Exchange (RFC 8693) with short-lived tokens
- No SQLite access from MCP service
