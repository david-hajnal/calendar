# Program Design: Flux Image Automation

## Files

### Created
- `docs/plans/flux-image-automation/03-program-design.md` — this file
- `deploy/flux/overlays/production/charts/core-helmrelease.yaml` — HelmRelease for core (replaces core-helmchart.yaml)
- `deploy/flux/overlays/production/charts/mcp-helmrelease.yaml` — HelmRelease for MCP (replaces mcp-helmchart.yaml)
- `deploy/flux/overlays/production/flux-system/image-repository-core.yaml` — ImageRepository for calendar-core
- `deploy/flux/overlays/production/flux-system/image-repository-mcp.yaml` — ImageRepository for calendar-mcp
- `deploy/flux/overlays/production/flux-system/image-policy-core.yaml` — ImagePolicy for core (semver desc)
- `deploy/flux/overlays/production/flux-system/image-policy-mcp.yaml` — ImagePolicy for MCP (semver desc)
- `deploy/flux/overlays/production/flux-system/image-update-core.yaml` — ImageUpdateAutomation for core
- `deploy/flux/overlays/production/flux-system/image-update-mcp.yaml` — ImageUpdateAutomation for MCP

### Changed
- `deploy/flux/overlays/production/kustomization.yaml` — resources point to helmrelease.yaml files
- `deploy/flux/overlays/production/flux-system/kustomization.yaml` — add image automation resources
- `deploy/flux/overlays/production/flux-system/gotk-components.yaml` — regenerate with image controllers
- `.github/workflows/build-core.yml` — add semver tags, tag push trigger
- `.github/workflows/build-mcp.yml` — add semver tags, tag push trigger

### Deleted
- `deploy/flux/overlays/production/charts/core-helmchart.yaml` — replaced by helmrelease
- `deploy/flux/overlays/production/charts/core-helmchart.yaml.env` — template file, not needed
- `deploy/flux/overlays/production/charts/mcp-helmchart.yaml` — replaced by helmrelease
- `deploy/flux/overlays/production/charts/mcp-helmchart.yaml.env` — template file, not needed
- `deploy/flux/gitrepository.yaml` — redundant, gotk-sync.yaml covers this
- `deploy/flux/kustomization.yaml` — redundant, gotk-sync.yaml covers this

## Types & signatures

### HelmRelease (core)
```yaml
apiVersion: helm.toolkit.fluxcd.io/v2
kind: HelmRelease
metadata:
  name: commoncal-core
  namespace: flux-system
spec:
  targetNamespace: commoncal
  chart:
    spec:
      chart: ./deploy/helm/commoncal
      version: "0.1.0"
      sourceRef:
        kind: GitRepository
        name: flux-system
      interval: 10m
  interval: 10m
  values:
    image:
      repository: ghcr.io/david-hajnal/calendar-core
      tag: "{{ .Values.image.tag }}"  # flux setter comment
      pullPolicy: IfNotPresent
    # ... all existing values preserved
  install:
    remediation:
      retries: 3
  upgrade:
    remediation:
      retries: 3
  rollback:
    timeout: 5m0s
  suspend: false
```

### HelmRelease (MCP)
Same structure as core, with:
- `name: commoncal-mcp`
- `chart: ./deploy/helm/commoncal-mcp`
- `image.repository: ghcr.io/david-hajnal/calendar-mcp`
- `tag: "{{ .Values.image.tag }}"`
- `dependsOn: [{name: commoncal-core}]`
- MCP-specific env values

### ImageRepository
```yaml
apiVersion: image.toolkit.fluxcd.io/v1beta2
kind: ImageRepository
metadata:
  name: <core|mcp>
  namespace: flux-system
spec:
  image: ghcr.io/david-hajnal/calendar-<core|mcp>
  provider: ghcr
```

### ImagePolicy
```yaml
apiVersion: image.toolkit.fluxcd.io/v1beta2
kind: ImagePolicy
metadata:
  name: <core|mcp>-policy
  namespace: flux-system
spec:
  imageRepositoryRef:
    name: <core|mcp>
  policy:
    semver:
      range: ">=0.0.0"
  sort: desc
```

### ImageUpdateAutomation
```yaml
apiVersion: image.toolkit.fluxcd.io/v1beta2
kind: ImageUpdateAutomation
metadata:
  name: flux-system
  namespace: flux-system
spec:
  gitRepositoryRef:
    name: flux-system
  push:
    type: github
    branch: main
  commitTemplate:
    title: "chore(deploy): update image tags"
    body: "{{range .UpdatedImages}}{{.Name}}: {{.FromVersion}} -> {{.ToVersion}}{{\"\n\"}}{{end}}"
  interval: 2m0s
  update:
    path: ./deploy/flux/overlays/production
  policy:
    imageNames:
      - name: <core|mcp>-policy
```

### CI tag generation
```yaml
# build-core.yml / build-mcp.yml
- name: Extract metadata
  id: meta
  uses: docker/metadata-action@v5
  with:
    images: ghcr.io/david-hajnal/calendar-<core|mcp>
    tags: |
      type=semver,pattern={{version}}
      type=sha,format=long
      type=ref,event=branch
```

## Call stack

