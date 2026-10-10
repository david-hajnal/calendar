# MCP "Host header is not allowed" — Remediation Plan

**Status**: Source fix complete; production deployment incomplete  
**Prepared**: 2026-10-10  
**Root cause**: rmcp SDK default Host allowlist blocks public hostnames

---

## Executive summary

The MCP server at `https://mcal.hajnal.space/mcp` rejects requests from OpenCode
with `Forbidden: Host header is not allowed`. This is **not** an OpenCode
configuration issue — it is the `rmcp` SDK's `StreamableHttpServerConfig` default
which only admits `localhost`, `127.0.0.1`, and `::1`.

A source fix has been implemented, tested, published, and the MCP image has been
updated. **The remaining blocker is that the authorization service is not deployed
to production**, so even with the Host fix, OpenCode cannot complete the OAuth
lifecycle to obtain a valid token.

---

## Root cause analysis

### Phase 1: Reproduce + Minimise

**Confirmed symptom**: `POST https://mcal.hajnal.space/mcp` with a valid Bearer
token returns `HTTP 403: Forbidden: Host header is not allowed`.

**Minimised repro**: Any HTTP request to the MCP endpoint with a Host header
matching the public hostname (`mcal.hajnal.space`) is rejected when the rmcp
transport is built with `StreamableHttpServerConfig::default()` (no custom
allowlist).

**Load-bearing elements**:
1. rmcp SDK default `StreamableHttpServerConfig` — admits only loopback hosts
2. Traefik ingress routing to the MCP pod on port 80 — passes the public Host
3. Bearer middleware wrapping the transport — auth succeeds before Host check

### Phase 2: Hypotheses

| # | Hypothesis | Prediction | Rank |
|---|-----------|-----------|------|
| 1 | rmcp's default `StreamableHttpServerConfig` rejects non-loopback Host headers | Building the transport with `.with_allowed_hosts(["mcal.hajnal.space"])` will admit the request | 1 |
| 2 | Traefik rewrites the Host header before reaching the pod | Disabling Traefik Host rewrite while keeping the rmcp fix will still reject | 2 |
| 3 | Cloudflare WAF blocks the Host header | Direct pod access while keeping the same Host will still reject | 3 |
| 4 | The OpenCode `headers.Host` config is being overridden by the HTTP library | Setting the Host in OpenCode config has no effect on the underlying TCP connection | 4 |

**Confirmed**: Hypothesis 1 is correct. The fix is in `mcp-server/src/transport.rs`
line 17: `.with_allowed_hosts([host])` where `host` is derived from
`MCP_PUBLIC_RESOURCE_URL`.

### Phase 3: Evidence

- **Source fix**: `935009319ffd7d873ea5fd8f9a7afd52d23a1195` —
  `mcp-server/src/transport.rs` + `mcp-server/src/main.rs:61`
- **Published image**: `ghcr.io/david-hajnal/calendar-mcp:sha-935009319ffd7d873ea5fd8f9a7afd52d23a1195`
- **MCP HelmRelease tag**: Updated to `sha-935009319ffd7d873ea5fd8f9a7afd52d23a1195`
- **Local regression tests**: 4 transport tests pass (public Host → 200, unexpected Host → 403)
- **rmcp version**: 3.1.4 (pinned in `mcp-server/Cargo.toml`)

---

## Current state assessment

### What is deployed and working

| Component | Status | Details |
|-----------|--------|---------|
| MCP server image | ✅ Fixed | Running `sha-935009319ffd7d873ea5fd8f9a7afd52d23a1195` |
| MCP Host validation | ✅ Fixed | `transport::server_config` derives hostname from `MCP_PUBLIC_RESOURCE_URL` |
| Unauthenticated challenge | ✅ Working | Returns `401 Bearer` with correct `resource_metadata` URL |
| Protected-resource metadata | ✅ Working | Valid JSON at `/.well-known/oauth-protected-resource` |
| Core service | ✅ Running | Auth bridge enabled, browser login/consent working |
| Auth service | ❌ Not deployed | HelmRelease exists in production overlay but is **not included** in kustomization |

### What is NOT deployed

| Component | Blocker |
|-----------|---------|
| Auth HelmRelease | Not in `deploy/flux/overlays/production/kustomization.yaml` |
| Auth image with DCR scope fix | Not deployed (current tag `sha-935009319ffd7d873ea5fd8f9a7afd52d23a1195` is MCP image tag, not auth) |
| Auth PVC / SQLite storage | Not provisioned |
| Auth DNS / TLS | Not provisioned |
| Auth secrets (signing, cookie, bridge) | Not created |
| Auth ingress routing | Not active |

### Auth image status

The auth service has its own separate image (`ghcr.io/david-hajnal/calendar-auth`).
The DCR scope validation fix is at commit `dbba0965c7182148f5286912975e7753e3380bd4`.
This is an **ancestor** of the current HEAD. Its published image tag must be
verified in GHCR before deployment.

---

## Remediation plan

### Step 1: Verify auth image publication

```sh
# Verify the DCR-fix image exists in GHCR with correct manifest
ghcr.io/david-hajnal/calendar-auth:sha-dbba0965c7182148f5286912975e7753e3380bd4

# Also verify the latest image (if a newer build has been published)
ghcr.io/david-hajnal/calendar-auth:sha-1ebaab8ee4e5fcb0548267c984c68f0e4fa232f8
```

**Decision**: Use the image whose publication run is verified and whose manifest
includes the DCR scope fix. The cutover candidate currently points to
`sha-1ebaab8ee4e5fcb0548267c984c68f0e4fa232f8`.

### Step 2: Provision auth infrastructure

