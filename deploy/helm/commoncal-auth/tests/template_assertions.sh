#!/usr/bin/env sh
set -eu

chart_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
rendered=$(mktemp)
ingress_section=$(mktemp)
trap 'rm -f "$rendered" "$ingress_section"' EXIT

require_source() {
  if ! grep -F -q -- "$2" "$1"; then
    echo "missing required chart configuration: $2" >&2
    exit 1
  fi
}

if ! command -v helm >/dev/null 2>&1; then
  echo 'helm is not installed; checking the chart source for slice 5 acceptance gates' >&2

  # Two services: public (OIDC) and internal (bridge).
  require_source "$chart_dir/templates/service-public.yaml" 'kind: Service'
  require_source "$chart_dir/templates/service-internal.yaml" 'kind: Service'
  # Ingress exists and is public-only.
  require_source "$chart_dir/templates/ingress.yaml" 'kind: Ingress'
  require_source "$chart_dir/templates/ingress.yaml" 'commoncal-auth.publicServiceName'
  # Network policy enforces bridge isolation.
  require_source "$chart_dir/templates/networkpolicy.yaml" 'kind: NetworkPolicy'
  require_source "$chart_dir/templates/networkpolicy.yaml" 'commoncalNamespace'
  # Migration job.
  require_source "$chart_dir/templates/migration-job.yaml" 'kind: Job'
  require_source "$chart_dir/templates/migration-job.yaml" 'src/migrate.mjs'
  # PDB.
  require_source "$chart_dir/templates/pdb.yaml" 'kind: PodDisruptionBudget'
  # Secrets by reference only.
  require_source "$chart_dir/templates/deployment.yaml" 'secretKeyRef:'
  require_source "$chart_dir/templates/deployment.yaml" 'name: {{ .Values.secrets.name }}'
  require_source "$chart_dir/values.yaml" 'runAsNonRoot: true'
  require_source "$chart_dir/values.yaml" 'type: RuntimeDefault'
  exit 0
fi

helm template commoncal-auth "$chart_dir" --namespace commoncal > "$rendered"

# Flux's Helm post-renderer rejects duplicate mapping keys even though
# `helm lint` and `helm template` accept them.
python3 "$chart_dir/../../../scripts/validate-yaml.py" "$rendered"

# --- Kinds present ---------------------------------------------------------
grep -q 'kind: Deployment' "$rendered"
grep -q 'kind: Service' "$rendered"
grep -q 'kind: Ingress' "$rendered"
grep -q 'kind: NetworkPolicy' "$rendered"
grep -q 'kind: Job' "$rendered"
grep -q 'kind: PodDisruptionBudget' "$rendered"
grep -q 'kind: ConfigMap' "$rendered"
grep -q 'kind: ServiceAccount' "$rendered"

# --- Security context ------------------------------------------------------
grep -q 'runAsNonRoot: true' "$rendered"
grep -q 'runAsUser: 65534' "$rendered"
grep -q 'type: RuntimeDefault' "$rendered"
grep -q 'automountServiceAccountToken: false' "$rendered"

# --- Two services ----------------------------------------------------------
grep -q 'name: commoncal-auth-public' "$rendered"
grep -q 'name: commoncal-auth-internal' "$rendered"
grep -A10 'name: commoncal-auth-public' "$rendered" | grep -q 'app.kubernetes.io/component: public'
grep -A10 'name: commoncal-auth-internal' "$rendered" | grep -q 'app.kubernetes.io/component: internal'

# Service selectors continue to target the authorization Deployment pods.
grep -A20 'name: commoncal-auth-public' "$rendered" | grep -q 'app.kubernetes.io/component: authorization'
grep -A20 'name: commoncal-auth-internal' "$rendered" | grep -q 'app.kubernetes.io/component: authorization'

# --- Ingress is public-only (no internal service reference) -----------------
# The Ingress must reference the public service, never the internal one.
sed -n '/kind: Ingress/,/^---/p' "$rendered" > "$ingress_section"
if grep -q 'commoncal-auth-internal' "$ingress_section"; then
  echo 'Ingress must not reference the internal (bridge) service' >&2
  exit 1
fi
grep -q 'name: commoncal-auth-public' "$ingress_section"

# --- No secret values in rendered output -----------------------------------
# Strip comments, then assert none of the known secret material appears.
if grep -vE '^[[:space:]]*#' "$rendered" | grep -qiE \
  'oidc-lab-only|slice1-cookie-key|slice1-loopback-bridge|postgres://oidc|slice1-test-rs256'; then
  echo 'rendered output must not contain secret values' >&2
  exit 1
fi

# --- Secrets injected by reference only ------------------------------------
grep -q 'secretKeyRef:' "$rendered"
grep -q 'name: commoncal-auth-secrets' "$rendered"
# The bridge key must come from a Secret reference, not a literal.
if ! awk '
  /- name: LAB_BRIDGE_KEY/ { in_bridge=1; next }
  in_bridge && /valueFrom:/ { has_reference=1 }
  in_bridge && /^[[:space:]]+- name:/ { in_bridge=0 }
  END { exit has_reference ? 0 : 1 }
' "$rendered"; then
  echo 'LAB_BRIDGE_KEY must be supplied by a Secret reference' >&2
  exit 1
fi

# --- Network policy enforces bridge isolation ------------------------------
# The bridge port (4001) must be reachable only from the commoncal namespace.
grep -q 'kubernetes.io/metadata.name: commoncal' "$rendered"
grep -q 'port: 4001' "$rendered"
# The public port (4000) is reachable from the ingress namespace.
grep -q 'kubernetes.io/metadata.name: traefik' "$rendered"
grep -q 'port: 4000' "$rendered"

# --- Migration job uses the migration entrypoint ---------------------------
grep -q 'src/migrate.mjs' "$rendered"
grep -q 'name: commoncal-auth-migrate' "$rendered"
grep -A30 'name: commoncal-auth-migrate' "$rendered" | grep -q 'app.kubernetes.io/component: migration'

# --- PDB -------------------------------------------------------------------
grep -q 'minAvailable: 1' "$rendered"

# --- Issuer / resource consistency (non-secret config) ---------------------
grep -q 'LAB_ISSUER' "$rendered"
grep -q 'LAB_RESOURCE_URL' "$rendered"
grep -q 'LAB_COMMONCAL_URL' "$rendered"

echo 'commoncal-auth chart assertions passed'
