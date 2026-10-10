# Production OAuth DCR investigation — 2026-10-10

The existing fix is present and correct. The repository was clean at the start,
at source revision `1ebaab8ee4e5fcb0548267c984c68f0e4fa232f8`. No auth validation,
Rust code, production configuration, or live resources were changed by this
investigation. Deployment guidance was corrected because tag promotion alone
does not deploy the dormant auth release.

## Cause and evidence

Installed `oidc-provider` is 9.12.0. Its `lib/helpers/client_schema.js` captures
`configuration.scopes` and rejects any client metadata scope absent from that
set with the exact reported error. Resource-server scopes configured through
`getResourceServerInfo` do not satisfy this registration check.

`slice1-lab/auth-server/src/server.mjs` already includes the known CommonCal
catalog alongside `openid` and `offline_access` in `configuration.scopes`.
Commit `dbba0965c7182148f5286912975e7753e3380bd4` introduced the fix and is an
ancestor of the inspected HEAD. Removing only this configuration addition in a
disposable copy reproduced the exact supported-scope validation failure.

The provider's installed authorization scope check enforces the client's
registered scope allowlist. Its Grant implementation supports
`rejectOIDCScope`; the application rejects CommonCal's OIDC interpretation and
adds those scopes only to the configured resource grant. Missing/wrong resource
requests fail, and the independent core grant constrains calendar access.
Installed `check_pkce.js` admits only S256, and the application requires PKCE.
Callback restrictions and issuer/audience validation remain intact.

`Dockerfile.auth` copies the entire auth source directory and runs
`src/production.mjs`, which imports `server.mjs`. The actual locally built Node
22 image passes the fix's integration proof.

The [last recorded live auth rollout](MCP-PRODUCTION-SUCCESSFUL-COMMANDS-2026-10-09.md)
selected `sha-e239a1c511c2991f0caf03198e56dedc1a9e57ef`, an ancestor of the fix.
That record also leaves publishing the registration fix outstanding. This
supports an outdated deployed image as the production cause; it does not prove
the current live image. Public discovery reads returned HTTP 403 from this
environment, and cluster/registry publication could not be verified.

The main promotion workflow builds/scans all three images, publishes immutable
SHA tags, verifies manifests, then updates all three production HelmRelease
tags. However, `deploy/flux/overlays/production/kustomization.yaml` includes only
core and MCP. Auth's HelmRelease is excluded. The cutover candidate is suspended
and stale, and the live-deployment record requires persisting the verified live
configuration before resuming root Flux. Do not activate the candidate unchanged.

## Verified coverage

| Check | Result |
| --- | --- |
| Disposable copy with scope configuration fix removed | Exact original error reproduced |
| `scripts/dev.sh auth-check`, using Node 24 LTS | DCR and production integration passed |
| `scripts/dev.sh auth-storage-check`, actual Node 22 Docker image | 7 unit tests, DCR, production integration, encrypted backup/recovery passed |
| `scripts/dev.sh auth-image-scan` | Trivy HIGH/CRITICAL gate passed, no reported vulnerabilities or secrets |
| `cargo test --manifest-path mcp-server/Cargo.toml --locked --test integration -- --skip phase3_real_auth_server_token_validates` | 34 passed |
| `cargo test --manifest-path backend/Cargo.toml --locked mcp_consent::tests` | 17 consent tests passed |

The DCR proof registers a public client with
`commoncal.calendar.metadata.read`, rejects unknown registration scopes and
unregistered authorization scopes, completes consent and S256 code exchange,
and verifies the resource JWT signature, issuer, audience and scope. Stored
grants prove the CommonCal permission is resource-only and explicitly rejected
as an OIDC permission. Missing/wrong resource, undecided consent and denial are
covered. MCP tests cover calendar filtering including empty calendar consent,
issuer/audience rejection, and discovery issuer mismatch. Backend tests cover
selected calendars, live membership intersection, CSRF/session boundaries,
denial and retries that cannot widen grants.

Initial sandboxed tests failed because loopback networking was prohibited; the
relevant suites passed when loopback access was allowed. The legacy PostgreSQL
real-token helper was excluded from the MCP rerun; the production SQLite token
flow was exercised separately in both host and Docker proofs. No Rust source
changed, so Rust formatting/Clippy checks were not required.

