# Implementation Plan: Rate Limiting on Authenticated Write Endpoints

## 1. Algorithm Choice

**Fixed window rate limiter** (reuses pattern from `FixedWindowLoginRateLimiter` in `login.rs:47`).

Rationale:
- Already exists in codebase, proven pattern
- Simple, no external dependencies
- Adequate for per-user authenticated rate limiting (not DDoS mitigation)
- Easy to test with mock clock
- Window size: 60 seconds per tier

## 2. Rate Limit Configuration Tiers

| Tier | Endpoints | Limit | Rationale |
|------|-----------|-------|-----------|
| **Critical** | `PUT /api/v1/calendars/:id/acl/:user_id`<br>`DELETE /api/v1/calendars/:id/acl/:user_id`<br>`POST /api/v1/calendars/:id/transfer` | 10 req / 60s | ACL/transfer = privilege escalation vector |
| **Standard** | `POST /api/v1/calendars/:calendar_id/events`<br>`PATCH /api/v1/calendars/:calendar_id/events/:event_id`<br>`DELETE /api/v1/calendars/:calendar_id/events/:event_id`<br>`PATCH .../occurrences/:recurrence_id`<br>`PATCH .../occurrences/:recurrence_id/following`<br>`POST /api/v1/calendars/:id/external-feeds`<br>`DELETE /api/v1/external-feeds/:feed_id`<br>`POST .../external-feeds/:feed_id/disable`<br>`POST .../external-feeds/:feed_id/refresh` | 30 req / 60s | Event/feed mutations, normal usage |
| **Permissive** | `POST /api/v1/calendars`<br>`PATCH /api/v1/calendars/:id`<br>`DELETE /api/v1/calendars/:id`<br>`POST /api/v1/calendars/:id/archive`<br>`POST /api/v1/calendars/:id/restore`<br>`POST/DELETE /api/v1/views/:id/*` | 60 req / 60s | Calendar/view management, lower frequency |

**Bypass**: Superadmin users (`session.user.is_superadmin == true`) bypass all write rate limits.

**Key**: Per-user (`user:{user_id}`), not per-calendar. User is the identity boundary.

## 3. File Changes

### 3.1 New file: `backend/src/rate_limiter.rs`

```rust
// Core types and implementation (~180 lines)

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

/// Fixed window rate limiter with configurable window and per-key counters.
/// Thread-safe, uses in-memory HashMap.
pub struct FixedWindowRateLimiter {
    max_requests: u32,
    window_seconds: i64,
    buckets: Mutex<HashMap<String, RateLimitBucket>>,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}

pub struct RateLimitConfig {
    pub max_requests: u32,
    pub window_seconds: i64,
}

/// Rate limit tiers keyed by endpoint path pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateLimitTier {
    Critical,  // 10 req / 60s
    Standard,  // 30 req / 60s
    Permissive, // 60 req / 60s
}

impl RateLimitTier {
    pub fn config(&self) -> RateLimitConfig { ... }
}

/// Returns the tier for a given write endpoint path.
/// Returns None for non-write or non-matching paths.
pub fn write_endpoint_tier(method: &str, path: &str) -> Option<RateLimitTier> { ... }

/// Rate limit key for a write endpoint.
pub struct WriteRateLimitKey {
    pub user_id: i64,
    pub tier: RateLimitTier,
}

impl FixedWindowRateLimiter {
    pub fn new(max_requests: u32, window_seconds: i64) -> Self { ... }
    pub fn new_with_clock(max_requests: u32, window_seconds: i64, clock: Arc<dyn Fn() -> i64 + Send + Sync>) -> Self { ... }

    /// Check if request is allowed. Returns (allowed, retry_after_seconds).
    pub fn check(&self, key: &WriteRateLimitKey) -> (bool, i64) { ... }
}
```

**Implementation details**:

