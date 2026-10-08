# Production MCP connection fix plan

Status: proposed  
Prepared: 2026-09-17  
Target client: OpenCode (using LM Studio/Qwen as its model provider)

## Outcome

Make `https://mcal.hajnal.space/mcp` a standards-compatible, authenticated
Streamable HTTP MCP endpoint that OpenCode can authorize with OAuth and use to
call CommonCal tools.

OpenCode is already the MCP client. LM Studio supplies the model and Qwen is the
model; neither replaces the MCP client. No additional MCP client is required.
MCP Inspector may be used only as an independent diagnostic tool.

## Verified current state

Production was probed on 2026-09-17:

- `POST /mcp` with `initialize` reaches the service but returns JSON-RPC
  `-32601 Method not found`.
- unauthenticated `tools/list` returns nine names, without complete MCP tool
  schemas.
- unauthenticated `tools/call` returns `401`, but its `WWW-Authenticate` header
  points to `http://127.0.0.1:3001/...` rather than the public production URL.
- protected-resource metadata at
  `https://mcal.hajnal.space/.well-known/oauth-protected-resource` is valid JSON
  and advertises `https://cal.hajnal.space` as the authorization server.
- `https://cal.hajnal.space/.well-known/openid-configuration` returns the
  CommonCal SPA HTML, not authorization-server metadata.
- `opencode mcp auth commoncal-production` fails on `initialize` before OAuth
  discovery can begin.

This means production is reachable but is not currently connectable by a
standard MCP client.

## Confirmed root causes

### 1. Production does not implement the MCP lifecycle

The production route mounts a custom Axum handler
(`mcp-server/src/main.rs:54-78`). Its dispatcher recognizes only `tools/list`
and `tools/call`; every other method, including `initialize`, returns `-32601`
(`mcp-server/src/gateway.rs:103-185`). Authentication is performed only inside
the `tools/call` branch (`mcp-server/src/gateway.rs:214-254`), so OpenCode never
receives an OAuth challenge for its initial request.

The custom wire responses are also incomplete:

- `tools/list` emits only `{ "name": ... }`, with no description or
  `inputSchema`, and does not preserve the JSON-RPC request id
  (`mcp-server/src/gateway.rs:187-212`).
- tool results use project-specific serialization rather than SDK-generated MCP
  content/result envelopes.

`rmcp` is already a production dependency, and the repository contains a
working SDK-backed reference in `slice1-lab/src/bin/mcp_echo.rs`. That lab
supports initialization, Streamable HTTP sessions, typed schemas, OAuth
middleware, and all tool categories. It explicitly says it is not the
production server (`slice1-lab/src/bin/mcp_echo.rs:1-13`).

### 2. The production OAuth challenge and JWT contract are inconsistent

Both production 401 builders hard-code the lab protected-resource URL
(`mcp-server/src/gateway.rs:456-541`).

The JWT validator says it uses discovery, but actually fetches
`<issuer>/.well-known/oauth-jwks` directly
(`mcp-server/src/oauth.rs:304-350`). It must instead fetch authorization-server
metadata and follow its `jwks_uri`.

Production also requires private namespaced claims such as
`https://commoncal.tld/user_id` and a custom scope array
(`mcp-server/src/oauth.rs:24-46,158-177`). The proven authorization server and
lab validator use standard claims: numeric `sub`, `client_id`, space-delimited
`scope`, `jti`, and `amr` (`slice1-lab/src/jwt.rs:12-33,147-184`). A token from
the repository's own authorization server would therefore still be rejected by
the production MCP server.

### 3. The authorization service is not deployed or routed in production

The production Flux kustomization includes only core and MCP
(`deploy/flux/overlays/production/kustomization.yaml:3-5`). The existing auth
HelmRelease is excluded and was deliberately removed because PostgreSQL was not
available (`deploy/PROBLEM-02-remove-auth.md`). Consequently, the advertised
issuer host is handled by core's `/` ingress and falls through to the SPA.

The dormant auth values cannot simply be re-enabled:

