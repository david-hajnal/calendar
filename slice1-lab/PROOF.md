# Slice 1 — Tracer Bullet Proof Spec

Disposable, loopback-only lab. No production deployment, no `MCP_OAUTH_ISSUER` change, no README edits.

## Pins (verified 2026-08-27 against primary sources)

| Component | Pin | Note |
|---|---|---|
| Hydra image | `oryd/hydra:v26.2.0` | Latest published. `v26.3.10` (DCR strict-schema fix) announced 2026-08-24, **not yet published** — see caveat P1. |
| Ory chart (production, Slice 4) | `0.63.0` | Verified in `k8s.ory.com/helm/charts/index.yaml`; its default image tag is `v26.2.0`, so the chart and image are pinned independently. |
| `rmcp` | `3.1.4` | Latest stable on crates.io; requires Rust >= 1.88 (we have 1.95). |

## Lab topology (all loopback)

```text
harness (lab_prove)
  |  DCR, auth, token, MCP calls
  v
Hydra public  http://localhost:4444   (oryd/hydra:v26.2.0, --dev, DSN=postgres)
Hydra admin   http://localhost:4445   (loopback only, never public)
PostgreSQL    http://localhost:5432   (disposable)
stub-adapter  http://localhost:8080   (login/consent stub, one fixed subject)
mcp-echo      http://localhost:3001   (rmcp SDK-backed, one calendar_list tool)
```

- Issuer: `http://localhost:4444/`
- Resource/audience: `http://localhost:3001/mcp`
- Scope catalog (fixed): `commoncal.calendar.metadata.read`, `commoncal.availability.read`, `commoncal.event.read.basic`, `commoncal.event.read.details`, `commoncal.event.create`, `commoncal.event.update`, `commoncal.event.delete`, `commoncal.reminder.read`, `commoncal.reminder.write`
- Fixed test subject: `1` (numeric, matches future CommonCal user ID contract)

## Proof items

### P1 — DCR positive (public client, strict schema)
- `POST /oauth2/register` with `token_endpoint_auth_method=none`, `grant_types=[authorization_code, refresh_token]`, `response_types=[code]`, exact loopback redirect `http://127.0.0.1:8321/callback`, `client_name=commoncal-lab`.
- Assert: `201`, `client_id` present, no `client_secret`, `redirect_uris` echoes the exact loopback URI.
- **Strict-schema check:** the response must NOT contain unset optional fields (`client_uri`, `logo_uri`, `policy_uri`, `tos_uri`, `contacts`, `jwks`) as `""`/`null`/`{}`. RFC 7591 marks them optional; a strict client accepts them only when absent or correctly typed.
- **EXPECTED GAP on v26.2.0:** this strict check fails (fields returned as empty/null). Re-run on `v26.3.10` once published. The positive DCR (registration succeeds, correct shape otherwise) must still pass.

### P2 — DCR negative (fail closed)
- Wildcard redirect `http://127.0.0.1:*/callback` rejected (4xx).
- Malformed redirect `not-a-url` rejected (4xx).
- Non-loopback public redirect `https://attacker.example/cb` — Hydra accepts it (open DCR); the lab records this as the known open-DCR property. Consent remains the authority boundary (P5). This is NOT a stop condition per the approved plan, but is recorded as a caveat.
- Oversized body rejected (413) — Hydra enforces a max body size.

### P3 — S256 PKCE + exact loopback redirect
- Generate `code_verifier` (43-128 chars), `code_challenge = BASE64URL(SHA256(verifier))`, `code_challenge_method=S256`.
- Drive: `/oauth2/auth` -> stub login (accept, subject=1) -> stub consent (accept, intersection scopes + exact audience) -> `redirect_uri?code=...&state=...`.
- Assert: callback URI exactly equals the registered redirect; `state` round-trips; `code` present.

### P4 — Token exchange + JWT claim shape (Gate 3 contract)
- `POST /oauth2/token` with `grant_type=authorization_code`, `code`, `code_verifier`, `redirect_uri`.
- Assert: `access_token` is a 3-part JWT; `refresh_token` is opaque (not a JWT).
- Decode + validate the access token (signature via discovery `jwks_uri`):
  - `iss` == `http://localhost:4444/`
  - `aud` contains exactly `http://localhost:3001/mcp`
  - `sub` == `1` (numeric contract)
  - `client_id` == the DCR client id
  - `scope` == the consent-approved intersection (string, space-separated)
  - `jti`, `iat`, `exp` present; `exp > iat`; `exp - iat <= 600` (10m TTL)
  - `amr` present (from login `authentication_methods`)
- JWKS: fetch `{issuer}/.well-known/oauth-authorization-server` -> `jwks_uri`; fetch JWKS; match `kid`; verify RS256 signature.

### P5 — Consent grants only the intersection (fail closed)
- Request scopes = catalog scopes + `evil.unknown.scope` + `offline_access`.
- Stub consent must grant ONLY `requested ∩ catalog ∩ fixed-approval`. Assert the token `scope` does NOT contain `evil.unknown.scope`.
- Stub must reject (and the flow must fail) if the requested audience is not the exact MCP resource.
- Stub must reject a replayed/unknown challenge (4xx from Hydra admin -> stub returns error, no redirect).

### P6 — SDK-backed MCP endpoint (rmcp 3.1.4)
- `POST /mcp` `initialize` -> 200, `serverInfo` present, `capabilities.tools` present.
- `POST /mcp` `tools/list` -> exactly one tool named `calendar_list` with a non-empty input schema.
- `POST /mcp` `tools/call` `calendar_list` -> one hardcoded calendar (id, name, url) returned as text content.
- Auth: the endpoint validates the Bearer JWT (iss/aud/exp/sub/client_id/scope) before any tool call.

### P7 — Negative MCP + token cases (fail closed)
- Unauthenticated `POST /mcp` -> `401` with `WWW-Authenticate: Bearer` challenge referencing protected-resource metadata.
- `tools/call` with no token -> 401 (no tool result leaked).
- Token with wrong `aud` (e.g. `http://localhost:3001/other`) -> rejected.
- Token with wrong `iss` -> rejected.
- Token with bad signature (flip a payload char) -> rejected.
- Expired token (`exp` in the past) -> rejected.
- Code replay: exchange the same `code` twice -> second exchange fails (4xx).
- Missing `code_verifier` (or wrong verifier) -> token exchange fails (4xx).

## Run commands

```sh
cd slice1-lab
docker compose up -d postgres hydra          # disposable infra
cargo run --bin stub-adapter &               # :8080
cargo run --bin mcp-echo &                   # :3001
cargo run --bin lab_prove                    # drives P1-P7, prints PASS/FAIL per item
docker compose down -v                       # tear down disposable state
```

## Stop conditions (from the approved plan)

- If Hydra cannot safely produce the required JWT claims / audience / consent-approved scope intersection -> stop, propose Gate 2 revision. (P4/P5)
- If open DCR can broaden token authorization beyond explicit CommonCal consent -> stop, propose Gate 2 revision. (P5: consent intersection must hold even though DCR is open)
- No custom OAuth registration/token proxy to conceal either failure.
- Hydra admin API never exposed publicly (loopback only in the lab).
- No real secret values or tokens in Git, logs, fixtures, docs, or the final response.
