# MCP acceptance harness (Phase 0)

A standalone, black-box acceptance harness for the production CommonCal MCP
endpoint. It targets a configurable production-component URL and asserts:

- **Phase 0.1** — the JSON-RPC `initialize` handshake is standards-compatible
  (MCP Streamable HTTP);
- **Phase 0.2** — the public OAuth challenge and discovery contract hold;
- **Phase 0.3** — DCR, Authorization Code + S256 PKCE, token exchange, and the
  authenticated MCP lifecycle (`initialize`, `notifications/initialized`,
  `tools/list`, `tools/call calendar_list`) complete;
- **Phase 0.4** — wrong issuer/audience/signature/expiry/client tokens, missing
  grant, revoked grant, and broadened grant fail closed, and two concurrent
  authenticated clients cannot observe each other's identity.

This harness is **not** the slice1-lab. It does not launch the lab binaries and
does not treat the lab binary as a release artifact. It reuses the lab's
fixture shapes (the `initialize` request shape from
`slice1-lab/negative_tests.py` case 10, the DCR request shape from
`slice1-lab/negative_tests.py` `fresh_dcr`, the standard access-token claim
contract from `slice1-lab/src/jwt.rs`, the scope catalog from
`slice1-lab/src/common.rs`, and the negative-token / fail-closed cases from
`slice1-lab/negative_tests.py` cases 3-9 and `slice1-lab/PROOF.md` P7), and the
MCP/OAuth standards-compliance expectations. It owns its own production-facing
configuration, lifecycle, reporting, and cleanup: it is a pure HTTP client,
starts no servers, and leaves no processes behind.

## Baseline expectation (Phase 0.1)

Against the **current** production component, `initialize` is **not**
standards-compatible. The custom gateway dispatcher (`mcp-server/src/gateway.rs`)
only recognizes `tools/list` and `tools/call`, so `initialize` returns HTTP 400
with JSON-RPC error `-32601 Method not found`.

The harness asserts standards-compliance and therefore **exits non-zero**
against the current production component. That failure is the **intentional,
asserted exit condition** for this phase: it documents the current broken state
that later phases (Phase 2, `mcp-rmcp-gateway`) must fix. The harness is the
regression gate: it stays red until the production component implements a
standards-compatible `initialize`.

## Phase 0.2: public challenge and OAuth discovery

`discovery.js` asserts the public OAuth discovery contract against the same
configurable production-component URL:

- **A1** — an unauthenticated JSON-RPC `initialize` request receives HTTP 401
  (an OAuth challenge), not a protocol-dispatch error (e.g. HTTP 400 with
  JSON-RPC `-32601`);
- **A2** — the 401 challenge carries a Bearer `WWW-Authenticate` header whose
  `resource_metadata` parameter is the configured public protected-resource
  metadata URL (the MCP endpoint origin plus
  `/.well-known/oauth-protected-resource`) and never a loopback or lab URL
  (e.g. the hard-coded `http://127.0.0.1:3001` in
  `mcp-server/src/gateway.rs:456-541`);
- **A3** — the protected-resource metadata document parses as JSON and names
  the configured authorization-server issuer in `authorization_servers`;
- **A4** — the authorization-server metadata document (RFC 8414 location, with
  the OIDC discovery location as fallback) parses as JSON and supplies usable
  `authorization_endpoint`, `token_endpoint`, `registration_endpoint`, and
  `jwks_uri` values (absolute http(s) URLs; https when the issuer is public).

Baseline expectation (Phase 0.2): against the **current** production component,
A1, A2, and A4 **fail** and A3 passes. Verified current state (2026-09-17,
`docs/MCP-PRODUCTION-FIX-PLAN.md`): `initialize` returns HTTP 400 with
`-32601` (no challenge is ever issued for the initial request); the 401
builders that do exist hard-code the lab loopback URL;
`https://mcal.hajnal.space/.well-known/oauth-protected-resource` is valid JSON
advertising `https://cal.hajnal.space`; and
`https://cal.hajnal.space/.well-known/openid-configuration` returns the
CommonCal SPA HTML, not authorization-server metadata. The harness therefore
**exits non-zero** against the current production component — the intentional,
asserted exit condition for this phase and the regression gate for later
phases.

