# Plan: Add rate limiting on authenticated write endpoints

## Severity: HIGH

## Problem
No rate limiting exists on authenticated write endpoints (event CRUD, calendar management, ACL changes, invitation sending). Any authenticated user can send unlimited requests per second.

## Attack
1. Obtain any valid session (phishing, leak, XSS)
2. Fire unlimited POST/PUT/PATCH/DELETE requests to `/api/v1/calendars/:id/events`, `/api/v1/calendars/:id/acl/:user_id`, etc.
3. No throttling, no cooldown, no cap

## Impact
- Event flood: delete/recreate events at arbitrary rate
- ACL spam: grant arbitrary roles to arbitrary users at unlimited rate
- Calendar transfer spam: transfer ownership rapidly
- Data manipulation without cost barrier

## Data Flow
`User request` → `http.rs` route handlers → service layer → database (no rate limiting middleware)

## Fix Plan
1. Add rate limiting middleware for authenticated endpoints
2. Prioritize: ACL endpoints > event CRUD > calendar CRUD
3. Use per-user or per-calendar rate limits
4. Consider the existing `FixedWindowLoginRateLimiter` pattern or a more general-purpose limiter
5. Configure thresholds based on expected legitimate usage

## Files to Modify
- `backend/src/http.rs` (route registration, middleware wiring)
- `backend/src/login.rs` (rate limiter types, can be generalized)
- `backend/src/main.rs` (middleware stack configuration)

## Discovered by: red team independent
