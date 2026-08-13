# Status: hardening-observations

- Gate 1 — Product: APPROVED 2026-08-10
- Gate 2 — Architecture: APPROVED 2026-08-10
- Gate 3 — Program Design: APPROVED 2026-08-10
- Gate 4 — Slice plan: APPROVED 2026-08-10

## Slices
- [x] Slice 1 — 5a: recurrence UNTIL span limit + COUNT parse-time rejection
- [x] Slice 2 — 5b: PBKDF2 key derivation
- [x] Slice 3 — 5c: AES-GCM encrypt/decrypt

## Notes for a fresh session
- Plan source: docs/plan/05-hardening-observations-impl.md
- 5d and 5e already done (Referrer-Policy and X-Content-Type-Options headers present)
- `encrypt_secret`/`decrypt_secret` called in `external_feed.rs:215` and `external_feed.rs:274`
- `EventServiceError` in `event.rs:1762` — no `ComplexityLimitExceeded` variant yet
- `map_event_error` in `http.rs:1758` — no `ComplexityLimitExceeded` mapping yet
- `aes-gcm` already in Cargo.toml (line 8)
- `pbkdf2 = "0.12"` needs to be added to Cargo.toml
- Existing recurrence tests in `backend/tests/recurrence.rs`
- Existing security tests in `backend/tests/security.rs`
