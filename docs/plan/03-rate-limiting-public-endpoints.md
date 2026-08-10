# Plan: Add rate limiting on public view endpoints

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

## Data Flow
`Request` → `http.rs:1305-1345` → `resolve_publication` → DB lookup → event serialization → response (no rate limiting)

## Fix Plan
1. Add IP-based rate limiting for public endpoints
2. Use a lower threshold than authenticated endpoints (no session context)
3. Consider caching for frequently accessed public views
4. Add request size limits for event enumeration responses
5. Monitor for unusual access patterns

## Files to Modify
- `backend/src/http.rs` (public route handlers, middleware wiring)
- `backend/src/shared_view.rs` (public view resolution)
- `backend/src/login.rs` (rate limiter types, generalize if needed)

## Discovered by: focused agent for shared view token validation
