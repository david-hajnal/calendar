# Deployment Guide

## Overview

Core and MCP are deployed to Kubernetes via Flux GitOps. Images are published to GHCR and automatically promoted via Flux image automation.

## Architecture

```
Git push (v* tag)
  → CI builds image
  → Immutable tag pushed to GHCR
  → Flux ImageRepository detects new tag
  → Flux ImagePolicy selects newest semver
  → Flux ImageUpdateAutomation commits tag to HelmRelease
  → Flux Kustomization reconciles
  → HelmRelease upgrades release
  → Kubernetes rolls out new pods
```

## Namespace

Both applications deploy to the `commoncal` namespace.

## Images

- Core: `ghcr.io/david-hajnal/calendar-core`
- MCP: `ghcr.io/david-hajnal/calendar-mcp`

Tags follow semver: `v1.0.0`, `v1.0.1`, etc.

## Manual Deployment (Pinned)

To deploy a specific version:

1. Edit `deploy/flux/overlays/production/charts/core-helmrelease.yaml`:
   ```yaml
   image:
     tag: "v1.0.0"  # change to desired version
   ```

2. Edit `deploy/flux/overlays/production/charts/mcp-helmrelease.yaml`:
   ```yaml
   image:
     tag: "v1.0.0"  # change to desired version
   ```

3. Commit and push to `main`.

4. Flux will reconcile within 10 minutes.

## Suspend Image Automation

To pause automatic image promotion during an incident:

```bash
# Suspend the ImageUpdateAutomation
kubectl annotate imageupdateautomation image-update-core \
  fluxcd.io/suspend=true --namespace=flux-system

# Resume
kubectl annotate imageupdateautomation image-update-core \
  fluxcd.io/suspend=true --overwrite --namespace=flux-system \
  -f -  # or remove the annotation
kubectl delete annotation imageupdateautomation/image-update-core \
  fluxcd.io/suspend --namespace=flux-system
```

## Reconcile Resources

```bash
# Reconcile all Flux resources
flux reconcile kustomization flux-system --namespace=flux-system

# Reconcile specific HelmRelease
flux reconcile helmrelease commoncal-core --namespace=flux-system
flux reconcile helmrelease commoncal-mcp --namespace=flux-system

# Reconcile image policy
flux reconcile imagepolicy image-policy-core --namespace=flux-system
flux reconcile imagepolicy image-policy-mcp --namespace=flux-system
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

2. Revert the commit:
   ```bash
   git revert <commit-hash>
   git push
   ```

3. Flux will reconcile and roll back to the previous version.

## GHCR Credentials

Images are published to `ghcr.io/david-hajnal/`. The repo must be public for Flux ImageRepository to pull without credentials. If private, create a Secret:

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

- `commoncal-session` — session encryption (key: `SESSION_SECRET`)
- `commoncal-tls` — TLS certificate (cert-manager)

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
```

## Validation

Run local validation before pushing:

```bash
sh scripts/validate-deploy.sh
```

This checks:
- Helm lint
- Helm template rendering
- YAML syntax
- No mutable tags (latest/main) in production
- Flux setter comments present

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
