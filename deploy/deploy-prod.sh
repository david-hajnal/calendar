#!/usr/bin/env bash
# Deploy the CommonCal core and MCP stack with secrets from the environment.
#
# Required (loaded from deploy/.env when present):
#   SESSION_SECRET, BACKUP_ENCRYPTION_KEY_HEX
#   MCP_INTERNAL_API_KEY, MCP_SESSION_SECRET, MCP_DOMAIN, MCP_OAUTH_ISSUER
# Optional:
#   IMAGE_TAG (required only for direct Helm deployment; Flux uses Git's tags)
#   DOMAIN (default: cal.hajnal.space)
#   MCP_INTERNAL_API_BASE (default: https://$DOMAIN)
#   TLS_SECRET_NAME, CORE_HELM_RELEASE_NAME, MCP_HELM_RELEASE_NAME, NAMESPACE
#   GHCR_TOKEN (direct Helm mode only; rejected under Flux ownership), DRY_RUN=1

set -euo pipefail

DEPLOY_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ -f "$DEPLOY_DIR/.env" ]]; then
  source "$DEPLOY_DIR/.env"
fi

: "${SESSION_SECRET:?ERROR: SESSION_SECRET is required. Set it in $DEPLOY_DIR/.env or export it}"
: "${BACKUP_ENCRYPTION_KEY_HEX:?ERROR: BACKUP_ENCRYPTION_KEY_HEX is required. Set it in $DEPLOY_DIR/.env or export it}"
: "${MCP_INTERNAL_API_KEY:?ERROR: MCP_INTERNAL_API_KEY is required. Set it in $DEPLOY_DIR/.env or export it}"
: "${MCP_SESSION_SECRET:?ERROR: MCP_SESSION_SECRET is required. Set it in $DEPLOY_DIR/.env or export it}"
: "${MCP_DOMAIN:?ERROR: MCP_DOMAIN is required. Set it in $DEPLOY_DIR/.env or export it}"
: "${MCP_OAUTH_ISSUER:?ERROR: MCP_OAUTH_ISSUER is required and must be the HTTPS issuer exposing OAuth metadata/JWKS}"

if [[ ! "$BACKUP_ENCRYPTION_KEY_HEX" =~ ^([[:xdigit:]]{2}){16,}$ ]]; then
  echo "ERROR: BACKUP_ENCRYPTION_KEY_HEX must be an even number of hexadecimal characters (at least 32)" >&2
  exit 1
fi

NAMESPACE="${NAMESPACE:-commoncal}"
CORE_RELEASE="${CORE_HELM_RELEASE_NAME:-${HELM_RELEASE_NAME:-commoncal}}"
MCP_RELEASE="${MCP_HELM_RELEASE_NAME:-commoncal-mcp}"
CORE_CHART_DIR="$DEPLOY_DIR/helm/commoncal"
MCP_CHART_DIR="$DEPLOY_DIR/helm/commoncal-mcp"
CORE_VALUES_FILE="$DEPLOY_DIR/values-production.yaml"
MCP_VALUES_FILE="$DEPLOY_DIR/values-mcp-production.yaml"
DOMAIN="${DOMAIN:-cal.hajnal.space}"
MCP_INTERNAL_API_BASE="${MCP_INTERNAL_API_BASE:-https://$DOMAIN}"
TLS_SECRET_NAME="${TLS_SECRET_NAME:-commoncal-tls}"
GHCR_TOKEN="${GHCR_TOKEN:-}"

if [[ "$CORE_RELEASE" == "$MCP_RELEASE" ]]; then
  echo "ERROR: core and MCP Helm release names must be distinct" >&2
  exit 1
fi
for https_value in "$MCP_OAUTH_ISSUER" "$MCP_INTERNAL_API_BASE"; do
  if [[ "$https_value" != https://* ]]; then
    echo "ERROR: MCP_OAUTH_ISSUER and MCP_INTERNAL_API_BASE must use HTTPS in production" >&2
    exit 1
  fi
done

case "${DRY_RUN:-0}" in
  0|"") kubectl_apply_args=(apply -f -); dry_run=0 ;;
  1) kubectl_apply_args=(apply --dry-run=server -f -); dry_run=1 ;;
  *) echo "ERROR: DRY_RUN must be either 0 or 1" >&2; exit 1 ;;
esac

for command_name in kubectl; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "ERROR: required command not found: $command_name" >&2
    exit 1
  fi
