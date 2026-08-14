# Architecture: Flux Image Automation

## Fit

Changes touch three layers:

1. **Flux deployment model** (`deploy/flux/overlays/production/charts/`) — replace HelmChart with HelmRelease
2. **Flux reconciliation** (`deploy/flux/`) — consolidate root resources, remove redundancy
3. **CI + image automation** (`.github/workflows/`, `deploy/flux/overlays/production/`) — immutable tags, ImageRepository/ImagePolicy/ImageUpdateAutomation

No application code changes. No Helm chart template changes. No frontend/backend changes.

## Endpoints

None. This is deployment infrastructure only.

## Data

No new Kubernetes resources that store application data. New Flux CRDs:

- `HelmRelease` (replaces `HelmChart` for core and MCP)
- `ImageRepository` (one per image, reads GHCR)
- `ImagePolicy` (one per image, selects newest by semver)
- `ImageUpdateAutomation` (one per image, commits tag changes to Git)

Existing resources preserved: StatefulSet (core), Deployment (MCP), PVCs, Secrets, Services, Ingress, CronJob (backup).

## Flow

### Main deployment flow (current → new)

**Current (broken model):**
```
Git push → Flux Kustomization watches repo → HelmChart resource creates HelmChart artifact → NO HelmRelease → nothing deploys
```

**New:**
```
Git push → app code changes → CI builds → immutable tag pushed to GHCR
  → Flux ImagePolicy selects newest tag
  → ImageUpdateAutomation commits tag to Git
  → Flux Kustomization reads commit
  → HelmRelease upgrades chart with new image tag
  → Kubernetes rolls out StatefulSet/Deployment
```

### Image automation loop
```
ImageUpdateAutomation polls ImageRepository every 2min
  → detects new main-<timestamp>-<sha> tag in GHCR
  → selects newest via numerical timestamp policy
  → commits tag change to HelmRelease values under deploy/flux/overlays/production/
  → pushes to main branch
  → Flux sync picks up the commit
  → HelmRelease upgrades
```

## External

- **GHCR** (`ghcr.io/david-hajnal/calendar-core`, `ghcr.io/david-hajnal/calendar-mcp`) — image registry. Must be accessible from cluster. If private, `imagePullSecrets` with `.dockerconfigjson` for GHCR.
- **Flux image controllers** — `image-reflector-controller` and `image-automation-controller` must be installed in `flux-system` namespace. Currently not present in gotk-components.yaml (Flux v2.9.4 bootstrap).
- **CI secrets** — `GITHUB_TOKEN` (auto-injected, provides GHCR write + Git push via `gh`). No additional secrets needed if GHCR repo is public.
- **Namespace** — `commoncal` (authoritative, from root kustomization). Confirmed by existing HelmRelease targetNamespace.

## Key decisions

1. **Namespace**: `commoncal` — both HelmRelease targetNamespace. No `production` namespace exists in manifests.
2. **Image tag format**: `main-<unix-timestamp>-<sha>` — chronologically sortable, immutable, traceable.
3. **Policy type**: `numerical` on timestamp extracted from tag — picks newest build.
4. **Two-phase rollout**: pinned images first (safe migration), then automation (after verification).
5. **Loop prevention**: CI skips when only `deploy/flux/**` changes and no app code changes.
6. **Bootstrap**: keep gotk-sync.yaml as single source of truth. Remove root-level gitrepository.yaml and kustomization.yaml.
