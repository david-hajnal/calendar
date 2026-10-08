# Deployment Guide

## Overview

Auth, Core, and MCP are deployed to Kubernetes via Flux GitOps. Images are
published to GHCR and promoted by an immutable Git commit referencing the
exact `sha-<40 hex commit>` that CI built and scanned.

The auth server is a Node.js OIDC provider retained for a future authentication
cutover. Its HelmRelease manifest is retained for future use but excluded from
the production Kustomization until its SQLite storage, credentials and live acceptance gates are complete.
Core therefore deploys without an auth dependency or private bridge
configuration, and MCP continues to use its existing issuer.

## Promotion model

Every push to `main` triggers CI. CI builds and scans all three images
(auth, core, MCP). On success, the `promote-main.yml` workflow publishes
`main` and `sha-<commit>` tags for all three images, then atomically commits
the immutable SHA tag to all three HelmReleases. Flux reconciles that commit.

- Pull-request runs cannot publish or promote images.
- Auth, Core, and MCP advance together in one promotion commit.
- Workloads use immutable `sha-<40 hex commit>` tags; `main` is only a
  registry convenience tag.
- The bot-commit guard prevents promotion loops when the promotion commit
  itself triggers a new run.
- The latest-main race guard ensures only the current HEAD of main is promoted.

### Event flow

```
push to main
  → CI (checks + deploy-validation) runs
  → promote-main.yml triggers (only on push to main, not PR)
  → CI builds and scans all three images
  → CI publishes main + sha-<commit> for all three images
  → CI verifies all registry manifests
  → CI commits all three immutable tags to main (bot guard active)
  → Flux Kustomization reconciles the commit
  → active HelmReleases upgrade (core → mcp; auth remains excluded)
  → Kubernetes rolls out new pods
```

## Architecture

```
Browser ──(Cloudflare)──► Ingress ──► core (StatefulSet)
                                           │
                                           └──► mcp (Deployment)

auth (HelmRelease excluded; no installed resources or core bridge wiring)
```

- **auth** — Node.js OIDC provider HelmRelease retained in Git, but excluded
  from production until the dedicated issuer is provisioned and verified.
