# Plan: Rate limiting on admin invitation creation

## Severity: HIGH

## Problem
The admin invitation creation endpoint (`POST /api/v1/admin/invitations`) has no rate limiting. A compromised superadmin session can send unlimited invitations per second, each triggering an email send.

## Attack
1. Compromise superadmin session
2. Fire rapid POST requests to `/api/v1/admin/invitations` with sequential emails
3. Each request triggers an email send
4. Exhaust email provider quota, trigger spam filters, damage sender reputation

## Impact
- Email spam at scale
- Cost via email provider billing
- Sender reputation damage
- Potential IP/service blacklisting

## Implementation

### Architecture
Separate `AdminInvitationRateLimiterState` in `backend/src/admin_invitation_rate_limit.rs`.
Uses `FixedWindowRateLimiter` with `check_by_key()` — no new tier added to `RateLimitTier`.

### Rate limit config
- **Limit:** 5 requests per 60-second window per admin user
- **Key format:** `admin:{user_id}`
- **Superadmin bypass:** NO — superadmins are rate-limited too (admin abuse prevention)
- **Enabled:** Production only (`APP_ENV == "production"`)

### Code locations
- `backend/src/admin_invitation_rate_limit.rs` — `AdminInvitationRateLimiterState`, `check_admin_invitation_rate_limit()`
- `backend/src/http.rs:1855` — `invite_user` handler calls `check_admin_invitation_rate_limit`
- `backend/src/main.rs:157-164` — production-only instantiation

### Tests
- `admin_invitation_rate_limit.rs` — 7 unit tests
- `http.rs` — 2 integration tests (`test_admin_invitation_rate_limiting`, `test_admin_invitation_rate_limit_includes_superadmin`)

## Production config (main.rs)
```rust
FixedWindowRateLimiter::new(5, 60)  // 5 req/60s per admin
```

## Security review checklist
- [x] Rate limit values appropriate for production (5/min — conservative for email cost)
- [x] Superadmin NOT bypassed (intentional — prevents admin abuse)
- [x] Retry-After header present on 429 responses
- [x] No information leakage in rate-limit error messages
- [x] Bucket key collision impossible (user_id is unique)
- [x] Buckets evicted when window expired (check_by_key retain)
- [ ] In-memory only — doesn't work across multiple instances (acceptable for current deployment)

## Discovered by: red team independent