done
for required_file in \
  "$CORE_CHART_DIR/Chart.yaml" "$MCP_CHART_DIR/Chart.yaml" \
  "$CORE_VALUES_FILE" "$MCP_VALUES_FILE"; do
  if [[ ! -f "$required_file" ]]; then
    echo "ERROR: required deployment file is missing: $required_file" >&2
    exit 1
  fi
done

: "${KUBECONFIG:?ERROR: KUBECONFIG is not set. Export it or run from the k3s host.}"

# Scratch space for TLS certificate material. Never contains contents that are
# logged; the trap removes it on any exit path (success or failure). TLS_WORKDIR
# may be pre-set (for example by tests) to observe cleanup; it defaults to a
# private mktemp directory.
TLS_WORKDIR="${TLS_WORKDIR:-$(mktemp -d)}"
trap 'rm -rf "$TLS_WORKDIR"' EXIT HUP INT TERM

# Validate an existing TLS Secret for reuse. Checks, in order:
#   1. Secret type is kubernetes.io/tls.
#   2. tls.crt and tls.key are present and decode.
#   3. The certificate parses.
#   4. Both configured domains are covered (openssl x509 -checkhost).
#   5. The private key matches the certificate (public-key comparison).
#   6. At least 30 days (2592000s) of validity remain (openssl x509 -checkend).
# Returns nonzero with a specific message on the first failed check.
validate_tls_secret() {
  local secret_name="$1"
  local ns="$2"

  local secret_type
  secret_type=$(kubectl get secret "$secret_name" -n "$ns" -o jsonpath='{.type}' 2>/dev/null || true)
  if [[ "$secret_type" != "kubernetes.io/tls" ]]; then
    echo "ERROR: secret '$ns/$secret_name' has type '${secret_type:-<missing>}', expected kubernetes.io/tls." >&2
    return 1
  fi

  local crt="$TLS_WORKDIR/validate.crt"
  local key="$TLS_WORKDIR/validate.key"
  if ! kubectl get secret "$secret_name" -n "$ns" -o jsonpath='{.data.tls\.crt}' 2>/dev/null | openssl base64 -d -A >"$crt"; then
    echo "ERROR: could not read tls.crt from '$ns/$secret_name'." >&2
    return 1
  fi
  if ! kubectl get secret "$secret_name" -n "$ns" -o jsonpath='{.data.tls\.key}' 2>/dev/null | openssl base64 -d -A >"$key"; then
    echo "ERROR: could not read tls.key from '$ns/$secret_name'." >&2
    return 1
  fi

  if ! openssl x509 -in "$crt" -noout -text >/dev/null 2>&1; then
    echo "ERROR: '$ns/$secret_name' does not contain a valid TLS certificate." >&2
    return 1
  fi

  local host
  for host in "$DOMAIN" "$MCP_DOMAIN"; do
    if ! openssl x509 -in "$crt" -noout -checkhost "$host" >/dev/null 2>&1; then
      echo "ERROR: existing TLS secret '$ns/$secret_name' does not cover '$host'." >&2
      echo "Reissue it with SANs for both '$DOMAIN' and '$MCP_DOMAIN', or choose a new TLS_SECRET_NAME; it was not overwritten." >&2
      return 1
    fi
  done

  local cert_pub key_pub
  cert_pub=$(openssl x509 -in "$crt" -noout -pubkey 2>/dev/null || true)
  key_pub=$(openssl pkey -in "$key" -pubout 2>/dev/null || true)
  if [[ -z "$cert_pub" || -z "$key_pub" || "$cert_pub" != "$key_pub" ]]; then
    echo "ERROR: the private key in '$ns/$secret_name' does not match its certificate." >&2
    return 1
  fi

  if ! openssl x509 -in "$crt" -noout -checkend 2592000 >/dev/null 2>&1; then
    echo "ERROR: the certificate in '$ns/$secret_name' expires within 30 days." >&2
    return 1
  fi

  return 0
}

