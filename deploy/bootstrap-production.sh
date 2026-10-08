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
#   AUTH_DATABASE_URL           - PostgreSQL DSN for the authorization server (slice 5)
#   AUTH_BRIDGE_KEY             - shared secret for the private bridge (slice 5)
#   AUTH_COOKIE_KEYS            - comma-separated distinct cookie signing keys (slice 5)
#   AUTH_SIGNING_KID            - JWKS key ID for the authorization server (slice 5)
#   GITHUB_TOKEN                - GitHub PAT with repo write access
#   DOMAIN                      - core domain (default: cal.hajnal.space)
#   MCP_DOMAIN                  - MCP domain (default: mcal.hajnal.space)
#   NAMESPACE                   - k8s namespace (default: commoncal)
#   FLUX_OWNER                  - GitHub owner (default: david-hajnal)
#   FLUX_REPO                   - GitHub repo name (default: calendar)
#   FLUX_VERSION                - Flux toolkit version (default: v2.9.4)
#
# Optional env vars:
#   MCP_OAUTH_ISSUER_HOLD       - new issuer to HOLD (accept for validation)
#                                 without cutover (slice 5). If set, the MCP
#                                 server accepts tokens from both the primary
#                                 and held issuers.
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
for var in SESSION_SECRET BACKUP_ENCRYPTION_KEY_HEX MCP_INTERNAL_API_KEY MCP_SESSION_SECRET MCP_OAUTH_ISSUER AUTH_DATABASE_URL AUTH_BRIDGE_KEY AUTH_COOKIE_KEYS AUTH_SIGNING_KID GITHUB_TOKEN; do
  if [[ -z "${!var:-}" ]]; then
    echo "ERROR: $var is required" >&2
    exit 1
  fi
done

if [[ ! "$BACKUP_ENCRYPTION_KEY_HEX" =~ ^([[:xdigit:]]{2}){16,}$ ]]; then
  echo "ERROR: BACKUP_ENCRYPTION_KEY_HEX must be an even number of hexadecimal characters (at least 32)" >&2
  exit 1
fi

# Match the production runtime's comma-separated cookie-key rotation set.
if ! AUTH_COOKIE_KEYS="$AUTH_COOKIE_KEYS" python3 - <<'KEYS'
import os, sys
keys = [key.strip() for key in os.environ['AUTH_COOKIE_KEYS'].split(',')]
sys.exit(0 if len(keys) >= 2 and len(set(keys)) == len(keys) and all(len(key) >= 32 for key in keys) else 1)
KEYS
then
  echo "ERROR: AUTH_COOKIE_KEYS requires at least two distinct keys of 32+ characters" >&2
  exit 1
fi
: "${AUTH_JWKS_FILE:?AUTH_JWKS_FILE must point to the private production JWKS}"
[[ -r "$AUTH_JWKS_FILE" ]] || { echo 'ERROR: AUTH_JWKS_FILE is not readable' >&2; exit 1; }

# AUTH_DATABASE_URL must be a PostgreSQL DSN.
if [[ ! "$AUTH_DATABASE_URL" =~ ^postgres(ql)?:// ]]; then
  echo "ERROR: AUTH_DATABASE_URL must be a PostgreSQL DSN (postgres:// or postgresql://)" >&2
  exit 1
fi

echo "==> Ensuring namespace '$NAMESPACE' exists..."
kubectl create namespace "$NAMESPACE" --dry-run=client -o yaml | kubectl apply -f -

source "$(dirname "$ENV_FILE")/core-secret.sh"
core_secret_create_args=()
ensure_core_secret

echo "==> Creating MCP secret '$NAMESPACE/commoncal-mcp-secrets'..."
# MCP_OAUTH_ISSUER_HOLD is optional: when set, the MCP server holds the new
# issuer (accepts tokens from it) without making it primary (no cutover).
MCP_SECRET_ARGS=(
  --from-literal=mcp-internal-api-key="$MCP_INTERNAL_API_KEY"
  --from-literal=mcp-session-secret="$MCP_SESSION_SECRET"
  --from-literal=mcp-oauth-issuer="$MCP_OAUTH_ISSUER"
)
if [[ -n "${MCP_OAUTH_ISSUER_HOLD:-}" ]]; then
  MCP_SECRET_ARGS+=(--from-literal=mcp-oauth-issuer-hold="$MCP_OAUTH_ISSUER_HOLD")
fi
kubectl create secret generic commoncal-mcp-secrets \
  "${MCP_SECRET_ARGS[@]}" \
  -n "$NAMESPACE" \
  --dry-run=client -o yaml | kubectl apply -f -

echo "==> Creating auth secret '$NAMESPACE/commoncal-auth-secrets'..."
# The authorization server's secrets. The chart never creates this Secret;
# it is created here out-of-band and referenced by the Helm chart.
source "$(dirname "${BASH_SOURCE[0]}")/auth-secret.sh"
apply_auth_secret "$NAMESPACE" apply -f -

echo "==> Bootstrapping Flux (path: deploy/flux/overlays/production)..."
# flux v2.9.4 reads the PAT from the GITHUB_TOKEN env var (loaded above) and
# uses it when --token-auth is set (instead of an SSH deploy key).
flux bootstrap github \
  --owner="$FLUX_OWNER" \
  --repository="$FLUX_REPO" \
  --namespace=flux-system \
  --token-auth \
  --path=deploy/flux/overlays/production \
  --version="$FLUX_VERSION"

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
