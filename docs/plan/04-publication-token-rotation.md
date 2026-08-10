# Plan: Fix publication token rotation to revoke old token

## Severity: MEDIUM

## Problem
Rotating a public view publication token does not revoke the old token. Users cannot effectively revoke public access without deleting the publication entirely.

## Attack
1. User creates a public view publication
2. User rotates the token (believing old token is invalidated)
3. Old token still works — old URLs still grant access
4. If old token was shared/leaked, access persists indefinitely

## Impact
- Publication access persists after rotation
- Cannot effectively revoke public access
- Security incident response impaired (cannot revoke without deletion)

## Data Flow
`shared_view.rs:336-369` `rotate_publication` → UPDATE token_hash/token_prefix → old token still valid (no revoked_at set)

## Fix Plan
1. Set `revoked_at = NOW()` on the old token before inserting the new one
2. Or add a WHERE clause comparing current token hash
3. Add audit log entry for token rotation
4. Consider adding a token history table to track rotation chain
5. Add API documentation clarifying rotation behavior

## Files to Modify
- `backend/src/shared_view.rs` (rotate_publication method)
- `backend/src/http.rs` (publication rotate handler, audit logging)
- `backend/migrations/` (add token_history table if implementing history)

## Discovered by: focused agent for shared view token validation
