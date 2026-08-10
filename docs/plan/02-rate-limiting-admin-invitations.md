# Plan: Add rate limiting on admin invitation creation

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

## Data Flow
`POST /admin/invitations` → `http.rs:1731` → `AdminService::invite` → `deliver_or_revoke` → email send

## Fix Plan
1. Add rate limiting specific to the invitation creation endpoint
2. Use per-admin rate limits (not just IP-based)
3. Set lower threshold than general write endpoints (each request has real-world cost)
4. Consider a sliding window algorithm for smoother rate limiting
5. Add monitoring/alerting for unusual invitation volume

## Files to Modify
- `backend/src/http.rs` (route handler, middleware wiring)
- `backend/src/admin.rs` (invite_user handler)
- `backend/src/login.rs` (rate limiter types, generalize if needed)

## Discovered by: red team independent
