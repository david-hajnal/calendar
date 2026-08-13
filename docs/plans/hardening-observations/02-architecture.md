# Architecture: Security hardening

## Fit
- `backend/src/recurrence.rs` — add UNTIL span constant, span check in `expand_occurrences`
- `backend/src/security.rs` — replace `derive` with PBKDF2, replace `encrypt_secret`/`decrypt_secret` with AES-GCM
- `backend/src/http.rs` — add `ComplexityLimitExceeded` to `map_event_error`
- `backend/src/event.rs` — add `ComplexityLimitExceeded` variant to `EventServiceError`
- `backend/Cargo.toml` — add `pbkdf2 = "0.12"` dev dependency
- `backend/tests/recurrence.rs` — add UNTIL span limit tests
- `backend/tests/security.rs` — add PBKDF2 and AES-GCM tests

## Endpoints
none — no new API surface, only error behavior change (ComplexityLimitExceeded now returns 400 instead of being mapped to InvalidInput)

## Data
no schema changes

## Flow
- **Recurrence path:** event creation → `validate_recurrence()` in `event.rs:1565` → `RecurrenceRule::parse()` → `expand_occurrences()` → span check → `ComplexityLimitExceeded` → `EventServiceError::ComplexityLimitExceeded` → `map_event_error()` → 400 Bad Request
- **Encryption path:** `external_feed.rs:215` → `encrypt_secret()` → AES-GCM (same return type `Vec<u8>`) / `external_feed.rs:274` → `decrypt_secret()` → AES-GCM (same return type `Option<Vec<u8>>`)

## External
- `pbkdf2 = "0.12"` — dev dependency only, not runtime
- `aes-gcm = "0.10"` — already present, no version change
