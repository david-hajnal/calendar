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
python3 "$chart_dir/../../../scripts/validate-yaml.py" "$rendered"

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

# Production TLS assertions: render with the production ingress values and
# verify the Ingress references the shared TLS Secret and uses websecure.
prod_values=$(mktemp)
prod_rendered=$(mktemp)
trap 'rm -f "$rendered" "$prod_values" "$prod_rendered"' EXIT
cat > "$prod_values" <<'VALUES'
image:
  tag: test-image-tag
ingress:
  annotations:
    traefik.ingress.kubernetes.io/router.entrypoints: websecure
  tls:
    - secretName: commoncal-tls
      hosts:
        - cal.example.test
        - mcal.example.test
VALUES
helm template commoncal "$chart_dir" -f "$prod_values" > "$prod_rendered"

grep -F -q 'secretName: commoncal-tls' "$prod_rendered" || {
  echo 'production Ingress must reference the commoncal-tls Secret' >&2
  exit 1
}
grep -F -q 'traefik.ingress.kubernetes.io/router.entrypoints: websecure' "$prod_rendered" || {
  echo 'production Ingress must use the websecure entrypoint' >&2
  exit 1
}
if grep -F -q 'cert-manager.io/cluster-issuer' "$prod_rendered"; then
  echo 'production Ingress must not carry a cert-manager cluster-issuer annotation' >&2
  exit 1
fi

# --- Authorization bridge assertions (slice 5) -----------------------------
# Render with the bridge enabled and verify the wiring is correct: the bridge
# key comes from a Secret reference (no value rendered), the bridge URL is
# non-secret config, and the NetworkPolicy egress permits the bridge.
bridge_rendered=$(mktemp)
trap 'rm -f "$rendered" "$prod_values" "$prod_rendered" "$bridge_rendered"' EXIT
helm template commoncal "$chart_dir" \
  --set image.tag=test-image-tag \
  --set config.appOrigin=https://cal.example.test \
  --set authBridge.enabled=true \
  --set authBridge.url=http://commoncal-auth-internal.commoncal.svc:80 \
  --set authBridge.secretName=commoncal-auth-secrets \
  --set authBridge.secretKey=LAB_BRIDGE_KEY \
  > "$bridge_rendered"

# Bridge key must be a Secret reference.
grep -q 'name: AUTH_BRIDGE_KEY' "$bridge_rendered"
if ! awk '
  /- name: AUTH_BRIDGE_KEY/ { in_bridge=1; next }
  in_bridge && /valueFrom:/ { has_reference=1 }
  in_bridge && /^[[:space:]]+- name:/ { in_bridge=0 }
  END { exit has_reference ? 0 : 1 }
' "$bridge_rendered"; then
  echo 'AUTH_BRIDGE_KEY must be supplied by a Secret reference' >&2
  exit 1
fi

# Bridge URL must be non-secret config.
grep -q 'AUTH_BRIDGE_URL: "http://commoncal-auth-internal.commoncal.svc:80"' "$bridge_rendered"

# NetworkPolicy egress must permit the bridge (component: authorization).
grep -q 'app.kubernetes.io/component: authorization' "$bridge_rendered"

# No secret value may be rendered into the bridge wiring.
if grep -vE '^[[:space:]]*#' "$bridge_rendered" | grep -qiE \
  'slice1-loopback-bridge|oidc-lab-only'; then
  echo 'bridge wiring must not render secret values' >&2
  exit 1
fi

# Default (bridge disabled) must not render the bridge env var.
default_rendered=$(mktemp)
trap 'rm -f "$rendered" "$prod_values" "$prod_rendered" "$bridge_rendered" "$default_rendered"' EXIT
helm template commoncal "$chart_dir" \
  --set image.tag=test-image-tag \
  --set config.appOrigin=https://cal.example.test \
  > "$default_rendered"
if grep -q 'name: AUTH_BRIDGE_KEY' "$default_rendered"; then
  echo 'AUTH_BRIDGE_KEY must not be rendered when authBridge.enabled is false' >&2
  exit 1
fi

echo 'commoncal chart assertions passed'
