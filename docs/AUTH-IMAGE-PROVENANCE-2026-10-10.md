# Auth image provenance — 2026-10-10

## Findings

The image referenced by production Git is **post-fix**, not pre-fix. The actual running image remains unverified because this workstation has no Kubernetes current context. The supplied live discovery result (`openid`, `offline_access` only) shows pre-fix behavior; it does not identify an image digest. Do not equate a dormant Git manifest with a running deployment.

Recovered from GitHub, `be11209364609959de179d4f3932e292d6b2344c` is “Fix CommonCal scope validation during OAuth client registration,” authored 2026-10-09 18:35:03 UTC, committed 18:35:09 UTC, parent `33ccc5f029b2866fac0401ffc806a8f06ead69ae`. Its server.mjs line 218 includes both scope catalogs, line 430 rejects the OIDC interpretation, and defaultResource contains the missing-resource guard.

[Build run 37974317737](https://github.com/david-hajnal/calendar/actions/runs/37974317737) checks out be11209, builds/scans all three images and successfully publishes the auth SHA tag at 18:40:32 UTC. GHCR currently returns the same digest recorded by the push:

```
ghcr.io/david-hajnal/calendar-auth:sha-be11209364609959de179d4f3932e292d6b2344c
sha256:606ada431ecdab4bbc8c9b0960584e36296ccd1e8395fa30dfd1e8a080bbd342
```

The image's source layer is `sha256:5d6e9a6dba0f78c41ec67c7dee61337456d19f767ce7e4e8d4ebc22aad292608`. Downloaded blob SHA-256 was checked. Its `app/src/server.mjs` is byte-for-byte identical to the recovered commit file and includes the fix. This directly proves image contents, beyond trusting its tag.

## Promotion contradiction resolved

The same build log says `[main e29a162] chore(deploy): promote be11209...`, then `be11209..e29a162 HEAD -> main`. GitHub still retains [original promotion e29a162](https://github.com/david-hajnal/calendar/commit/e29a1624f4890f86f5662f7b78902ba34be7cd1c): parent be11209, bot author and committer at 18:40:42 UTC.

Current replacement b56e8df preserves the bot author/time and promotion message but has parent dbba096 and human committer david-hajnal at 18:50:51 UTC. Replacement fix dbba096 preserves be11209's author/time and parent but has a human commit time of 18:50:13 UTC. This proves history rewriting/replaying, option (c). It does not establish which exact Git command performed the rewrite. Local reflogs and unreachable-commit enumeration contain neither original commit; GitHub retains them. No `--lost-found` writes are necessary to enumerate unreachable objects.

The original workflow respected parent = source SHA. The rewritten promotion retained the original image tag, which remains valid and contains the fix. The mismatch is not evidence of a bad build.

## Deployment cause and remaining limit

`deploy/flux/overlays/production/kustomization.yaml` includes only core and MCP. Auth's production HelmRelease is excluded; promotion edits a file outside the applied resource graph. The cutover candidate is suspended. The local successful-commands record describes root Flux as paused pending persistence of working live values and records auth at pre-fix e239a1c. These explain how a fixed image can be published/promoted in Git without updating live auth.

The evidence supports an unapplied auth rollout as the cause. A definitive statement about the current running image requires the following cluster reads. If its imageID equals the verified digest, investigate ingress routing/another backend or cached discovery instead of rebuilding the same image. No live discovery re-fetch was performed during this investigation, per the supplied ground truth.

## Ordered fix and verification

Run cluster commands on the configured production operator host, with its existing KUBECONFIG. Do not expose Secrets.

1. Record current runtime/init image references, pod imageIDs, auth suspension and root Flux suspension:

```sh
kubectl get deployment commoncal-auth -n commoncal -o jsonpath='{.spec.template.spec.containers[*].image}{"\n"}{.spec.template.spec.initContainers[*].image}{"\n"}'
kubectl get pods -n commoncal -l app.kubernetes.io/name=commoncal-auth -o jsonpath='{range .items[*]}{.metadata.name}{" "}{.status.containerStatuses[*].imageID}{"\n"}{end}'
kubectl get helmrelease commoncal-auth -n flux-system -o jsonpath='{.spec.suspend}{" "}{.spec.values.image.repository}{":"}{.spec.values.image.tag}{"\n"}'
kubectl get kustomization flux-system -n flux-system -o jsonpath='{.spec.suspend}{"\n"}'
```

2. Select the **already published and directly verified be11209 image** above. No rebuild/retag is needed. Do not invent a sha-dbba096 tag: the rewritten hash has no demonstrated build. If policy requires a reachable source, publish a reviewed current main descendant containing dbba096 through the normal push-triggered workflow; verify all three image manifests and use its own full SHA tag. Never relabel the old image as a new source revision. Account for the workflow's core/MCP promotion before pushing.

3. Preserve working values, keys, PVC and rollback reference; take the documented encrypted backup. Keep root Flux paused while recovering the live release. Confirm the live image repository is calendar-auth, then patch only its tag. If auth is suspended, resume it deliberately before reconciliation:

```sh
kubectl patch helmrelease commoncal-auth -n flux-system --type=merge -p '{"spec":{"values":{"image":{"tag":"sha-be11209364609959de179d4f3932e292d6b2344c"}}}}'
# Only when the read above reports auth is suspended:
flux resume helmrelease commoncal-auth -n flux-system
flux reconcile helmrelease commoncal-auth -n flux-system --force --reset --with-source
kubectl rollout status deployment/commoncal-auth -n commoncal --timeout=10m
```

4. Repeat step 1 and require the running auth imageID to match `sha256:606ada431ecdab4bbc8c9b0960584e36296ccd1e8395fa30dfd1e8a080bbd342`. Verify migration/init references and readiness. Fetch discovery with cache bypass and assert all nine catalog scopes:

```sh
curl --fail --silent --show-error -H 'Cache-Control: no-cache' "https://auth.hajnal.space/.well-known/openid-configuration?verify=$(date +%s)" > /tmp/commoncal-discovery.json
python3 - <<'PY'
import json,re
from pathlib import Path
src=Path('slice1-lab/auth-server/src/server.mjs').read_text().split('const SCOPE_CATALOG = [',1)[1].split('];',1)[0]
expected=set(re.findall(r"['\"](commoncal\.[a-z.]+)['\"]",src))
actual=set(json.load(open('/tmp/commoncal-discovery.json'))['scopes_supported'])
assert len(expected)==9, expected
assert expected <= actual, sorted(expected-actual)
print('PASS: all nine commoncal scopes advertised')
PY
```

Run from the repository checkout. Verify `/ready`, retry the client's DCR login with `commoncal.calendar.metadata.read`, complete browser consent, then perform an authenticated read-only calendar_list and check selected-calendar isolation. Discovery alone is not an end-to-end MCP proof.

5. Persist the verified live values and chosen tag into the production auth HelmRelease, add `charts/auth-helmrelease.yaml` to the production kustomization resources, and reconcile core/MCP desired values with their working live state. Validate `kubectl kustomize deploy/flux/overlays/production` and review the manifest diff. Commit/push the reviewed desired state, allow the build/promotion workflow to finish, and check its final tag changes before resuming root Flux. Do not apply the stale cutover overlay wholesale.

6. Once Git matches the intended running stack:

```sh
flux reconcile source git flux-system -n flux-system
flux resume kustomization flux-system -n flux-system
flux reconcile kustomization flux-system -n flux-system --with-source
flux reconcile helmrelease commoncal-auth -n flux-system --with-source
kubectl rollout status deployment/commoncal-auth -n commoncal --timeout=10m
```

Repeat imageID, readiness, discovery and MCP checks after reconciliation. The GitRepository polls every minute; root Kustomization and HelmRelease intervals are ten minutes. Explicit reconciliation accelerates these checks. Auth will be managed by Flux only after inclusion in the applied resource graph and appropriate suspension states.

No production changes were performed. Current pod identity and end-to-end live verification remain blocked on configured cluster access.
