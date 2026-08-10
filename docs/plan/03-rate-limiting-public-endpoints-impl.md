# Implementation Plan: Rate limiting on public endpoints

## 1. Rate limit configuration

**Algorithm:** IP-based fixed-window rate limiter (new, separate from user-based limiter)

**Limits:**
- `GET /api/v1/public/views/:token` — 30 requests per 60 seconds per IP
- `GET /api/v1/public/views/:token/events` — 20 requests per 60 seconds per IP
- Lower thresholds because no session context, higher resource cost per request

**Key format:** `public:{ip}`

## 2. Code changes

### 2a. `backend/src/rate_limiter.rs`

Add new public-facing rate limiter type. Create `PublicRateLimiter` struct:

```rust
/// In-memory fixed-window rate limiter for public (unauthenticated) endpoints.
pub struct PublicRateLimiter {
    max_requests: u32,
    window_seconds: i64,
    buckets: Mutex<HashMap<String, RateLimitBucket>>,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl PublicRateLimiter {
    pub fn new(max_requests: u32, window_seconds: i64) -> Self {
        Self {
            max_requests,
            window_seconds,
            buckets: Mutex::new(HashMap::new()),
            clock: Arc::new(|| Utc::now().timestamp()),
        }
    }

    pub fn allow(&self, key: &str) -> bool {
        let now = (self.clock)();
        let mut buckets = self.buckets.lock().unwrap();
        let bucket = buckets.entry(key.to_string()).or_insert_with(|| {
            RateLimitBucket {
                window_started_at: now,
                attempts: 0,
            }
        });
        if now - bucket.window_started_at >= self.window_seconds {
            bucket.window_started_at = now;
            bucket.attempts = 0;
        }
        if bucket.attempts >= self.max_requests {
            return false;
        }
        bucket.attempts += 1;
        true
    }
}
```

### 2b. `backend/src/http.rs`

Add new state struct for public rate limiter (around line 2482, near `ApplicationState`):

```rust
#[derive(Clone)]
pub struct PublicRateLimiterState {
    pub metadata_limiter: Arc<PublicRateLimiter>,
    pub events_limiter: Arc<PublicRateLimiter>,
}
```

Add public rate limiter middleware function (after `public_response_headers` at line ~1401):

```rust
async fn public_rate_limit_middleware(
    State(state): State<PublicRateLimiterState>,
    connect_info: Option<ConnectInfo<SocketAddr>>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, RateLimitExceeded> {
    let ip = connect_info
        .map(|ci| ci.0.ip().to_string())
        .unwrap_or_else(|| "unknown".to_owned());

    let path = request.uri().path();
    let allowed = if path.starts_with("/api/v1/public/views/") && path.ends_with("/events") {
        state.events_limiter.allow(&format!("public_events:{ip}"))
    } else if path.starts_with("/api/v1/public/views/") {
        state.metadata_limiter.allow(&format!("public_meta:{ip}"))
    } else {
        true
    };

    if !allowed {
        tracing::warn!(ip, path, error_code = "public_endpoint_rate_limited");
        return Err(RateLimitExceeded { retry_after: 60 });
    }
    Ok(next.run(request).await)
}
```

Wire middleware in `build_application_router` (around line 569-578, where public routes are created):

```rust
if state.shared_view_service.is_some() && state.event_service.is_some() {
    let public_limiter = state.public_rate_limiter.clone();
    let public = Router::new()
        .route("/api/v1/public/views/:token", get(read_public_view))
        .route(
            "/api/v1/public/views/:token/events",
            get(list_public_view_events),
        )
        .layer(middleware::from_fn(public_response_headers))
        .layer(middleware::from_fn_with_state(
            public_limiter,
            public_rate_limit_middleware,
        ));
    router = router.merge(public);
}
```

Add `public_rate_limiter` field to `ApplicationState` (line ~2496):

```rust
pub public_rate_limiter: Option<PublicRateLimiterState>,
```

Update all `build_router_*` functions to pass `public_rate_limiter: None` or a configured instance.

### 2c. `backend/src/shared_view.rs`

No changes needed — rate limiting is at the HTTP layer, not the service layer.

## 3. Test plan

### 3a. Unit tests in `rate_limiter.rs`

```rust
#[test]
fn test_public_rate_limiter_allows_within_limit() {
    let limiter = PublicRateLimiter::new(5, 60);
    for _ in 0..5 {
        assert!(limiter.allow("test_key"));
    }
}

#[test]
fn test_public_rate_limiter_blocks_over_limit() {
    let limiter = PublicRateLimiter::new(3, 60);
    for _ in 0..3 {
        assert!(limiter.allow("test_key"));
    }
    assert!(!limiter.allow("test_key"));
}

#[test]
fn test_public_rate_limiter_separate_ips() {
    let limiter = PublicRateLimiter::new(1, 60);
    assert!(limiter.allow("ip:1.2.3.4"));
    assert!(!limiter.allow("ip:1.2.3.4"));
    assert!(limiter.allow("ip:5.6.7.8"));
}
```

### 3b. Integration tests

Test that `/api/v1/public/views/:token` returns 429 after exceeding limit.
Test that `/api/v1/public/views/:token/events` has separate limit from metadata endpoint.
Test that invalid tokens still return 404 (not 429).

## 4. Security review checklist

- [ ] IP extraction correct (handles forwarded headers if behind proxy)
- [ ] Separate limits for metadata vs events endpoints
- [ ] No session-based bypass possible
- [ ] Bucket key collision impossible (IP addresses are unique identifiers)
- [ ] In-memory buckets bounded (no eviction — monitor memory)
- [ ] 429 response includes Retry-After header

## 5. Dependencies

No new crates. Reuses `RateLimitBucket` from `rate_limiter.rs`.

## 6. Production considerations

- In-memory rate limiting doesn't work across multiple instances. For production, consider Redis-backed limiter.
- Add `x-forwarded-for` header parsing if behind a load balancer/proxy.
- Monitor memory usage of rate limiter buckets under high traffic.
