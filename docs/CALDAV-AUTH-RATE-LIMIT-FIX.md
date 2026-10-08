# CalDAV authentication rate-limit repair

Date: 2026-10-03. Implemented and locally verified; not deployed. Native Apple
Calendar sync remains unverified until a manual deployment and device refresh.

## Confirmed cause

Apple discovery returned 207 for the principal, calendar home and calendar
collections, then both initial REPORT requests returned 429. Although
`DAV_AUTH_MAX_ATTEMPTS = 10` and `DAV_AUTH_WINDOW_SECONDS = 60` described a
failed-authentication limit, `authenticate_dav_request` called the consuming
`check_auth_rate_limit` before authenticating supplied credentials. Successful
discovery therefore spent the same allowance as invalid credentials.

The limiter is shared across clones of `CaldavAccountService`, users and DAV
paths. Its client key is the trimmed first `X-Forwarded-For` token. Missing,
unreadable and now empty first tokens share the `unknown` fallback. A second
forwarded token does not create an independent key.

## Final policy

- The first invalid supplied credential starts a 60-second window. Ten invalid
  attempts receive 401; subsequent requests receive 429 until that window ends.
- Successful authentication neither consumes nor clears the failure budget.
  Missing Authorization receives a free 401 challenge while below the cap;
  malformed supplied Authorization consumes a failure.
- An already-blocked key receives 429 even with valid or missing credentials.
  Credential verification is skipped until expiry, retaining brute-force
  protection. `Retry-After` is the remaining whole seconds, at least one; at
  exactly 60 seconds the key is admitted again.
- Persistence/infrastructure errors retain the existing 401 response but do
  not consume the invalid-credential budget.
- A per-client asynchronous lock serializes admission and verification so
  concurrent attempts cannot exceed ten failures. Independent keys retain
  independent budgets. Active or queued locks are not evicted.

The existing forwarded-header trust boundary is unchanged: trusted ingress
must overwrite client-supplied `X-Forwarded-For`, and direct backend access must
be isolated. No proxy configuration was found to verify that deployment
assumption. The fallback remains shared rather than inventing an identity.

This repair leaves discovery and password-persistence fixes, credential
hashing, secrets and unrelated working-tree changes intact.

## Verification

QA and security review passed without reported defects. All 192 Rust tests
and four Python fixture tests passed; Rust formatting passed.

| Suite | Passed |
| --- | ---: |
| CalDAV authentication unit tests | 6 |
| CalDAV HTTP/router tests | 50 |
| `caldav_event_resource` integration tests | 104 |
| `middleware_regression` and `session_middleware` | 20 |
| `apple_calendar_fixtures` | 12 |
| Python smoke fixture tests | 4 |

Router regressions run with shared middleware and cover more than ten valid
requests, three discovery traversals through `/dav/`, principal, home and two
collections followed by both initial REPORTs, independent/shared keys,
missing/malformed credentials, persistence errors, and concurrent requests.
An injected clock checks 429 with `Retry-After: 60`, then `1` at second 59,
and successful admission at second 60 on the same service instance. Twenty-four
concurrent invalid attempts produce exactly ten 401s and fourteen 429s.

The prior smoke sequence could stay under ten authenticated requests. The
extended script performs twelve authenticated root PROPFINDs before initial
REPORTs and explicitly discovers each advertised calendar collection. Its
fixture reproducing the old ten-request cap now fails as intended. Diagnostics
retain credential-safe output. Loopback fixture tests passed with sandbox
network permission.

Reproduce from the repository root:

```sh
cargo test --manifest-path backend/Cargo.toml --locked --lib caldav::auth::tests
cargo test --manifest-path backend/Cargo.toml --locked --lib caldav::http::tests
cargo test --manifest-path backend/Cargo.toml --locked --test caldav_event_resource
cargo test --manifest-path backend/Cargo.toml --locked --test middleware_regression --test session_middleware
cargo test --manifest-path backend/Cargo.toml --locked --test apple_calendar_fixtures
python3 scripts/test-caldav-smoke.py
cargo fmt --manifest-path backend/Cargo.toml --all -- --check
```

## Post-deployment verification

Deploy manually through the existing deployment procedure. Use the existing
connection password to preserve the password-persistence check; do not rotate
secrets for this repair. Run the following in Bash from the repository root
without shell tracing. The password is entered invisibly and never placed in a
command-line argument or literal:

```bash
export CALDAV_ORIGIN=https://cal.hajnal.space
read -r -p 'Happening email: ' CALDAV_USERNAME
export CALDAV_USERNAME
read -r -s -p 'Existing connection password: ' CALDAV_PASSWORD
printf '\n'
export CALDAV_PASSWORD
python3 scripts/caldav-smoke.py
unset CALDAV_USERNAME CALDAV_PASSWORD
```

Expect the discovery burst and initial REPORTs to complete without 429. If the
same client key was already blocked by failures, wait out its `Retry-After`
before rerunning. Repeat with the same password after a restart to confirm
password persistence remains intact.

Then refresh the native Apple Calendar account. Record macOS version, time and
the observed result. If it fails, capture the **first failing request's method,
path without query, status and User-Agent**, with timestamp for correlation.
Use the credential-safe proxy capture procedure in
[DEPLOYMENT.md](DEPLOYMENT.md); if the actual ingress lacks such a capture,
configure only those fields manually before retrying. Backend access logs lack
User-Agent, and safe proxy capture is not claimed to be preconfigured. Exclude
Authorization, Cookie, passwords, request/response bodies and query secrets.
Inspect per-property statuses when an outer 207 contains a failure. Tests alone
do not establish that native Apple initial sync is fixed.

## Review handoff

Suggested title: `fix(caldav): charge authentication limit only for failures`

Suggested description: Successful DAV discovery consumed the ten-request
authentication allowance and blocked initial REPORTs. Count invalid supplied
credentials instead, serialize each client's admission and verification, and
retain 429 for every request from an already-blocked key until expiry. Add
router/middleware regressions and a smoke burst that detects the old cap.
Validation: 192 Rust tests, four Python fixture tests and formatting passed;
native Apple refresh remains a post-deployment check.

Relevant history SME: David Hajnal (`david-hajnal`, `david@hajnal.space`); GitHub
handle was not confirmed. No commit, push or deployment was performed.