Local proof image manifest-list digest:
`sha256:b478bebc0bb6f0a9199df58776f419c470852591b1d31de984371712022f03f9`.
This local proof digest is not a published GHCR digest or proof of the deployed
linux/amd64 image.

## Exact source and release image

Minimum fixed source:
`dbba0965c7182148f5286912975e7753e3380bd4`.
Corresponding image, if publication is verified:
`ghcr.io/david-hajnal/calendar-auth:sha-dbba0965c7182148f5286912975e7753e3380bd4`.

The source tested in this investigation is
`1ebaab8ee4e5fcb0548267c984c68f0e4fa232f8`; its deployment candidate is
`ghcr.io/david-hajnal/calendar-auth:sha-1ebaab8ee4e5fcb0548267c984c68f0e4fa232f8`.
Verify the successful publication run and registry digest before selecting it.
If a later main revision is published, verify it contains the fix and select its
own immutable SHA tag. The promotion workflow is triggered by pushes to main,
not workflow dispatch. Do not reset main to an older revision to obtain a tag.

## Operator deployment steps (not executed)

1. On a configured operator host, inspect only image references and suspension
   state; do not export Secrets, environment values, cookies, codes or tokens:

   ```sh
   kubectl get deployment commoncal-auth -n commoncal \
     -o jsonpath='{.spec.template.spec.containers[*].image}{"\n"}{.spec.template.spec.initContainers[*].image}{"\n"}'
   kubectl get pods -n commoncal -l app.kubernetes.io/name=commoncal-auth \
     -o jsonpath='{range .items[*]}{.metadata.name}{" "}{.status.containerStatuses[*].imageID}{"\n"}{end}'
   kubectl get helmrelease commoncal-auth -n flux-system \
     -o jsonpath='{.spec.suspend}{" "}{.spec.values.image.repository}{":"}{.spec.values.image.tag}{"\n"}'
   kubectl get kustomization flux-system -n flux-system \
     -o jsonpath='{.spec.suspend}{"\n"}'
   ```

2. Verify CI and the successful `Promote main to production` run for the chosen
   fixed source, including all three immutable registry manifests. If absent,
   publish the reviewed fixed source through the normal main workflow. Keep
   the existing root Flux pause while reconciling live configuration; automatic
   core/MCP promotion must be accounted for before publishing.
3. Preserve the current issuer/resource, callback policy, PVC, signing/cookie/
   bridge keys, TLS, mail and image-pull configuration. Retain rollback image
   references and an encrypted backup. This scope fix needs no new schema or
   credential changes. For an already running auth release, a tag-only update
   preserves its other values. After verifying the exact candidate above exists:

   ```sh
   kubectl patch helmrelease commoncal-auth -n flux-system --type=merge \
     -p '{"spec":{"values":{"image":{"tag":"sha-1ebaab8ee4e5fcb0548267c984c68f0e4fa232f8"}}}}'
   flux reconcile helmrelease commoncal-auth -n flux-system --force --reset --with-source
   kubectl rollout status deployment/commoncal-auth -n commoncal --timeout=10m
   ```

   Confirm the live repository is `ghcr.io/david-hajnal/calendar-auth` first.
   Reconciliation also requires an active auth HelmRelease; review suspension
   state before proceeding. Keep root Flux paused, and check that migration and
   runtime resolve to the chosen image and that `/ready` succeeds.
4. Retry the reported Codex login, complete real browser login and selected
   calendar consent, then perform an authenticated read-only `calendar_list`
   MCP tool call. Record success and calendar isolation without recording any
   tokens, credentials, cookies or authorization codes. Registration alone is
   insufficient to claim a working connection.
5. Persist verified live values and image tags in Git, include auth in the
   production overlay, and preserve the intended active suspension states.
   Validate the manifests and review the resulting desired state against the
   running stack before resuming root Flux. Do not copy the stale cutover
   candidate wholesale. See [the cutover guide](../deploy/flux/overlays/auth-cutover/README.md)
   and [auth operations](AUTH-PRODUCTION.md).

Remaining blockers: cluster access and Flux CLI are unavailable here; current
running image/digest and fixed-image publication remain unverified; public
discovery returned 403. No production modification or authenticated production
MCP tool call was performed, so the connection is not established by this report.