```sh
node mcp-acceptance/discovery.js
# or, for a production-equivalent deployment:
MCP_URL=https://staging.example.com/mcp \
MCP_OAUTH_ISSUER=https://auth.staging.example.com \
  node mcp-acceptance/discovery.js
```

| Variable             | Default                          | Description                              |
| -------------------- | -------------------------------- | ---------------------------------------- |
| `MCP_URL`            | `https://mcal.hajnal.space/mcp`  | The MCP endpoint URL to probe.           |
| `MCP_OAUTH_ISSUER`   | `https://cal.hajnal.space`       | The configured authorization-server issuer. |
| `MCP_TIMEOUT_MS`     | `15000`                          | Request timeout in milliseconds.         |
| `MCP_RECORD_DIR`     | OS temp directory                | Where the deterministic record is written. |

Exit codes: `0` all assertions pass; `1` one or more assertions fail (the
intentional Phase 0.2 baseline against the current production component);
`2` harness error. The deterministic record is written to
`$MCP_RECORD_DIR/mcp-acceptance-discovery.json`.

## Phase 0.3: OAuth lifecycle

`oauth-lifecycle.js` asserts the full OAuth + MCP lifecycle against the same
configurable production-component URL:

- **L1** — DCR (RFC 7591) registration succeeds with the supported OpenCode
  loopback redirect shape (201, `client_id` present, no `client_secret`);
- **L2** — Authorization Code flow with `state` and S256 PKCE completes (code
  present, state round-trips);
- **L3** — token exchange completes and yields a resource-bound access token
  (3-part JWT) without manual JWT construction;
- **L4** — with that token, `initialize` succeeds (200, `protocolVersion`,
  `capabilities`, `serverInfo`);
- **L5** — `notifications/initialized` is accepted (202);
- **L6** — `tools/list` returns the expected nine tools, each with a
  description and a complete input schema;
- **L7** — `tools/call calendar_list` succeeds (200, content block).

Baseline expectation (Phase 0.3): against the **current** production component,
L1-L7 **fail** because the authorization service is not deployed (the
advertised issuer host falls through to the CommonCal SPA) and the MCP gateway
does not implement the MCP lifecycle. The harness therefore **exits non-zero**
against the current production component — the intentional, asserted exit
condition for this phase and the regression gate for later phases.

```sh
node mcp-acceptance/oauth-lifecycle.js
# or, for a production-equivalent deployment:
MCP_URL=https://staging.example.com/mcp \
MCP_OAUTH_ISSUER=https://auth.staging.example.com \
  node mcp-acceptance/oauth-lifecycle.js
```

| Variable             | Default                          | Description                              |
| -------------------- | -------------------------------- | ---------------------------------------- |
| `MCP_URL`            | `https://mcal.hajnal.space/mcp`  | The MCP endpoint URL to probe.           |
| `MCP_OAUTH_ISSUER`   | `https://cal.hajnal.space`       | The configured authorization-server issuer. |
| `MCP_REDIRECT`       | `http://127.0.0.1:8765/callback` | The admitted loopback redirect URI.      |
| `MCP_TIMEOUT_MS`     | `15000`                          | Request timeout in milliseconds.         |
| `MCP_RECORD_DIR`     | OS temp directory                | Where the deterministic record is written. |

Exit codes: `0` all assertions pass; `1` one or more assertions fail (the
intentional Phase 0.3 baseline against the current production component);
`2` harness error. The deterministic record is written to
`$MCP_RECORD_DIR/mcp-acceptance-oauth-lifecycle.json`.

## Phase 0.4: security & isolation

`security-isolation.js` asserts the fail-closed security and
concurrency-isolation contract against the same configurable
production-component URL:

- **S1** — wrong-issuer token fails closed (401 + public `WWW-Authenticate`);
- **S2** — wrong-audience token fails closed (401 + public `WWW-Authenticate`);
- **S3** — wrong-signature token fails closed (401 + public `WWW-Authenticate`);
- **S4** — expired token fails closed (401 + public `WWW-Authenticate`);
- **S5** — wrong-client token fails closed (401 + public `WWW-Authenticate`);
- **S6** — missing grant denies `calendar_list` without disclosing calendars;
- **S7** — revoked grant denies `calendar_list` without disclosing calendars;
- **S8** — broadened grant does not disclose unauthorized calendars;
- **S9** — two concurrent authenticated clients cannot observe each other's
  identity, calendars, session, or grant state.

