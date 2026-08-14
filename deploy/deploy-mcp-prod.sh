#!/usr/bin/env bash
# Deploy commoncal-mcp to production with secrets from environment variables.
#
# Required env vars:
#   CALENDAR_API_URL  - URL of the commoncal API
#
# Optional env vars:
#   DOMAIN                      - Production domain (default: cal.hajnal.space)
#   IMAGE_TAG                   - Docker image tag (default: main)
#   TLS_SECRET_NAME             - TLS secret name (default: commoncal-tls)
#   HELM_RELEASE_NAME           - Helm release name (default: commoncal-mcp)
#   NAMESPACE                   - Kubernetes namespace (default: commoncal)
#   SERVER                      - Remote server IP for image loading
#   DRY_RUN                     - set to "1" for --dry-run

set -euo pipefail

# Load .env file if it exists
DEPLOY_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ -f "$DEPLOY_DIR/.env" ]]; then
  set -a
  source "$DEPLOY_DIR/.env"
  set +a
fi

# Fail fast if required env vars are missing
: "${CALENDAR_API_URL:?ERROR: CALENDAR_API_URL is required. Export it: export CALENDAR_API_URL=http://commoncal:3000/api}"

NAMESPACE="${NAMESPACE:-commoncal}"
RELEASE="${HELM_RELEASE_NAME:-commoncal-mcp}"
CHART_DIR="$DEPLOY_DIR/helm/commoncal-mcp"
VALUES_FILE="$DEPLOY_DIR/values-mcp-production.yaml"
DOMAIN="${DOMAIN:-cal.hajnal.space}"
IMAGE_TAG="${IMAGE_TAG:-main}"
TLS_SECRET_NAME="${TLS_SECRET_NAME:-commoncal-tls}"
SERVER="${SERVER:-}"
ROOT_DIR="$(cd "$DEPLOY_DIR/.." && pwd)"

case "${DRY_RUN:-0}" in
  0|"")
    kubectl_apply_args=(apply -f -)
    dry_run=0
    ;;
  1)
    kubectl_apply_args=(apply --dry-run=server -f -)
    dry_run=1
    ;;
  *)
    echo "ERROR: DRY_RUN must be either 0 or 1" >&2
    exit 1
    ;;
esac

for command_name in kubectl helm; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "ERROR: required command not found: $command_name" >&2
    exit 1
  fi
done

if [[ ! -f "$CHART_DIR/Chart.yaml" ]]; then
  echo "ERROR: Helm chart is missing under $DEPLOY_DIR" >&2
  exit 1
fi

# Build MCP Docker image
echo "==> Building MCP Docker image '$IMAGE_TAG'..."
docker build -f "$ROOT_DIR/mcp-server/Dockerfile" -t "calendar-mcp:$IMAGE_TAG" "$ROOT_DIR/mcp-server"
echo "==> Image built successfully"

# Deploy with Helm
echo "==> Deploying $RELEASE to $NAMESPACE..."
helm_args=(
  upgrade --install "$RELEASE" "$CHART_DIR"
  --namespace "$NAMESPACE"
  --set-string image.tag="$IMAGE_TAG"
  --set-string image.repository="commoncal/calendar-mcp"
  --set-string domain="$DOMAIN"
  --set-string "ingress.hosts[0].host=$DOMAIN"
  --set-string "ingress.tls[0].secretName=$TLS_SECRET_NAME"
  --set-string "ingress.tls[0].hosts[0]=$DOMAIN"
  --set-string "env.CALENDAR_API_URL=$CALENDAR_API_URL"
  --timeout=15m
)

if ((dry_run)); then
  helm_args+=(--dry-run)
fi

helm "${helm_args[@]}"

echo "==> Done. Release: $RELEASE, Namespace: $NAMESPACE"
