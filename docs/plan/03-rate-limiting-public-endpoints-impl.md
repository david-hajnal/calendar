# Implementation Plan: Rate limiting on public endpoints

## 1. Rate limit configuration

**Algorithm:** IP-based fixed-window rate limiter (reuses `FixedWindowRateLimiter` from `rate_limiter.rs`)

**Limits:**
- 100 requests per 60-second window per IP (all public endpoints share this limit)
- Production and staging only (`APP_ENV != "development"`)

**Key format:** `public:{ip}`

**IP extraction:** `x-forwarded-for` header (first IP in chain) → `ConnectInfo` → `"unknown"`

## 2. Code changes

### 2a. `backend/src/public_rate_limit.rs` (NEW FILE)

```rust
pub struct PublicRateLimiterState {
    pub limiter: Arc<FixedWindowRateLimiter>,
}

pub async fn public_rate_limit_middleware(
    State(limiter_state): State<PublicRateLimiterState>,
    connect_info: Option<ConnectInfo<StdSocketAddr>>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, RateLimitExceeded> {
    let ip = request
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|v| v.trim().to_string())
        .or_else(|| connect_info.map(|ci| ci.ip().to_string()))
        .unwrap_or_else(|| "unknown".to_owned());
    let key = format!("public:{}", ip);
    let (allowed, retry_after) = limiter_state.limiter.check_by_key(&key);
    if !allowed {
        return Err(RateLimitExceeded { retry_after });
    }
    Ok(next.run(request).await)
}
```

Key design decisions:
- Single limiter for all public endpoints (not separate metadata/events limiters)
- `x-forwarded-for` support for proxy scenarios
- Falls back to `ConnectInfo` for direct connections
- `"unknown"` fallback for edge cases

### 2b. `backend/src/http.rs`

Wire middleware on public router (line ~639):

```rust
if let Some(limiter) = state.public_rate_limiter.clone() {
    let public = Router::new()
        .route("/api/v1/public/views/:token", get(read_public_view))
        .route("/api/v1/public/views/:token/events", get(list_public_view_events))
        .layer(middleware::from_fn(public_response_headers))
        .layer(middleware::from_fn_with_state(
            limiter,
            crate::public_rate_limit::public_rate_limit_middleware,
        ));
    router = router.merge(public);
}
```

Add `public_rate_limiter` field to `ApplicationState`:

```rust
pub public_rate_limiter: Option<PublicRateLimiterState>,
```

Update all `build_router_*` functions to accept `public_rate_limiter` parameter.

### 2c. `backend/src/main.rs`

Production/staging instantiation:

```rust
let public_rate_limiter = if std::env::var("APP_ENV").ok().as_deref() == Some("development") {
    None
} else {
    Some(PublicRateLimiterState {
        limiter: Arc::new(FixedWindowRateLimiter::new(100, 60)),
    })
};
```

## 3. Test plan

### 3a. Unit tests in `public_rate_limit.rs`
- `test_allows_under_limit` — requests within limit succeed
- `test_blocks_over_limit` — requests over limit return 429
- `test_retry_after_header` — Retry-After header present
- `test_unknown_ip_fallback` — "unknown" IP handling
- `test_independent_ips` — different IPs have independent limits
- `test_no_limiter_configured` — graceful degradation

### 3b. Integration tests in `http.rs`
- `test_public_endpoint_rate_limiting` — end-to-end rate limiting
- `test_public_endpoint_independent_ips` — IP isolation

## 4. Security review checklist

- [x] IP extraction correct (x-forwarded-for + ConnectInfo fallback)
- [x] Single limit for all public endpoints (simpler, adequate for current traffic)
- [x] No session-based bypass possible
- [x] Bucket key collision impossible (IP addresses are unique identifiers)
- [x] Buckets evicted when window expired (check_by_key retain)
- [x] 429 response includes Retry-After header
- [ ] In-memory buckets don't grow unbounded — fixed with eviction

## 5. Dependencies

No new crates. Reuses `FixedWindowRateLimiter` from `rate_limiter.rs`.

## 6. Production considerations

- In-memory rate limiting doesn't work across multiple instances. For production, consider Redis-backed limiter.
- `x-forwarded-for` parsing handles proxy scenarios (first IP in chain).
- Monitor memory usage of rate limiter buckets under high traffic.
