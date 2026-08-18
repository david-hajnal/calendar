#!/usr/bin/env bash
# Deploy commoncal to production with secrets from environment variables.
#
# Required env vars (loaded from deploy/.env when present):
#   SESSION_SECRET              - encryption key for sessions
#   BACKUP_ENCRYPTION_KEY_HEX   - hex-encoded backup encryption key
#   IMAGE_TAG                   - Published container image tag
#
# Optional env vars:
#   CORE_DOMAIN / DOMAIN        - Production core domain (default: cal.hajnal.space)
#   TLS_SECRET_NAME             - TLS secret name (default: commoncal-tls)
#   HELM_RELEASE_NAME           - Helm release name (default: commoncal)
#   NAMESPACE                   - Kubernetes namespace (default: commoncal)
#   DRY_RUN                     - set to "1" for --dry-run

set -euo pipefail

DEPLOY_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Load .env file if it exists
if [[ -f "$DEPLOY_DIR/.env" ]]; then
  source "$DEPLOY_DIR/.env"
fi

# Fail fast if required env vars are missing
: "${SESSION_SECRET:?ERROR: SESSION_SECRET is required. Set it in $DEPLOY_DIR/.env or export it}"
: "${BACKUP_ENCRYPTION_KEY_HEX:?ERROR: BACKUP_ENCRYPTION_KEY_HEX is required. Set it in $DEPLOY_DIR/.env or export it}"
: "${IMAGE_TAG:?ERROR: IMAGE_TAG is required. Set it in $DEPLOY_DIR/.env or export it}"

# Explicitly export only the vars we need (avoid leaking debug flags etc.)
export SESSION_SECRET BACKUP_ENCRYPTION_KEY_HEX IMAGE_TAG GHCR_TOKEN

if [[ ! "$BACKUP_ENCRYPTION_KEY_HEX" =~ ^[[:xdigit:]]{32,}$ ]]; then
  echo "ERROR: BACKUP_ENCRYPTION_KEY_HEX must contain at least 32 hexadecimal characters" >&2
  exit 1
fi

NAMESPACE="${NAMESPACE:-commoncal}"
RELEASE="${HELM_RELEASE_NAME:-commoncal}"
CHART_DIR="$DEPLOY_DIR/helm/commoncal"
VALUES_FILE="$DEPLOY_DIR/values-production.yaml"
DOMAIN="${DOMAIN:-cal.hajnal.space}"
TLS_SECRET_NAME="${TLS_SECRET_NAME:-commoncal-tls}"
GHCR_TOKEN="${GHCR_TOKEN:-}"

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

# Use explicit KUBECONFIG only — do not auto-detect k3s (causes hangs when unreachable)
: "${KUBECONFIG:?ERROR: KUBECONFIG is not set. Export it or run from the k3s host.}"

# Verify kubectl context before deploying
CTX=$(kubectl config current-context 2>/dev/null) || CTX="(none)"
echo "==> Current kubectl context: $CTX"

# Ensure namespace exists
echo "==> Ensuring namespace '$NAMESPACE' exists..."
kubectl get namespace "$NAMESPACE" >/dev/null 2>&1 || kubectl create namespace "$NAMESPACE"

echo "==> Ensuring secret '$NAMESPACE/commoncal-session' exists..."
SECRET_TMPFILE=$(mktemp)
trap 'rm -f "$SECRET_TMPFILE"' EXIT
kubectl create secret generic commoncal-session \
  --from-literal=SESSION_SECRET="$SESSION_SECRET" \
  --from-literal=BACKUP_ENCRYPTION_KEY_HEX="$BACKUP_ENCRYPTION_KEY_HEX" \
  -n "$NAMESPACE" \
  --dry-run=client -o yaml > "$SECRET_TMPFILE"
 kubectl apply -f "$SECRET_TMPFILE"

 # Ensure the TLS secret exists. A self-signed cert is sufficient behind
 # Cloudflare (Full mode) because Cloudflare terminates TLS at the edge and
 # does not validate the origin cert. Only create it when missing so a real
 # cert-manager-issued cert is never clobbered on a later deploy.
 echo "==> Ensuring TLS secret '$NAMESPACE/$TLS_SECRET_NAME' exists..."
 if ! kubectl get secret "$TLS_SECRET_NAME" -n "$NAMESPACE" >/dev/null 2>&1; then
   if ! command -v openssl >/dev/null 2>&1; then
     echo "ERROR: openssl is required to generate the self-signed TLS cert" >&2
     exit 1
   fi
   echo "    '$TLS_SECRET_NAME' not found — generating self-signed cert for $DOMAIN..."
   TLS_TMPDIR=$(mktemp -d)
    cat > "$TLS_TMPDIR/openssl.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions = v3