- **core** — Rust StatefulSet. The application backend; currently has no auth
  HelmRelease dependency or auth bridge configuration. Also serves the
  CalDAV account surface (`/dav/`) used by Apple Calendar on macOS and iOS;
  see [Connect an Apple Calendar client](#connect-an-apple-calendar-client).
- **mcp** — Rust Deployment. The MCP server. Depends on core and continues to
  use the existing OAuth issuer (the auth issuer has not been cut over).

## Namespace

Core and MCP deploy to the `commoncal` namespace. Auth resources are absent
while the auth HelmRelease is excluded from production.

Flux is the normal production deployment authority. Active HelmReleases use
the explicit release names `commoncal` and `commoncal-mcp`; this avoids Flux's
cross-namespace `commoncal-commoncal` default.

Reconcile Flux directly while auth is excluded; `deploy/deploy-prod.sh` still
supports the legacy three-release and direct-Helm workflows. Flux deploys the
image tags and chart values committed to its Git source, so local `IMAGE_TAG`
and direct chart overrides do not affect Flux reconciliation.

### TLS model (two-hop)

Production TLS uses a two-hop model:

1. **Browser → Cloudflare edge:** Cloudflare Universal SSL provides a
   publicly-trusted certificate for `*.hajnal.space`. Cloudflare must proxy
   both DNS records (`cal.hajnal.space` and `mcal.hajnal.space`) and use
   SSL/TLS mode **Full** (not `Flexible` or `Full (strict)`).
2. **Cloudflare → origin (Traefik):** the origin presents a self-signed
   certificate stored in the `commoncal-tls` Kubernetes Secret. Cloudflare in
   `Full` mode does not validate the origin certificate — it only requires
   that the origin speaks TLS.

The deploy script manages the origin Secret:

- **First run:** if `commoncal-tls` is absent or invalid, the script generates
  a self-signed RSA 2048-bit / SHA-256 certificate (365 days) covering both
  `DOMAIN` and `MCP_DOMAIN`, and creates the Secret in the `commoncal`
  namespace. The private key is never logged.
- **Subsequent runs:** if a valid Secret already exists (correct type,
  matching key, both SANs present, more than 30 days to expiry), the script
  reuses it and does not regenerate.
- **30-day expiry guard:** if the existing certificate expires within 30 days,
  the script regenerates it.
- **Manual rotation:** delete the Secret (`kubectl delete secret commoncal-tls
  -n commoncal`) and re-run the deploy script to force regeneration.

No cert-manager, Let's Encrypt, or ACME dependency is required.

The MCP NetworkPolicy allows egress HTTPS to non-private IPv4 addresses only.
On a dual-stack cluster, make sure the OAuth issuer and the core domain resolve
to IPv4 for MCP egress.

## Connect an MCP client

After the MCP HelmRelease has reconciled and `/health/ready` succeeds, connect
clients to the public streamable-HTTP endpoint:

```
https://mcal.hajnal.space/mcp
```

Use a client that supports OAuth-protected MCP servers and streamable HTTP.
Give it the endpoint above; it must discover resource metadata at:

```
https://mcal.hajnal.space/.well-known/oauth-protected-resource
```

The client should then start its OAuth authorization flow with the issuer
advertised by that metadata. Do not copy an access token into a configuration
file or share it between users. The client sends its user-specific bearer token
with each `POST /mcp` request; the server resolves that user's grant from
CommonCal core and only exposes permitted calendars and tools.

For a manual smoke test, use an MCP Inspector or another OAuth-capable MCP
client, complete sign-in in its browser window, and run `tools/list`. Then use
`calendar_list` to confirm the client sees only calendars covered by its MCP
grant. If discovery or login fails, first verify:

```bash
curl -fsS https://mcal.hajnal.space/.well-known/oauth-protected-resource
curl -fsSI https://mcal.hajnal.space/health/ready
```

`/health/ready` returning `200` confirms MCP SQLite storage is usable; it does
not validate a specific user's OAuth grant. Grant failures must be diagnosed in
the MCP client or CommonCal core, without logging or pasting bearer tokens.

For an emergency direct deployment, first suspend all three Flux HelmReleases,
then run the same script. With all releases suspended (or absent), it deploys
all three workloads directly with Helm and requires `IMAGE_TAG` set to an
immutable `sha-<40 hex commit>` tag. It ensures the `commoncal-tls` TLS
Secret exists and is valid (generating a self-signed certificate on first run
if needed), then deploys all Ingresses referencing that Secret. Resume Flux
only after reconciling the direct deployment back into Git. A mixed state with
any active HelmRelease is rejected to prevent split ownership.

## Connect an Apple Calendar client

Core exposes a real two-way CalDAV account surface on the same origin as the
web app. Apple Calendar on macOS and iOS connects to it as a standard CalDAV
account; Happening is the source of truth and never receives Apple
Account/iCloud credentials.

The DAV root is:

```
https://cal.hajnal.space/dav/
```

Discovery accepts GET, HEAD and PROPFIND at `/.well-known/caldav` and returns
307 with `Cache-Control: no-cache` to the canonical `/dav/` root.
[RFC 6764 §5](https://www.rfc-editor.org/rfc/rfc6764#section-5) permits 307;
[RFC 9110 §15.4.8](https://www.rfc-editor.org/rfc/rfc9110#section-15.4.8)
preserves the method and body when following it. Native Apple behavior still
requires a client check. No fallback aliases are added for `/` or `/principals/`.
At the DAV root, `OPTIONS` advertises the `1, 2, access-control, calendar-access`
capabilities, and the principal/calendar-home `PROPFIND` responses expose the
user's active calendars with their names, colors, and role privileges.

### Issue a connection password

Connection passwords are revocable, per-device secrets. They are shown once
at creation and stored only as a domain-separated keyed HMAC; the plaintext is never persisted
or logged.

From the web app, open **Settings → Calendar connections** and create a
password for the device. The API equivalent is:

```bash
# Issue a new connection password (returns the plaintext once).
curl -fsS -X POST https://cal.hajnal.space/api/v1/calendar-connections/apple/passwords \
  -H 'Content-Type: application/json' \
  -H 'Cookie: <session-cookie>' \
  -d '{"label":"My iPhone"}'
```

The response carries the `username` (the account email), the one-time
`password`, and the `server_url` to enter in the device.

### Add the account on the device

- **macOS:** System Settings → Internet Accounts → Add Account → Other CalDAV
  Account. Server `cal.hajnal.space`, username the account email, password the
  one-time secret.
- **iOS:** Settings → Mail → Accounts → Add Account → Other → Other CalDAV
  Account. Same server, username, and password.

The device then performs discovery, lists the calendars, and syncs events.
Initial sync is a full `sync-collection` snapshot; subsequent syncs are
incremental and carry an opaque sync token. Supported CRUD (create, update,
delete) and all-day and recurring events round-trip through the domain
services, so changes made in Apple Calendar appear in Happening and vice
versa.

### Revoke a connection

Revoking a password immediately invalidates it; the next DAV request from that
device fails with `401`. Revoking one device's password does not affect the
others.

```bash
# Revoke a single connection password.
curl -fsS -X DELETE \
  https://cal.hajnal.space/api/v1/calendar-connections/apple/passwords/<id> \
  -H 'Cookie: <session-cookie>'

# Disconnect all devices (revokes every active password).
curl -fsS -X DELETE https://cal.hajnal.space/api/v1/calendar-connections/apple \
  -H 'Cookie: <session-cookie>'
```

To verify a revocation took effect, a DAV request with the old credentials
must return `401`:

```bash
curl -fsSI -u '<email>:<old-password>' https://cal.hajnal.space/dav/
# expect: HTTP/2 401
```

### Deployment notes

- The CalDAV surface shares the core origin and the two-hop TLS model
  described above; no separate certificate or ingress is required.
- DAV authentication is rate-limited independently of the browser/API routes,
  and request bodies and REPORT payloads are bounded. Logs are redacted and
  never contain credentials or event bodies.
- Public ICS/WebCal export is a one-off convenience and is **not** the
  account-level CalDAV synchronization described here.

## Images

- Auth: `ghcr.io/david-hajnal/calendar-auth`
- Core: `ghcr.io/david-hajnal/calendar-core`
- MCP: `ghcr.io/david-hajnal/calendar-mcp`

Tags are `main` (convenience) and `sha-<40 hex commit>` (immutable, production).
Version-based tags (`vX.Y.Z`) are retired; they no longer build images or
promote production.

## Pin to a Known-Good SHA

To pin to a known-good version:

1. Find the promotion commit for the desired SHA:
   ```bash
   git log --grep="chore(deploy): promote" --oneline
   ```

2. Edit all three HelmRelease tags to the known-good immutable SHA:
   ```yaml
   # deploy/flux/overlays/production/charts/auth-helmrelease.yaml
   image:
     tag: "sha-abc123def456..."

   # deploy/flux/overlays/production/charts/core-helmrelease.yaml
   image:
     tag: "sha-abc123def456..."

   # deploy/flux/overlays/production/charts/mcp-helmrelease.yaml
   image:
     tag: "sha-abc123def456..."
   ```

3. Commit and push to `main`.

4. Flux will reconcile within 10 minutes.

## Reconcile Resources

```bash
# Reconcile all Flux resources
flux reconcile kustomization flux-system --namespace=flux-system

# Reconcile specific HelmRelease
flux reconcile helmrelease commoncal --namespace=flux-system
flux reconcile helmrelease commoncal-mcp --namespace=flux-system
```

## Revert Promotion Commit

To revert an image promotion:

1. Find the promotion commit:
   ```bash
   git log --grep="chore(deploy): promote" --oneline
   ```

2. Revert the commit:
   ```bash
   git revert <commit-hash>
   git push
   ```

3. Flux will reconcile and roll back to the previous version.

## GHCR Credentials

Images are published to `ghcr.io/david-hajnal/`. `GHCR_TOKEN` in `deploy/.env`
only applies to direct Helm deployments; under Flux ownership the script rejects
it. If the packages are private, create a Kubernetes pull Secret:

```bash
kubectl create secret docker-registry ghcr-credentials \
  --namespace=commoncal \
  --docker-server=ghcr.io \
  --docker-username=<username> \
  --docker-password=<token> \
  --docker-email=<email>
```

Then add to each HelmRelease's `imagePullSecrets`.

## Production Secrets

- `commoncal-auth-secrets` — `AUTH_BRIDGE_KEY`, comma-separated distinct
  `AUTH_COOKIE_KEYS`, `AUTH_SIGNING_KID`, and private `AUTH_JWKS`.
- `commoncal-auth-backup` — public age recipient under `AGE_RECIPIENT`;
  keep the private age identity recoverable outside the cluster.
- `commoncal-auth-tls` — public TLS for `auth.hajnal.space`.
- `commoncal-session` — session encryption (key: `SESSION_SECRET`) and backup encryption (key: `BACKUP_ENCRYPTION_KEY_HEX`)
- `commoncal-mcp-secrets` — the shared internal API key, MCP session secret, and HTTPS OAuth issuer (`mcp-oauth-issuer`)
- `commoncal-tls` — self-signed TLS certificate for the origin hop (covers both `cal.hajnal.space` and `mcal.hajnal.space`)

The shared certificate covers both the core and MCP domains. Both Ingresses
reference the same Secret. Browsers receive Cloudflare Universal SSL; the
self-signed certificate is only presented on the Cloudflare-to-origin hop.

`BACKUP_ENCRYPTION_KEY_HEX` must be an even number of hexadecimal characters (at least 32); 64-hex (32-byte) keys remain backward-compatible.

## Auth SQLite storage and migration

Production auth uses `/app/data/auth.sqlite` on the retained `commoncal-auth-data`
PVC. It has exactly one replica and Recreate rollouts. The initContainer runs
`src/migrate.mjs` on that same volume before auth starts; production startup only
checks readiness and never auto-migrates. Inspect it with:

```bash
kubectl logs -n commoncal deployment/commoncal-auth -c migrate
kubectl rollout status deployment/commoncal-auth -n commoncal
```

There is no production PostgreSQL dependency or database password. The daily
`commoncal-auth-backup` CronJob takes a consistent online SQLite snapshot, checks
integrity, encrypts it with age and retains 14 days of encrypted archives on the
same PVC. Export backups to a separate failure domain. Never copy only a live
SQLite main file while WAL is active.

Follow [the SQLite auth setup and recovery guide](AUTH-PRODUCTION.md). An existing
PostgreSQL issuer requires a stopped-issuer import before activating SQLite;
keep the original database, PVCs and signing/cookie keys available for rollback.

## Issuer Cutover

The MCP OAuth issuer is held at `mcp-oauth-issuer` (the MCP server's own
issuer) until the auth server is fully operational. The cutover to the auth
server's issuer is an explicit, manual step:

1. **Verify the auth server is healthy:**
   ```bash
   curl -s https://auth.hajnal.space/.well-known/openid-configuration
   ```

2. **Update the MCP issuer:**
   ```bash
   kubectl -n commoncal patch secret commoncal-mcp-secrets \
     --type merge -p '{"data":{"OAUTH_ISSUER":"https://auth.hajnal.space"}}'
   kubectl -n commoncal rollout restart deployment commoncal-mcp
   ```

3. **Verify MCP OAuth flow:**
   ```bash
   curl -sI https://mcal.hajnal.space/mcp | head -5
   ```

> **Warning:** The issuer cutover is irreversible without a rollback. If the
> auth server is not healthy, do not cutover. The MCP server will reject
> tokens from the new issuer if the JWKS endpoint is unreachable.

## Rollback

To roll back a release:

1. **Revert the promotion commit:**
   ```bash
   git log --grep="chore(deploy): promote" --oneline
   git revert <commit-hash>
   git push
   ```

To roll back the auth server specifically:

1. Revert the auth HelmRelease tag to a version compatible with the existing
   SQLite schema. Preserve the auth PVC and signing/cookie keys.
2. The migration initContainer is idempotent when the schema is already current.
3. If a schema rollback needs recovery, stop auth and backup jobs first. Restore
   a verified decrypted SQLite snapshot to the retained PVC while no process is
   using it, with UID/GID 65534 and mode 0600. Restart the compatible image and
   verify readiness and OAuth continuation. Follow [the auth recovery guide](AUTH-PRODUCTION.md);
   never replace a running SQLite file or restore a PostgreSQL dump into it.

## TLS Cutover Checklist

Execute in this exact order. Each step must succeed before proceeding to the
next.

### Pre-cutover

1. **Confirm proxied DNS.** Both `cal.hajnal.space` and `mcal.hajnal.space`
   must be proxied (orange cloud) in Cloudflare:
   ```bash
   dig +short A cal.hajnal.space    # expect Cloudflare anycast IPs
   dig +short A mcal.hajnal.space   # expect Cloudflare anycast IPs
   ```
2. **Set Cloudflare SSL/TLS mode to `Full`.** In the Cloudflare dashboard
   (SSL/TLS → Edge to Origin), select **Full**.
   - Do **not** use `Full (strict)` — it validates the origin certificate and
     will reject the self-signed cert with error **526**.
   - Do **not** use `Flexible` — it allows plain-HTTP origin traffic, which
     breaks HSTS and the HTTPS-only OAuth flow required by both applications.
3. **Back up the existing TLS Secret** (if one exists):
   ```bash
   kubectl get secret commoncal-tls -n commoncal -o yaml \
     > commoncal-tls-backup-$(date +%Y%m%d).yaml
   ```

### Cutover

4. **Deploy.** Run the deploy script (Flux or direct mode):
   ```bash
   bash deploy/deploy-prod.sh
   ```
   On first run this generates the self-signed certificate and creates the
   `commoncal-tls` Secret. On subsequent runs it reuses the existing Secret.

5. **Verify both edge endpoints** (browser-facing, Cloudflare Universal SSL):
   ```bash
   curl -sI https://cal.hajnal.space | head -5
   curl -sI https://mcal.hajnal.space/mcp | head -5
   ```
   Both must return `200` or `301`/`302` with a valid TLS handshake.

6. **Verify direct-origin SNI** (Cloudflare-to-origin hop, self-signed):
   ```bash
   ORIGIN_IP=<k3s-node-public-ip>
   openssl s_client -connect "$ORIGIN_IP":443 -servername cal.hajnal.space </dev/null 2>/dev/null \
     | openssl x509 -noout -subject -issuer -dates -ext subjectAltName
   openssl s_client -connect "$ORIGIN_IP":443 -servername mcal.hajnal.space </dev/null 2>/dev/null \
     | openssl x509 -noout -subject -issuer -dates -ext subjectAltName
   ```
   Both must present a self-signed certificate containing both SANs
   (`cal.hajnal.space` and `mcal.hajnal.space`).

7. **Inspect logs.** Check for TLS errors in the ingress controller and
   application logs:
   ```bash
   kubectl logs -n kube-system -l app.kubernetes.io/name=traefik --tail=50
   kubectl logs -n commoncal -l app.kubernetes.io/name=commoncal --tail=50
   kubectl logs -n commoncal -l app.kubernetes.io/name=commoncal-mcp --tail=50
   ```
   No TLS handshake errors, no 526/521/525 Cloudflare error codes.

### Post-cutover (optional)

8. **Delete cluster cert-manager resources (optional).** Only if cert-manager
   is no longer needed by any other workload:
   ```bash
   # First: inventory all Certificate and ClusterIssuer resources
   kubectl get certificates -A
   kubectl get clusterissuers
   ```
   If no other Certificate resources exist, you may uninstall cert-manager:
   ```bash
   helm uninstall cert-manager -n cert-manager
   kubectl delete namespace cert-manager
   kubectl delete crd certificates.cert-manager.io
   kubectl delete crd challengers.cert-manager.io
   kubectl delete crd orders.cert-manager.io
   kubectl delete crd issuers.cert-manager.io
   kubectl delete crd clusterissuers.cert-manager.io
   ```
   **Do not delete cert-manager if any other Certificate resource depends on
   it.**

### Rollback

If the cutover must be reverted:

1. Restore the previous trusted TLS Secret:
   ```bash
   kubectl apply -f commoncal-tls-backup-<date>.yaml
   ```
2. Switch Cloudflare SSL/TLS mode back to the previous mode (typically
   **Full (strict)** if a trusted cert was in place before):
   - Cloudflare dashboard → SSL/TLS → Edge to Origin → select previous mode.
3. Verify both domains:
   ```bash
   curl -sI https://cal.hajnal.space
   curl -sI https://mcal.hajnal.space
   ```
4. If the previous mode was `Full (strict)`, the restored trusted certificate
   must be valid for both domains. If it was `Full`, the self-signed cert is
   acceptable.

> **Warning:** Switching Cloudflare to `Full (strict)` while the origin still
> presents the self-signed certificate causes Cloudflare error **526**
> (Invalid SSL Certificate). Always restore a trusted origin certificate
> before enabling `Full (strict)`.

## TLS Verification

Verify the Cloudflare edge certificate (browser-facing):

```bash
# Edge certificate (should be a Cloudflare/Google trusted cert)
openssl s_client -connect cal.hajnal.space:443 -servername cal.hajnal.space </dev/null 2>/dev/null \
  | openssl x509 -noout -issuer -subject -dates

openssl s_client -connect mcal.hajnal.space:443 -servername mcal.hajnal.space </dev/null 2>/dev/null \
  | openssl x509 -noout -issuer -subject -dates
```

Verify the origin certificate (Cloudflare-to-origin hop, self-signed):

```bash
# Replace <ORIGIN_IP> with the k3s node's public IP
openssl s_client -connect <ORIGIN_IP>:443 -servername cal.hajnal.space </dev/null 2>/dev/null \
  | openssl x509 -noout -subject -issuer -dates -ext subjectAltName

openssl s_client -connect <ORIGIN_IP>:443 -servername mcal.hajnal.space </dev/null 2>/dev/null \
  | openssl x509 -noout -subject -issuer -dates -ext subjectAltName
```

Inspect the Kubernetes Secret:

```bash
kubectl get secret commoncal-tls -n commoncal -o jsonpath='{.type}'
# Expected: kubernetes.io/tls

kubectl get secret commoncal-tls -n commoncal \
  -o jsonpath='{.data.tls\.crt}' | base64 -d \
  | openssl x509 -noout -subject -issuer -dates -ext subjectAltName
```

## TLS Rollback

If the self-signed origin certificate must be reverted to a previously-trusted
certificate:

1. Restore the previous trusted Secret (backed up before cutover):
   ```bash
   kubectl apply -f commoncal-tls-trusted-backup.yaml
   ```
2. Switch Cloudflare SSL/TLS mode back to **Full (strict)** in the Cloudflare
   dashboard (SSL/TLS → Edge to Origin).
3. Verify both domains:
   ```bash
   curl -sI https://cal.hajnal.space
   curl -sI https://mcal.hajnal.space
   ```

> **Warning:** Switching Cloudflare to `Full (strict)` while the origin still
> presents the self-signed certificate causes Cloudflare error **526**
> (Invalid SSL Certificate). Always restore a trusted origin certificate
> before enabling `Full (strict)`.

## Monitoring

```bash
# Check HelmRelease status
flux get helmreleases --namespace=flux-system

# Check workloads
kubectl get statefulset -n commoncal
kubectl get deployment -n commoncal
```

## Validation

Run local validation before pushing:

```bash
bash scripts/validate-deploy.sh
```

This checks:
- Helm lint (auth, core, mcp)
- Helm template rendering (auth, core, mcp)
- Kustomize build of the production overlay
- No mutable tags (`latest`/`main`) in production; every HelmRelease uses an
  immutable `sha-<40 hex commit>` tag
- Rendered Flux resources conform to the installed CRD schemas
- No retired version-release or semver promotion references remain
- YAML syntax
- Chart template assertions (auth, core, mcp)
- Bridge isolation (no private ingress, no secret values)
- Flux dependency topology (core → mcp, with auth excluded and no core
  auth dependency or bridge configuration)
- Issuer consistency (no cutover)

## Ad-hoc SQLite Console

Open a read-only SQLite console on the production database:

```bash
sudo ./deploy/sqlite-prod.sh
```

Open a writable console (requires typed confirmation):

```bash
sudo ./deploy/sqlite-prod.sh --write
```

### Requirements

- Run from an SSH session on the production server
- `kubectl` installed and configured with local k3s kubeconfig (`/etc/rancher/k3s/k3s.yaml`)
- Root access (`sudo`)
- Interactive terminal (tty)

### Safety

- Read-only is the default
- Write mode requires typing the pod name exactly to confirm
- The console pod is automatically cleaned up on exit, Ctrl-C, or after 1 hour
- Only one console session is allowed at a time
- The pod has no network connectivity (deny-all NetworkPolicy)
- The pod runs as non-root UID 1000 with a read-only root filesystem
- The live database PVC is mounted directly; no database files are copied

### Troubleshooting

- **Pod fails to start**: Check PVC attachment — `kubectl describe pvc <pvc-name> -n commoncal`
- **Another session active**: `kubectl delete pod commoncal-sqlite-console -n commoncal`
- **NetworkPolicy missing**: Deploy the Helm chart or create the policy manually
- **Database not found**: The database file must exist on the core pod before opening a console
- **`attempt to write a readonly database (8)`**: The console pod is a *separate* pod
  mounting the same PVC. SQLite in WAL mode needs to write the `-wal`/`-shm`
  files, whose ownership (UID 1000, held by the live core pod) the console pod
  cannot match — so writes fail even in `--write` mode. For one-off writes,
  exec directly into the core pod instead (see below).

## MCP Database Backup and Restore

The MCP server keeps its local state in one SQLite file:

- **Path in the pod**: `/app/data/mcp-server.db` (WAL mode, so `-wal`/`-shm`
  sidecar files may exist alongside it)
- **PVC**: `commoncal-mcp-data` in the `commoncal` namespace
  (`ReadWriteOnce`, retained on Helm uninstall)
- **Deployment**: `commoncal-mcp`, **exactly one replica** — SQLite allows a
  single writer, and the Helm chart schema rejects any other replica count.
  Never run two MCP pods against the same PVC.

The production image ships the `sqlite3` CLI for the procedures below.

### Backup (online, service stays up)

`VACUUM INTO` produces a consistent snapshot of a live WAL database:

```bash
POD=$(kubectl get pods -n commoncal -l app.kubernetes.io/name=commoncal-mcp \
  -o jsonpath='{.items[0].metadata.name}')

# Consistent snapshot while the service keeps serving
kubectl exec -n commoncal "$POD" -- \
  sqlite3 /app/data/mcp-server.db "VACUUM INTO '/app/tmp/mcp-server-backup.db';"

# Pull the snapshot out of the pod
kubectl cp "commoncal/$POD:/app/tmp/mcp-server-backup.db" ./mcp-server-backup.db

# Verify the snapshot before trusting it
sqlite3 ./mcp-server-backup.db "PRAGMA integrity_check;"   # must print ok
```

### Restore (service must be stopped)

The single writer must be down while the database file is replaced:

```bash
# 1. Stop the single writer (the PVC is retained)
kubectl scale deployment commoncal-mcp -n commoncal --replicas=0

# 2. Start a temporary pod on the retained PVC
kubectl run mcp-restore -n commoncal --rm -it --restart=Never \
  --image=busybox:1.36 -- sleep infinity \
  --overrides='{"spec":{"containers":[{"name":"mcp-restore","volumeMounts":[{"name":"data","mountPath":"/app/data"}]}],"volumes":[{"name":"data","persistentVolumeClaim":{"claimName":"commoncal-mcp-data"}}]}}'

# 3. Replace the database file, clear WAL sidecars, and fix ownership
#    (the MCP pod runs as UID 1000)
kubectl cp ./mcp-server-backup.db commoncal/mcp-restore:/app/data/mcp-server.db
kubectl exec -n commoncal mcp-restore -- sh -c \
  'chown 1000:1000 /app/data/mcp-server.db && rm -f /app/data/mcp-server.db-wal /app/data/mcp-server.db-shm'

# 4. Remove the temporary pod and start the service
kubectl delete pod mcp-restore -n commoncal
kubectl scale deployment commoncal-mcp -n commoncal --replicas=1
```

### Verify after restore

```bash
POD=$(kubectl get pods -n commoncal -l app.kubernetes.io/name=commoncal-mcp \
  -o jsonpath='{.items[0].metadata.name}')

# Database integrity — must print ok
kubectl exec -n commoncal "$POD" -- \
  sqlite3 /app/data/mcp-server.db "PRAGMA integrity_check;"

# Service readiness — /health/ready confirms MCP SQLite storage is usable
kubectl get pods -n commoncal -l app.kubernetes.io/name=commoncal-mcp
curl -sI https://mcal.hajnal.space/mcp | head -5
```

## Change the admin password

There is no `set-password` CLI command or HTTP endpoint. The password is a
bcrypt hash (cost 12) stored in `users.password_hash`, matched by
`normalized_email`. The default admin row is `admin@localhost`.

> **Do not use `deploy/sqlite-prod.sh --write` for this.** The console pod
> hits `attempt to write a readonly database (8)` (WAL/SHM ownership, see
> Troubleshooting above). Run the write inside the **core pod**, which already
> holds the database open and has write access.

### 1. Generate a bcrypt hash (cost 12)

```bash
# Option A: python3 + bcrypt
HASH=$(python3 -c 'import bcrypt; print(bcrypt.hashpw(b"NEW_PASSWORD", bcrypt.gensalt(12)).decode())')

# Option B: htpasswd (strip the "user:" prefix)
# HASH=$(htpasswd -BbnC 12 admin NEW_PASSWORD | cut -d: -f2)

echo "$HASH"   # sanity check: 60 chars, starts with $2b$12$ / $2y$12$
```

### 2. Write it into the core pod's database

```bash
kubectl exec -n commoncal commoncal-0 -- \
  sqlite3 /app/data/commoncal.sqlite \
  "PRAGMA busy_timeout=5000; UPDATE users SET password_hash='$HASH' WHERE normalized_email='admin@localhost'; SELECT changes();"
```

`SELECT changes();` must print `1`. If it prints `0`, the email did not match —
check the row with `SELECT normalized_email FROM users;`.

> **Quoting:** the hash is passed as the value of the shell variable `$HASH`,
> so its `$` characters are **not** re-expanded by the shell (variable
> expansion is not recursive). If you inline a literal hash inside the
> double-quoted SQL string instead, you must escape every `$` as `\$` or the
> shell will mangle it.

### 3. Enable password login and restart the pod

Password login is **off by default in production** (`password_login_enabled`
defaults to `false`); the `POST /api/v1/auth/password-login` route is only
registered when it is `true`. Set the env var and restart so the pod picks it
up:

```bash
# Quick one-liner (Flux will revert unless you also update Helm values):
k -n commoncal set env statefulset/commoncal PASSWORD_LOGIN_ENABLED=true
k -n commoncal rollout restart statefulset commoncal

# Or via ConfigMap:
kubectl -n commoncal patch configmap commoncal \
  --type merge -p '{"data":{"PASSWORD_LOGIN_ENABLED":"1"}}'}
kubectl -n commoncal rollout restart statefulset commoncal
```

> **Flux reverts bare `kubectl` edits.** The durable fix is to add
> `PASSWORD_LOGIN_ENABLED: "1"` to the Helm values source
> (`deploy/helm/commoncal/templates/configmap.yaml` /
> `deploy/values-production.yaml`) and let Flux reconcile it, or the patch
> above will be rolled back on the next reconciliation.

### 4. Verify

```bash
curl -s -X POST https://cal.hajnal.space/api/v1/auth/password-login \
  -H 'Content-Type: application/json' \
  -d '{"email":"admin@localhost","password":"NEW_PASSWORD"}'
```

A `405 Method Not Allowed` means the route is not registered — the env var did
not reach the running pod (Flux reverted the ConfigMap, or the pod has not
restarted). Confirm with
`kubectl -n commoncal get configmap commoncal -o yaml | grep PASSWORD_LOGIN_ENABLED`
and `kubectl -n commoncal get pods`.


## CalDAV compatibility repair handoff

Code-level verification is separate from native Apple account setup. No
successful macOS setup or production rollout is established by the local tests.

The repair covers these defects:

- Missing/unsupported properties and propname now use namespace-preserving XML
  elements; empty status groups are omitted and explicit selections are honored.
- XML parsing restores namespace scope and validates document/report grammar;
  object PROPFIND, authenticated principal metadata, Depth rules and report
  property selection share the corrected property handling.
- Calendar-query applies component/range/recurrence semantics and reports result
  overflow. Unsupported filters or calendar-data projections fail explicitly.
- Sync tokens are direct multistatus children, collection/user/visibility-bound
  opaque URIs. Snapshots retain their high-water mark, pages continue explicitly,
  changes coalesce by resource, and durable tombstones survive event deletion.
  Concurrent writes can require retry (503); old tokens require full resync
  (403 `DAV:valid-sync-token`). Migration `0028_caldav_visibility_epochs.sql`
  persists ACL epochs so revoke/regrant cannot revive a previous token.
- HTTP status/capability/privilege claims, write preconditions and validators,
  and iCalendar escaping, folding and timezone representation are corrected.
  Parsing respects quoted parameter delimiters and escaped commas in categories;
  URL properties retain URI values. Imported metadata is persisted and existing
  imports are backfilled on a successful HTTP 200 feed refresh.

### Post-deployment checks

After the normal release applies migration 0028, run the read-only semantic
smoke from the repository root. Enter a newly generated device password at the
hidden prompt; do not paste credentials into command arguments or enable shell
tracing. The script follows advertised same-origin principal/home/collection
URLs, validates XML property statuses and checks initial-sync continuations.

```sh
export CALDAV_ORIGIN=https://cal.hajnal.space
read -r -p 'Happening email: ' CALDAV_USERNAME
export CALDAV_USERNAME
read -r -s -p 'Connection password: ' CALDAV_PASSWORD
printf '\n'
export CALDAV_PASSWORD
python3 scripts/caldav-smoke.py
unset CALDAV_USERNAME CALDAV_PASSWORD
```

The prompts above use bash (`bash` first if your interactive shell is zsh).
Success prints a calendar count, never credentials. A nonzero exit means the
smoke failed; inspect server-side status logs without printing request headers.
This smoke does not test write operations or prove native Apple compatibility.

Then add a new Other CalDAV Account on macOS using the server, Happening email
and a fresh connection password. Confirm account acceptance, expected writable,
read-only and free/busy calendars, first sync and subsequent refresh. On a test
calendar, create/read/update/delete an event in each app and verify the other
app receives every change; include an all-day event and recurrence exception.
Check imported-event restrictions and revoke a test connection to confirm 401.
Record macOS version, setup time and result; do not record the password.

If account setup still fails, capture the corresponding reverse-proxy access
records with only timestamp, request **method**, URL **path** (without query),
response **status**, and **User-Agent**. Keep Authorization, Cookie, passwords,
request/response bodies and token-bearing query strings out of logs and shared
traces. Correlate the setup time and User-Agent with discovery, PROPFIND and
REPORT requests; inspect individual propstat statuses for 207 responses. This
is a capture procedure, not a claim that safe logging is already configured.

Residual checks: native Apple request sequences, TLS/public-host redirects and
production middleware remain deployment checks; concurrent workloads may need
503 retries and token-format changes cause a one-time full resync. The bundled
chrono-tz transition data ends in 2099; recurring schedules beyond that horizon
need refreshed timezone data before relying on future DST behavior. Local test
results belong in the accompanying change report, including unavailable checks.

Primary protocol references: [RFC 4918 (WebDAV)](https://www.rfc-editor.org/rfc/rfc4918),
[RFC 3744 (privileges/principals)](https://www.rfc-editor.org/rfc/rfc3744),
[RFC 4791 (CalDAV)](https://www.rfc-editor.org/rfc/rfc4791),
[RFC 5397 (current-user-principal)](https://www.rfc-editor.org/rfc/rfc5397),
[RFC 6764 (discovery)](https://www.rfc-editor.org/rfc/rfc6764),
[RFC 6578 (sync)](https://www.rfc-editor.org/rfc/rfc6578),
[RFC 5545 (iCalendar)](https://www.rfc-editor.org/rfc/rfc5545), and
[RFC 9110 (HTTP, including PUT validators §9.3.4)](https://www.rfc-editor.org/rfc/rfc9110#section-9.3.4).


### Local verification

- `cargo test --manifest-path backend/Cargo.toml`: 571 passed across 43 suites,
  including 104 CalDAV integration tests and 12 Apple fixture tests.
- `cargo clippy --manifest-path backend/Cargo.toml --all-targets -- -D warnings`:
  passed.
- `cargo fmt --manifest-path backend/Cargo.toml -- --check` and `git diff --check`:
  passed.
- `pnpm --dir e2e exec playwright test tests/caldav-account.spec.ts`: two projects
  passed (desktop Firefox and mobile WebKit).
- `scripts/caldav-smoke.py`: passed against a local server using API-issued
  device credentials; production and native macOS verification remain pending.

QA's last completed verdict was PASS WITH CONCERNS before the final metadata
regressions. Its final summary hit an agent usage limit; the orchestrator
verified the completed final 104-test QA log and full suite directly.

### Suggested change description

Commit subject and PR title: `fix(caldav): repair discovery and synchronization compatibility`

Repair namespace-aware DAV properties, discovery, report parsing, recurrence
queries and collection-bound paginated sync. Correct HTTP/iCalendar behavior,
preserve imported metadata, and add ACL epoch migration, regressions and a
credential-safe smoke script. Native macOS setup remains a post-deployment
check. Validation: 571 backend tests, strict Clippy, formatting, two browser
E2E projects and local semantic smoke passed.

Domain reviewer from repository history: david-hajnal <david@hajnal.space>;
no verified GitHub handle.


## Discovery and key persistence rollout

The discovery defect was confirmed: the well-known route accepted GET only,
so Apple's PROPFIND received 405. Router/middleware regression tests now follow
307 with the original DAV request body, Depth and same-origin authentication,
then validate the root, current-user-principal, calendar home and collections.
The semantic smoke checks both GET and PROPFIND discovery, redirect targets and
XML properties. Passing these checks does not establish native Apple sync.

A separate confirmed persistence defect was found in `SecretKey::derive`:
PBKDF2 used a new random salt for every process and discarded it. Consequently,
identical `SESSION_SECRET` values produced different keys on restart. The fix
uses the stable domain salt `commoncal/session-key/v1`; connection-password
HMAC hashing remains unchanged. Tests prove a credential survives a new service
instance with the same database and secret, fails under deliberate key change,
and still works under its original key afterward. Failed authentication does
not itself revoke or rewrite it.

### First rollout: required recovery

Back up the database and configuration before promoting this change through
the normal release/Flux process. Do not deploy automatically. This first
rollout changes the effective derived key: the old discarded salts cannot be
recovered from `SESSION_SECRET`, the database or ordinary backups alone.
Existing pre-fix connection passwords are expected to fail once. Reissue device
passwords and sessions, recreate outstanding invitation/login/public-share
links, perform a full CalDAV resync and reconfigure external feed URLs encrypted
with the old key. Account passwords (bcrypt), account data and events are not
rehashed or deleted. There is no automatic database migration or credential
revocation for this repair. Rolling back the old binary creates another random
key and does not recover the previous key.

After this transition, normal restarts and deployments preserve credentials
when the database and `SESSION_SECRET` stay the same. Production startup rejects
missing or empty `SESSION_SECRET`; random fallback is limited to development.

### Secret ownership and deployment guard

The core Helm deployment sets `APP_ENV=production` and injects `SESSION_SECRET`
from `commoncal-session` through `secretKeyRef`. The current configuration uses
one core replica and SQLite at `/app/data/commoncal.sqlite` on a PVC. Flux owns
the chart and ConfigMap; no Git-managed core Secret was found in this checkout.

Both `deploy/deploy-prod.sh` and `deploy/bootstrap-production.sh` now preserve
an existing `commoncal-session` object in full, including its backup key. A
matching supplied session secret permits deployment; mismatch, missing/empty
key in an existing object, or read failure stops it. A missing Secret is created.
`deploy-prod.sh` sources `deploy/.env`, which overrides exported variables;
bootstrap gives an already-exported value precedence. Ensure the effective
value matches the stored key rather than regenerating it during releases.

Intentional rotation is a separate operator action. Supply the new value via a
secure environment, disable shell tracing, then run:

```bash
CONFIRM_SESSION_KEY_ROTATION=rotate bash deploy/rotate-session-secret.sh
```

This patches only the session key and does not restart pods. Coordinate restart
of every core process afterward; mixed keys reject credentials intermittently.
Rotation requires the same credential/link/feed recovery described above.

### Production investigation and post-deployment checks

No production inspection was available during this repair. Actual Secret
replacement, differing injected values, database replacement and explicit
revocation remain unconfirmed hypotheses. The old scripts could replace the
Secret; that capability alone does not prove it happened.

Compare SHA-256 fingerprints without displaying secrets (bash; disable tracing):

```bash
set +x
set -o pipefail
kubectl -n commoncal get secret commoncal-session \
  -o 'jsonpath={.data.SESSION_SECRET}' | python3 -c '
import base64, hashlib, sys
try:
    secret = base64.b64decode(sys.stdin.read(), validate=True)
    if not secret:
        raise ValueError()
except Exception:
    sys.exit("Missing/empty or invalid session key; fingerprint unavailable")
print(hashlib.sha256(secret).hexdigest())'
kubectl -n commoncal get secret commoncal-session \
  -o 'jsonpath={.metadata.uid}{" "}{.metadata.resourceVersion}{"\n"}'
kubectl -n commoncal get pods
read -r -p 'Core pod name: ' CORE_POD
kubectl -n commoncal exec "$CORE_POD" -- sh -c \
  'test -n "$SESSION_SECRET" && printf %s "$SESSION_SECRET" | sha256sum'
```

Record Secret UID/resourceVersion before and after deployment: a changed UID
indicates object replacement; a changed resourceVersion indicates an update and
does not by itself prove key rotation. Repeat the pod fingerprint for every core
pod and after restart/deployment. Equal fingerprints verify injected values, but could not prevent the old random
salt defect. Inspect PVC identity and SQLite path, credential counts and
`revoked_at`, plus relevant `audit_log` actions/timestamps. Never select or log
`token_hash`, password material, authorization headers or feed URLs. Preserve
before/after evidence to distinguish revocation or database replacement.

Use the hidden-prompt smoke command in **CalDAV compatibility repair handoff →
Post-deployment checks** with an existing password issued after this repair.
Keep that same password for another restart/deployment and rerun it; this is the
persistence check. An old pre-fix password cannot validate persistence through
the first corrective rollout. Confirm both discovery methods reach `/dav/`,
with the expected principal/home and calendar count.

Retry Apple account setup or refresh afterward. Capture the **first failing**
request's method, path, status and User-Agent using the credential-safe proxy
procedure above. Backend access logging defaults to OFF and lacks User-Agent,
so those logs alone cannot supply the requested trace. Record native-client
results separately from smoke results before claiming Apple compatibility.


### Verification and suggested change description for this repair

The current repair's full `cargo test --manifest-path backend/Cargo.toml` run
exited successfully, including 183 library tests and 104 CalDAV integration
tests among the complete integration suites. Three smoke fixtures passed,
including discovery and initial sync for two calendars. Secret preservation,
explicit rotation, deployment-stack and chart checks passed, as did backend
formatting and `git diff --check`. These results describe this repair separately
from the earlier compatibility handoff above; production and native Apple
verification remain pending.

Proposed commit subject and PR title:
`fix(caldav): preserve discovery methods and deployment credentials`

Accept PROPFIND discovery with a method/body-preserving 307, validate GET and
PROPFIND discovery through the router and semantic smoke, and stabilize session
key derivation across processes. Preserve existing production secrets during
normal deployments and require explicit rotation. Document first-rollout
credential/link/feed recovery and subsequent password persistence checks.
Domain reviewer from repository history: david-hajnal <david@hajnal.space>;
no verified GitHub handle. No commit, PR or deployment is created by this handoff.
