# Program Design: Security hardening

## Files

### `backend/src/recurrence.rs`
- Add `const MAX_UNTIL_SPAN_SECONDS: i64 = 5 * 365 * 86400` after line 13
- Add span check in `expand_occurrences` before iteration loop (after line 157)
- Keep existing `COUNT` parse-time check at line 63-65 (already rejects >1M)

### `backend/src/event.rs`
- Add `ComplexityLimitExceeded` variant to `EventServiceError` enum at line 1762

### `backend/src/http.rs`
- Add `EventServiceError::ComplexityLimitExceeded` arm to `map_event_error` at line 1758

### `backend/Cargo.toml`
- Add `pbkdf2 = "0.12"` to `[dev-dependencies]` section

### `backend/tests/recurrence.rs`
- Test UNTIL span > 5 years returns `ComplexityLimitExceeded`
- Test UNTIL span within 5 years accepted
- Test COUNT=1000000 rejected at parse time (already tested at line 245, verify still passes)

### `backend/tests/security.rs`
- Test `derive` produces different output for different inputs
- Test `derive` is deterministic for same input
- Test AES-GCM encrypt/decrypt roundtrip
- Test AES-GCM decrypt with wrong key returns None
- Test AES-GCM decrypt with tampered ciphertext returns None

## Types & signatures

### `recurrence.rs`
```rust
const MAX_UNTIL_SPAN_SECONDS: i64 = 5 * 365 * 86400;
```

### `event.rs`
```rust
pub enum EventServiceError {
    // ... existing variants ...
    ComplexityLimitExceeded,
}
```

### `http.rs`
```rust
EventServiceError::ComplexityLimitExceeded => ApiError {
    status: StatusCode::BAD_REQUEST,
    code: "recurrence_too_complex",
    message: "Recurrence rule exceeds complexity limits",
    current_version: None,
},
```

### `security.rs` — derive
```rust
pub fn derive(secret: &[u8]) -> Self {
    let salt = getrandom::get_bytes::<16>().expect("random source unavailable");
    let mut output = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(secret, &salt, 100_000, &mut output);
    Self(output)
}
```

### `security.rs` — encrypt_secret
```rust
pub fn encrypt_secret(&self, plaintext: &[u8]) -> Vec<u8> {
    let mut nonce = [0u8; 12];
    getrandom::fill(&mut nonce).expect("random source unavailable");
    let cipher = aes_gcm::Aes256Gcm::new_from_slice(&self.0).expect("key must be 32 bytes");
    let ciphertext = cipher.encrypt(&nonce.into(), plaintext).expect("encryption failed");
    [nonce.as_slice(), &ciphertext[..]].concat()
}
```

### `security.rs` — decrypt_secret
```rust
pub fn decrypt_secret(&self, encoded: &[u8]) -> Option<Vec<u8>> {
    if encoded.len() < 12 {
        return None;
    }
    let (nonce, ciphertext) = encoded.split_at(12);
    let cipher = aes_gcm::Aes256Gcm::new_from_slice(&self.0).expect("key must be 32 bytes");
    cipher.decrypt(&nonce.into(), ciphertext).ok()
}
```

## Call stack

### Recurrence span check
`expand_occurrences()` → span calculation → `if span > MAX_UNTIL_SPAN_SECONDS { ComplexityLimitExceeded }`

### Error surface
`EventServiceError::ComplexityLimitExceeded` → `map_event_error()` → `StatusCode::BAD_REQUEST`

### Crypto swap
`encrypt_secret()` → `aes_gcm::Aes256Gcm::encrypt()` → nonce + ciphertext
`decrypt_secret()` → `aes_gcm::Aes256Gcm::decrypt()` → plaintext or None

## Test plan

### Recurrence tests
1. `test_until_span_exceeds_five_years` — `FREQ=DAILY;UNTIL=99991231T235959Z` → `ComplexityLimitExceeded`
2. `test_until_span_within_five_years` — `FREQ=DAILY;UNTIL=20280101T090000Z` → accepted
3. `test_count_one_million_rejected` — `FREQ=DAILY;COUNT=1000001` → `ComplexityLimitExceeded` (already exists at line 245)

### Security tests
4. `test_derive_different_inputs_produce_different_keys` — `derive([0]) != derive([1])`
5. `test_derive_same_input_deterministic` — `derive([0]) == derive([0])`
6. `test_encrypt_decrypt_roundtrip` — `decrypt(encrypt(data)) == data`
7. `test_decrypt_wrong_key_returns_none` — decrypt with different key → None
8. `test_decrypt_tampered_ciphertext_returns_none` — modify ciphertext bytes → None

## Least confident decisions
1. PBKDF2 100k iterations — fast on modern hardware for Rust (~50ms), acceptable for key derivation
2. `aes_gcm::Aes256Gcm::new_from_slice` unwraps — key is always 32 bytes from `derive`, safe
3. `getrandom::get_bytes::<16>()` — uses getrandom 0.4 API (already in deps)
