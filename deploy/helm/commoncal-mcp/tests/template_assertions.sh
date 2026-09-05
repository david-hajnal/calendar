#!/usr/bin/env sh
set -eu

chart_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
rendered=$(mktemp)
trap 'rm -f "$rendered"' EXIT

require_source() {
  if ! grep -F -q -- "$2" "$1"; then
    echo "missing required MCP chart configuration: $2" >&2
    exit 1
  fi
}

if ! command -v helm >/dev/null 2>&1; then
  echo 'helm is not installed; checking MCP chart source' >&2

  require_source "$chart_dir/values.schema.json" '"const": 1'
  require_source "$chart_dir/values.yaml" 'MCP_DATABASE_PATH: /app/data/mcp-server.db'
  require_source "$chart_dir/templates/pvc.yaml" 'kind: PersistentVolumeClaim'
  require_source "$chart_dir/templates/pvc.yaml" 'helm.sh/resource-policy: keep'
  require_source "$chart_dir/templates/networkpolicy.yaml" 'cidr: 0.0.0.0/0'
  require_source "$chart_dir/templates/networkpolicy.yaml" '10.0.0.0/8'
  require_source "$chart_dir/templates/networkpolicy.yaml" '172.16.0.0/12'
  require_source "$chart_dir/templates/networkpolicy.yaml" '192.168.0.0/16'
  require_source "$chart_dir/templates/deployment.yaml" 'type: Recreate'
  require_source "$chart_dir/templates/deployment.yaml" 'persistentVolumeClaim:'
  require_source "$chart_dir/templates/deployment.yaml" 'claimName: {{ include "commoncal-mcp.fullname" . }}-data'
  require_source "$chart_dir/templates/deployment.yaml" '.Values.existingSecret.apiKeyKeyName'
  require_source "$chart_dir/templates/ingress.yaml" '{{- range .paths }}'
  require_source "$chart_dir/templates/deployment.yaml" 'mountPath: /app/data'
  require_source "$chart_dir/templates/deployment.yaml" 'containerPort: 3001'
  require_source "$chart_dir/templates/deployment.yaml" 'name: MCP_OAUTH_ISSUER'
  require_source "$chart_dir/templates/deployment.yaml" '.Values.existingSecret.oauthIssuerKeyName'
  require_source "$chart_dir/templates/deployment.yaml" 'mountPath: /app/tmp'
  exit 0
fi

helm template commoncal-mcp "$chart_dir" > "$rendered"

grep -q 'name: MCP_DATABASE_PATH' "$rendered"
grep -q 'value: "/app/data/mcp-server.db"' "$rendered"
grep -q 'type: Recreate' "$rendered"
grep -q 'kind: PersistentVolumeClaim' "$rendered"
grep -q 'helm.sh/resource-policy: keep' "$rendered"
grep -q 'kind: NetworkPolicy' "$rendered"
grep -q 'cidr: 0.0.0.0/0' "$rendered"
grep -q '10.0.0.0/8' "$rendered"
grep -q '172.16.0.0/12' "$rendered"
grep -q '192.168.0.0/16' "$rendered"
grep -q 'claimName: commoncal-mcp-data' "$rendered"
grep -q 'mountPath: /app/data' "$rendered"
grep -q 'containerPort: 3001' "$rendered"
grep -q 'name: MCP_OAUTH_ISSUER' "$rendered"
grep -q 'mountPath: /app/tmp' "$rendered"
grep -q 'name: MCP_INTERNAL_API_KEY' "$rendered"
grep -q 'key: mcp-internal-api-key' "$rendered"
grep -q 'host: "mcp.example.com"' "$rendered"
grep -q 'path: "/mcp"' "$rendered"
grep -q 'path: "/.well-known/oauth-protected-resource"' "$rendered"
grep -q 'name: BIND_ADDRESS' "$rendered"
grep -q 'value: "0.0.0.0:3001"' "$rendered"
grep -q 'httpGet:' "$rendered"
grep -q 'path: /health/ready' "$rendered"
grep -q 'path: /health/live' "$rendered"

