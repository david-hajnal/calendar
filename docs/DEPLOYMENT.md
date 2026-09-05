# Deployment Guide

## Overview

Auth, Core, and MCP are deployed to Kubernetes via Flux GitOps. Images are
published to GHCR and automatically promoted via Flux image automation.

The auth server is a Node.js OIDC provider that fronts the core and MCP
applications. It uses a managed PostgreSQL database for user/session state and
exposes a private bridge endpoint that core uses for authenticated API calls.

## Promotion model

Publication of a new `vX.Y.Z` container image is the production deployment
trigger. `scripts/release.sh` bumps versions, commits the release metadata, and
pushes the tag — it does **not** change the HelmRelease image tags. CI builds
and publishes both versioned images, and only then does Flux promote them:

- Flux never reconciles a version before its container image exists: both
  `vX.Y.Z` images are published to GHCR before either can be promoted.
- Auth, Core, and MCP advance **together**: one `ImageUpdateAutomation`
  (`image-update-automation`) commits all three HelmRelease tags. The three
  ImagePolicies are evaluated independently, so the tags can land in up to
  three commits within ~1 minute (a transient mixed state). All three images
  already exist in that window, so no release ever points at a missing image.
- Only stable `vX.Y.Z` tags qualify. `main`, SHA, `latest`, and prerelease
  tags are ignored by the ImagePolicies.
- Rollback is a Git revert of the promotion commit(s) (or an explicit pin,
  with the automation suspended).

## Architecture

```
release tag vX.Y.Z
  → CI builds all three images, publishes vX.Y.Z to GHCR
  → Flux ImageRepository detects the new tags
  → Flux ImagePolicy selects the highest stable vX.Y.Z
  → Flux ImageUpdateAutomation commits all three tags to main
  → Flux Kustomization reconciles the commit
  → HelmReleases upgrade (auth → core → mcp)
  → Kubernetes rolls out new pods
```

### Component topology

```
Browser ──(Cloudflare)──► Ingress ──► auth (public)
                                  │
                                  ├──► auth (private bridge) ◄── core (egress)
                                  │
                                  └──► core (StatefulSet)
                                           │
                                           └──► mcp (Deployment)
```

- **auth** — Node.js OIDC provider. Public service for browser-facing OIDC
  endpoints; private bridge service for core's authenticated API calls.
  Backed by managed PostgreSQL.
- **core** — Rust StatefulSet. The application backend. Talks to the auth
  bridge for session validation.
- **mcp** — Rust Deployment. The MCP server. Uses the auth issuer for OAuth.

## Namespace

All three applications deploy to the `commoncal` namespace.

Flux is the normal production deployment authority. The HelmReleases use the
explicit release names `commoncal-auth`, `commoncal`, and `commoncal-mcp`;
this avoids Flux's cross-namespace `commoncal-commoncal` default.

Run `deploy/deploy-prod.sh` for either deployment authority. When all three
HelmReleases are active, the script applies the runtime Secrets and reconciles
the Flux Kustomization followed by the auth, core, and MCP HelmReleases; it
does not deploy the workloads with direct Helm or create a self-signed
certificate. Flux deploys the image tags and chart values committed to its Git
source, so `IMAGE_TAG` and the direct chart overrides are ignored in this mode.
Flux mode also requires the canonical `commoncal` namespace and
`commoncal-auth`/`commoncal`/`commoncal-mcp` release names; remove any legacy
name overrides from `deploy/.env`. `GHCR_TOKEN` is rejected in this mode because
Flux pulls images with its own ImageRepository credentials; configure the pull
Secret on the HelmReleases in Git instead.

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

For an emergency direct deployment, first suspend all three Flux HelmReleases,
then run the same script. With all releases suspended (or absent), it deploys
all three workloads directly with Helm and requires `IMAGE_TAG`. It ensures the
`commoncal-tls` TLS Secret exists and is valid (generating a self-signed
certificate on first run if needed), then deploys all Ingresses referencing
that Secret. Resume Flux only after reconciling the direct deployment back into
Git. A mixed state with any active HelmRelease is rejected to prevent
split ownership.