# Generate a self-signed certificate (RSA 2048, SHA-256, 365 days) whose SANs
# cover both production domains, then create the TLS Secret. The subject CN is
# DOMAIN; the SAN extension is the identity source of truth. Temporary key,
# certificate, and config live in TLS_WORKDIR and are removed by the EXIT trap;
# no certificate or key bytes are ever printed to stdout/stderr.
generate_self_signed_secret() {
  local secret_name="$1"
  local ns="$2"

  local config="$TLS_WORKDIR/openssl.cnf"
  local keyout="$TLS_WORKDIR/tls.key"
  local certout="$TLS_WORKDIR/tls.crt"

  {
    printf '%s\n' \
      '[req]' \
      'distinguished_name = req_dn' \
      'x509_extensions = v3_req' \
      'prompt = no' \
      '' \
      '[req_dn]' \
      "CN = $DOMAIN" \
      '' \
      '[v3_req]' \
      "subjectAltName = DNS:$DOMAIN,DNS:$MCP_DOMAIN"
  } >"$config"

  openssl req -x509 -newkey rsa:2048 -sha256 -days 365 -nodes \
    -keyout "$keyout" -out "$certout" -config "$config"

  kubectl create secret tls "$secret_name" \
    --cert="$certout" --key="$keyout" \
    -n "$ns" --dry-run=client -o yaml | kubectl "${kubectl_apply_args[@]}"
}

# Ensure the shared TLS Secret exists and is valid. On first run it generates a
# self-signed certificate; on later runs it validates and reuses the existing
# Secret. An existing Secret that is invalid or near expiry aborts the
# deployment with manual rotation steps and is never overwritten.
ensure_tls_secret() {
  local secret_name="$TLS_SECRET_NAME"
  local ns="$NAMESPACE"

  if kubectl get secret "$secret_name" -n "$ns" >/dev/null 2>&1; then
    if validate_tls_secret "$secret_name" "$ns"; then
      echo "    Existing TLS certificate '$ns/$secret_name' is valid; reusing it."
      return 0
    fi
    {
      echo "ERROR: TLS secret '$ns/$secret_name' is invalid or expiring within 30 days." >&2
      echo "It was not overwritten. To rotate it manually:" >&2
      echo "  1. Generate a certificate covering '$DOMAIN' and '$MCP_DOMAIN'." >&2
      echo "  2. kubectl delete secret $secret_name -n $ns" >&2
      echo "  3. kubectl create secret tls $secret_name --cert=<fullchain.pem> --key=<privkey.pem> -n $ns" >&2
      echo "  4. Rerun the deployment." >&2
    }
    exit 1
  fi

  echo "==> Generating self-signed TLS certificate for '$DOMAIN' and '$MCP_DOMAIN'..."
  generate_self_signed_secret "$secret_name" "$ns"
  echo "    Created TLS secret '$ns/$secret_name'."
}

active_flux_releases=0
for flux_release in commoncal commoncal-mcp; do
  flux_status=$(kubectl get helmrelease "$flux_release" --namespace flux-system \
    -o jsonpath='{.metadata.name}{"\t"}{.spec.suspend}' 2>/dev/null || true)
  if [[ "$flux_status" == "$flux_release" || "$flux_status" == "$flux_release"$'\t'* ]]; then
    if [[ "$flux_status" != "$flux_release"$'\ttrue' ]]; then
      active_flux_releases=$((active_flux_releases + 1))
    fi
  fi
done

case "$active_flux_releases" in
  2)
    deploy_mode=flux
    if ! command -v flux >/dev/null 2>&1; then
      echo "ERROR: active Flux HelmReleases manage production, but the flux command is not installed" >&2
      exit 1
    fi
    if [[ "$NAMESPACE" != commoncal || "$CORE_RELEASE" != commoncal || "$MCP_RELEASE" != commoncal-mcp ]]; then
      echo "ERROR: Flux manages namespace 'commoncal' with releases 'commoncal' and 'commoncal-mcp'." >&2
      echo "Remove NAMESPACE, HELM_RELEASE_NAME, CORE_HELM_RELEASE_NAME, and MCP_HELM_RELEASE_NAME overrides for Flux deployment." >&2
      exit 1
    fi
    if [[ -n "$GHCR_TOKEN" ]]; then
      echo "ERROR: GHCR_TOKEN is ignored under Flux ownership: Flux pulls images with its own ImageRepository credentials." >&2
      echo "Remove GHCR_TOKEN from '$DEPLOY_DIR/.env', or add the pull Secret to the Flux HelmReleases in Git." >&2
      exit 1
    fi
    if ! command -v openssl >/dev/null 2>&1; then
      echo "ERROR: openssl is required to manage the self-signed origin TLS Secret" >&2
      exit 1
    fi
    ;;
  0)
    deploy_mode=helm
    : "${IMAGE_TAG:?ERROR: IMAGE_TAG is required for direct Helm deployment. Set it in $DEPLOY_DIR/.env or export it}"
    for command_name in helm openssl; do
      if ! command -v "$command_name" >/dev/null 2>&1; then
        echo "ERROR: required command not found: $command_name" >&2
        exit 1
      fi
    done
    ;;
  *)
    echo "ERROR: production has mixed deployment ownership: only $active_flux_releases of 2 Flux HelmReleases are active." >&2
    echo "Resume both HelmReleases for Flux deployment, or suspend both for direct Helm deployment." >&2
    exit 1
    ;;
