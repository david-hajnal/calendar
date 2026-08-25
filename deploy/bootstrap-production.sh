#!/usr/bin/env bash
set -euo pipefail

# Bootstrap production cluster with secrets and Flux.
#
# Required env vars:
#   SESSION_SECRET              - session encryption key
#   BACKUP_ENCRYPTION_KEY_HEX   - hex-encoded backup encryption key (even hex, at least 32 chars; 64-hex backward-compatible)
#   MCP_INTERNAL_API_KEY        - MCP internal API key
#   MCP_SESSION_SECRET          - MCP session secret
#   MCP_OAUTH_ISSUER            - HTTPS OAuth issuer exposing the JWKS endpoint
#   GITHUB_TOKEN                - GitHub PAT with repo write access
#   DOMAIN                      - core domain (default: cal.hajnal.space)
#   MCP_DOMAIN                  - MCP domain (default: mcal.hajnal.space)
#   NAMESPACE                   - k8s namespace (default: commoncal)
#   FLUX_OWNER                  - GitHub owner (default: david-hajnal)
#   FLUX_REPO                   - GitHub repo name (default: calendar)
#   FLUX_VERSION                - Flux toolkit version (default: v2.9.4)
#
# Values may be provided as environment variables or in deploy/.env (gitignored).
# Variables already set in the environment take precedence over the file.
#
# Usage:
#   ./deploy/bootstrap-production.sh
#   (or export the vars above / put them in deploy/.env)

# Load deploy/.env if present. Values already set in the environment win.
ENV_FILE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/.env"
if [[ -f "$ENV_FILE" ]]; then
  echo "==> Loading environment from $ENV_FILE"
  while IFS= read -r line || [[ -n "$line" ]]; do
    [[ -z "${line// }" ]] && continue
    [[ "$line" =~ ^[[:space:]]*# ]] && continue
    [[ "$line" =~ ^[[:space:]]*(export[[:space:]]+)?([A-Za-z_][A-Za-z0-9_]*)=(.*)$ ]] || continue
    key="${BASH_REMATCH[2]}"
    value="${BASH_REMATCH[3]}"
    if [[ "$value" =~ ^\"(.*)\"$ ]]; then value="${BASH_REMATCH[1]}"; fi
    if [[ "$value" =~ ^\'(.*)\'$ ]]; then value="${BASH_REMATCH[1]}"; fi
    if [[ -z "${!key:-}" ]]; then
      export "$key=$value"
    fi
  done < "$ENV_FILE"
fi

NAMESPACE="${NAMESPACE:-commoncal}"
DOMAIN="${DOMAIN:-cal.hajnal.space}"
MCP_DOMAIN="${MCP_DOMAIN:-mcal.hajnal.space}"
FLUX_OWNER="${FLUX_OWNER:-david-hajnal}"
FLUX_REPO="${FLUX_REPO:-calendar}"
FLUX_VERSION="${FLUX_VERSION:-v2.9.4}"
TLS_SECRET_NAME="${TLS_SECRET_NAME:-commoncal-tls}"

echo "==> Validating required environment variables..."
for var in SESSION_SECRET BACKUP_ENCRYPTION_KEY_HEX MCP_INTERNAL_API_KEY MCP_SESSION_SECRET MCP_OAUTH_ISSUER GITHUB_TOKEN; do
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
  --from-literal=mcp-oauth-issuer="$MCP_OAUTH_ISSUER" \
  -n "$NAMESPACE" \
  --dry-run=client -o yaml | kubectl apply -f -

echo "==> Bootstrapping Flux (path: deploy/flux/overlays/production)..."
flux bootstrap github \
  --owner="$FLUX_OWNER" \
  --repository="$FLUX_REPO" \
  --namespace=flux-system \
  --personal-access-token="$GITHUB_TOKEN" \
  --path=deploy/flux/overlays/production \
  --version="$FLUX_VERSION" \
  --components-extra=image-reflector-controller,image-automation-controller

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