### Legacy duplicate cleanup

Clusters previously reconciled without `spec.releaseName` may still contain the
legacy Helm release and StatefulSet named `commoncal-commoncal`. Before removing
it, use `helm list -n commoncal` and inspect both StatefulSets/PVCs to confirm
that `commoncal` is the live release holding the intended database. Suspend the
Flux HelmReleases during that inspection. Only then uninstall the verified
stale release with `helm uninstall commoncal-commoncal -n commoncal`; never
delete its PVC until the live database location has been confirmed.

## Images

- Auth: `ghcr.io/david-hajnal/calendar-auth`
- Core: `ghcr.io/david-hajnal/calendar-core`
- MCP: `ghcr.io/david-hajnal/calendar-mcp`

Tags follow semver: `v1.0.0`, `v1.0.1`, etc.

## Manual Deployment (Pinned)

To deploy a specific version:

1. Edit `deploy/flux/overlays/production/charts/auth-helmrelease.yaml`:
   ```yaml
   image:
     tag: "v1.0.0"  # change to desired version
   ```

2. Edit `deploy/flux/overlays/production/charts/core-helmrelease.yaml`:
   ```yaml
   image:
     tag: "v1.0.0"  # change to desired version
   ```

3. Edit `deploy/flux/overlays/production/charts/mcp-helmrelease.yaml`:
   ```yaml
   image:
     tag: "v1.0.0"  # change to desired version
   ```

4. Commit and push to `main`.

5. Flux will reconcile within 10 minutes.

## Suspend Image Automation

To pause automatic image promotion during an incident:

```bash
# Suspend the ImageUpdateAutomation
kubectl annotate imageupdateautomation image-update-automation \
  fluxcd.io/suspend=true --namespace=flux-system

# Resume (removes the annotation)
kubectl annotate imageupdateautomation image-update-automation \
  fluxcd.io/suspend- --namespace=flux-system
```

## Reconcile Resources

```bash
# Reconcile all Flux resources
flux reconcile kustomization flux-system --namespace=flux-system

# Reconcile specific HelmRelease
flux reconcile helmrelease commoncal-auth --namespace=flux-system
flux reconcile helmrelease commoncal --namespace=flux-system
flux reconcile helmrelease commoncal-mcp --namespace=flux-system

# Reconcile image policy
flux reconcile imagepolicy image-policy-auth --namespace=flux-system
flux reconcile imagepolicy image-policy-core --namespace=flux-system
flux reconcile imagepolicy image-policy-mcp --namespace=flux-system

# Reconcile image update automation
flux reconcile imageupdateautomation image-update-automation --namespace=flux-system
```

## Pin Known-Good Tag

To pin to a known-good version (disables automation for that release):

1. Edit the HelmRelease tag to the known-good version
2. Remove or annotate the ImageUpdateAutomation as suspended
3. Commit changes

## Revert Automation Commit

To revert an automated image tag change:

1. Find the automation commit:
   ```bash
   git log --grep="chore(deploy): update.*image" --oneline
   ```

2. Revert the commit (a promotion may be one or two commits — revert each):
   ```bash
   git revert <commit-hash>
   git push
   ```

3. Flux will reconcile and roll back to the previous version.

## GHCR Credentials

Images are published to `ghcr.io/david-hajnal/`. The repo must be public for Flux ImageRepository to pull without credentials. `GHCR_TOKEN` in `deploy/.env` only applies to direct Helm deployments; under Flux ownership the script rejects it. If the repo is private, create a Secret for the Flux side:

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