esac

CTX=$(kubectl config current-context 2>/dev/null) || CTX="(none)"
echo "==> Current kubectl context: $CTX"

echo "==> Ensuring namespace '$NAMESPACE' exists..."
if ! kubectl get namespace "$NAMESPACE" >/dev/null 2>&1; then
  if ((dry_run)); then
    kubectl create namespace "$NAMESPACE" --dry-run=client -o yaml | kubectl "${kubectl_apply_args[@]}"
  else
    kubectl create namespace "$NAMESPACE"
  fi
fi

echo "==> Ensuring TLS secret '$NAMESPACE/$TLS_SECRET_NAME'..."
ensure_tls_secret

echo "==> Applying core secret '$NAMESPACE/commoncal-session'..."
kubectl create secret generic commoncal-session \
  --from-literal=SESSION_SECRET="$SESSION_SECRET" \
  --from-literal=BACKUP_ENCRYPTION_KEY_HEX="$BACKUP_ENCRYPTION_KEY_HEX" \
  -n "$NAMESPACE" --dry-run=client -o yaml | kubectl "${kubectl_apply_args[@]}"

echo "==> Applying MCP secret '$NAMESPACE/commoncal-mcp-secrets'..."
kubectl create secret generic commoncal-mcp-secrets \
  --from-literal=mcp-internal-api-key="$MCP_INTERNAL_API_KEY" \
  --from-literal=mcp-session-secret="$MCP_SESSION_SECRET" \
  --from-literal=mcp-oauth-issuer="$MCP_OAUTH_ISSUER" \
  -n "$NAMESPACE" --dry-run=client -o yaml | kubectl "${kubectl_apply_args[@]}"

if [[ "$deploy_mode" == flux ]]; then
  if ((dry_run)); then
    echo "==> Flux owns production; dry-run validated the runtime secrets without reconciling."
    echo "    A real run will deploy the image tags and chart values committed to Flux's Git source."
    exit 0
  fi

  echo "==> Flux owns production; reconciling Git-managed releases (IMAGE_TAG and direct Helm values are ignored)..."
  flux reconcile kustomization flux-system --namespace flux-system --with-source
  flux reconcile helmrelease commoncal --namespace flux-system --with-source
  flux reconcile helmrelease commoncal-mcp --namespace flux-system --with-source

  flux_core_domain=$(kubectl get helmrelease commoncal --namespace flux-system \
    -o jsonpath='{.spec.values.domain}')
  flux_mcp_domain=$(kubectl get helmrelease commoncal-mcp --namespace flux-system \
    -o jsonpath='{.spec.values.domain}')
  flux_tls_secret=$(kubectl get helmrelease commoncal --namespace flux-system \
    -o jsonpath='{.spec.values.ingress.tls[0].secretName}')
  flux_mcp_tls_secret=$(kubectl get helmrelease commoncal-mcp --namespace flux-system \
    -o jsonpath='{.spec.values.ingress.tls[0].secretName}')
  if [[ -z "$flux_core_domain" || -z "$flux_mcp_domain" || -z "$flux_tls_secret" ]]; then
    echo "ERROR: reconciled Flux HelmReleases are missing domain or TLS secret values" >&2
    exit 1
  fi
  if [[ "$flux_mcp_tls_secret" != "$flux_tls_secret" ]]; then
    echo "ERROR: Flux core and MCP ingresses must reference the same TLS Secret" >&2
    exit 1
  fi
  if ! validate_tls_secret "$flux_tls_secret" "$NAMESPACE"; then
    echo "ERROR: the shared TLS Secret '$NAMESPACE/$flux_tls_secret' referenced by the Flux HelmReleases is invalid." >&2
    exit 1
  fi
  echo "    Shared TLS Secret '$NAMESPACE/$flux_tls_secret' is valid."

  # Secret contents are external to the HelmRelease pod templates. Restart the
  # workloads so rotated credentials take effect even when the chart is unchanged.
  kubectl rollout restart statefulset "$CORE_RELEASE" --namespace "$NAMESPACE"
  kubectl rollout restart deployment "$MCP_RELEASE" --namespace "$NAMESPACE"
  kubectl rollout status statefulset "$CORE_RELEASE" --namespace "$NAMESPACE" --timeout=15m
  kubectl rollout status deployment "$MCP_RELEASE" --namespace "$NAMESPACE" --timeout=15m

  core_image=$(kubectl get statefulset "$CORE_RELEASE" --namespace "$NAMESPACE" \
    -o jsonpath='{.spec.template.spec.containers[0].image}' 2>/dev/null || true)
  mcp_image=$(kubectl get deployment "$MCP_RELEASE" --namespace "$NAMESPACE" \
    -o jsonpath='{.spec.template.spec.containers[0].image}' 2>/dev/null || true)
  echo "==> Done through Flux. Core: ${core_image:-$CORE_RELEASE}, MCP: ${mcp_image:-$MCP_RELEASE}, Namespace: $NAMESPACE"
  exit 0
