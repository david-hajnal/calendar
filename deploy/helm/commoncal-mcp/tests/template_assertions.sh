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