`FixedWindowRateLimiter::check()` logic (mirrors `login.rs:79-99`):
1. Compute bucket key: `format!("user:{}:tier:{}", key.user_id, tier_name(key.tier))`
2. Get current time from clock
3. Lock buckets HashMap
4. Get or create bucket for key
5. If `now - bucket.window_started_at >= window_seconds`: reset bucket (new window)
6. If `bucket.attempts >= max_requests`: return `(false, window_seconds - (now - bucket.window_started_at))`
7. Increment `bucket.attempts`, return `(true, 0)`

`write_endpoint_tier()` logic:
1. Match on `method` (must be POST/PATCH/DELETE)
2. Match on `path` patterns:
   - `*/acl/*/` + (PUT or DELETE) -> Critical
   - `*/transfer` + POST -> Critical
   - `*/events` + (POST/PATCH/DELETE) -> Standard
   - `*/occurrences/*/` + (PATCH or DELETE) -> Standard
   - `*/occurrences/*/following` + PATCH -> Standard
   - `*/external-feeds/*/` + (DELETE) -> Standard
   - `*/external-feeds/*/disable` + POST -> Standard
   - `*/external-feeds/*/refresh` + POST -> Standard
   - `*/calendars` + (POST/PATCH/DELETE) -> Permissive
   - `*/calendars/*/archive` + POST -> Permissive
   - `*/calendars/*/restore` + POST -> Permissive
   - `*/views/*/` + (POST/PATCH/DELETE) -> Permissive
3. Return None for non-matching paths

### 3.2 New file: `backend/src/write_rate_limit.rs`

```rust
// Middleware (~60 lines)

use axum::{
    Extension,
    http::{HeaderName, Request, StatusCode},
    middleware::Next,
    response::Response,
    State,
};
use crate::rate_limiter::{FixedWindowRateLimiter, WriteRateLimitKey, write_endpoint_tier};
use crate::sessions::AuthenticatedSession;

pub struct WriteRateLimiterState {
    pub limiter: FixedWindowRateLimiter,
}

/// Middleware that applies per-user rate limiting to write endpoints.
/// Must run AFTER authenticated_session (which provides AuthenticatedSession).
pub async fn write_rate_limit_middleware(
    State(limiter_state): State<WriteRateLimiterState>,
    Extension(session): Extension<AuthenticatedSession>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, RateLimitExceeded> {
    // Superadmin bypass
    if session.user.is_superadmin {
        return Ok(next.run(request).await);
    }

    // Only rate limit write methods
    let tier = match write_endpoint_tier(request.method().as_str(), request.uri().path()) {
        Some(tier) => tier,
        None => return Ok(next.run(request).await),
    };

    let key = WriteRateLimitKey {
        user_id: session.user.id,
        tier,
    };

    let (allowed, retry_after) = limiter_state.limiter.check(&key);

    if !allowed {
        tracing::warn!(
            user_id = session.user.id,
            tier = ?tier,
            retry_after = retry_after,
            "write endpoint rate limited"
        );
        return Err(RateLimitExceeded { retry_after });
    }

    Ok(next.run(request).await)
}

#[derive(Debug)]
pub struct RateLimitExceeded {
    pub retry_after: i64,
}

impl axum::response::IntoResponse for RateLimitExceeded {
    fn into_response(self) -> Response {
        let mut response = Response::new(axum::body::Body::from(
            serde_json::json!({
                "error": {
                    "code": "rate_limited",
                    "message": "Too many requests, try again later",
                }
            }).to_string(),
        ));
        *response.status_mut() = StatusCode::TOO_MANY_REQUESTS;
        response.headers_mut().insert(
            HeaderName::from_static("x-retry-after"),
            axum::http::HeaderValue::from(self.retry_after),
        );
        response
    }
}
```

### 3.3 Modify: `backend/src/lib.rs`

Add module declarations:
```rust
pub mod rate_limiter;
pub mod write_rate_limit;
```

### 3.4 Modify: `backend/src/http.rs`

**Change 1: Update `ApplicationState` struct (line 2432-2446)**

Add field:
```rust
pub write_rate_limiter: Option<WriteRateLimiterState>,
```

**Change 2: Update `authenticated_session` middleware (line 1862-1876)**