prompt = no
[dn]
CN = $DOMAIN
[v3]
subjectAltName = DNS:$DOMAIN
EOF
   openssl req -x509 -nodes -newkey rsa:2048 -days 3650 \
     -config "$TLS_TMPDIR/openssl.cnf" \
     -keyout "$TLS_TMPDIR/tls.key" \
     -out "$TLS_TMPDIR/tls.crt" >/dev/null
   kubectl create secret tls "$TLS_SECRET_NAME" \
     --cert="$TLS_TMPDIR/tls.crt" \
     --key="$TLS_TMPDIR/tls.key" \
     -n "$NAMESPACE" \
     --dry-run=client -o yaml | kubectl apply -f -
   rm -rf "$TLS_TMPDIR"
   echo "    Created self-signed TLS secret '$TLS_SECRET_NAME'."
 else
   echo "    TLS secret '$TLS_SECRET_NAME' already exists — leaving it untouched."
 fi

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
  --set-string "ingress.hosts[0].host=$DOMAIN" \
  --set-string "ingress.hosts[0].paths[0].path=/"
  --set-string "ingress.tls[0].secretName=$TLS_SECRET_NAME"
  --set-string "ingress.tls[0].hosts[0]=$DOMAIN"
  --set-string existingSecret.name=commoncal-session
  --timeout=15m
)

# Configure imagePullSecrets for GHCR if token is provided
if [[ -n "$GHCR_TOKEN" ]]; then
  echo "==> Ensuring GHCR imagePullSecret 'commoncal-ghcr-creds' exists..."
  kubectl create secret docker-registry commoncal-ghcr-creds \
    --docker-server=https://ghcr.io \
    --docker-username=_token \
    --docker-password="$GHCR_TOKEN" \
    --docker-email="" \
    -n "$NAMESPACE" \
    --dry-run=client -o yaml | kubectl apply -f -
  helm_args+=(
    --set-string "imagePullSecrets[0].name=commoncal-ghcr-creds"
  )
fi

if ((dry_run)); then
  helm_args+=(--dry-run)
fi

helm "${helm_args[@]}"

if ((!dry_run)); then
  echo "==> Waiting for the $RELEASE StatefulSet rollout..."
  kubectl rollout status statefulset "$RELEASE" \
    --namespace "$NAMESPACE" \
    --timeout=15m

  # Post-deploy health check: verify pods are Ready
  echo "==> Checking pod readiness..."
  TOTAL=$(kubectl get pods --selector "app.kubernetes.io/instance=$RELEASE" \
    --namespace "$NAMESPACE" \
    --no-headers 2>/dev/null | wc -l | tr -d ' ')
  READY=$(kubectl get pods --selector "app.kubernetes.io/instance=$RELEASE" \
    --namespace "$NAMESPACE" \
    -o jsonpath='{range .items[*]}{.metadata.name}{"\t"}{.status.conditions[?(@.type=="Ready")].status}{"\n"}{end}' 2>/dev/null | grep -c "Ready$" || true)
  if (( READY < TOTAL )); then
    echo "WARNING: Only $READY/$TOTAL pods are Ready" >&2
    kubectl get pods --selector "app.kubernetes.io/instance=$RELEASE" \
      --namespace "$NAMESPACE" -o wide
  else
    echo "==> All $TOTAL pods are Ready"
  fi
fi

echo "==> Done. Release: $RELEASE, Namespace: $NAMESPACE"
