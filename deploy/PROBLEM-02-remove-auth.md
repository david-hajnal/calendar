# Problem: Remove Auth Service from Production Deployment

## Goal
Suspend the auth HelmRelease and clean up all auth-related resources from the production deployment. Auth requires PostgreSQL which is unavailable; core backend uses SQLite and does not need the auth service.

## Current State

### Auth Service
- **HelmRelease:** `deploy/flux/overlays/production/charts/auth-helmrelease.yaml`
- **Image:** `ghcr.io/david-hajnal/calendar-auth`
- **Tag:** `sha-03078da4a055b1e024fe221f9d9ceaa35a0a1bf5`
- **Status:** Active in Flux (needs suspension)
- **Requirement:** PostgreSQL database (unavailable)
- **Purpose:** OAuth2/OIDC delegation — not a current requirement

### Auth Dependencies in Core
- **File:** `deploy/flux/overlays/production/charts/core-helmrelease.yaml`
- **`dependsOn` (lines 18-21):** Core waits for auth to be ready before deploying
- **`authBridge` config (lines 72-77):** Core is configured to call the auth bridge at `http://commoncal-auth-internal.commoncal.svc:80`
  - `enabled: true`
  - `timeoutMs: 5000`
  - `secretName: commoncal-auth-secrets`
  - `secretKey: LAB_BRIDGE_KEY`

### Auth Dependencies in MCP
- **File:** `deploy/flux/overlays/production/charts/mcp-helmrelease.yaml`
- **`oauthIssuerHoldKeyName` (line 77):** `mcp-oauth-issuer-hold` — holds the new issuer value for future cutover
- MCP still references `mcp-oauth-issuer` as the primary OAuth issuer (unchanged)

## What Needs to Be Done

### 1. Suspend auth HelmRelease
- **File:** `deploy/flux/overlays/production/charts/auth-helmrelease.yaml`
- Add `spec.suspend: true` to prevent Flux from reconciling it
- This is the minimal change — Flux will stop managing auth resources

### 2. Remove auth dependency from core HelmRelease
- **File:** `deploy/flux/overlays/production/charts/core-helmrelease.yaml`
- Remove the `dependsOn` block (lines 18-21) that references `commoncal-auth`
- Without this, Flux won't block core deployment waiting for a suspended release

### 3. Disable authBridge in core HelmRelease
- **File:** `deploy/flux/overlays/production/charts/core-helmrelease.yaml`
- Set `authBridge.enabled: false` (line 73)
- OR remove the entire `authBridge` block (lines 72-77)
- Core backend must not try to call a non-existent auth service

### 4. Clean up auth-related secrets (optional, manual)
- Secret: `commoncal-auth-secrets` in `commoncal` namespace
- Contains: `DATABASE_URL`, `LAB_BRIDGE_KEY`, `AUTH_COOKIE_KEYS`, `AUTH_SIGNING_KID`
- These are created by `deploy-prod.sh` (line 322-327) — the script will need updating too

### 5. Update deploy-prod.sh to skip auth
- **File:** `deploy/deploy-prod.sh`
- Remove auth secret creation (lines 319-327)
- Remove auth from required env vars (lines 28-31)
- Remove auth chart validation (line 81)
- Remove auth from rollout order (lines 369, 372, 376-377, 469-470, 477)
- Remove auth from Helm deployment args (lines 386-402, 467-470)
- Update the `active_flux_releases` check (line 223) from 3 to 2
- Update the Flux mode case from `3` to `2` (line 234)
- Update the error message about mixed deployment ownership (line 277)

### 6. Clean up auth HelmRelease file (optional)
- **File:** `deploy/flux/overlays/production/charts/auth-helmrelease.yaml`
- Either delete it or keep it commented out for future reference
- If keeping, ensure it won't be accidentally re-enabled

## Key Decisions

1. **Suspend vs delete:** Suspend the HelmRelease first. Delete the file later after confirming nothing breaks.
2. **authBridge:** Must be disabled. Core will fail if it tries to call the auth bridge URL.
3. **MCP OAuth issuer:** MCP's primary `mcp-oauth-issuer` remains unchanged. The `oauthIssuerHoldKeyName` can be cleared to empty.
4. **Secrets:** Don't delete `commoncal-auth-secrets` until confirmed no component references it.

## Related Files

| File | Purpose |
|------|---------|
| `deploy/flux/overlays/production/charts/auth-helmrelease.yaml` | Auth HelmRelease (suspend this) |
| `deploy/flux/overlays/production/charts/core-helmrelease.yaml` | Core HelmRelease (remove dependsOn + authBridge) |
| `deploy/flux/overlays/production/charts/mcp-helmrelease.yaml` | MCP HelmRelease (clear oauthIssuerHoldKeyName) |
| `deploy/deploy-prod.sh` | Deployment script (remove auth paths) |
| `deploy/.env.example` | Remove AUTH_* env vars |

## Verification

After removal:
1. `kubectl get helmrelease -n flux-system` — auth should show `Suspended: true`
2. `kubectl get pods -n commoncal` — no auth pods running
3. `kubectl get secret -n commoncal` — `commoncal-auth-secrets` still exists (don't delete yet)
4. Core and MCP should deploy independently without auth dependency
5. Health check endpoints on `cal.hajnal.space` and `mcal.hajnal.space` should work
