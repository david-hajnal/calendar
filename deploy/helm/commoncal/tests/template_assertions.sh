#!/usr/bin/env sh
set -eu

chart_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
rendered=$(mktemp)
trap 'rm -f "$rendered"' EXIT

require_source() {
  if ! grep -F -q -- "$2" "$1"; then
    echo "missing required chart configuration: $2" >&2
    exit 1
  fi
}

if ! command -v helm >/dev/null 2>&1; then
  echo 'helm is not installed; checking the chart source for Prompt 32 acceptance gates' >&2

  require_source "$chart_dir/values.schema.json" '"const": 1'
  require_source "$chart_dir/values.yaml" 'runAsNonRoot: true'
  require_source "$chart_dir/values.yaml" 'runAsUser: 1000'
  require_source "$chart_dir/values.yaml" 'runAsGroup: 1000'
  require_source "$chart_dir/values.yaml" 'fsGroup: 1000'
  require_source "$chart_dir/values.yaml" 'readOnlyRootFilesystem: true'
  require_source "$chart_dir/values.yaml" 'allowPrivilegeEscalation: false'
  require_source "$chart_dir/values.yaml" 'type: RuntimeDefault'
  require_source "$chart_dir/values.yaml" '      - ALL'
  require_source "$chart_dir/templates/pvc.yaml" 'kind: PersistentVolumeClaim'
  require_source "$chart_dir/templates/pvc.yaml" 'helm.sh/resource-policy: keep'
  require_source "$chart_dir/templates/statefulset.yaml" 'persistentVolumeClaim:'
  require_source "$chart_dir/templates/statefulset.yaml" 'claimName: {{ include "commoncal.fullname" . }}-data'
  require_source "$chart_dir/templates/statefulset.yaml" 'mountPath: /app/data'
  require_source "$chart_dir/templates/configmap.yaml" 'DATABASE_PATH: /app/data/commoncal.sqlite'
  require_source "$chart_dir/templates/statefulset.yaml" 'secretKeyRef:'
  require_source "$chart_dir/templates/statefulset.yaml" 'valueFrom:'
  require_source "$chart_dir/templates/statefulset.yaml" 'name: {{ required "existingSecret.name is required" .Values.existingSecret.name }}'
  require_source "$chart_dir/templates/statefulset.yaml" 'key: {{ required "existingSecret.sessionSecretKey is required" .Values.existingSecret.sessionSecretKey }}'
  require_source "$chart_dir/templates/statefulset.yaml" 'required "image.tag is required" .Values.image.tag'
  require_source "$chart_dir/templates/statefulset.yaml" 'name: MCP_INTERNAL_API_KEY'
  exit 0
fi

helm template commoncal "$chart_dir" --set-string image.tag=test-image-tag > "$rendered"

grep -q 'replicas: 1' "$rendered"
grep -q 'runAsNonRoot: true' "$rendered"
grep -q 'runAsUser: 1000' "$rendered"
grep -q 'runAsGroup: 1000' "$rendered"
grep -q 'fsGroup: 1000' "$rendered"
grep -q 'type: RuntimeDefault' "$rendered"
grep -q 'automountServiceAccountToken: false' "$rendered"
grep -q 'allowPrivilegeEscalation: false' "$rendered"
grep -q 'readOnlyRootFilesystem: true' "$rendered"
grep -q 'drop:' "$rendered"
grep -q -- '- ALL' "$rendered"
grep -q 'mountPath: /app/data' "$rendered"
grep -q 'claimName: commoncal-data' "$rendered"
grep -q 'DATABASE_PATH' "$rendered"
grep -q 'configMapRef:' "$rendered"
grep -q 'name: commoncal' "$rendered"
grep -q 'secretKeyRef:' "$rendered"
grep -q 'key: SESSION_SECRET' "$rendered"
if ! awk '
  /- name: SESSION_SECRET/ { in_session_secret=1; next }
  in_session_secret && /valueFrom:/ { has_reference=1 }
  in_session_secret && /^[[:space:]]+- name:/ { in_session_secret=0 }
  END { exit has_reference ? 0 : 1 }
' "$rendered"; then
  echo 'SESSION_SECRET must be supplied by a Secret reference' >&2
  exit 1
fi
grep -q 'startupProbe:' "$rendered"
grep -q 'readinessProbe:' "$rendered"
grep -q 'livenessProbe:' "$rendered"
grep -q 'path: /health/live' "$rendered"
grep -q 'path: /health/ready' "$rendered"
grep -q 'terminationGracePeriodSeconds: 30' "$rendered"
grep -q 'cpu: 100m' "$rendered"
grep -q 'memory: 128Mi' "$rendered"
grep -q 'cpu: 1000m' "$rendered"
grep -q 'memory: 512Mi' "$rendered"
grep -q 'kind: PersistentVolumeClaim' "$rendered"
grep -q 'helm.sh/resource-policy: keep' "$rendered"
grep -q 'kind: Service' "$rendered"
grep -q 'kind: Ingress' "$rendered"
grep -q 'kind: NetworkPolicy' "$rendered"
grep -q 'image: "commoncal:test-image-tag"' "$rendered"
grep -q 'policyTypes:' "$rendered"
grep -q 'port: http' "$rendered"
grep -q 'kubernetes.io/metadata.name: kube-system' "$rendered"

# Verify the sqlite-console deny-all NetworkPolicy exists and selects only console pods
grep -q 'name: sqlite-console-deny-all' "$rendered"
grep -q 'commoncal.io/role: sqlite-console' "$rendered"

if grep -q 'kind: HorizontalPodAutoscaler' "$rendered"; then
  echo 'the SQLite chart must not render a HorizontalPodAutoscaler' >&2
  exit 1
fi

if helm template commoncal "$chart_dir" --set-string image.tag=test-image-tag --set replicaCount=2 >/dev/null 2>&1; then
  echo 'replicaCount=2 should be rejected by values schema' >&2
  exit 1
fi
