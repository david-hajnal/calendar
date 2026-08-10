# Implementation Plan: Hardening observations

## 5a. Recurrence rule bounds

### Problem
- `recurrence.rs:12`: `MAX_RULE_COUNT = 1_000_000` but expansion limited to 1,000 occurrences
- `parse_until` (line 94-98) accepts `UNTIL=99991231T235959Z` with no span cap
- `ComplexityLimitExceeded` silently converted to `InvalidInput` in error mapping

### Code changes

#### `backend/src/recurrence.rs`

Add UNTIL span constant (line ~14):

```rust
const MAX UNTIL_SPAN_SECONDS: i64 = 5 * 365 * 86400; // 5 years
```

Update `parse_until` (line 94-98):

```rust
fn parse_until(value: &str) -> Result<DateTime<Utc>, RecurrenceError> {
    let naive = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%SZ")
        .map_err(|_| RecurrenceError::InvalidUntil)?;
    let until = naive.and_utc();
    // Add span check in RecurrenceRule::parse after parsing until
    Ok(until)
}
```

Add span validation in `RecurrenceRule::parse` (after line 68):

```rust
"UNTIL" => {
    let parsed_until = parse_until(value)?;
    // Validate span from event start (caller must provide start; do this in expand_occurrences)
    until = Some(parsed_until);
}
```

In `expand_occurrences` (line 142), add span check before expansion:

```rust
if let Some(until) = &event.rule.until {
    let span = (*until - event.starts_at.with_timezone(&Utc)).num_seconds();
    if span > MAX_UNTIL_SPAN_SECONDS {
        return Err(RecurrenceError::ComplexityLimitExceeded);
    }
}
```

#### `backend/src/http.rs`

Update `map_event_error` (line 1682-1696) to surface `ComplexityLimitExceeded`:

```rust
EventServiceError::ComplexityLimitExceeded => ApiError {
    status: StatusCode::BAD_REQUEST,
    code: "recurrence_too_complex",
    message: "Recurrence rule exceeds complexity limits",
    current_version: None,
},
```

### Test plan
- Test `FREQ=DAILY;UNTIL=99991231T235959Z` returns `ComplexityLimitExceeded`
- Test `FREQ=DAILY;COUNT=1000000` returns `ComplexityLimitExceeded`
- Test valid UNTIL within 5-year span is accepted
- Test COUNT=1000000+ is rejected at parse time

## 5b. Weak key derivation

### Problem
`security.rs:27-32`: `SecretKey::derive` uses single SHA-256 with no salt/iterations.

### Code changes

#### `backend/src/security.rs`

Replace `derive` implementation:

```rust
pub fn derive(secret: &[u8]) -> Self {
    // Use PBKDF2 with 100k iterations and random salt
    let salt = getrandom::get_bytes::<16>().expect("random source unavailable");
    let mut output = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(secret, &salt, 100_000, &mut output);
    Self(output)
}
```

Add dependency to `Cargo.toml`:

```toml
pbkdf2 = "0.12"
```

### Test plan
- Test `derive` produces different output for different inputs
- Test `derive` is deterministic for same input
- Test derived key can encrypt/decrypt (integration)

## 5c. Custom crypto construction

### Problem
`security.rs:36-77`: HMAC-based stream cipher is non-standard.

### Code changes

#### `backend/src/security.rs`

Replace `encrypt_secret` and `decrypt_secret` with AES-GCM:

```rust
pub fn encrypt_secret(&self, plaintext: &[u8]) -> Vec<u8> {
    let mut nonce = [0u8; 12];
    getrandom::fill(&mut nonce).expect("random source unavailable");
    let cipher = aes_gcm::Aes256Gcm::new_from_slice(&self.0).expect("key must be 32 bytes");
    let ciphertext = cipher
        .encrypt(&nonce.into(), plaintext)
        .expect("encryption failed");
    [nonce.as_slice(), &ciphertext[..]].concat()
}

pub fn decrypt_secret(&self, encoded: &[u8]) -> Option<Vec<u8>> {
    if encoded.len() < 12 {
        return None;
    }
    let (nonce, ciphertext) = encoded.split_at(12);
    let cipher = aes_gcm::Aes256Gcm::new_from_slice(&self.0).expect("key must be 32 bytes");
    cipher
        .decrypt(&nonce.into(), ciphertext)
        .ok()
}
```

No new dependencies — `aes-gcm` already in `Cargo.toml` (line 8).

### Test plan
- Test encrypt/decrypt roundtrip
- Test decrypt with wrong key returns None
- Test decrypt with tampered ciphertext returns None
- Test nonce uniqueness (no reuse)

## 5d. Missing Referrer-Policy header

### Code changes

#### `backend/src/http.rs`

Already present at line 1426-1428. No change needed.

```rust
headers.insert(
    axum::http::header::REFERRER_POLICY,
    HeaderValue::from_static("strict-origin-when-cross-origin"),
);
```

## 5e. Missing X-Content-Type-Options header

### Code changes

#### `backend/src/http.rs`

Already present at line 1422-1424. No change needed.

```rust
headers.insert(
    axum::http::header::X_CONTENT_TYPE_OPTIONS,
    HeaderValue::from_static("nosniff"),
);
```

## Security review checklist

- [ ] UNTIL span limit (5 years) appropriate
- [ ] COUNT vs expansion mismatch documented or aligned
- [ ] PBKDF2 iteration count (100k) appropriate for Rust performance
- [ ] AES-GCM nonce never reused
- [ ] Referrer-Policy header present on all responses
- [ ] X-Content-Type-Options header present on all responses
- [ ] All existing callers of `encrypt_secret`/`decrypt_secret` audited

## Dependencies

- `pbkdf2 = "0.12"` — new dev dependency for PBKDF2-HMAC-SHA256
- `aes-gcm` — already present in `Cargo.toml` (line 8)

## Test plan summary

1. Unit tests for recurrence bounds (5a)
2. Unit tests for key derivation (5b)
3. Unit tests for AES-GCM encrypt/decrypt (5c)
4. Integration test verifying security headers on responses (5d, 5e)
5. Audit all callers of `encrypt_secret`/`decrypt_secret` for correctness after crypto swap