### Deployment flow (manual tag → deployment)
```
Developer pushes git tag (v1.0.0)
  → GitHub triggers build-core.yml / build-mcp.yml (on push tags)
    → docker/metadata-action generates semver tag + sha tag
    → docker/build-push-action pushes to GHCR
  → Flux ImageRepository polls GHCR (every 2min)
    → detects new tag
  → Flux ImagePolicy evaluates semver
    → selects newest tag
  → Flux ImageUpdateAutomation commits tag to HelmRelease values
    → pushes commit to main
  → Flux Kustomization detects commit (every 10min)
    → reconciles HelmRelease with new tag
  → Helm controller upgrades release
    → Kubernetes rolls out StatefulSet/Deployment
```

### CI build flow (application change → deployment)
```
Developer pushes to main
  → GitHub triggers build-core.yml / build-mcp.yml (on push branches: [main])
    → docker/metadataAction generates sha tag + branch tag
    → docker/build-push-action pushes to GHCR
  → Flux ImageRepository detects new sha-tagged image
  → Flux ImagePolicy (semver desc) does NOT select sha tag (not semver)
    → no ImageUpdateAutomation commit
    → no deployment (correct — sha tags are dev-only)
```

### Loop prevention
```
ImageUpdateAutomation pushes tag update to main
  → GitHub triggers build-core.yml / build-mcp.yml
    → CI detects only deploy/flux/** changed (no app code changes)
    → CI skips build
    → no new image pushed (correct — no app changes)
```

## Test plan

### Test: helm-release-valid-core
- Assert HelmRelease has `helm.toolkit.fluxcd.io/v2` apiVersion
- Assert `targetNamespace: commoncal`
- Assert `chart.spec.sourceRef.kind: GitRepository`
- Assert `chart.spec.sourceRef.name: flux-system`
- Assert `install.remediation.retries` is set
- Assert `upgrade.remediation.retries` is set
- Assert `timeout` is set
- Assert image tag uses flux setter comment `{{ .Values.image.tag }}`
- Assert all existing values from core-helmchart.yaml are preserved

### Test: helm-release-valid-mcp
- Assert HelmRelease has `helm.toolkit.fluxcd.io/v2` apiVersion
- Assert `targetNamespace: commoncal`
- Assert `dependsOn` contains `commoncal-core`
- Assert `chart.spec.sourceRef.kind: GitRepository`
- Assert image tag uses flux setter comment
- Assert all existing values from mcp-helmchart.yaml are preserved
- Assert `env.CALENDAR_API_URL: http://commoncal-core:3000/api`

### Test: image-repository-valid
- Assert ImageRepository exists for both core and MCP
- Assert `image` field matches GHCR path
- Assert `provider: ghcr`

### Test: image-policy-valid
- Assert ImagePolicy exists for both core and MCP
- Assert `policy.semver.range` is set
- Assert `sort: desc`
- Assert `imageRepositoryRef.name` matches corresponding ImageRepository

### Test: image-update-valid
- Assert ImageUpdateAutomation exists
- Assert `push.branch: main`
- Assert `commitTemplate` has descriptive title
- Assert `interval: 2m0s`
- Assert `update.path: ./deploy/flux/overlays/production`

### Test: ci-semver-tags
- Assert build-core.yml has `type=semver` in tags
- Assert build-mcp.yml has `type=semver` in tags
- Assert build-core.yml has `type=sha,format=long` in tags
- Assert build-mcp.yml has `type=sha,format=long` in tags
- Assert both have `on.push.tags: ['v*']` trigger
- Assert loop prevention: `if: github.event.head_commit.msg != 'chore(deploy): update image tags'`

### Test: redundant-resources-removed
- Assert deploy/flux/gitrepository.yaml does not exist
- Assert deploy/flux/kustomization.yaml does not exist
- Assert core-helmchart.yaml does not exist
- Assert mcp-helmchart.yaml does not exist

### Test: kustomization-consistency
- Assert overlay kustomization.yaml references helmrelease.yaml not helmchart.yaml
- Assert flux-system kustomization.yaml includes all new image automation resources
- Assert no circular dependsOn in any resource

## Least confident decisions

1. **GHCR provider**: Using `provider: ghcr` in ImageRepository. This requires the ImageRepository to have access to GHCR. If the repo is public, no secret is needed. If private, we need a Secret with `.dockerconfigjson` for GHCR. Need to confirm GHCR visibility.

2. **Loop prevention strategy**: Using commit message filtering (`github.event.head_commit.msg != 'chore(deploy): update image tags'`) as the primary loop prevention. This is simple but fragile — if the commit message template changes, the guard breaks. Alternative: check file paths in the commit (`github.event.commits[*].modified`) for `deploy/flux/**` only. A hybrid approach (check both paths AND message) is more robust.

3. **ImageUpdateAutomation naming**: Using a single `ImageUpdateAutomation` named `flux-system` that updates both core and MCP. This means one commit updates both images. Alternative: separate ImageUpdateAutomation per image for independent promotion. The task says "core and MCP can be promoted and rolled back independently" — this suggests separate automation. Need to decide.

4. **Semver range**: Using `">=0.0.0"` which accepts all semver tags. If we want to restrict to a specific range (e.g., `1.x.x`), this should be tightened. The range should match the expected version trajectory.

5. **Push type**: Using `type: github` for ImageUpdateAutomation push. This uses GH_TOKEN automatically. Need to confirm this works with the repository's branch protection rules and token permissions.

6. **Two-phase migration**: The task specifies pinned images first, then automation. The program design creates both simultaneously. The two-phase approach is implemented via: (a) initial commit pins specific semver tags, (b) verification happens, (c) ImageUpdateAutomation is enabled after verification. In practice, both are committed together but the pinned tags serve as the safe baseline.
