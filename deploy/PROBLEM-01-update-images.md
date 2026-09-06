# Problem: Update Core and MCP Images to Latest Main

## Goal
Update `commoncal` (core) and `commoncal-mcp` HelmReleases from v2.1.0 (`sha-03078da4a055b1e024fe221f9d9ceaa35a0a1bf5`) to the latest main branch image tag.

## Current State

### Core (`commoncal`)
- **File:** `deploy/flux/overlays/production/charts/core-helmrelease.yaml:37`
- **Image:** `ghcr.io/david-hajnal/calendar-core`
- **Current tag:** `sha-03078da4a055b1e024fe221f9d9ceaa35a0a1bf5`
- **Running version:** v2.1.0
- **Pod:** `commoncal-0` (StatefulSet)

### MCP (`commoncal-mcp`)
- **File:** `deploy/flux/overlays/production/charts/mcp-helmrelease.yaml:36`
- **Image:** `ghcr.io/david-hajnal/calendar-mcp`
- **Current tag:** `sha-03078da4a055b1e024fe221f9d9ceaa35a0a1bf5`
- **Running version:** v2.1.0
- **Pod:** `commoncal-mcp` (Deployment)

Both share the same commit SHA tag.

## Key Constraints

1. **Flux-managed deployment** — production uses Flux (all 3 HelmReleases active). Tags are set by `promote-main.yml` CI workflow. Direct `IMAGE_TAG` env var is ignored under Flux ownership.
2. **Immutable tags required** — tags must be `sha-<40 hex>` format, not `main` or `latest`.
3. **Auth is suspended** — core's `dependsOn: commoncal-auth` dependency must be removed before updating core.
4. **Rollout order** — auth -> core -> mcp. With auth removed, core deploys first, then mcp.

## What Needs to Be Done

### 1. Remove auth dependency from core HelmRelease
- **File:** `deploy/flux/overlays/production/charts/core-helmrelease.yaml:18-21`
- Remove the `dependsOn` block referencing `commoncal-auth`
- Also remove `authBridge` config block (lines 72-77) since auth service is gone
- Remove `mcpInternalApiSecret` references if they depend on auth secrets

### 2. Find latest main image tags
- Check CI pipeline (`promote-main.yml`) for how tags are generated
- Find the latest commit SHA for both `calendar-core` and `calendar-mcp` repos
- Tags follow pattern `sha-<40 hex>`

### 3. Update image tags in HelmReleases
- Update `core-helmrelease.yaml:37` with latest core main tag
- Update `mcp-helmrelease.yaml:36` with latest MCP main tag
- Ensure both tags are immutable `sha-<40 hex>` format

### 4. Trigger Flux reconciliation
- After committing tag changes, run: `flux reconcile kustomization flux-system --namespace flux-system --with-source`
- Or wait for Flux's 10m interval to pick up the changes

## Related Files

| File | Purpose |
|------|---------|
| `deploy/flux/overlays/production/charts/core-helmrelease.yaml` | Core HelmRelease (image tag on line 37) |
| `deploy/flux/overlays/production/charts/mcp-helmrelease.yaml` | MCP HelmRelease (image tag on line 36) |
| `deploy/deploy-prod.sh` | Deployment script (Flux mode path, lines 329-383) |
| `deploy/.env.example` | Environment template |

## Verification

After update:
1. `kubectl get pods -n commoncal` — pods should restart with new images
2. `kubectl describe pod <core-pod> -n commoncal` — verify image tag
3. `kubectl describe pod <mcp-pod> -n commoncal` — verify image tag
4. Check app health endpoints on `cal.hajnal.space` and `mcal.hajnal.space`
