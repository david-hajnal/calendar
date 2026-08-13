# Slice Plan: MCP Server

## Slices

- [ ] Slice 1 — tracer bullet: mcp-server crate scaffold, empty tools/list, axum router with /mcp route, health check
- [ ] Slice 2 — internal API client: InternalClient struct, token exchange, mTLS header auth, 12 internal endpoint methods, tests
- [ ] Slice 3 — OAuth validation: token signature/issuer/audience/expiry validation, JWKS fetching, TokenContext type
- [ ] Slice 4 — McpGrant model + enforcement: McpGrant type, get_grant, check_calendar_access, check_tool_permission, tests
- [ ] Slice 5 — calendar_list tool: full authorization pipeline + calendar_list wired to internal API
- [ ] Slice 6 — availability_find tool: availability_find with time range validation (max 31 days), access level control
- [ ] Slice 7 — event_get tool: event_get with access level (details vs free_busy), tool permission enforcement
- [ ] Slice 8 — event_search tool: event_search with time range, max 100 events, pagination token
- [ ] Slice 9 — event_create tool: event_create with input validation, empty title rejection, tool permission check
- [ ] Slice 10 — event_update tool: event_update with version conflict detection, tool permission check
- [ ] Slice 11 — event_delete_prepare + commit: two-phase delete, intent creation, intent expiry, event version check on commit
- [ ] Slice 12 — reminder_set tool: reminder_set with event existence check, tool permission check
- [ ] Slice 13 — idempotency: idempotency key storage in MCP SQLite, replay detection, expired key cleanup, tests
- [ ] Slice 14 — rate limiting: fixed-window rate limiter per tool risk tier, rate limit headers in response
- [ ] Slice 15 — audit logging: mcp_audit table, log_invocation, log_deletion, credential sanitization verification
- [ ] Slice 16 — DPoP validation: DPoP proof validation, key pair loading, compatibility mode for non-DPoP clients
- [ ] Slice 17 — McpGrant management API + frontend: 5 new frontend routes, edit permissions modal, delete confirmation page
- [ ] Slice 18 — k3s manifests + container hardening: namespace, deployment, NetworkPolicy, security context, Dockerfile
