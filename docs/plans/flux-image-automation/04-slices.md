# Slice Plan: Flux Image Automation

## Slice 1 — Replace HelmChart with HelmRelease (pinned images)

**Goal**: Replace invalid HelmChart resources with proper HelmRelease resources. Pin to known-good versions.

**Files created:**
- `deploy/flux/overlays/production/charts/core-helmrelease.yaml` — HelmRelease for core
- `deploy/flux/overlays/production/charts/mcp-helmrelease.yaml` — HelmRelease for MCP

**Files modified:**
- `deploy/flux/overlays/production/kustomization.yaml` — update resource references

**Files deleted:**
- `deploy/flux/overlays/production/charts/core-helmchart.yaml`
- `deploy/flux/overlays/production/charts/core-helmchart.yaml.env`
- `deploy/flux/overlays/production/charts/mcp-helmchart.yaml`
- `deploy/flux/overlays/production/charts/mcp-helmchart.yaml.env`

**Details:**
- Core HelmRelease: `helm.toolkit.fluxcd.io/v2`, targetNamespace `commoncal`, chart `./deploy/helm/commoncal`, image tag pinned to `v1.0.0` (placeholder — user will set actual version)
- MCP HelmRelease: same structure, dependsOn `commoncal-core`, image tag pinned to `v1.0.0`
- All existing values from helmchart.yaml preserved (ingress, persistence, resources, backup, env)
- Install remediation: retries=3, upgrade remediation: retries=3
- Rollback timeout: 5m0s
- Health checks: enabled=true, progressDeadlineSeconds=600

**Verification:**
- `helm lint deploy/helm/commoncal` passes
- `helm lint deploy/helm/commoncal-mcp` passes
- `kustomize build deploy/flux/overlays/production` renders without errors
- No PVC name changes (StatefulSet name unchanged)

---

## Slice 2 — Consolidate Flux reconciliation

**Goal**: Remove redundant root-level Flux resources. Keep gotk-sync.yaml as single source of truth.

**Files deleted:**
- `deploy/flux/gitrepository.yaml` — redundant, gotk-sync.yaml GitRepository covers this
- `deploy/flux/kustomization.yaml` — redundant, gotk-sync.yaml Kustomization covers this

**Files modified:**
- None (purely deletive)

**Details:**
- `gitrepository.yaml` points to same repo as gotk-sync.yaml (david-hajnal/calendar)
- `kustomization.yaml` points to same path as gotk-sync.yaml Kustomization
- Both root resources conflict with the bootstrap configuration
- After deletion, only gotk-sync.yaml defines the GitRepository and Kustomization

**Verification:**
- `kustomize build deploy/flux/overlays/production` still renders correctly
- Flux bootstrap would produce identical configuration
- No circular dependencies introduced

---

## Slice 3 — CI: semver tags for core and MCP

**Goal**: Update CI workflows to publish sortable immutable semver tags.

**Files modified:**
- `.github/workflows/build-core.yml`
- `.github/workflows/build-mcp.yml`

**Details:**
- Add `on.push.tags: ['v*']` trigger to both workflows
- Update `docker/metadata-action` tags:
  - `type=semver,pattern={{version}}` — semver tags from git tags
  - `type=sha,format=long,prefix=sha-` — long SHA tags for dev builds
  - `type=ref,event=branch` — branch tags (main) for dev builds
- Keep `push` conditional on `github.event_name == 'push'`
- Remove ineffective semver pattern (now properly triggered by tag pushes)
- Tag format: `v<major>.<minor>.<patch>` (e.g., `v1.0.0`)

**Loop prevention in CI:**
- Add condition to skip builds when only deployment files changed:
  ```yaml
  if: |
    github.event_name == 'push' &&
    (github.event.head_commit.msg != 'chore(deploy): update image tags' ||
     github.event.commits[*].modified contains 'Dockerfile' ||
     github.event.commits[*].modified contains 'Dockerfile.mcp' ||
     github.event.commits[*].modified contains 'backend/' ||
     github.event.commits[*].modified contains 'mcp-server/')
  ```

**Verification:**
- Workflow syntax validates (yaml lint)
- Tag push triggers build
- Branch push triggers build
- Deploy-only commits are skipped

---

## Slice 4 — Flux image automation (ImageRepository + ImagePolicy + ImageUpdateAutomation)

**Goal**: Install image reflection and automation controllers. Configure image promotion.

**Files created:**
- `deploy/flux/overlays/production/flux-system/image-repository-core.yaml`
- `deploy/flux/overlays/production/flux-system/image-repository-mcp.yaml`
- `deploy/flux/overlays/production/flux-system/image-policy-core.yaml`
- `deploy/flux/overlays/production/flux-system/image-policy-mcp.yaml`
- `deploy/flux/overlays/production/flux-system/image-update-core.yaml`
- `deploy/flux/overlays/production/flux-system/image-update-mcp.yaml`

**Files modified:**
- `deploy/flux/overlays/production/flux-system/kustomization.yaml` — add new resources
- `deploy/flux/overlays/production/flux-system/gotk-components.yaml` — regenerate with image controllers
- `deploy/flux/overlays/production/charts/core-helmrelease.yaml` — add flux setter comment for tag
- `deploy/flux/overlays/production/charts/mcp-helmrelease.yaml` — add flux setter comment for tag

