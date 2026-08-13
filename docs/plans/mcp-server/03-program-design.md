# Program Design: MCP Server

## Files

### New files (mcp-server crate)

**`mcp-server/Cargo.toml`** — crate manifest. Dependencies: `axum`, `tokio`, `serde`, `serde_json`, `sqlx` (sqlite), `hyper`, `http`, `tower`, `tracing`, `jsonwebtoken` (JWT validation), `x509-cert` (mTLS), `sha2`, `uuid`.

**`mcp-server/src/main.rs`** — binary entry point. Loads config from env, creates `Gateway`, connects to DB, builds axum router with `/mcp` route + `/.well-known/oauth-protected-resource`, calls `axum::serve`.

**`mcp-server/src/config.rs`** — `McpConfig` struct parsed from env vars (MCP_OAUTH_ISSUER, MCP_INTERNAL_API_BASE, MCP_INTERNAL_API_KEY, MCP_SESSION_SECRET, MCP_DATABASE_PATH, DPOP_KEY_PATH, etc.). `from_env() -> Result<McpConfig, ConfigError>`.

**`mcp-server/src/db.rs`** — `connect_db(path) -> SqlitePool`, migration runner (4 tables: mcp_grant, delete_intent, idempotency_key, mcp_audit).

**`mcp-server/src/gateway.rs`** — `Gateway` struct holding config, DB pool, internal client. Methods: `new()`, `validate_token(req) -> Result<TokenContext, TokenError>`, `execute_tool(ctx, tool_name, params) -> Result<ToolOutput, ToolError>`, `handle_mcp_request(req) -> Response`.

**`mcp-server/src/oauth.rs`** — OAuth validation module. Types: `AccessToken`, `DpoProof`, `TokenValidationResult`. Functions: `validate_access_token(token, issuer) -> Result<TokenValidationResult, TokenError>`, `validate_dpop_proof(token, proof, nonce) -> Result<(), DpopError>`, `load_jwks(issuer) -> Result<JwkSet, JwksError>`.

**`mcp-server/src/mcp_grant.rs`** — McpGrant persistence. Types: `McpGrant`, `GrantQuery`. Functions: `get_grant(pool, user_id, client_id) -> Result<Option<McpGrant>, DbError>`, `check_calendar_access(grant, calendar_id) -> bool`, `check_tool_permission(grant, tool_name) -> bool`, `revoke_grant(pool, grant_id) -> Result<(), DbError>`.

**`mcp-server/src/tools/mod.rs`** — tool module entry point. Dispatch: `dispatch(tool_name, ctx, params) -> Result<ToolOutput, ToolError>`.

**`mcp-server/src/tools/calendar_list.rs`** — `fn handle(ctx: &TokenContext) -> Result<CalendarListOutput, ToolError>`

**`mcp-server/src/tools/availability_find.rs`** — `fn handle(ctx: &TokenContext, calendar_ids: Vec<String>, from: String, to: String) -> Result<AvailabilityOutput, ToolError>`

**`mcp-server/src/tools/event_get.rs`** — `fn handle(ctx: &TokenContext, calendar_id: String, event_id: String) -> Result<EventOutput, ToolError>`

**`mcp-server/src/tools/event_search.rs`** — `fn handle(ctx: &TokenContext, calendar_ids: Vec<String>, from: String, to: String, query: Option<String>) -> Result<EventSearchOutput, ToolError>`

**`mcp-server/src/tools/event_create.rs`** — `fn handle(ctx: &TokenContext, calendar_id: String, mutation: EventMutation, operation_id: Option<String>) -> Result<EventOutput, ToolError>`

**`mcp-server/src/tools/event_update.rs`** — `fn handle(ctx: &TokenContext, calendar_id: String, event_id: String, mutation: EventMutation, operation_id: Option<String>) -> Result<EventOutput, ToolError>`

**`mcp-server/src/tools/reminder_set.rs`** — `fn handle(ctx: &TokenContext, event_id: String, reminder: ReminderInput) -> Result<ReminderOutput, ToolError>`

**`mcp-server/src/tools/event_delete_prepare.rs`** — `fn handle(ctx: &TokenContext, calendar_id: String, event_id: String) -> Result<DeletePrepareOutput, ToolError>`

**`mcp-server/src/tools/event_delete_commit.rs`** — `fn handle(ctx: &TokenContext, intent_id: String) -> Result<DeleteCommitOutput, ToolError>`