The middleware currently returns `Result<Response, ApiError>`. Modify to also accept the rate limiter state and call the rate limit check before `next.run()`:

```rust
async fn authenticated_session(
    State(manager): State<SessionManager>,
    State(rate_limiter): Option<State<WriteRateLimiterState>>,
    mut request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, ApiError> {
    let session = manager
        .authenticate(session_cookie(request.headers()))
        .await
        .map_err(map_session_error)?;

    // Rate limit check (only when rate limiter is configured)
    if let Some(limiter_state) = rate_limiter {
        use crate::rate_limiter::{WriteRateLimitKey, write_endpoint_tier};
        use crate::write_rate_limit::WriteRateLimiterState;

        if session.user.is_superadmin {
            // bypass
        } else if let Some(tier) = write_endpoint_tier(request.method().as_str(), request.uri().path()) {
            let key = WriteRateLimitKey {
                user_id: session.user.id,
                tier,
            };
            let (allowed, retry_after) = limiter_state.limiter.check(&key);
            if !allowed {
                tracing::warn!(
                    user_id = session.user.id,
                    tier = ?tier,
                    "write endpoint rate limited"
                );
                return Err(ApiError::rate_limited_with_retry(retry_after));
            }
        }
    }

    manager
        .enforce_csrf(request.method(), request.headers(), &session)
        .map_err(map_session_error)?;
    request.extensions_mut().insert(session);
    Ok(next.run(request).await)
}
```

**Alternative approach** (cleaner - separate middleware layer):

Keep `authenticated_session` unchanged. Add a new middleware `write_rate_limit` that wraps the protected routes. This requires adding the rate limiter to `ApplicationState` and applying it as a layer on the `protected` router.

```rust
// In build_application_router(), after creating the `protected` router (around line 668):

let protected = if let Some(limiter) = state.write_rate_limiter.clone() {
    protected.layer(middleware::from_fn_with_state(
        limiter,
        write_rate_limit_middleware,
    ))
} else {
    protected
};
```

**Recommended: separate middleware layer** (avoids modifying `authenticated_session` which is tested extensively).

**Change 3: Add `rate_limited_with_retry` to `ApiError` (line 2569)**

```rust
fn rate_limited_with_retry(retry_after: i64) -> Self {
    Self {
        status: StatusCode::TOO_MANY_REQUESTS,
        code: "rate_limited",
        message: "Too many requests, try again later",
        current_version: None,
    }
}
```

### 3.5 Modify: `backend/src/main.rs`

**Change 1: Import new types (line 1-22)**

```rust
use commoncal_backend::{
    // ... existing imports ...
    rate_limiter::FixedWindowRateLimiter,
    write_rate_limit::WriteRateLimiterState,
};
```

**Change 2: Create rate limiter instance (after line 89, before router creation)**

```rust
let write_rate_limiter = Some(WriteRateLimiterState {
    limiter: FixedWindowRateLimiter::new(30, 60), // default tier used for check
});
```

Note: The `FixedWindowRateLimiter` is created with a default max (30) and window (60s). The actual per-tier limits are applied in `write_endpoint_tier()` + `RateLimitTier::config()`. The constructor parameters are only used for the bucket structure; tier-specific limits are checked in `check()`.

**Change 3: Pass to `ApplicationState` (in the router builder call at line 137)**

The `build_router_with_auth_flows_sessions_admin_calendars_views_and_external_feeds` function signature must be updated to accept the rate limiter:

```rust
pub fn build_router_with_auth_flows_sessions_admin_calendars_views_and_external_feeds<L>(
    // ... existing params ...
    write_rate_limiter: Option<WriteRateLimiterState>,
) -> Router
```

Or simpler: add a new builder method or extend the existing one. The cleanest approach is to add the rate limiter as an optional parameter to the existing builder.

### 3.6 Modify: All `build_router_*` functions in `http.rs`

Each builder function creates `ApplicationState` and must pass `write_rate_limiter: None` (except the production builder which passes `Some(...)`).

