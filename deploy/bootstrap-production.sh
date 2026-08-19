#!/usr/bin/env bash
set -euo pipefail

# Bootstrap production cluster with secrets and Flux.
#
# Required env vars:
#   SESSION_SECRET              - session encryption key
#   BACKUP_ENCRYPTION_KEY_HEX   - hex-encoded backup encryption key (even hex, at least 32 chars; 64-hex backward-compatible)
#   MCP_INTERNAL_API_KEY        - MCP internal API key
#   MCP_SESSION_SECRET          - MCP session secret
#   GITHUB_TOKEN                - GitHub PAT with repo write access
#   DOMAIN                      - core domain (default: cal.hajnal.space)
#   MCP_DOMAIN                  - MCP domain (default: mcal.hajnal.space)
#   NAMESPACE                   - k8s namespace (default: commoncal)
#   FLUX_OWNER                  - GitHub owner (default: david-hajnal)
#   FLUX_REPO                   - GitHub repo name (default: calendar)
#
# Usage:
#   SESSION_SECRET=xxx BACKUP_ENCRYPTION_KEY_HEX=xxx MCP_INTERNAL_API_KEY=xxx MCP_SESSION_SECRET=xxx GITHUB_TOKEN=xxx ./deploy/bootstrap-production.sh

NAMESPACE="${NAMESPACE:-commoncal}"
DOMAIN="${DOMAIN:-cal.hajnal.space}"
MCP_DOMAIN="${MCP_DOMAIN:-mcal.hajnal.space}"
FLUX_OWNER="${FLUX_OWNER:-david-hajnal}"
FLUX_REPO="${FLUX_REPO:-calendar}"
TLS_SECRET_NAME="${TLS_SECRET_NAME:-commoncal-tls}"

echo "==> Validating required environment variables..."
for var in SESSION_SECRET BACKUP_ENCRYPTION_KEY_HEX MCP_INTERNAL_API_KEY MCP_SESSION_SECRET GITHUB_TOKEN; do
  if [[ -z "${!var:-}" ]]; then
    echo "ERROR: $var is required" >&2
    exit 1
  fi
done

if [[ ! "$BACKUP_ENCRYPTION_KEY_HEX" =~ ^([[:xdigit:]]{2}){16,}$ ]]; then
  echo "ERROR: BACKUP_ENCRYPTION_KEY_HEX must be an even number of hexadecimal characters (at least 32)" >&2
  exit 1
fi

echo "==> Ensuring namespace '$NAMESPACE' exists..."
kubectl create namespace "$NAMESPACE" --dry-run=client -o yaml | kubectl apply -f -

echo "==> Creating core secret '$NAMESPACE/commoncal-session'..."
kubectl create secret generic commoncal-session \
  --from-literal=SESSION_SECRET="$SESSION_SECRET" \
  --from-literal=BACKUP_ENCRYPTION_KEY_HEX="$BACKUP_ENCRYPTION_KEY_HEX" \
  -n "$NAMESPACE" \
  --dry-run=client -o yaml | kubectl apply -f -

echo "==> Creating MCP secret '$NAMESPACE/commoncal-mcp-secrets'..."
kubectl create secret generic commoncal-mcp-secrets \
  --from-literal=mcp-internal-api-key="$MCP_INTERNAL_API_KEY" \
  --from-literal=mcp-session-secret="$MCP_SESSION_SECRET" \
  -n "$NAMESPACE" \
  --dry-run=client -o yaml | kubectl apply -f -

echo "==> Bootstrapping Flux (path: deploy/flux/overlays/production)..."
flux bootstrap github \
  --owner="$FLUX_OWNER" \
  --repository="$FLUX_REPO" \
  --namespace=flux-system \
  --personal-access-token="$GITHUB_TOKEN" \
  --path=deploy/flux/overlays/production

echo "==> Waiting for Flux reconciliation..."
sleep 5
flux reconcile kustomization flux-system --namespace=flux-system

echo "==> Verifying resources..."
echo "HelmRelease status:"
flux get helmreleases -A
echo ""
echo "Kustomization status:"
flux get kustomizations -A
echo ""
echo "Workloads:"
kubectl get statefulset,deployment,ingress -n "$NAMESPACE"
echo ""
echo "==> Bootstrap complete."
echo "    Namespace: $NAMESPACE"
echo "    Core domain: $DOMAIN"
echo "    MCP domain: $MCP_DOMAIN"
echo "    TLS secret: $TLS_SECRET_NAME"