Baseline expectation (Phase 0.4): against the **current** production component,
S1-S9 **fail** because the authorization service is not deployed and the MCP
gateway does not implement token validation, grant enforcement, or concurrency
isolation. The harness therefore **exits non-zero** against the current
production component — the intentional, asserted exit condition for this phase
and the regression gate for later phases.

```sh
node mcp-acceptance/security-isolation.js
# or, for a production-equivalent deployment:
MCP_URL=https://staging.example.com/mcp \
MCP_OAUTH_ISSUER=https://auth.staging.example.com \
  node mcp-acceptance/security-isolation.js
```

| Variable             | Default                          | Description                              |
| -------------------- | -------------------------------- | ---------------------------------------- |
| `MCP_URL`            | `https://mcal.hajnal.space/mcp`  | The MCP endpoint URL to probe.           |
| `MCP_OAUTH_ISSUER`   | `https://cal.hajnal.space`       | The configured authorization-server issuer. |
| `MCP_REDIRECT`       | `http://127.0.0.1:8765/callback` | The admitted loopback redirect URI.      |
| `MCP_TIMEOUT_MS`     | `15000`                          | Request timeout in milliseconds.         |
| `MCP_RECORD_DIR`     | OS temp directory                | Where the deterministic record is written. |

Exit codes: `0` all assertions pass; `1` one or more assertions fail (the
intentional Phase 0.4 baseline against the current production component);
`2` harness error. The deterministic record is written to
`$MCP_RECORD_DIR/mcp-acceptance-security-isolation.json`.

## Requirements

- Node.js 18 or newer (global `fetch`). CI uses Node 22.
- No third-party dependencies. The harness uses only Node built-ins.

## Invocation

Run the focused command from the repository root:

```sh
node mcp-acceptance/harness.js
```

By default this targets the production endpoint
(`https://mcal.hajnal.space/mcp`). To target a production-equivalent deployment
or a local instance, set `MCP_URL`:

```sh
# Target a production-equivalent deployment (e.g. a staging ingress)
MCP_URL=https://staging.example.com/mcp node mcp-acceptance/harness.js

# Target a locally running production component (same mcp-server binary)
MCP_URL=http://127.0.0.1:3001/mcp node mcp-acceptance/harness.js
```

### Configuration (environment variables)

| Variable               | Default                          | Description                                  |
| ---------------------- | -------------------------------- | -------------------------------------------- |
| `MCP_URL`              | `https://mcal.hajnal.space/mcp`  | The MCP endpoint URL to probe.               |
| `MCP_PROTOCOL_VERSION` | `2025-03-26`                     | The `protocolVersion` to negotiate.          |
| `MCP_CLIENT_NAME`      | `commoncal-mcp-acceptance`       | `clientInfo.name` in the initialize request. |
| `MCP_CLIENT_VERSION`   | `0.1.0`                          | `clientInfo.version` in the initialize req.  |
| `MCP_TIMEOUT_MS`       | `15000`                          | Request timeout in milliseconds.             |
| `MCP_RECORD_DIR`       | OS temp directory                | Where the deterministic response record is written. |

### Exit codes

| Code | Meaning                                                        |
| ---- | -------------------------------------------------------------- |
| `0`  | `initialize` is standards-compatible (PASS).                   |
| `1`  | `initialize` is NOT standards-compatible (FAIL) — the intentional Phase 0.1 baseline exit condition against the current production component. |
| `2`  | Harness error (invalid configuration, network failure, unexpected error). |

### Deterministic record

The harness writes a deterministic record of the request and response (object
keys sorted, volatile headers dropped) to
`$MCP_RECORD_DIR/mcp-acceptance-initialize.json` (default: the OS temp
directory). The path is printed on every run.

## CI usage

In CI, point the harness at a production-equivalent deployment and treat a
non-zero exit as a real failure once the production component is fixed. During
Phase 0 the harness is expected to be red; record the failure as the baseline.

```sh
# Example CI step (production-equivalent deployment)
MCP_URL=https://staging.example.com/mcp \
  MCP_RECORD_DIR="$RUNNER_TEMP/mcp-acceptance" \
  node mcp-acceptance/harness.js
```

## Verification (harness self-test)