**`mcp-server/src/internal_client.rs`** — HTTP client for CommonCal internal API. Types: `InternalClient`, `InternalRequest`. Functions: `new(api_base, tls_config) -> InternalClient`, `get_user_status(pool, user_id) -> Result<UserStatus, InternalError>`, `get_calendars(pool, user_id) -> Result<Vec<CalendarInfo>, InternalError>`, `get_calendar_role(pool, user_id, calendar_id) -> Result<CalendarRole, InternalError>`, `get_event(pool, calendar_id, event_id) -> Result<EventInfo, InternalError>`, `search_events(pool, calendar_id, range) -> Result<Vec<EventInfo>, InternalError>`, `create_event(pool, calendar_id, mutation) -> Result<EventInfo, InternalError>`, `update_event(pool, calendar_id, event_id, mutation) -> Result<EventInfo, InternalError>`, `create_delete_intent(pool, params) -> Result<DeleteIntent, InternalError>`, `get_delete_intent(pool, intent_id) -> Result<DeleteIntent, InternalError>`, `commit_delete_intent(pool, intent_id) -> Result<(), InternalError>`, `get_mcp_grants(pool, user_id, client_id) -> Result<Vec<McpGrant>, InternalError>`, `create_mcp_grant(pool, grant) -> Result<McpGrant, InternalError>`, `revoke_mcp_grant(pool, grant_id) -> Result<(), InternalError>`, `check_idempotency(pool, key) -> Result<Option<IdempotentResult>, InternalError>`, `record_idempotency(pool, key) -> Result<(), InternalError>`, `exchange_token(ctx) -> Result<InternalToken, InternalError>`.

**`mcp-server/src/security.rs`** — security middleware. Types: `RiskTier` (0-3), `RateLimiter`. Functions: `classify_risk(tool_name) -> RiskTier`, `check_rate_limit(tier, user_id, client_id) -> Result<(), RateLimitError>`, `check_auth_strength(auth_strength, tier) -> Result<(), AuthError>`, `require_recent_auth(auth_time, max_age) -> Result<(), AuthError>`.

**`mcp-server/src/audit.rs`** — audit logging. Functions: `log_invocation(ctx, tool, resource_ids, auth_result, latency, result_type, operation_id)`, `log_deletion(ctx, event_id, event_version, confirmation_method)`.

**`mcp-server/src/error.rs`** — error types: `TokenError`, `GrantError`, `ToolError`, `InternalError`, `SecurityError`, `ConfigError`. Conversion to HTTP status codes.

**`mcp-server/src/output_schema.rs`** — output schema types for structured MCP responses. Types: `ToolOutput`, `CalendarListOutput`, `AvailabilityOutput`, `EventOutput`, `EventSearchOutput`, `ReminderOutput`, `DeletePrepareOutput`, `DeleteCommitOutput`, `ConfirmationUrl`.

### Changed files (existing backend)

**`backend/migrations/0020_mcp_grants.sql`** — new migration. Creates `mcp_grant` table + index in CommonCal SQLite.

**`backend/src/internal_mcp.rs`** (new) — internal MCP API handlers. Functions: `get_user_status`, `list_calendars_for_mcp`, `get_calendar_role_for_mcp`, `get_event_for_mcp`, `search_events_for_mcp`, `create_event_for_mcp`, `update_event_for_mcp`, `create_delete_intent`, `get_delete_intent`, `commit_delete_intent`, `get_mcp_grants`, `create_mcp_grant`, `revoke_mcp_grant`, `check_idempotency`, `record_idempotency`.

**`backend/src/http.rs`** — add `/internal/mcp/*` routes behind mTLS middleware.

**`backend/src/lib.rs`** — add `pub mod internal_mcp;`.

**`backend/src/main.rs`** — wire `internal_mcp` routes into router.

**`frontend/src/routes/settings/mcp-grants.tsx`** (new) — McpGrant management page component.

**`frontend/src/routes/settings/mcp-grants/edit.tsx`** (new) — Edit permissions modal.

**`frontend/src/routes/mcp/confirm/[intent_id].tsx`** (new) — Deletion confirmation page.

## Types & signatures