- issuer, resource, and CommonCal URL are all set to
  `https://cal.hajnal.space`, although the resource must be
  `https://mcal.hajnal.space/mcp`
  (`deploy/flux/overlays/production/charts/auth-helmrelease.yaml:34-47`);
- auth and core would both claim the same hostname and `/` prefix;
- the service requires PostgreSQL persistence and production secrets;
- its DCR callback policy currently admits a fixed lab loopback callback rather
  than the exact OpenCode loopback shape needed in production;
- lab-only test endpoints and callback helpers must not ship enabled.

### 4. CommonCal consent and grant handling was proven only in the lab

The candidate authorization server redirects interactions to CommonCal
consent, but the production backend/frontend has no corresponding handoff and
consent flow. Helm contains dormant `authBridge` settings, but production code
does not implement their use.

Current grant-management endpoints are placeholders: list always returns an
empty array and create writes `user_id = 0`
(`backend/src/mcp_grant_management.rs:60-117`). The working, session-bound
consent/grant behavior exists only in `slice1-lab/src/bin/commoncal.rs`.

### 5. Core's MCP internal API is not protected

The API-key validator is unused, and its current logic succeeds when the
expected secret is empty (`backend/src/mcp_internal.rs:206-223`). The entire
internal router is merged without authentication middleware
(`backend/src/main.rs:224-288`). Since the MCP server calls core through the
public HTTPS origin, this must be fixed before MCP is enabled in production.

## Target architecture and decisions

1. **Use `rmcp` in the production server.** Promote the proven SDK pattern;
   do not extend the hand-written JSON-RPC dispatcher.
2. **Use a dedicated authorization-server hostname.** Default this plan to
   `https://auth.hajnal.space` to avoid the core SPA ingress. Confirm the final
   hostname before creating DNS/TLS and use that one value everywhere.
3. **Use the standard token contract already proven by the lab.** Required:
   `iss`, numeric `sub`, exact MCP `aud`, `exp`, `iat`, `client_id`, and `scope`.
   Optional: `jti`, `amr`, and `acr`.
4. **Keep CommonCal authoritative for users, sessions, calendar membership, and
   MCP grants.** The authorization server owns OAuth protocol state only.
5. **Use SQLite for authorization-server state.** The 2026-10-08 decision
   supersedes the initial PostgreSQL plan. One auth replica owns a retained PVC;
   migrations, online encrypted backups and recovery remain required.
6. **Pass identity per request/session.** Do not copy the lab's global
   `CURRENT_CLAIMS` slot (`slice1-lab/src/bin/mcp_echo.rs:46-62`); it is only
   safe for sequential lab traffic.

## Implementation plan

### Phase 0 — Add failing acceptance coverage

Dispatch this phase as four bounded tasks, in order, to keep the production
acceptance work independently runnable:

1. `mcp-acceptance-harness-foundation` — target configuration, reusable
   fixtures, deterministic baseline `initialize` probe, and intentional current
   failure.
2. `mcp-acceptance-discovery` — unauthenticated challenge plus public resource
   and authorization-server metadata assertions.
3. `mcp-acceptance-oauth-lifecycle` — DCR, S256 PKCE, token exchange, and the
   authenticated MCP lifecycle through `calendar_list`.
4. `mcp-acceptance-security-isolation` — negative token/grant cases and
   concurrent client identity isolation.

Phase 2 depends on task 4, so no protocol replacement is accepted without the
full Phase 0 regression harness in place.

Add black-box tests before changing behavior:

- unauthenticated `initialize` receives `401` with the public
  `resource_metadata` URL;
- protected-resource and authorization-server metadata are valid JSON;
- DCR, Authorization Code + S256 PKCE, and token exchange complete;
- authenticated `initialize`, `notifications/initialized`, `tools/list`, and
  `tools/call calendar_list` succeed;
- wrong issuer, audience, signature, expiry, client, missing grant, revoked
  grant, and broadened grant fail closed;
- two concurrent authenticated clients cannot observe each other's identity.

Build this as a production-component harness, reusing fixtures and assertions
from `slice1-lab`, rather than treating the lab binary itself as the release
artifact.

