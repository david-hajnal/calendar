# Plan: Rate limiting on public view endpoints

## Severity: HIGH

## Problem
Public view endpoints (`GET /api/v1/public/views/:token` and `GET /api/v1/public/views/:token/events`) have no rate limiting. Unauthenticated actors can send unlimited requests per second.

## Attack
1. Obtain or guess a public view token (email logs, referer leaks, brute-force infeasible due to 256-bit space)
2. Fire unlimited requests to `/public/views/:token/events`
3. Each request triggers full DB lookup + event serialization + ACL checks
4. Exhaust database connections, cause resource exhaustion

## Impact
- Resource exhaustion (DB connections, CPU, memory)
- Token validation probing (valid token → 200/404, invalid → 404)
- Denial of service for legitimate public view access

## Implementation

### Architecture
Separate `PublicRateLimiterState` in `backend/src/public_rate_limit.rs`.
Uses `FixedWindowRateLimiter` with `check_by_key()` — single limiter for all public endpoints (not separate metadata/events limiters).

### Rate limit config
- **Limit:** 100 requests per 60-second window per IP
- **Key format:** `public:{ip}`
- **IP extraction:** `x-forwarded-for` header (first IP in chain) → falls back to `ConnectInfo` → falls back to `"unknown"`
- **Enabled:** All environments except development (`APP_ENV != "development"`)

### Code locations
- `backend/src/public_rate_limit.rs` — `PublicRateLimiterState`, `public_rate_limit_middleware()`
- `backend/src/http.rs:639-644` — middleware wired on public router
- `backend/src/main.rs:149-155` — instantiation (production: 100 req/60s)

### Tests
- `public_rate_limit.rs` — 6 unit tests
- `http.rs` — 2 integration tests (`test_public_endpoint_rate_limiting`, `test_public_endpoint_independent_ips`)

## Production config (main.rs)
```rust
FixedWindowRateLimiter::new(100, 60)  // 100 req/60s per IP
```

## Security review checklist
- [x] IP extraction correct (x-forwarded-for + ConnectInfo fallback)
- [x] Single limit for all public endpoints (simpler, adequate for current traffic patterns)
- [x] No session-based bypass possible
- [x] Bucket key collision impossible (IP addresses are unique identifiers)
- [x] Buckets evicted when window expired (check_by_key retain)
- [x] 429 response includes Retry-After header
- [ ] In-memory only — doesn't work across multiple instances (acceptable for current deployment)

## Production considerations
- In-memory rate limiting doesn't work across multiple instances. For production, consider Redis-backed limiter.
- `x-forwarded-for` parsing handles proxy scenarios (first IP in chain).
- Monitor memory usage of rate limiter buckets under high traffic.

## Discovered by: focused agent for shared view token validation