fi

core_helm_args=(
  upgrade --install "$CORE_RELEASE" "$CORE_CHART_DIR"
  --namespace "$NAMESPACE" --reset-values --values "$CORE_VALUES_FILE"
  --set-string fullnameOverride="$CORE_RELEASE"
  --set-string image.tag="$IMAGE_TAG"
  --set-string domain="$DOMAIN"
  --set-string config.appOrigin="https://$DOMAIN"
  --set-string "ingress.hosts[0].host=$DOMAIN"
  --set-string "ingress.hosts[0].paths[0].path=/"
  --set-string "ingress.tls[0].secretName=$TLS_SECRET_NAME"
  --set-string "ingress.tls[0].hosts[0]=$DOMAIN"
  --set-string "ingress.tls[0].hosts[1]=$MCP_DOMAIN"
  --set-string existingSecret.name=commoncal-session
  --set-string mcpInternalApiSecret.name=commoncal-mcp-secrets
  --set-string mcpInternalApiSecret.key=mcp-internal-api-key
  --timeout=15m
)
mcp_helm_args=(
  upgrade --install "$MCP_RELEASE" "$MCP_CHART_DIR"
  --namespace "$NAMESPACE" --reset-values --values "$MCP_VALUES_FILE"
  --set-string fullnameOverride="$MCP_RELEASE"
  --set-string image.tag="$IMAGE_TAG"
  --set-string domain="$MCP_DOMAIN"
  --set-string "ingress.tls[0].secretName=$TLS_SECRET_NAME"
  --set-string "ingress.tls[0].hosts[0]=$MCP_DOMAIN"
  --set-string existingSecret.name=commoncal-mcp-secrets
  --set-string existingSecret.apiKeyKeyName=mcp-internal-api-key
  --set-string existingSecret.sessionSecretKeyName=mcp-session-secret
  --set-string existingSecret.oauthIssuerKeyName=mcp-oauth-issuer
  --set-string "env.MCP_DOMAIN=$MCP_DOMAIN"
  --set-string "env.MCP_INTERNAL_API_BASE=$MCP_INTERNAL_API_BASE"
  --set-string "env.MCP_PUBLIC_RESOURCE_URL=https://$MCP_DOMAIN/mcp"
  --timeout=15m
)

if [[ -n "$GHCR_TOKEN" ]]; then
  echo "==> Applying GHCR image pull secret..."
  kubectl create secret docker-registry commoncal-ghcr-creds \
    --docker-server=https://ghcr.io --docker-username=_token \
    --docker-password="$GHCR_TOKEN" --docker-email="" \
    -n "$NAMESPACE" --dry-run=client -o yaml | kubectl "${kubectl_apply_args[@]}"
  core_helm_args+=(--set-string 'imagePullSecrets[0].name=commoncal-ghcr-creds')
  mcp_helm_args+=(--set-string 'imagePullSecrets[0].name=commoncal-ghcr-creds')
fi
if ((dry_run)); then
  core_helm_args+=(--dry-run)
  mcp_helm_args+=(--dry-run)
fi

echo "==> Deploying core release '$CORE_RELEASE'..."
helm "${core_helm_args[@]}"
echo "==> Deploying MCP release '$MCP_RELEASE'..."
helm "${mcp_helm_args[@]}"

if ((!dry_run)); then
  kubectl rollout status statefulset "$CORE_RELEASE" --namespace "$NAMESPACE" --timeout=15m
  kubectl rollout status deployment "$MCP_RELEASE" --namespace "$NAMESPACE" --timeout=15m
fi

echo "==> Done. Core: $CORE_RELEASE, MCP: $MCP_RELEASE, Namespace: $NAMESPACE"