- `commoncal-auth-secrets` — auth server secrets:
  - `DATABASE_URL` — PostgreSQL connection string for the auth database
  - `LAB_BRIDGE_KEY` — shared secret for the private bridge endpoint
  - `AUTH_COOKIE_KEYS` — JSON array of cookie signing keys (for rotation)
  - `AUTH_SIGNING_KID` — current signing key ID
- `commoncal-session` — session encryption (key: `SESSION_SECRET`) and backup encryption (key: `BACKUP_ENCRYPTION_KEY_HEX`)
- `commoncal-mcp-secrets` — the shared internal API key, MCP session secret, and HTTPS OAuth issuer (`mcp-oauth-issuer`)
- `commoncal-tls` — self-signed TLS certificate for the origin hop (covers both `cal.hajnal.space` and `mcal.hajnal.space`)

The shared certificate covers both the core and MCP domains. Both Ingresses
reference the same Secret. Browsers receive Cloudflare Universal SSL; the
self-signed certificate is only presented on the Cloudflare-to-origin hop.

`BACKUP_ENCRYPTION_KEY_HEX` must be an even number of hexadecimal characters (at least 32); 64-hex (32-byte) keys remain backward-compatible.

## Auth Migration

The auth server uses a managed PostgreSQL database. Schema migrations run
automatically via a Helm pre-install/pre-upgrade Job in the `commoncal-auth`
chart. The migration Job:

1. Runs before the auth Deployment starts
2. Applies all pending migrations in order
3. Exits 0 on success, non-zero on failure
4. Blocks the auth Deployment rollout on failure

To run migrations manually (e.g., after a failed rollout):

```bash
kubectl create job --namespace=commoncal \
  --from=cronjob/commoncal-auth-migrate \
  commoncal-auth-migrate-manual
kubectl logs -n commoncal job/commoncal-auth-migrate-manual --follow
```

To inspect migration status:

```bash
kubectl get jobs -n commoncal -l app.kubernetes.io/name=commoncal-auth
kubectl logs -n commoncal job/commoncal-auth-migrate --tail=50
```

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

1. **Suspend image automation** to prevent re-promotion:
   ```bash
   kubectl annotate imageupdateautomation image-update-automation \
     fluxcd.io/suspend=true --namespace=flux-system
   ```

2. **Revert the promotion commit(s):**
   ```bash
   git log --grep="chore(deploy): update.*image" --oneline
   git revert <commit-hash>
   git push
   ```

3. **Resume image automation** after confirming the rollback:
   ```bash
   kubectl annotate imageupdateautomation image-update-automation \
     fluxcd.io/suspend- --namespace=flux-system
   ```

To roll back the auth server specifically:

1. Revert the auth HelmRelease tag to the previous version
2. The migration Job is idempotent — it will not re-run if the schema is
   already at the target version
3. If the migration was destructive, restore the database from backup:
   ```bash
   pg_restore --dbname=commoncal_auth --clean --if-exists \
     --username=auth < backup-file.dump
   ```

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

# Check image repositories
flux get imgrepo --namespace=flux-system

# Check image policies
flux get imgpolicy --namespace=flux-system

# Check image update automation
flux get imageupdateautomation --namespace=flux-system

# Check workloads
kubectl get statefulset -n commoncal
kubectl get deployment -n commoncal
kubectl get job -n commoncal -l app.kubernetes.io/name=commoncal-auth
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
- No mutable tags (latest/main) in production
- Each HelmRelease tag carries a valid `$imagepolicy` setter marker
  pointing at an existing ImagePolicy
- Image resources conform to the installed Flux CRD schemas, and
  `gotk-components.yaml` carries both image controllers and the image CRDs
- `scripts/release.sh` does not modify production image tags
- YAML syntax
- Chart template assertions (auth, core, mcp)
- Bridge isolation (no private ingress, no secret values)
- Flux dependency order (auth → core → mcp)
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
  --type merge -p '{"data":{"PASSWORD_LOGIN_ENABLED":"1"}}'
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
