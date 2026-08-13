# Slice plan: Security hardening

## Slice 1 — recurrence UNTIL span limit
- `recurrence.rs`: add `MAX_UNTIL_SPAN_SECONDS` constant + span check in `expand_occurrences`
- `event.rs`: add `ComplexityLimitExceeded` variant to `EventServiceError`
- `http.rs`: add `ComplexityLimitExceeded` → 400 mapping in `map_event_error`
- `tests/recurrence.rs`: UNTIL span > 5yr test + UNTIL span within 5yr test
- Prove: `cargo test recurrence` passes

## Slice 2 — PBKDF2 key derivation
- `security.rs`: replace `derive` with PBKDF2
- `Cargo.toml`: add `pbkdf2 = "0.12"` to dev-dependencies
- `tests/security.rs`: derive determinism + different-input tests
- Prove: `cargo test security` passes

## Slice 3 — AES-GCM encrypt/decrypt
- `security.rs`: replace `encrypt_secret`/`decrypt_secret` with AES-GCM
- `tests/security.rs`: encrypt/decrypt roundtrip, wrong-key, tampered-ciphertext tests
- Prove: `cargo test security` passes
