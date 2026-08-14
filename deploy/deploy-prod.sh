#!/usr/bin/env bash
# Deploy commoncal to production with secrets from environment variables.
#
# Required env vars:
#   SESSION_SECRET              - encryption key for sessions
#   BACKUP_ENCRYPTION_KEY_HEX   - hex-encoded backup encryption key
#
# Optional env vars:
#   DOMAIN                      - Production domain (default: cal.hajnal.space)
#   MCP_DOMAIN                  - MCP subdomain (default: same as DOMAIN)
#   IMAGE_TAG                   - Docker image tag (default: latest)
#   TLS_SECRET_NAME             - TLS secret name (default: commoncal-tls)
#   HELM_RELEASE_NAME           - Helm release name (default: commoncal)
#   NAMESPACE                   - Kubernetes namespace (default: production)
#   DRY_RUN                     - set to "1" for --dry-run

set -euo pipefail

NAMESPACE="${NAMESPACE:-production}"
RELEASE="${HELM_RELEASE_NAME:-commoncal}"
CHART_DIR="./deploy/helm/commoncal"
DOMAIN="${DOMAIN:-cal.hajnal.space}"
MCP_DOMAIN="${MCP_DOMAIN:-$DOMAIN}"
IMAGE_TAG="${IMAGE_TAG:-latest}"
TLS_SECRET_NAME="${TLS_SECRET_NAME:-commoncal-tls}"

# Validate required secrets
if [ -z "${SESSION_SECRET:-}" ]; then
  echo "ERROR: SESSION_SECRET is not set" >&2
  echo "Export it: export SESSION_SECRET=\$(openssl rand -hex 32)" >&2
  exit 1
fi

if [ -z "${BACKUP_ENCRYPTION_KEY_HEX:-}" ]; then
  echo "ERROR: BACKUP_ENCRYPTION_KEY_HEX is not set" >&2
  echo "Export it: export BACKUP_ENCRYPTION_KEY_HEX=\$(openssl rand -hex 32)" >&2
  exit 1
fi

# Create or update the secret
echo "==> Ensuring secret '$NAMESPACE/commoncal-session' exists..."
kubectl create secret generic commoncal-session \
  --from-literal=SESSION_SECRET="$SESSION_SECRET" \
  --from-literal=BACKUP_ENCRYPTION_KEY_HEX="$BACKUP_ENCRYPTION_KEY_HEX" \
  -n "$NAMESPACE" \
  --dry-run=client -o yaml | kubectl apply -f -

# Deploy with Helm
echo "==> Deploying $RELEASE to $NAMESPACE..."
helm upgrade --install "$RELEASE" "$CHART_DIR" \
  --namespace "$NAMESPACE" \
  --set image.tag="$IMAGE_TAG" \
  --set domain="$DOMAIN" \
  --set config.appOrigin="https://$DOMAIN" \
  --set existingSecret.name=commoncal-session \
  --wait \
  --timeout=10m \
  "${DRY_RUN:+--dry-run}"

echo "==> Done. Release: $RELEASE, Namespace: $NAMESPACE"