```rust
// mcp-server/src/config.rs
pub struct McpConfig {
    pub oauth_issuer: String,
    pub internal_api_base: String,
    pub internal_api_key: String,
    pub session_secret: String,
    pub database_path: PathBuf,
    pub dpop_key_path: Option<PathBuf>,
    pub rate_limit_enabled: bool,
    pub tracing_level: String,
}

// mcp-server/src/oauth.rs
pub struct TokenValidationResult {
    pub user_id: i64,
    pub oauth_client_id: String,
    pub scopes: Vec<String>,
    pub auth_strength: AuthStrength,
    pub auth_time: i64,
    pub token_id: String,
    pub expires_at: i64,
}

pub enum AuthStrength {
    Passwordless,
    Passkey,
    Mfa,
}

// mcp-server/src/mcp_grant.rs
pub struct McpGrant {
    pub grant_id: String,
    pub user_id: i64,
    pub oauth_client_id: String,
    pub allowed_calendar_ids: Vec<i64>,
    pub allow_availability: bool,
    pub allow_event_titles: bool,
    pub allow_event_details: bool,
    pub allow_create: bool,
    pub allow_update: bool,
    pub allow_delete: bool,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

// mcp-server/src/security.rs
pub enum RiskTier {
    Tier0,  // availability — read-only, minimal data
    Tier1,  // event details — sensitive read
    Tier2,  // create/update — mutation + strong auth
    Tier3,  // delete — step-up + confirmation
}

// mcp-server/src/gateway.rs
pub struct Gateway {
    config: McpConfig,
    db_pool: SqlitePool,
    internal_client: InternalClient,
    rate_limiter: Arc<RateLimiter>,
}

impl Gateway {
    pub async fn new(config: McpConfig) -> Result<Self, GatewayError>;
    pub async fn validate_token(&self, req: &McpRequest) -> Result<TokenContext, TokenError>;
    pub async fn execute_tool(&self, ctx: TokenContext, tool_name: &str, params: Value) -> Result<ToolOutput, ToolError>;
    pub async fn handle_mcp_request(&self, req: Request) -> Response;
}

// mcp-server/src/tools/*.rs — per-tool handlers
pub struct CalendarListOutput { calendars: Vec<CalendarSummary> }
pub struct AvailabilityOutput { slots: Vec<AvailabilitySlot> }
pub struct EventOutput { event: EventSummary, access: AccessLevel }
pub struct EventSearchOutput { events: Vec<EventSummary>, next_page: Option<String> }
pub struct DeletePrepareOutput { intent_id: String, event_summary: EventSummary, expires_at: i64, confirmation_required: bool }
pub struct DeleteCommitOutput { deleted: bool }
pub struct ConfirmationUrl { url: String }

// mcp-server/src/audit.rs
pub fn log_invocation(
    pool: &SqlitePool,
    user_id: i64,
    client_id: String,
    grant_id: Option<String>,
    tool: &str,
    resource_ids: Option<String>,
    auth_result: &str,
    scope: Option<String>,
    auth_strength: &str,
    latency_ms: u64,
    result_type: &str,
    operation_id: Option<String>,
) -> Result<(), AuditError>;
```

## Call stack

### Tool execution (main flow)
```
Gateway::handle_mcp_request(req)
  └─ Gateway::validate_token(req)
       ├─ oauth::validate_access_token(token, issuer)
       ├─ oauth::validate_dpop_proof(token, proof)
       ├─ mcp_grant::get_grant(user_id, client_id)
       ├─ security::classify_risk(tool_name) -> RiskTier
       ├─ security::check_rate_limit(tier, user_id, client_id)
       └─ security::check_auth_strength(auth_strength, tier)
  └─ tools::dispatch(tool_name, ctx, params)
       └─ tools::{tool_name}::handle(ctx, params)
            └─ internal_client::{operation}(ctx, params)
                 └─ internal_client::exchange_token(ctx) -> InternalToken
                 └─ reqwest::post(internal_api_url) with InternalToken + mTLS
  └─ audit::log_invocation(...)
```

### Two-phase delete
```
Gateway::execute_tool("event_delete_prepare", ctx, params)
  └─ tools::event_delete_prepare::handle(ctx, calendar_id, event_id)
       └─ internal_client::get_event(ctx, calendar_id, event_id)
       └─ internal_client::create_delete_intent(ctx, event_id, event_version)
  └─ audit::log_invocation(...)

Gateway::execute_tool("event_delete_commit", ctx, params)
  └─ tools::event_delete_commit::handle(ctx, intent_id)
       └─ internal_client::get_delete_intent(ctx, intent_id)
       └─ internal_client::get_event(ctx, calendar_id, event_id)  // verify event unchanged
       └─ internal_client::commit_delete_intent(ctx, intent_id)
  └─ audit::log_deletion(...)
```

### McpGrant revocation (frontend flow)
```
Frontend: DELETE /api/v1/settings/mcp-grants/:grant_id
  └─ Backend: revoke_mcp_grant(grant_id)
       └─ sqlx::UPDATE mcp_grant SET revoked_at = now WHERE grant_id = ?
       └─ sqlx::DELETE FROM mcp_grant WHERE grant_id = ?  (or soft-delete)
```

## Test plan

### OAuth validation
- `test_validate_token_valid_dpop` — valid token + valid DPoP proof → success
- `test_validate_token_expired` — expired token → TokenError::Expired
- `test_validate_token_wrong_audience` — token with aud=api.commoncal.tld → TokenError::InvalidAudience
- `test_validate_token_missing_dpop` — no DPoP proof on DPoP-required endpoint → TokenError::MissingDpop
- `test_validate_token_invalid_dpop` — tampered DPoP proof → TokenError::InvalidDpop
- `test_validate_token_wrong_issuer` — token from unknown issuer → TokenError::InvalidIssuer
- `test_validate_token_revoked` — revoked token → TokenError::Revoked

