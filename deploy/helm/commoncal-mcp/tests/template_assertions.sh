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
  require_source "$chart_dir/templates/deployment.yaml" 'type: Recreate'
  require_source "$chart_dir/templates/deployment.yaml" 'persistentVolumeClaim:'
  require_source "$chart_dir/templates/deployment.yaml" 'claimName: {{ include "commoncal-mcp.fullname" . }}-data'
  require_source "$chart_dir/templates/deployment.yaml" 'mountPath: /app/data'
  require_source "$chart_dir/templates/deployment.yaml" 'containerPort: 3001'
  require_source "$chart_dir/templates/deployment.yaml" 'mountPath: /app/tmp'
  exit 0
fi

helm template commoncal-mcp "$chart_dir" > "$rendered"

grep -q 'name: MCP_DATABASE_PATH' "$rendered"
grep -q 'value: "/app/data/mcp-server.db"' "$rendered"
grep -q 'type: Recreate' "$rendered"
grep -q 'kind: PersistentVolumeClaim' "$rendered"
grep -q 'helm.sh/resource-policy: keep' "$rendered"
grep -q 'claimName: commoncal-mcp-data' "$rendered"
grep -q 'mountPath: /app/data' "$rendered"
grep -q 'containerPort: 3001' "$rendered"
grep -q 'mountPath: /app/tmp' "$rendered"
grep -q 'host: "cal.hajnal.space"' "$rendered"
grep -q 'path: "/mcp"' "$rendered"

if helm template commoncal-mcp "$chart_dir" --set replicaCount=2 >/dev/null 2>&1; then
  echo 'replicaCount=2 should be rejected for the SQLite deployment' >&2
  exit 1
fi