**Details:**

ImageRepository (core):
- `image: ghcr.io/david-hajnal/calendar-core`
- `provider: ghcr`

ImageRepository (MCP):
- `image: ghcr.io/david-hajnal/calendar-mcp`
- `provider: ghcr`

ImagePolicy (core):
- `imageRepositoryRef.name: image-repository-core`
- `policy.semver.range: ">=0.0.0"`
- `sort: desc`

ImagePolicy (MCP):
- `imageRepositoryRef.name: image-repository-mcp`
- `policy.semver.range: ">=0.0.0"`
- `sort: desc`

ImageUpdateAutomation (core):
- `gitRepositoryRef.name: flux-system`
- `push.type: github`
- `push.branch: main`
- `commit.title: "chore(deploy): update core image to {{ .Tag }}"`
- `interval: 2m0s`
- `update.path: ./deploy/flux/overlays/production`
- `policy.imageNames[0].name: image-policy-core`

ImageUpdateAutomation (MCP):
- Same structure, `policy.imageNames[0].name: image-policy-mcp`

**Flux setter comments in HelmRelease:**
```yaml
image:
  # fluxcd/image-automation: tag={{ .Values.image.tag }}
  repository: ghcr.io/david-hajnal/calendar-core
  tag: "v1.0.0"
  pullPolicy: IfNotPresent
```

**Verification:**
- `kustomize build deploy/flux/overlays/production` renders all new resources
- YAML schema validates against Flux CRD schemas
- No duplicate resource names

---

## Slice 5 — Validation and operational safeguards

**Goal**: Add CI validation for deployment changes. Document operational procedures.

**Files modified:**
- `.github/workflows/ci.yml` — add deploy validation job

**Files created:**
- `scripts/validate-deploy.sh` — validation script

**Details:**

CI validation job (`ci.yml`):
```yaml
deploy-validation:
  runs-on: ubuntu-latest
  steps:
    - uses: actions/checkout@v4
    - uses: fluxcd/flux2/action@v2
    - uses: helm/kind-action@v1
    - name: Create kind cluster
      run: kind create cluster --name flux-validation
    - name: Load images
      run: kind load docker-image ...
    - name: Apply Flux
      run: kubectl apply -f deploy/flux/overlays/production/flux-system/gotk-components.yaml
    - name: Wait for Flux controllers
      run: kubectl wait ...
    - name: Apply manifests
      run: kubectl apply -f deploy/flux/overlays/production/
    - name: Validate HelmReleases
      run: flux get helmreleases --all-namespaces
    - name: Validate images are not latest/main
      run: |
        grep -r 'tag: latest' deploy/flux/ && exit 1
        grep -r 'tag: "main"' deploy/flux/ && exit 1
```

Validation script (`scripts/validate-deploy.sh`):
- `helm lint deploy/helm/commoncal`
- `helm lint deploy/helm/commoncal-mcp`
- `helm template deploy/helm/commoncal` (renders without errors)
- `helm template deploy/helm/commoncal-mcp` (renders without errors)
- `kustomize build deploy/flux/overlays/production` (validates kustomize)
- Schema validation against Kubernetes/Flux CRD schemas
- Check no mutable tags (latest, main) in production manifests
- Check all automated image values have flux setter comments

**Documentation:**
- Add `DEPLOYMENT.md` with operational procedures:
  - How to suspend/resume image automation
  - How to reconcile source, Kustomization, Helm releases, image policies
  - How to pin a known-good tag
  - How to revert the automation commit
  - Where GHCR credentials and production Secrets are provisioned

**Verification:**
- CI passes with all validation steps
- `scripts/validate-deploy.sh` exits 0 on valid manifests
- `scripts/validate-deploy.sh` exits 1 on mutable tags

---

## Slice 6 — End-to-end verification

**Goal**: Verify the complete chain works from tag push to deployment.

**Steps:**
1. Pick a known-good image version (e.g., current deployed version)
2. Tag and push `v1.0.0` for both core and MCP
3. Verify images appear in GHCR with the tag
4. Wait for Flux ImageRepository to detect (up to 2min)
5. Wait for Flux ImagePolicy to select (semver desc)
6. Wait for ImageUpdateAutomation to commit (up to 2min)
7. Verify commit appears in repo with updated image tags
8. Wait for Flux Kustomization to reconcile (up to 10min)
9. Verify HelmRelease upgrades to new image
10. Verify pods roll out with new image digest
11. Verify PVCs remain bound (no data loss)
12. Verify ingress, login, calendar API, MCP endpoint work
13. Verify Flux reports no stalled/failed resources
14. Revert the automation commit
15. Verify workloads return to previous version

**Verification:**
- All pods report expected image digest
- StatefulSet PVC not replaced
- Ingress still routes correctly
- MCP still connects to core API
- Flux `get hr -A` shows Ready
- Flux `get imgrepo -A` shows updated lastImage
- Flux `get imgpolicy -A` shows latestImage