Exit gate: the new tests reproduce the current `initialize` failure and fail
for the expected reasons.

### Phase 1 — Secure the core internal boundary

- Apply API-key middleware to the complete `/internal/mcp/*` router and any
  MCP-only reminder/token-exchange route.
- Fail startup when `MCP_INTERNAL_API_KEY` is absent or empty in production.
- Compare credentials in constant time and return `401` for missing/invalid
  keys without logging secret values.
- Confirm NetworkPolicy permits MCP-to-core traffic and denies unrelated pods;
  keep TLS on the current public service path unless a separately scoped
  private-service change is approved.
- Add route-level tests covering every protected route with missing, invalid,
  and valid keys.

Exit gate: no MCP internal operation is reachable without the configured key.

### Phase 2 — Replace the custom MCP gateway with `rmcp`

- Mount `rmcp`'s Streamable HTTP service at `/mcp` and implement
  `ServerHandler`.
- Put bearer-token authentication around the complete MCP service so the first
  unauthenticated `initialize` receives the standards-compliant 401 challenge.
- Derive `resource_metadata` from `MCP_PUBLIC_RESOURCE_URL`; remove all
  loopback constants.
- Port all nine tools to typed SDK handlers with descriptions, complete JSON
  input schemas, MCP content blocks, request ids, and protocol error mapping.
- Preserve the existing domain behavior behind the handlers: live grant lookup,
  authorization, audit, rate limits, idempotency, and confirmation semantics.
- Store validated identity in request/session-safe context. Add a concurrency
  isolation test.
- Remove or retire the custom dispatcher after parity tests pass.

Also correct `calendar_list`'s permission mapping while porting it; it currently
checks the `availability_find` capability.

Exit gate: a generic Streamable HTTP client can complete the MCP lifecycle and
inspect valid schemas without authentication/context leakage.

### Phase 3 — Align discovery and access-token validation

- Fetch `/.well-known/oauth-authorization-server` (and support the OIDC
  discovery form where appropriate), verify that returned `issuer` exactly
  matches configuration, then follow its HTTPS `jwks_uri`.
- Parse numeric `sub` as CommonCal user id and standard `client_id` and `scope`
  claims. Validate signature, allowed algorithm, `kid`, exact `iss`, exact MCP
  audience, `exp`, and `iat`; retain a small clock skew.
- Add bounded metadata/JWKS caching, key rotation overlap, and one refresh on an
  unknown `kid`. Never accept a stale key indefinitely.
- Keep grant lookup on every tool call so grant revocation takes effect even
  while an access token remains valid.
- Add integration tests that validate tokens issued by the repository's real
  authorization-server implementation, not hand-built lookalike fixtures only.

Exit gate: the candidate issuer's token is accepted, while every negative token
fixture is rejected with the correct public challenge.

### Phase 4 — Promote consent and grant integration into core

- Port the lab's private interaction bridge client into the real backend using
  `AUTH_BRIDGE_URL`, timeout, and secret configuration.
- Add session-protected login continuation and consent endpoints/UI. Show the
  registered client, requested scopes, and calendars; never trust a user id from
  the browser payload.
- On approval, upsert exactly one active grant for the authenticated user and
  OAuth `client_id`, intersect requested scopes/calendars with the live allowed
  set, then resume the authorization interaction.
- On denial, create no grant. Make retries idempotent and prevent approval from
  broadening an existing grant accidentally.
- Replace all `user_id = 0` and empty-list placeholders. Make list, narrow, and
  revoke routes session-bound and ownership-checked.
- Preserve immediate revocation by checking the authoritative grant and live
  calendar membership for every tool invocation.

Exit gate: approve, deny, retry, narrow, cross-user, stale-membership, and revoke
tests from the lab pass against production core code.

Implementation update (2026-10-06): core now renders session-bound consent
with explicit calendar selection and the existing CSRF protection. Password and
email-link login return to the consent page using validated same-origin
continuations. The private interaction view binds consent to the OAuth login
subject; expired or mismatched interactions fail before grant writes.

