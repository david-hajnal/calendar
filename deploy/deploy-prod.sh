#!/usr/bin/env bash
# Deploy commoncal to production with secrets from environment variables.
#
# Required env vars (loaded from deploy/.env when present):
#   SESSION_SECRET              - encryption key for sessions
#   BACKUP_ENCRYPTION_KEY_HEX   - hex-encoded backup encryption key
#   IMAGE_TAG                   - Published container image tag
#
# Optional env vars:
#   DOMAIN                      - Production domain (default: cal.hajnal.space)
#   TLS_SECRET_NAME             - TLS secret name (default: commoncal-tls)
#   HELM_RELEASE_NAME           - Helm release name (default: commoncal)
#   NAMESPACE                   - Kubernetes namespace (default: production)
#   DRY_RUN                     - set to "1" for --dry-run

set -euo pipefail

DEPLOY_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Load .env file if it exists
if [[ -f "$DEPLOY_DIR/.env" ]]; then
  set -a
  source "$DEPLOY_DIR/.env"
  set +a
fi

# Fail fast if required env vars are missing
: "${SESSION_SECRET:?ERROR: SESSION_SECRET is required. Set it in $DEPLOY_DIR/.env or export it}"
: "${BACKUP_ENCRYPTION_KEY_HEX:?ERROR: BACKUP_ENCRYPTION_KEY_HEX is required. Set it in $DEPLOY_DIR/.env or export it}"
: "${IMAGE_TAG:?ERROR: IMAGE_TAG is required. Set it in $DEPLOY_DIR/.env or export it}"

if [[ ! "$BACKUP_ENCRYPTION_KEY_HEX" =~ ^[[:xdigit:]]{64}$ ]]; then
  echo "ERROR: BACKUP_ENCRYPTION_KEY_HEX must contain exactly 64 hexadecimal characters" >&2
  exit 1
fi

NAMESPACE="${NAMESPACE:-production}"
RELEASE="${HELM_RELEASE_NAME:-commoncal}"
CHART_DIR="$DEPLOY_DIR/helm/commoncal"
VALUES_FILE="$DEPLOY_DIR/values-production.yaml"
DOMAIN="${DOMAIN:-cal.hajnal.space}"
TLS_SECRET_NAME="${TLS_SECRET_NAME:-commoncal-tls}"

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

if [[ ! -f "$CHART_DIR/Chart.yaml" || ! -f "$VALUES_FILE" ]]; then
  echo "ERROR: Helm chart or production values file is missing under $DEPLOY_DIR" >&2
  exit 1
fi

echo "==> Ensuring secret '$NAMESPACE/commoncal-session' exists..."
kubectl create secret generic commoncal-session \
  --from-literal=SESSION_SECRET="$SESSION_SECRET" \
  --from-literal=BACKUP_ENCRYPTION_KEY_HEX="$BACKUP_ENCRYPTION_KEY_HEX" \
  -n "$NAMESPACE" \
  --dry-run=client -o yaml | kubectl "${kubectl_apply_args[@]}"

# Deploy with Helm
echo "==> Deploying $RELEASE to $NAMESPACE..."
helm_args=(
  upgrade --install "$RELEASE" "$CHART_DIR"
  --namespace "$NAMESPACE"
  --reset-values
  --values "$VALUES_FILE"
  --set-string image.tag="$IMAGE_TAG"
  --set-string domain="$DOMAIN"
  --set-string config.appOrigin="https://$DOMAIN"
  --set-string "ingress.hosts[0].host=$DOMAIN"
  --set-string "ingress.tls[0].secretName=$TLS_SECRET_NAME"
  --set-string "ingress.tls[0].hosts[0]=$DOMAIN"
  --set-string existingSecret.name=commoncal-session
  --timeout=15m
)

if ((dry_run)); then
  helm_args+=(--dry-run)
fi

helm "${helm_args[@]}"

if ((!dry_run)); then
  echo "==> Waiting for the $RELEASE StatefulSet rollout..."
  kubectl rollout status statefulset \
    --selector "app.kubernetes.io/instance=$RELEASE" \
    --namespace "$NAMESPACE" \
    --timeout=15m
fi

echo "==> Done. Release: $RELEASE, Namespace: $NAMESPACE"