Affected functions (all in `http.rs`):
- `build_router` (line 90)
- `build_router_with_readiness` (line 151)
- `build_router_with_readiness_and_password_login` (line 169)
- `build_router_with_invitation_consumer` (line 187)
- `build_router_with_login_service` (line 208)
- `build_router_with_auth_flows` (line 229)
- `build_router_with_sessions` (line 254)
- `build_router_with_auth_flows_and_sessions` (line 272)
- `build_router_with_admin` (line 298)
- `build_router_with_auth_flows_sessions_and_admin` (line 320)
- `build_router_with_calendars` (line 347)
- `build_router_with_auth_flows_sessions_admin_and_calendars` (line 369)
- `build_router_with_calendars_and_events` (line 398)
- `build_router_with_calendars_events_and_views` (line 421)
- `build_router_with_auth_flows_sessions_admin_calendars_views_and_external_feeds` (line 446)
- `build_router_with_auth_flows_sessions_admin_calendars_and_views` (line 482)

Each gets `write_rate_limiter: None` except the production path in `main.rs`.

## 4. Code - Exact Line References

### `rate_limiter.rs` (NEW FILE)

| Section | Lines | Content |
|---------|-------|---------|
| Struct `FixedWindowRateLimiter` | ~1-15 | Fields: max_requests, window_seconds, buckets, clock |
| Struct `RateLimitBucket` | ~17-21 | Fields: window_started_at, attempts |
| Enum `RateLimitTier` | ~23-35 | Critical/Standard/Permissive variants |
| `RateLimitTier::config()` | ~37-45 | Returns RateLimitConfig per tier |
| `FixedWindowRateLimiter::new()` | ~47-55 | Constructor with real clock |
| `FixedWindowRateLimiter::new_at()` | ~57-65 | Constructor with mock clock |
| `FixedWindowRateLimiter::check()` | ~67-95 | Core rate limit check logic |
| `write_endpoint_tier()` | ~97-155 | Path-based tier selection |
| Struct `WriteRateLimitKey` | ~157-162 | user_id + tier |
| Tests module | ~164-end | Unit tests |

### `write_rate_limit.rs` (NEW FILE)

| Section | Lines | Content |
|---------|-------|---------|
| Imports | ~1-10 | axum, crate modules |
| Struct `WriteRateLimiterState` | ~12-15 | Wraps FixedWindowRateLimiter |
| `write_rate_limit_middleware()` | ~17-60 | Middleware implementation |
| Struct `RateLimitExceeded` | ~62-65 | Error type |
| `IntoResponse for RateLimitExceeded` | ~67-85 | 429 response with Retry-After header |

### `lib.rs` (MODIFY)

| Line | Change |
|------|--------|
| ~after existing `pub mod` declarations | Add `pub mod rate_limiter;` and `pub mod write_rate_limit;` |

### `http.rs` (MODIFY)

| Line | Change |
|------|--------|
| ~29-57 (imports) | Add `use crate::rate_limiter::*;` and `use crate::write_rate_limit::*;` |
| ~512-705 (`build_application_router`) | Add rate limiter layer on `protected` router (~line 668) |
| ~2432-2446 (`ApplicationState`) | Add `write_rate_limiter: Option<WriteRateLimiterState>` field |
| ~2569 (`ApiError::rate_limited_with_retry`) | New method returning 429 with Retry-After |
| All `build_router_*` functions | Add `write_rate_limiter` parameter, pass through to `ApplicationState` |

### `main.rs` (MODIFY)

| Line | Change |
|------|--------|
| ~1-22 (imports) | Add `rate_limiter::FixedWindowRateLimiter`, `write_rate_limit::WriteRateLimiterState` |
| ~89-96 | Create `write_rate_limiter` instance |
| ~137-151 | Pass rate limiter to `build_router_with_auth_flows_sessions_admin_calendars_views_and_external_feeds` |

## 5. Test Plan

### 5.1 Unit tests: `rate_limiter.rs`

