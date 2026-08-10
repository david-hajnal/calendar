# Plan: Hardening — recurrence bounds, key derivation, security headers

## Severity: MEDIUM + LOW

## Problem
Multiple defense-in-depth gaps identified across recurrence rules, key derivation, and security headers.

## Findings

### 5a. Recurrence rule bounds insufficient (MEDIUM)
- Parser accepts COUNT up to 1,000,000 but expansion bounded by 1,000 occurrences
- UNTIL has no maximum date cap — `FREQ=DAILY;UNTIL=99991231T235959Z` parses successfully
- Expansion hits iteration limit but error silently converted to `InvalidInput`
- Attacker can create events that cause CPU exhaustion on event list queries

**Fix:**
1. Add UNTIL bound validation in parser (max span: 5 years)
2. Align COUNT with expansion limits or document the mismatch
3. Return specific error for `ComplexityLimitExceeded` instead of generic `InvalidInput`
4. Add monitoring for high-complexity recurrence rules

**Files:** `backend/src/recurrence.rs`, `backend/src/event.rs`

### 5b. Weak key derivation (MEDIUM)
- `SecretKey::derive` uses single SHA-256 with no salt/iterations
- SESSION_SECRET-based key lacks computational cost

**Fix:**
1. Replace with PBKDF2, scrypt, or argon2
2. Add per-instance salt
3. Configure iteration count appropriate for Rust performance

**Files:** `backend/src/security.rs`

### 5c. Custom crypto construction (MEDIUM)
- Non-standard HMAC-based stream cipher in `security.rs:36-77`
- Custom authenticated encryption is error-prone

**Fix:**
1. Replace with AES-GCM or ChaCha20-Poly1305
2. Use established crate (e.g., `aes-gcm`, `chacha20poly1305`)
3. Audit all usages of current construction

**Files:** `backend/src/security.rs`, all callers

### 5d. Missing Referrer-Policy header (LOW)
- Referrer leakage to third parties via link sharing

**Fix:**
1. Add `Referrer-Policy: strict-origin-when-cross-origin`
2. Configure in `http.rs` security headers middleware

**Files:** `backend/src/http.rs`

### 5e. Missing X-Content-Type-Options header (LOW)
- MIME sniffing risk on JSON API responses

**Fix:**
1. Add `X-Content-Type-Options: nosniff`
2. Configure in `http.rs` security headers middleware

**Files:** `backend/src/http.rs`

## Discovered by: synthesis (anchoring-suppressed), blue team independent
