# Implementation Plan: Rate limiting on admin invitations

## 1. Rate limit configuration

**Algorithm:** Per-admin fixed-window rate limiter (reuse `FixedWindowRateLimiter` from `rate_limiter.rs`)

**Limits:**
- 10 invitations per 60-second window per admin user
- Lower than `RateLimitTier::Critical` (10/60s) because each request has real-world email cost
- Superadmins bypass (current `write_rate_limit_middleware` already does this — keep as-is)

**Key format:** `admin_invite:{user_id}`

**Error response:** HTTP 429 with `Retry-After` header

## 2. Code changes

### 2a. `backend/src/rate_limiter.rs`

Add `AdminInvite` tier to `RateLimitTier` enum (line ~69):

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateLimitTier {
    Critical,
    Standard,
    Permissive,
    AdminInvite,   // NEW
}
```

Add config for `AdminInvite` in `RateLimitTier::config()` (line ~72):

```rust
RateLimitTier::AdminInvite => RateLimitConfig {
    max_requests: 10,
    window_seconds: 60,
},
```

Add display arm (line ~91):

```rust
RateLimitTier::AdminInvite => write!(f, "admin_invite"),
```

### 2b. `backend/src/write_rate_limit.rs`

Add admin invitation check in `write_rate_limit_middleware` (line ~55, after superadmin bypass):

```rust
// Before the existing tier check, add:
if request.uri().path() == "/api/v1/admin/invitations" && request.method() == "POST" {
    let key = WriteRateLimitKey {
        user_id: session.user.id,
        tier: RateLimitTier::AdminInvite,
    };
    let (allowed, retry_after) = limiter_state.limiter.check(&key);
    if !allowed {
        return Err(RateLimitExceeded { retry_after });
    }
    return Ok(next.run(request).await);
}
```

This short-circuits the tier detection for admin invitations, avoiding a parse through `write_endpoint_tier`.

### 2c. `backend/src/http.rs`

No changes needed — `write_rate_limit_middleware` is already wired on the protected router at line 710-714.

## 3. Test plan

### 3a. Unit tests in `write_rate_limit.rs`

Add test `test_admin_invite_rate_limit`:

```rust
#[tokio::test]
async fn test_admin_invite_rate_limit() {
    let limiter_state = make_limiter(3, 60, 1000);
    let session = make_session(1, false);
    let app = build_app(limiter_state, session);

    // 3 requests should succeed
    for i in 0..3 {
        let req = make_request("POST", "/api/v1/admin/invitations");
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "request {} should succeed", i + 1);
    }

    // 4th request should be rate limited
    let req = make_request("POST", "/api/v1/admin/invitations");
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
}
```

### 3b. Integration test in `backend/src/http.rs` or test module

Add test that verifies `GET /api/v1/admin/invitations` is NOT rate-limited by this specific check (it goes through normal tier detection).

### 3c. Resend endpoint

Test that `POST /api/v1/admin/invitations/:id/resend` is also rate-limited. Add to middleware:

```rust
if request.uri().path().starts_with("/api/v1/admin/invitations")
    && request.method() == "POST"
{
    // ... rate limit check
}
```

## 4. Security review checklist

- [ ] Rate limit values appropriate for production (10/min)
- [ ] Superadmin bypass is intentional (admin abuse is a separate concern)
- [ ] Retry-After header present on 429 responses
- [ ] No information leakage in rate-limit error messages
- [ ] Bucket key collision impossible (user_id is unique)
- [ ] In-memory buckets don't grow unbounded (existing `HashMap` has no eviction)

## 5. Dependencies

No new crates. Reuses `FixedWindowRateLimiter` from `rate_limiter.rs`.

## 6. Monitoring

Add tracing event when rate limit hits:

```rust
tracing::warn!(
    user_id = session.user.id,
    error_code = "admin_invite_rate_limited",
    "admin invitation rate limit exceeded"
);
```