**Prerequisites**: cluster access, Flux CLI, SSH to production server, age identity
for backup decryption.

1. **Bootstrap auth SQLite** (if not already done):
   ```sh
   NAMESPACE=commoncal bash deploy/provision-auth-tls.sh
   deploy/auth-prerequisites.sh
   ```

2. **Provision secrets** (signing key, cookie keys, bridge key):
   - Create `commoncal-auth-secrets` Secret with `AUTH_SIGNING_KID`, `AUTH_COOKIE_KEYS`,
     `AUTH_BRIDGE_KEY`
   - Create `commoncal-auth-backup` Secret with the age recipient

3. **Provision PVC**: 1Gi ReadWriteOnce for auth SQLite

4. **DNS + TLS**: `auth.hajnal.space` → Traefik `websecure` entrypoint

### Step 3: Deploy auth service

```sh
# Update the cutover candidate with the verified auth image tag
# Then patch the production kustomization to include auth

kubectl patch helmrelease commoncal-auth -n flux-system --type=merge \
  -p '{"spec":{"values":{"image":{"tag":"sha-<verified-auth-sha>"}}}}'

flux reconcile helmrelease commoncal-auth -n flux-system \
  --force --reset --with-source

kubectl rollout status deployment/commoncal-auth -n commoncal --timeout=10m
```

### Step 4: Add auth to production kustomization

The auth HelmRelease file already exists at
`deploy/flux/overlays/production/charts/auth-helmrelease.yaml` but is **not
included** in the active kustomization.

```yaml
# deploy/flux/overlays/production/kustomization.yaml
apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization
resources:
  - charts/core-helmrelease.yaml
  - charts/mcp-helmrelease.yaml
  - charts/auth-helmrelease.yaml    # ← ADD THIS LINE
```

**Decision**: Add `charts/auth-helmrelease.yaml` to the production kustomization
**only after** Step 3 (auth service is running and verified). This prevents Flux
from applying a non-functional release.

### Step 5: Complete browser OAuth flow

```sh
python3 scripts/auth-browser-proof.py \
  "$HOME/commoncal-oauth-proof-$(date +%s)"
```

Sign in with browser, approve calendar consent. Verify:
- DCR succeeds (no scope validation error)
- PKCE S256 code exchange returns token
- `proof.json` saved with access token

### Step 6: Verify OpenCode connection

```sh
# In OpenCode config, ensure the commoncal MCP server is configured:
# {
#   "mcp": {
#     "servers": {
#       "commoncal": {
#         "type": "remote",
#         "url": "https://mcal.hajnal.space/mcp",
#         "oauth": {
#           "client_id": "H0AVRdb8Q0p5TFDVilGfHBR_0np_h4GhAt5MU7LzJbP",
#           "redirect_uri": "http://127.0.0.1:7777/mcp/oauth/callback",
#           "callback_port": 7777,
#           "scope": "commoncal.calendar.metadata.read",
#           "auth_server_metadata_url": "https://auth.hajnal.space/.well-known/openid-configuration"
#         }
#       }
#     }
#   }
# }

# Run auth flow
opencode mcp auth commoncal

# Verify connection
opencode mcp list
```

Expected:
- OAuth DCR → registration succeeds
- Browser login → consent → token exchange
- `initialize` → 200
- `tools/list` → 9 tools with schemas
- `calendar_list` → returns granted calendars

### Step 7: Run public verification

```sh
python3 scripts/check-mcp-public-host.py \
  --proof-file "$HOME/commoncal-oauth-proof-*/proof.json"
```

Verifies:
- Authenticated `initialize` and `tools/list` for both `mcal.hajnal.space` and
  `mcal.hajnal.space:443`
- Negative token rejection (401)
- Unexpected Host rejection (403)

### Step 8: Persist in Git + resume root Flux

After all steps pass:
1. Commit the kustomization change (auth included)
2. Update the cutover candidate README with verified values
3. Verify manifests: `scripts/validate-deploy.sh`
4. Resume root Flux only after Git matches production

---

## Risk assessment

| Risk | Mitigation |
|------|-----------|
| Auth image tag mismatch | Verify digest in GHCR before patching |
| Auth PVC data loss | Existing PVC retained; initContainer prevents overlap |
| DNS/TLS misconfiguration | Use `deploy/provision-auth-tls.sh` which validates |
| Flux applies before auth ready | Add auth to kustomization only after Step 3 verification |
| Root Flux drift | Keep root Flux paused until Git matches production |
| Backup restoration failure | Offline restore drill completed previously; retain archive |

## Rollback plan

If any step fails:
1. **Auth deployment fails**: Remove `charts/auth-helmrelease.yaml` from kustomization,
   reconcile. MCP continues running with the fixed Host validation (unauthenticated
   requests get 401, which is the pre-fix behavior — no regression).
2. **Auth breaks existing state**: Retain the existing auth SQLite PVC and signing keys.
   The new HelmRelease references the same PVC name (`commoncal-auth-data`).
3. **MCP regression**: Revert the MCP HelmRelease tag to the prior image tag
   (`sha-def52b6c2db4e3325467c9b9c1eaa77a11418e3e`).

## Completion criteria

- [ ] Auth service running with correct image and verified `/ready` endpoint
- [ ] Auth HelmRelease included in production kustomization and reconciled
- [ ] Browser OAuth flow completes: DCR → consent → token
- [ ] OpenCode connects, authenticates, lists tools, calls `calendar_list`
- [ ] Public verification script passes all checks
- [ ] Production kustomization in Git matches live state
- [ ] Root Flux resumed with verified configuration
- [ ] No `Host header is not allowed` errors in any context
