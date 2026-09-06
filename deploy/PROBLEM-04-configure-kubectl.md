# Problem: Configure kubectl/kubeconfig on Production Server

## Goal
Set up kubectl access on the production server so deployment scripts and Flux reconciliation can manage Kubernetes resources.

## Current State

### kubectl Access
- **Status:** kubectl not configured on production server
- **Error in deploy script:** `deploy-prod.sh:89` — `KUBECONFIG` env var is required and not set
- **Production server:** k3s host (single-node Kubernetes)
- **Cluster:** Managed by Flux in `flux-system` namespace

### Deployment Modes
The deploy script (`deploy-prod.sh`) supports two modes:

1. **Flux mode** (preferred) — when all 3 HelmReleases are active in Flux
   - Uses `flux` CLI to reconcile
   - Requires `KUBECONFIG` to point to the cluster
   - Images managed via Git commits to Flux source

2. **Direct Helm mode** — when all 3 HelmReleases are suspended
   - Uses `helm upgrade --install` directly
   - Requires `IMAGE_TAG` env var with `sha-<40 hex>` tag
   - Also requires `KUBECONFIG`

**Both modes require kubectl access.** The script checks for kubectl on line 74-79.

### What kubectl Needs
- **KUBECONFIG:** Path to kubeconfig file with cluster credentials
- **kubectl binary:** Available in PATH
- **Current context:** Points to the correct cluster
- **flux CLI** (for Flux mode): Available in PATH

## What Needs to Be Done

### 1. Obtain kubeconfig from k3s host
On the k3s production server:
```bash
# For k3s (default location)
cat /etc/rancher/k3s/k3s.yaml

# Or for the k3s token-based approach
cat /var/lib/rancher/k3s/server/token
cat /etc/rancher/k3s/k3s.yaml
```

### 2. Configure local kubeconfig
Copy the kubeconfig to the local machine:
```bash
# Option A: Copy to default location
scp root@<production-server>:/etc/rancher/k3s/k3s.yaml ~/.kube/config

# Option B: Copy to custom location and set KUBECONFIG
scp root@<production-server>:/etc/rancher/k3s/k3s.yaml ~/k3s-prod.yaml
export KUBECONFIG=~/k3s-prod.yaml
```

### 3. Update kubeconfig server URL
The k3s kubeconfig may reference `127.0.0.1:6443`. Update to the server's reachable IP/hostname:
```bash
# Check current server
kubectl config current-cluster
kubectl config view -o jsonpath='{.clusters[0].cluster.server}'

# Update if needed
kubectl config set-cluster <cluster-name> --server=https://<server-ip>:6443
```

### 4. Verify access
```bash
kubectl get nodes
kubectl get pods -n flux-system
kubectl get helmrelease -n flux-system
```

### 5. Install flux CLI (if not present)
```bash
# Check if installed
flux --version

# Install if needed (macOS)
brew install fluxcd/tap/flux

# Or follow official docs
# https://fluxcd.io/flux/setup/
```

### 6. Update deploy script if needed
- The script requires `KUBECONFIG` env var (line 89)
- Consider adding a fallback to use `~/.kube/config` if KUBECONFIG is not set
- Or document that KUBECONFIG must be exported before running

## Key Constraints

1. **Security:** kubeconfig contains cluster credentials. Store securely, never commit to git.
2. **Network:** Production server must be reachable from the machine running kubectl.
3. **TLS:** k3s server may use self-signed cert. May need `insecure-skip-tls-verify: true` or CA cert.
4. **Both modes need kubectl:** Whether using Flux or direct Helm, kubectl is required.

## Related Files

| File | Purpose |
|------|---------|
| `deploy/deploy-prod.sh:89` | KUBECONFIG requirement check |
| `deploy/deploy-prod.sh:74-79` | kubectl binary check |
| `deploy/deploy-prod.sh:233-281` | Deployment mode detection (Flux vs Helm) |
| `deploy/deploy-prod.sh:329-383` | Flux mode deployment path |
| `deploy/deploy-prod.sh:386-481` | Direct Helm mode deployment path |

## Verification

After configuration:
1. `kubectl get nodes` — should show the k3s node
2. `kubectl get ns commoncal` — should show the commoncal namespace
3. `flux --version` — should show flux CLI version
4. `deploy/deploy-prod.sh --dry-run=1` — should run without KUBECONFIG errors