| Test | What it validates |
|------|-------------------|
| `test_check_allows_within_limit` | First N requests within limit return (true, 0) |
| `test_check_blocks_over_limit` | Request N+1 returns (false, retry_after) |
| `test_check_resets_after_window` | Requests after window expires start fresh |
| `test_check_independent_keys` | Different user_ids have independent counters |
| `test_check_independent_tiers` | Different tiers have independent counters per user |
| `test_check_retry_after_increases` | retry_after approaches window_seconds as window progresses |
| `test_write_endpoint_tier_critical_acl_put` | `PUT /api/v1/calendars/1/acl/2` -> Critical |
| `test_write_endpoint_tier_critical_acl_delete` | `DELETE /api/v1/calendars/1/acl/2` -> Critical |
| `test_write_endpoint_tier_critical_transfer` | `POST /api/v1/calendars/1/transfer` -> Critical |
| `test_write_endpoint_tier_standard_event_create` | `POST /api/v1/calendars/1/events` -> Standard |
| `test_write_endpoint_tier_standard_event_update` | `PATCH /api/v1/calendars/1/events/2` -> Standard |
| `test_write_endpoint_tier_standard_event_delete` | `DELETE /api/v1/calendars/1/events/2` -> Standard |
| `test_write_endpoint_tier_standard_occurrence_update` | `PATCH /api/v1/calendars/1/events/2/occurrences/3` -> Standard |
| `test_write_endpoint_tier_standard_occurrence_following` | `PATCH /api/v1/calendars/1/events/2/occurrences/3/following` -> Standard |
| `test_write_endpoint_tier_standard_feed` | `POST/DELETE/PATCH` on external-feeds -> Standard |
| `test_write_endpoint_tier_permissive_calendar` | `POST/PATCH/DELETE /api/v1/calendars` -> Permissive |
| `test_write_endpoint_tier_permissive_archive` | `POST /api/v1/calendars/1/archive` -> Permissive |
| `test_write_endpoint_tier_permissive_views` | `POST/PATCH/DELETE /api/v1/views/*/` -> Permissive |
| `test_write_endpoint_tier_read_endpoints_none` | GET endpoints return None |
| `test_write_endpoint_tier_auth_endpoints_none` | `/api/v1/auth/*` endpoints return None |
| `test_write_endpoint_tier_health_none` | `/health/*` endpoints return None |

### 5.2 Integration tests: `write_rate_limit.rs`

| Test | What it validates |
|------|-------------------|
| `test_middleware_allows_under_limit` | Requests within limit pass through |
| `test_middleware_blocks_over_limit` | Returns 429 after limit exceeded |
| `test_middleware_superadmin_bypass` | Superadmin not rate limited |
| `test_middleware_non_write_methods_unaffected` | GET/OPTIONS not rate limited |
| `test_middleware_retry_after_header` | 429 response includes X-Retry-After header |
| `test_middleware_no_limiter_configured` | When limiter is None, all requests pass |

### 5.3 Integration tests: end-to-end

| Test | What it validates |
|------|-------------------|
| `test_acl_rate_limit_critical_tier` | 11 ACL writes in 60s -> 11th returns 429 |
| `test_event_rate_limit_standard_tier` | 31 event writes in 60s -> 31st returns 429 |
| `test_per_user_independence` | Two users can each hit their own limit |
| `test_window_reset` | After 60s, counter resets and requests allowed again |
| `test_different_tiers_independent` | ACL limit and event limit are independent per user |

### 5.4 Existing tests to verify still pass

| Test file | Run |
|-----------|-----|
| `backend/src/http.rs` handler tests | All existing router/handler tests |
| `backend/src/sessions.rs` session middleware tests | Session auth still works |
| `backend/src/login.rs` rate limiter tests | Existing login rate limiter unaffected |
| E2E tests | Full request/response flow |

## 6. Migration Considerations

### 6.1 Zero-downtime deployment

- Rate limiter is **in-memory only** (HashMap). No database migration needed.
- Each process instance has independent counters. During rolling deployment, some requests may see slightly different limits during the transition. This is acceptable.
- No schema changes required.

### 6.2 Configuration

Rate limits should be configurable per environment:

```rust
// In config.rs or main.rs:
fn write_rate_limit_config() -> (u32, u32) {
    match std::env::var("APP_ENV").as_deref() {
        Ok("production") => (30, 60),  // default tier
        Ok("staging") => (60, 60),
        _ => (0, 0),  // 0 = disabled (development)
    }
}
```

**Development**: Rate limiting disabled by default (`write_rate_limiter: None`). Enable with `APP_ENV=staging` or `APP_ENV=production`.

### 6.3 Monitoring

- Add metrics counter for rate-limited requests (use existing tracing infrastructure):
  ```
  tracing::warn!(user_id = ..., tier = ?, "write_endpoint_rate_limited")
  ```
- These logs can be ingested by your logging platform to create alerts
- Consider adding a `x-rate-limit-remaining` header to responses for client-side feedback

### 6.4 Rollback plan

If rate limiting causes issues:
1. Deploy with `APP_ENV=development` to disable rate limiting
2. Or set `WRITE_RATE_LIMIT_DISABLED=1` env var to bypass the middleware

## 7. Security Review Checklist

- [ ] **Rate limit key collision**: Ensure `user_id` is from authenticated session (not user-supplied). Verified: `session.user.id` from `AuthenticatedSession` extension.
- [ ] **Bypass verification**: Superadmin bypass is intentional and documented. Verify no other bypass paths exist.
- [ ] **Memory bounds**: In-memory HashMap grows with unique user count. For production with many users, consider TTL-based eviction or a bounded map. Current implementation: unbounded HashMap.
- [ ] **Clock manipulation**: `clock` is set at construction time. In production, uses real system clock. Tests use mock clock. No clock manipulation vulnerability.
- [ ] **429 response body**: Does not leak internal state (bucket counts, window start times). Only returns generic message and retry_after.
- [ ] **Retry-After header**: Returns integer seconds. Clients should respect this.
- [ ] **ACL endpoint coverage**: Both PUT and DELETE on `/api/v1/calendars/:id/acl/:user_id` are covered.
- [ ] **Transfer endpoint coverage**: `POST /api/v1/calendars/:id/transfer` is covered.
- [ ] **Event occurrence endpoints**: Both single occurrence and "this and following" updates are covered.
- [ ] **External feed endpoints**: All write operations on external feeds are covered.
- [ ] **Shared view endpoints**: All write operations on views are covered.
- [ ] **No read endpoint leakage**: GET and OPTIONS requests are explicitly excluded from rate limiting.
- [ ] **Auth endpoints excluded**: `/api/v1/auth/*` endpoints are not rate-limited by this middleware (login endpoints have their own rate limiter in `login.rs`).
- [ ] **Idempotent operations**: DELETE operations are rate-limited. This is intentional (prevents bulk deletion), but clients should implement idempotency.
- [ ] **Superadmin audit**: Consider logging superadmin write operations for audit trail (separate concern).

## 8. Implementation Order

1. Create `backend/src/rate_limiter.rs` with `FixedWindowRateLimiter`, `RateLimitTier`, and `write_endpoint_tier()`
2. Create `backend/src/write_rate_limit.rs` with middleware
3. Add module declarations to `lib.rs`
4. Add `write_rate_limiter` field to `ApplicationState` in `http.rs`
5. Add rate limiter layer to `protected` router in `build_application_router()`
6. Update all `build_router_*` function signatures in `http.rs`
7. Update `main.rs` to create and pass rate limiter instance
8. Add `ApiError::rate_limited_with_retry()` method
9. Write unit tests for `rate_limiter.rs`
10. Write integration tests for middleware
11. Run existing test suite to verify no regressions
12. Security review

## 9. Dependencies

No new external dependencies required. Uses only:
- `std::collections::HashMap`
- `std::sync::{Arc, Mutex}`
- `std::time::{SystemTime, UNIX_EPOCH}`
- Existing `axum` middleware infrastructure
- Existing `tracing` for logging

## 10. Estimated Effort

- Implementation: 2-3 hours
- Tests: 1-2 hours
- Review: 30 minutes
- Total: 3.5-5.5 hours