Migration `0030_mcp_consent_receipt.sql` stores hashed handoff receipts so retries
retain the same grant and cannot reverse narrowing, recreate a revoked grant,
or switch users. Identical unconsumed authorization-server bridge decisions can
be retried after a lost response. Existing active grants may only narrow;
deliberately adding access requires revoking the grant and authorizing again.
Grant-management updates also reject permission and expiration widening, and
replacement is transactional. Core intersects granted calendars with live,
unarchived membership on every MCP grant lookup.

Consent routes are enabled only when both `AUTH_BRIDGE_URL` and
`AUTH_BRIDGE_SECRET` are configured. Deploy the matching authorization-server
interaction-view changes together with core: consent now requires the view's
`subject` to match the authenticated user. Production rollout and public-ingress
proof remain in Phases 5–7.

Local Phase 4 verification passed on 2026-10-06: 687 backend tests, 264 MCP
tests, and 138 frontend tests; backend formatting and Clippy with warnings
rejected; frontend TypeScript and ESLint. The consent submit script was also
executed under a DOM harness for approval, denial, CSRF, selected IDs, redirect,
and failure recovery. A real PostgreSQL proof of `HandoffStore` confirmed
identical-decision retries, subject/decision mismatch rejection, one-time
consumption, replay rejection, and expiry using a temporary table rolled back
after verification. Independent security review found no actionable Phase 4
issues. These are local component proofs; deployed browser OAuth acceptance
remains part of Phases 6–7.

### Phase 5 — Productionize and deploy the authorization server

- Provision backed-up persistent SQLite storage and run the provider schema migration before
  enabling the deployment.
- Remove/disable lab test endpoints, loopback callback server, in-memory audit
  assumptions, and lab-named environment variables from the production entry
  point.
- Configure:
  - issuer: the chosen dedicated HTTPS hostname;
  - resource: `https://mcal.hajnal.space/mcp`;
  - CommonCal consent origin: `https://cal.hajnal.space`;
  - private bridge: cluster-only service with a rotated bearer secret;
  - short access-token lifetime, refresh rotation/reuse detection, revocation,
    signing-key overlap, request limits, and structured redaction.
- Configure DCR for OpenCode's actual callback behavior: loopback host, exact
  callback path, and permitted ephemeral port. Continue rejecting wildcards,
  arbitrary remote HTTPS redirects, and custom schemes unless explicitly
  approved.
- Add DNS and TLS for the dedicated issuer. Publish valid JSON authorization
  metadata and JWKS without any SPA fallback.
- Correct the dormant HelmRelease, NetworkPolicies, PDB, secrets, resource
  values, health checks, and Flux dependency order, then add it to the production
  kustomization only after its prerequisites exist.

Exit gate: discovery, DCR, PKCE, token issue, refresh, revocation, restart
persistence, and key overlap pass against the deployed issuer before MCP points
clients to it.

### Phase 6 — Wire and validate the complete deployment

Local implementation prepared and validated on 2026-10-08; the live exit gate
remains open. The separate `deploy/flux/overlays/auth-cutover` candidate wires
auth → core → MCP (SQLite within auth), aligns the dedicated issuer/resource/core origin,
and injects the shared bridge Secret under the runtime names each component
expects. Every candidate HelmRelease is suspended and its copied image tag must
be replaced by a published matching build before activation. Active production
Flux remains unchanged. MCP uses one primary issuer source; its schema rejects
simultaneous Secret and literal environment sources.

`python3 scripts/test-auth-cutover-manifests.py` and the full
`scripts/validate-deploy.sh` pass. The cross-chart rendered assertions cover URL
agreement, public TLS, private bridge Service/NetworkPolicy, matching Secret
references, actual core environment names, migration/runtime readiness and
acyclic dependency order. The candidate README documents staged operator
unsuspension, required SMTP inputs and public-ingress discovery commands.

