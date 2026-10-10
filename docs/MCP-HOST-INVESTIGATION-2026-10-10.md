# MCP public host fix — 2026-10-10

Status: deployed implementation inspected; source fix and local checks complete.
Image publication, operator rollout and authenticated public verification remain
pending. No calendar events have been accessed or changed, and no token,
credential, cookie or authorization code has been printed.

## Root cause and deployed evidence

Operator kubectl output identifies ready pod `commoncal-mcp-957b85b8c-hpkc9`:

- Image: `ghcr.io/david-hajnal/calendar-mcp:sha-def52b6c2db4e3325467c9b9c1eaa77a11418e3e`
- Running imageID: `sha256:6d403202d52542400b0031f34a65c924fae65ddae70043d96efb665cbddcd8b4`
- Resource: `https://mcal.hajnal.space/mcp`; domain: `mcal.hajnal.space`

An independent registry request confirmed that immutable tag's digest exactly
matches the running imageID. Its successful normal publication is
[run 37940471739](https://github.com/david-hajnal/calendar/actions/runs/37940471739).
`git show def52b6:mcp-server/src/main.rs` constructs the transport with
`StreamableHttpServerConfig::default()`, and its lockfile pins rmcp 3.1.4.
That SDK default permits only `localhost`, `127.0.0.1`, and `::1`. Its validator
emits the exact reported `Forbidden: Host header is not allowed` response.
Bearer middleware wraps the transport, so authentication succeeds before the
public Host is rejected, whereas unauthenticated requests return 401 first.

The inspected live ingress uses Traefik `websecure`, exact hostname routing to
`commoncal-mcp:80`, and no attached middleware. No Host rewrite appears in the
repository ingress configuration. The fix requires no proxy Host rewriting or
OAuth changes. Global proxy forwarding configuration was not dumped; this
report does not claim a complete proxy audit.

## Change

Production startup now calls the shared `transport::server_config` builder,
which derives one exact hostname from the configured public resource URL and
sets rmcp's nonempty allowlist. It fails closed for malformed, missing-host,
non-HTTP(S), or wildcard resource URLs. DNS-rebinding host protection remains
enabled. DCR, PKCE S256, exact issuer/resource validation, scope enforcement,
and calendar grant enforcement implementations are unchanged.

rmcp's bare hostname entry admits that exact hostname with an explicit port,
including `mcal.hajnal.space:443`; a redundant :443 entry is unnecessary. This
follows the SDK's hostname matching semantics rather than implementing an
additional absent-or-443-only policy. Unexpected hostnames remain forbidden.
Forwarded Host is not used to authorize an unexpected actual Host.

## Checks

Before the fix, actual rmcp transport regression requests for both public Host
representations returned HTTP 403 instead of expected 200. An unexpected Host
returned 403 as expected. The regression now shares production's builder and
also exercises real bearer middleware using signed JWTs and mock discovery/JWKS.
Authenticated initialize and tools/list pass for ordinary Host and :443;
missing/invalid tokens and unexpected authenticated Host are rejected.

Local results:

- 214 MCP library tests passed.
- 34 existing integration tests passed; the external PostgreSQL auth-server
  token test `phase3_real_auth_server_token_validates` was excluded. Local signed
  token transport tests exercise the actual authentication middleware.
- Four transport regression tests passed after the fix.
- Five offline public-checker tests passed; formatting passed.

Live public requests before rollout reject missing/invalid tokens for both
initialize and tools/list: four HTTP 401 responses with exact
`resource_metadata="https://mcal.hajnal.space/.well-known/oauth-protected-resource"`.
This is negative-auth verification, not authenticated success after deployment.
The two previously saved local OAuth proofs are expired and were not used.

`scripts/check-mcp-public-host.py` runs the public read-only regression with a
private `--proof-file` from `auth-browser-proof.py` or private `--token-file`.
It verifies authenticated initialize/tools/list, both public Host forms,
negative-token challenges and unexpected Host rejection. It only sends
initialize, initialized notification and tools/list; no calendar tools are
called. Without a token it exits 2 and reports incomplete verification.
Unexpected public Host rejection may happen at ingress, so the local transport
regression independently establishes rmcp rejection.

## Deployment

The inspected MCP HelmRelease is active and retains the deployed image above.
Root Flux Kustomization is suspended at
`main@sha1:a4f0538496153fdf76679854c1abd143e97b75d2`.
Preserve that pause and existing live values during the targeted rollout.
The normal push-to-main workflow builds/scans/publishes all three immutable
images and promotes their Git tags. Root Flux's pause prevents applying those
Git changes to unrelated live releases during this operation.

After publication is verified, the operator should patch only the MCP
HelmRelease image tag, reconcile that release and verify rollout/imageID. Do
not reset release values or resume root Flux against stale recovery values.
Retain the old immutable MCP image as the rollback reference. Complete a fresh
browser login and run the public checker with its private proof file. Record
publication, rollout and authenticated results here only after execution.
