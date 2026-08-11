# Implementation Plan: Rate limiting on admin invitations

## 1. Rate limit configuration

**Algorithm:** Per-admin fixed-window rate limiter (reuses `FixedWindowRateLimiter` from `rate_limiter.rs`)

**Limits:**
- 5 invitations per 60-second window per admin user
- Production-only (`APP_ENV == "production"`)
- Lower threshold because each request triggers an email send

**Key format:** `admin:{user_id}`

**Error response:** HTTP 429 with `Retry-After` header

## 2. Code changes

### 2a. `backend/src/admin_invitation_rate_limit.rs` (NEW FILE)

Separate rate limiter file for admin invitations.

```rust
pub struct AdminInvitationRateLimiterState {
    pub limiter: Arc<FixedWindowRateLimiter>,
}

pub fn check_admin_invitation_rate_limit(
    limiter: &AdminInvitationRateLimiterState,
    user_id: i64,
) -> Result<(), RateLimitExceeded> {
    let key = format!("admin:{}", user_id);
    let (allowed, retry_after) = limiter.limiter.check_by_key(&key);
    if !allowed {
        Err(RateLimitExceeded { retry_after })
    } else {
        Ok(())
    }
}
```

Key design decisions:
- Uses `check_by_key()` instead of `check()` — no `WriteRateLimitKey` needed (admin users don't go through write middleware)
- No superadmin bypass — superadmins are rate-limited too (prevents admin abuse)
- Separate file to avoid coupling with `write_rate_limit_middleware`

### 2b. `backend/src/http.rs`

Add rate limit check in `invite_user` handler (line ~1855):

```rust
async fn invite_user(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Json(request): Json<InviteUserRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_superadmin(&session)?;
    if let Some(ref limiter) = state.admin_rate_limiter {
        check_admin_invitation_rate_limit(limiter, session.user.id)
            .map_err(|_| ApiError::rate_limited())?;
    }
    // ... rest of handler
}
```

Add `admin_rate_limiter` field to `ApplicationState`:

```rust
pub admin_rate_limiter: Option<AdminInvitationRateLimiterState>,
```

Update all `build_router_*` functions to accept `admin_rate_limiter: Option<AdminInvitationRateLimiterState>` parameter.

### 2c. `backend/src/main.rs`

Production-only instantiation:

```rust
let admin_rate_limiter = if std::env::var("APP_ENV").ok().as_deref() == Some("production") {
    let limiter = FixedWindowRateLimiter::new(5, 60);
    Some(AdminInvitationRateLimiterState {
        limiter: Arc::new(limiter),
    })
} else {
    None
};
```

## 3. Test plan

### 3a. Unit tests in `admin_invitation_rate_limit.rs`
- `test_check_allows_under_limit` — requests within limit succeed
- `test_check_blocks_over_limit` — requests over limit return 429
- `test_check_different_users_independent` — different users have independent limits
- `test_check_retry_after_value` — retry_after is 60
- `test_check_rate_limit_exceeded_into_response` — 429 status + Retry-After header
- `test_check_window_not_expired_within_window` — window expiration works
- `test_check_no_superadmin_bypass` — superadmins are rate-limited

### 3b. Integration tests in `http.rs`
- `test_admin_invitation_rate_limiting` — end-to-end rate limiting
- `test_admin_invitation_rate_limit_includes_superadmin` — superadmin not bypassed

## 4. Security review checklist

- [x] Rate limit values appropriate for production (5/min — conservative)
- [x] Superadmin NOT bypassed (intentional — admin abuse prevention)
- [x] Retry-After header present on 429 responses
- [x] No information leakage in rate-limit error messages
- [x] Bucket key collision impossible (user_id is unique)
- [x] Buckets evicted when window expired (check_by_key retain)
- [ ] In-memory buckets don't grow unbounded — fixed with eviction

## 5. Dependencies

No new crates. Reuses `FixedWindowRateLimiter` from `rate_limiter.rs`.

## 6. Monitoring

Add tracing event when rate limit hits (via `RateLimitExceeded.into_response()`):
- 429 response with `x-retry-after` header
- Log at WARN level when rate limit exceeded