The existing OAuth lifecycle harness assumes a mock immediate `/authorize`
redirect and cannot complete production `/auth` browser login/consent; the
security harness depends on mock token/grant hooks. Passing their mock suites
is not a live release proof. Public ingress, clean-cache OpenCode browser OAuth,
nine-tool/calendar acceptance, revocation, restart refresh, two-account isolation,
cluster NetworkPolicy and deployed backup/restore still require the accessible
production-equivalent environment and operator release gates.


- Enable core's auth bridge and deploy the completed consent/grant code.
- Set the MCP protected-resource metadata and validator issuer to the dedicated
  authorization hostname. Add manifest tests asserting that issuer, audience,
  resource metadata, ingress host, and TLS host agree across charts.
- Deploy first to a production-equivalent environment and run the black-box
  harness through ingress, including concurrency and negative cases.
- Canary production, monitor auth error classes, initialization failures, grant
  denials, tool latency, provider storage health, and MCP/core 5xx rates.

Exit gate: the full acceptance matrix below passes through public production
URLs, not pod port-forwards.

### Phase 7 — OpenCode acceptance and documentation

Validate with the supported OpenCode version and a clean OAuth cache:

```sh
opencode mcp add commoncal-production --url https://mcal.hajnal.space/mcp
opencode mcp auth commoncal-production
opencode mcp list
```

Complete browser login and consent, then verify:

1. authenticated `initialize` negotiates a supported protocol version;
2. `tools/list` returns all nine tools with schemas;
3. `calendar_list` returns only calendars selected in the grant;
4. narrowing or revoking the grant changes the next call immediately;
5. restarting OpenCode refreshes/re-authorizes without manual token copying.

Update `docs/MCP-CONNECTION.md` only after this proof passes. It should explain
the OpenCode/LM Studio/Qwen roles and should not ask users for JWTs, internal API
keys, pod access, or a second MCP client.

## Production acceptance matrix

| Area | Required result |
|---|---|
| MCP challenge | Unauthenticated `initialize` returns 401 and a public `resource_metadata` URL |
| Resource discovery | Protected-resource document is JSON and names the dedicated issuer |
| OAuth discovery | Authorization metadata is JSON with authorization, token, registration, and JWKS endpoints |
| Client registration | OpenCode DCR callback is accepted; unsafe redirect shapes are rejected |
| OAuth flow | Browser login + consent + S256 PKCE issues a resource-bound token |
| Token validation | Standard claims and exact issuer/audience validate; negative fixtures fail |
| MCP lifecycle | `initialize` and `notifications/initialized` succeed |
| Tool discovery | Nine tools have descriptions and complete input schemas |
| Authorization | Calls are limited by token scope, grant, live membership, and operation policy |
| Revocation | Grant revocation affects the next call; token/refresh revocation works |
| Isolation | Concurrent users/clients never share identity or session state |
| Internal API | Missing/invalid MCP API key is rejected on every protected core route |
| Persistence | OAuth state survives pod restart; migrations and backup/restore are proven |
| OpenCode | `mcp auth`, `mcp list`, and `calendar_list` succeed without token copying |

## Rollout and rollback

Roll out in this order:

1. core internal-API protection;
2. retained auth SQLite storage, issuer DNS/TLS, and authorization service without advertising it;
3. production core consent/grant flow and private bridge;
4. SDK-backed MCP deployment and production-equivalent end-to-end tests;
5. protected-resource metadata/issuer cutover;
6. OpenCode canary and general availability.

Keep prior core and MCP images, the previous issuer secret, and the Flux
revisions available during the canary. A rollback restores those revisions and
removes the new issuer from protected-resource metadata. OAuth key material and
the auth SQLite PVC (and any preserved legacy PostgreSQL volume) must not be deleted during rollback. The existing MCP path
is already unavailable to standard clients, so rollback protects the calendar
application first; it does not constitute a working MCP fallback.

## File map

Primary implementation areas:

- `mcp-server/src/main.rs` — mount SDK service and auth middleware
- `mcp-server/src/gateway.rs` — retire custom protocol dispatcher; retain reusable policy logic
- `mcp-server/src/oauth.rs` — discovery, standard claims, JWKS cache/rotation
- `mcp-server/src/tools/` — typed SDK tools and schemas
- `backend/src/mcp_internal.rs` and `backend/src/main.rs` — fail-closed internal API boundary
- `backend/src/mcp_grant_management.rs` — real session-owned grant management
- backend/frontend HTTP routes — login continuation and consent UI
- `slice1-lab/auth-server/` — promote into a production authorization-service package
- `deploy/helm/commoncal-auth/` — production auth chart
- `deploy/helm/commoncal/` — auth bridge and core secret/network wiring
- `deploy/helm/commoncal-mcp/` — public resource/issuer wiring
- `deploy/flux/overlays/production/` — issuer ingress, dependencies, and rollout
- `slice1-lab/` — reusable proof fixtures; not a production binary

## Prerequisites and explicit decision gates

Work may begin on tests, internal API security, MCP SDK adoption, and core
consent code immediately. Production cutover is blocked until all of these are
decided or supplied:

- final dedicated issuer hostname and DNS/TLS ownership;
- persistent auth SQLite storage, encrypted backups, and restore procedure;
- production OAuth signing keys, cookie keys, and bridge-key rotation process;
- the supported OpenCode version and its observed loopback callback shape;
- consent UI/product copy and the initial scope defaults.

## Definition of done

The fix is complete only when a clean OpenCode installation can add the remote
server, authenticate through the browser, approve a constrained grant, list the
nine MCP tools, and call `calendar_list` against production—without copying a
token, using `MCP_INTERNAL_API_KEY`, port-forwarding a pod, or installing a
second MCP client—and all negative/security tests remain green.

## Phase 5 local implementation status (2026-10-06, historical)

The PostgreSQL hosting and provisioning described in this section were superseded
by the SQLite decision below. Follow the current setup guide rather than these
historical steps.

The user selected `auth.hajnal.space` and in-cluster PostgreSQL. Production auth
now has explicit fail-closed configuration, durable audit/rate limiting, bounded
periodic retention, schema readiness, and public-only JWKS with rotation support.
Real PostgreSQL integration covers OAuth/PKCE, refresh reuse/revocation, restart
persistence, key overlap, disabled lab endpoints, and automatic retention.

Prepared a separate PostgreSQL image/chart with verified TLS, retained data and
backup volumes, encrypted daily backups, certificate reload, stdin secret
provisioning, and a read-only prerequisite gate. See [the rollout runbook](AUTH-PRODUCTION.md).
The active production overlay is unchanged. Phase 5 is not complete until the
images are published, the intended cluster is accessible, DNS/TLS and secrets are
provisioned, an encrypted backup and isolated restore are demonstrated, and the
issuer is deployed and verified. Off-cluster backup export remains required.


## SQLite migration update (2026-10-08)

The user selected SQLite to fit the small production server (one MCP client,
3–4 users). This supersedes the PostgreSQL hosting decision above. Production
OAuth provider state, handoffs, audit and rate limits now use SQLite WAL on one
retained PVC. Single-replica Recreate rollouts and a same-volume migration
initContainer prevent overlapping auth pods. Backups use a verified online
snapshot and age encryption; an isolated encrypted restore must prove refresh
continuation. The user's existing age identity remains usable.

The production candidate contains only auth/core/MCP; no PostgreSQL image,
service, certificates or database credentials are required. Existing PostgreSQL
storage is preserved and an optional read-only offline importer is available.
See [the updated setup guide](AUTH-PRODUCTION.md). Live publication, server
provisioning, off-cluster exports and ingress/OpenCode acceptance remain gates.

Local SQLite verification passed on 2026-10-08: seven unit/config tests,
production OAuth integration, and the actual Node 22.23.3 image running as UID
65534 with a read-only root filesystem. The image proof includes age-encrypted
online backup, isolated restore with refresh continuation, failed-encryption
archive preservation, successful retention pruning and real recipient validation.
A disposable PostgreSQL source import also passed for all four stores, timestamps,
consumption, source preservation and overwrite refusal. Rendered chart/cutover
contracts and the full deployment validation suite passed. These are local proofs;
live production state has not been converted or deployed.