A verification-only mock server exercises both the PASS and FAIL paths without
touching production. It is not a release artifact.

```sh
# Start the mock server in one terminal
node mcp-acceptance/mock/server.js

# In another terminal:
# FAIL path — mock mirrors the CURRENT production behavior (expect exit 1)
MCP_URL=http://127.0.0.1:3999/mcp-current node mcp-acceptance/harness.js
echo "exit: $?"   # -> 1

# PASS path — mock returns a standards-compatible initialize (expect exit 0)
MCP_URL=http://127.0.0.1:3999/mcp-compliant node mcp-acceptance/harness.js
echo "exit: $?"   # -> 0
```

Self-contained verifiers (they start and clean up the mock server themselves):

```sh
# Phase 0.1 harness: both exit paths (default mock port 3999)
node mcp-acceptance/verify.js

# Phase 0.2 discovery harness: both exit paths with focused A1/A2/A4
# violations on the FAIL path and a passing A3 (default mock port 3998,
# issuer host 3999; use --port to avoid collisions)
node mcp-acceptance/verify-discovery.js

# Phase 0.3 oauth-lifecycle harness: both exit paths (default mock port 4001,
# issuer host 4002; use --port to avoid collisions)
node mcp-acceptance/verify-oauth-lifecycle.js

# Phase 0.4 security-isolation harness: both exit paths (default mock port
# 4003, issuer host 4004; use --port to avoid collisions)
node mcp-acceptance/verify-security-isolation.js
```

The Phase 0.1/0.2 mock (`mock/server.js`) mirrors production's two-host
topology: the MCP host (MOCK_PORT) serves the MCP endpoint and the
protected-resource metadata; the issuer host (MOCK_ISSUER_PORT, default
MOCK_PORT + 1) serves the authorization-server metadata (SPA HTML in the
current mode, valid JSON in the compliant mode).

The Phase 0.3/0.4 mock (`mock/lifecycle-server.js`) is driven by a single
world selector (`MOCK_WORLD`, default `compliant`; `current` mirrors
production today). In the compliant world it implements the full OAuth + MCP
lifecycle (DCR, Authorization Code + S256 PKCE, token exchange, JWKS, and the
authenticated MCP lifecycle) plus verification-only test hooks (`/_test/*`)
for driving the negative token and grant-state cases. In the current world it
mirrors the verified current production state (the authorization service is
not deployed; the MCP gateway does not implement the MCP lifecycle).

## Files

- `harness.js` — the Phase 0.1 standalone black-box harness (initialize
  standards-compliance).
- `discovery.js` — the Phase 0.2 standalone black-box harness (public 401
  challenge + protected-resource and authorization-server metadata discovery).
- `oauth-lifecycle.js` — the Phase 0.3 standalone black-box harness (DCR,
  Authorization Code + S256 PKCE, token exchange, and the authenticated MCP
  lifecycle through `calendar_list`).
- `security-isolation.js` — the Phase 0.4 standalone black-box harness
  (fail-closed token/grant cases and concurrent-identity isolation).
- `lib.js` — shared helpers for the Phase 0.3/0.4 harnesses (OAuth flow, MCP
  calls, JWT helpers, deterministic JSON).
- `fixtures/initialize-request.json` — the `initialize` request fixture (reused
  from the slice1-lab request shape; owned by this harness).
- `verify.js` — self-contained verification of both Phase 0.1 harness exit
  paths.
- `verify-discovery.js` — self-contained verification of both Phase 0.2
  harness exit paths, including the focused A1/A2/A4 violations and the
  passing A3 on the FAIL path.
- `verify-oauth-lifecycle.js` — self-contained verification of both Phase 0.3
  harness exit paths.
- `verify-security-isolation.js` — self-contained verification of both Phase
  0.4 harness exit paths.
- `mock/server.js` — verification-only mock server for Phase 0.1/0.2 (PASS +
  FAIL fixtures, two-host topology).
- `mock/lifecycle-server.js` — verification-only mock server for Phase 0.3/0.4
  (full OAuth + MCP lifecycle, world selector, test hooks).
- `mock/oauth-provider.js` — verification-only mock OAuth provider (DCR,
  Authorization Code + S256 PKCE, token exchange, JWKS, token validation,
  grants, token minting).
- `README.md` — this document.