if helm template commoncal-mcp "$chart_dir" --set replicaCount=2 >/dev/null 2>&1; then
  echo 'replicaCount=2 should be rejected for the SQLite deployment' >&2
  exit 1
fi

# Production TLS assertions: render with the production ingress values and
# verify the Ingress references the shared TLS Secret and uses websecure.
prod_values=$(mktemp)
prod_rendered=$(mktemp)
trap 'rm -f "$rendered" "$prod_values" "$prod_rendered"' EXIT
cat > "$prod_values" <<'VALUES'
ingress:
  annotations:
    traefik.ingress.kubernetes.io/router.entrypoints: websecure
  tls:
    - secretName: commoncal-tls
      hosts:
        - mcal.example.test
VALUES
helm template commoncal-mcp "$chart_dir" -f "$prod_values" > "$prod_rendered"

grep -F -q 'secretName: "commoncal-tls"' "$prod_rendered" || {
  echo 'production MCP Ingress must reference the commoncal-tls Secret' >&2
  exit 1
}
grep -F -q 'traefik.ingress.kubernetes.io/router.entrypoints: websecure' "$prod_rendered" || {
  echo 'production MCP Ingress must use the websecure entrypoint' >&2
  exit 1
}
if grep -F -q 'cert-manager.io/cluster-issuer' "$prod_rendered"; then
  echo 'production MCP Ingress must not carry a cert-manager cluster-issuer annotation' >&2
  exit 1
fi

# --- Hold-issuer assertions (slice 5) --------------------------------------
# The MCP server can HOLD a new issuer (accept for validation) without
# cutover: the primary MCP_OAUTH_ISSUER is unchanged, and the held issuer is
# injected by Secret reference only (no value rendered).
hold_rendered=$(mktemp)
trap 'rm -f "$rendered" "$prod_values" "$prod_rendered" "$hold_rendered"' EXIT
helm template commoncal-mcp "$chart_dir" \
  --set existingSecret.oauthIssuerHoldKeyName=mcp-oauth-issuer-hold \
  > "$hold_rendered"

# Held issuer must be a Secret reference.
grep -q 'name: MCP_OAUTH_ISSUER_HOLD' "$hold_rendered"
if ! awk '
  /- name: MCP_OAUTH_ISSUER_HOLD/ { in_hold=1; next }
  in_hold && /valueFrom:/ { has_reference=1 }
  in_hold && /^[[:space:]]+- name:/ { in_hold=0 }
  END { exit has_reference ? 0 : 1 }
' "$hold_rendered"; then
  echo 'MCP_OAUTH_ISSUER_HOLD must be supplied by a Secret reference' >&2
  exit 1
fi

# Primary issuer must still be present and unchanged (no cutover).
grep -q 'name: MCP_OAUTH_ISSUER' "$hold_rendered"
grep -q 'key: mcp-oauth-issuer' "$hold_rendered"

# No secret value may be rendered into the issuer wiring.
if grep -vE '^[[:space:]]*#' "$hold_rendered" | grep -qiE \
  'oidc-lab-only|https://[^ ]*issuer'; then
  echo 'issuer wiring must not render secret values' >&2
  exit 1
fi

# Default (no hold key) must not render the hold env var.
default_rendered=$(mktemp)
trap 'rm -f "$rendered" "$prod_values" "$prod_rendered" "$hold_rendered" "$default_rendered"' EXIT
helm template commoncal-mcp "$chart_dir" > "$default_rendered"
if grep -q 'name: MCP_OAUTH_ISSUER_HOLD' "$default_rendered"; then
  echo 'MCP_OAUTH_ISSUER_HOLD must not be rendered when no hold key is set' >&2
  exit 1
fi

echo 'commoncal-mcp chart assertions passed'