### McpGrant enforcement
- `test_grant_calendar_access_allowed` — calendar in allowed list → true
- `test_grant_calendar_access_denied` — calendar not in allowed list → false
- `test_grant_tool_permission_allowed` — tool permission enabled → true
- `test_grant_tool_permission_denied` — tool permission disabled → false
- `test_grant_revoked` — revoked_at is set → none
- `test_grant_expired` — expires_at < now → none
- `test_grant_no_grant_exists` — no McpGrant record → none

### Authorization pipeline
- `test_auth_full_pass` — valid token + valid grant → ToolOk
- `test_auth_token_expired_denies` — expired token → ToolError
- `test_auth_grant_revoked_denies` — revoked grant → ToolError
- `test_auth_calendar_not_in_grant_denies` — calendar not in McpGrant → ToolError
- `test_auth_tool_permission_denied` — tool permission off → ToolError
- `test_auth_rate_limit_exceeded` — rate limit hit → ToolError
- `test_auth_weak_auth_tier2` — passkey auth on Tier2 → AuthError
- `test_auth_no_internal_acl` — user has no calendar ACL → ToolError

### Tool handlers
- `test_calendar_list_happy` — valid ctx → list of calendars
- `test_calendar_list_no_grant` — no McpGrant → ToolError
- `test_availability_find_happy` — valid ctx + time range → availability slots
- `test_availability_find_range_too_large` — >31 days → ToolError
- `test_event_get_happy` — valid ctx → event summary
- `test_event_get_no_details_permission` — allow_event_details=false → summary without details
- `test_event_search_happy` — valid ctx + search → event list
- `test_event_search_max_events` — 100 events returned, no more
- `test_event_create_happy` — valid ctx → created event
- `test_event_create_no_create_permission` → ToolError
- `test_event_create_empty_title` → ToolError
- `test_event_update_happy` — valid ctx → updated event
- `test_event_update_conflict` — stale version → ToolError
- `test_event_delete_prepare_happy` → intent_id + event summary
- `test_event_delete_prepare_event_changed` → ToolError
- `test_event_delete_commit_happy` — valid intent → deleted
- `test_event_delete_commit_intent_expired` → ToolError
- `test_event_delete_commit_intent_used` → ToolError
- `test_event_delete_commit_event_changed` → ToolError
- `test_event_delete_commit_no_delete_permission` → ToolError

### Internal client
- `test_internal_client_mtls_connect` — mTLS connection succeeds
- `test_internal_client_token_exchange` — returns short-lived token
- `test_internal_client_bad_response` — backend returns 500 → InternalError

### Idempotency
- `test_idempotency_same_key_returns_cached` — same operation_id → cached result
- `test_idempotency_different_args_rejected` — same key, different args → error
- `test_idempotency_expired_key_ignored` — >24h old → treated as new

### Audit
- `test_audit_log_written` — invocation logged with all fields
- `test_audit_no_credentials_logged` — no tokens/keys in audit record

## Least confident decisions

1. **McpGrant storage split** — McpGrant stored in BOTH CommonCal DB (source of truth) AND MCP service SQLite (cache). This creates a synchronization question. Decision: MCP service reads McpGrant from CommonCal internal API on every request (no local cache). Local SQLite McpGrant table is for McpGrant management (create/update/revoke by user), not for authorization lookups.

2. **MCP protocol implementation** — whether to use an existing Rust MCP SDK (e.g., `mcp` crate) or implement Streamable HTTP transport manually. Existing SDKs may not support the 2026-07-28 protocol version. Decision: implement minimal Streamable HTTP transport first, add SDK later if one matures.

3. **Internal API auth** — mTLS vs. shared secret for internal API authentication. mTLS is stronger but adds operational complexity. Decision: use a shared `MCP_INTERNAL_API_KEY` header for v1, migrate to mTLS when k3s NetworkPolicy + service mesh is in place.

4. **Deletion confirmation URL** — the elicited confirmation URL contains only a random intent handle (no user ID, no event data). The confirmation page must independently authenticate the user. Decision: use CommonCal's existing session mechanism — the user visits the URL while logged in, the page checks the session, then presents the deletion confirmation.

5. **Rate limiting implementation** — fixed-window vs. sliding-window. Fixed window is simpler and sufficient for v1. Decision: fixed-window rate limiter (same pattern as existing `FixedWindowRateLimiter` in commoncal-backend).
