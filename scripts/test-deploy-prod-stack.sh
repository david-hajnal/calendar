#!/bin/sh
set -eu

repository_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
deploy_script="$repository_root/deploy/deploy-prod.sh"
mcp_values="$repository_root/deploy/values-mcp-production.yaml"
mcp_ingress_template="$repository_root/deploy/helm/commoncal-mcp/templates/ingress.yaml"
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT HUP INT TERM

mkdir "$fixture/bin"
: >"$fixture/kubeconfig"
: >"$fixture/helm.log"
: >"$fixture/kubectl.log"
: >"$fixture/kubectl-stdin.log"
: >"$fixture/openssl-config.log"

cat >"$fixture/bin/kubectl" <<'EOF'
#!/bin/sh
set -eu

printf '%s\n' "$*" >>"$KUBECTL_LOG"

case "${1:-} ${2:-} ${3:-}" in
  "config current-context ")
    printf '%s\n' test-context
    ;;
  "get namespace "*)
    ;;
  "get secret "*)
    if [ "${TLS_EXISTING:-0}" = 1 ]; then
      printf '%s' ZHVtbXk=
    else
      # Force the deploy script to exercise certificate generation.
      exit 1
    fi
    ;;
  "get helmrelease "*)
    if [ "${FLUX_ACTIVE:-0}" = 1 ]; then
      printf '%s' "$3"
    fi
    ;;
  "create secret generic"|"create secret tls")
    printf '%s\n' 'apiVersion: v1' 'kind: Secret' 'metadata:' "  name: $4"
    ;;
  "apply --dry-run=server"*)
    cat >>"$KUBECTL_STDIN_LOG"
    ;;
  "apply -f "*)
    cat >/dev/null || true
    ;;
esac
EOF

cat >"$fixture/bin/openssl" <<'EOF'
#!/bin/sh
set -eu

if [ "${1:-}" = base64 ]; then
  cat
  exit 0
fi
if [ "${1:-}" = x509 ]; then
  printf '%s\n' 'X509v3 Subject Alternative Name:' "    ${TLS_CERT_SANS:-DNS:calendar.example.test, DNS:mcp.example.test}"
  exit 0
fi

config=
keyout=
certificate=
while [ "$#" -gt 0 ]; do
  case "$1" in
    -config) shift; config=$1 ;;
    -keyout) shift; keyout=$1 ;;
    -out) shift; certificate=$1 ;;
  esac
  shift
done

cp "$config" "$OPENSSL_CONFIG_LOG"
: >"$keyout"
: >"$certificate"
EOF

cat >"$fixture/bin/helm" <<'EOF'
#!/bin/sh
set -eu

release=$3
chart=$4
chart_name=${chart##*/}
case "$release" in
  *"$chart_name"*) resource_name=$release ;;
  *) resource_name=$release-$chart_name ;;
esac

{
  printf 'BEGIN release=%s chart=%s resource=%s\n' "$release" "$chart_name" "$resource_name"
  printf '%s\n' "$@"
  printf '%s\n' END
} >>"$HELM_LOG"
EOF

chmod +x "$fixture/bin/kubectl" "$fixture/bin/openssl" "$fixture/bin/helm"

run_stack() {
  PATH="$fixture/bin:$PATH" \
    KUBECONFIG="$fixture/kubeconfig" \
    KUBECTL_LOG="${KUBECTL_LOG_OVERRIDE:-$fixture/kubectl.log}" \
    KUBECTL_STDIN_LOG="$fixture/kubectl-stdin.log" \
    HELM_LOG="${HELM_LOG_OVERRIDE:-$fixture/helm.log}" \
    OPENSSL_CONFIG_LOG="$fixture/openssl-config.log" \
    SESSION_SECRET=test-session-secret \
    BACKUP_ENCRYPTION_KEY_HEX=00000000000000000000000000000000 \
    IMAGE_TAG=v9.8.7 \
    DOMAIN=calendar.example.test \
    MCP_DOMAIN=mcp.example.test \
    MCP_OAUTH_ISSUER=https://issuer.example.test \
    MCP_INTERNAL_API_BASE=https://calendar.example.test \
    MCP_INTERNAL_API_KEY=test-internal-api-key \
    MCP_SESSION_SECRET=test-mcp-session-secret \
    GHCR_TOKEN=test-ghcr-token \
    TLS_EXISTING="${TLS_EXISTING_OVERRIDE:-0}" \
    TLS_CERT_SANS="${TLS_CERT_SANS_OVERRIDE:-DNS:calendar.example.test, DNS:mcp.example.test}" \
    TLS_SECRET_NAME=commoncal-stack-tls \
    DRY_RUN="${DRY_RUN_OVERRIDE:-1}" \
    "$deploy_script"
}

run_stack >/dev/null

failures=0

require_line() {
  if ! grep -F -x -- "$1" "$2" >/dev/null; then
    echo "$3" >&2
    failures=$((failures + 1))
  fi
}

require_text() {
  if ! grep -F -- "$1" "$2" >/dev/null; then
    echo "$3" >&2
    failures=$((failures + 1))
  fi
}

require_line \
  "BEGIN release=commoncal chart=commoncal resource=commoncal" \
  "$fixture/helm.log" \
  "core must deploy as release/resource 'commoncal'"
require_line \
  "BEGIN release=commoncal-mcp chart=commoncal-mcp resource=commoncal-mcp" \
  "$fixture/helm.log" \
  "MCP must deploy as the distinct release/resource 'commoncal-mcp'"
require_line \
  'fullnameOverride=commoncal' \
  "$fixture/helm.log" \
  "core workload names must be pinned to the core release name"
require_line \
  'fullnameOverride=commoncal-mcp' \
  "$fixture/helm.log" \
  "MCP workload names must be pinned to the MCP release name"

require_text \
  'create secret generic commoncal-mcp-secrets --from-literal=mcp-internal-api-key=test-internal-api-key --from-literal=mcp-session-secret=test-mcp-session-secret --from-literal=mcp-oauth-issuer=https://issuer.example.test -n commoncal --dry-run=client -o yaml' \
  "$fixture/kubectl.log" \
  "deploy must create commoncal-mcp-secrets from all MCP secret inputs"
require_text \
  'apply --dry-run=server -f -' \
  "$fixture/kubectl.log" \
  "DRY_RUN=1 must server-dry-run the MCP secret apply"
require_text \
  'name: commoncal-mcp-secrets' \
  "$fixture/kubectl-stdin.log" \
  "the applied dry-run manifest must be commoncal-mcp-secrets"

if grep -F -- 'resource=commoncal-commoncal' "$fixture/helm.log" >/dev/null; then
  echo "deploy must not create an unexpected 'commoncal-commoncal' workload" >&2
  failures=$((failures + 1))
fi

require_line \
  'ingress.tls[0].secretName=commoncal-stack-tls' \
  "$fixture/helm.log" \
  "both ingresses must reference the managed TLS secret"
tls_reference_count=$(grep -F -x -c -- 'ingress.tls[0].secretName=commoncal-stack-tls' "$fixture/helm.log" || true)
if [ "$tls_reference_count" -ne 2 ]; then
  echo "expected core and MCP to each reference commoncal-stack-tls; found $tls_reference_count reference(s)" >&2
  failures=$((failures + 1))
fi

require_line \
  "$mcp_values" \
  "$fixture/helm.log" \
  "MCP must use values-mcp-production.yaml"
require_line \
  'domain=mcp.example.test' \
  "$fixture/helm.log" \
  "MCP Helm values must use MCP_DOMAIN"
require_line \
  'env.MCP_INTERNAL_API_BASE=https://calendar.example.test' \
  "$fixture/helm.log" \
  "MCP Helm values must wire MCP_INTERNAL_API_BASE"
require_line \
  'existingSecret.oauthIssuerKeyName=mcp-oauth-issuer' \
  "$fixture/helm.log" \
  "MCP Helm values must wire MCP_OAUTH_ISSUER from the Secret"
require_line \
  'env.MCP_PUBLIC_RESOURCE_URL=https://mcp.example.test/mcp' \
  "$fixture/helm.log" \
  "MCP Helm values must wire its public resource URL"
require_line \
  'ingress.tls[0].hosts[0]=mcp.example.test' \
  "$fixture/helm.log" \
  "MCP TLS ingress host must use MCP_DOMAIN"

dry_run_count=$(grep -F -x -c -- '--dry-run' "$fixture/helm.log" || true)
if [ "$dry_run_count" -ne 2 ]; then
  echo "DRY_RUN=1 must propagate to both Helm releases; found $dry_run_count dry-run argument(s)" >&2
  failures=$((failures + 1))
fi

pull_secret_count=$(grep -F -x -c -- 'imagePullSecrets[0].name=commoncal-ghcr-creds' "$fixture/helm.log" || true)
if [ "$pull_secret_count" -ne 2 ]; then
  echo "GHCR pull secret must be passed to both Helm releases; found $pull_secret_count reference(s)" >&2
  failures=$((failures + 1))
fi

require_text \
  'DNS.1 = calendar.example.test' \
  "$fixture/openssl-config.log" \
  "generated TLS certificate must cover the core domain"
require_text \
  'DNS.2 = mcp.example.test' \
  "$fixture/openssl-config.log" \
  "generated TLS certificate must cover the MCP domain"

require_text \
  '{{- range .Values.ingress.hosts }}' \
  "$mcp_ingress_template" \
  "MCP ingress must iterate configured host entries"
require_text \
  '{{- range .paths }}' \
  "$mcp_ingress_template" \
  "MCP ingress must iterate each host's nested paths"

guard_helm_log="$fixture/guard-helm.log"
: >"$guard_helm_log"
if (HELM_LOG_OVERRIDE="$guard_helm_log" FLUX_ACTIVE=1 run_stack >/dev/null 2>&1); then
  echo "manual deploy must fail while an active Flux HelmRelease exists" >&2
  failures=$((failures + 1))
elif [ -s "$guard_helm_log" ]; then
  echo "Flux ownership preflight must fail before invoking Helm" >&2
  failures=$((failures + 1))
fi

bad_tls_helm_log="$fixture/bad-tls-helm.log"
: >"$bad_tls_helm_log"
if (HELM_LOG_OVERRIDE="$bad_tls_helm_log" \
  TLS_EXISTING_OVERRIDE=1 \
  TLS_CERT_SANS_OVERRIDE='DNS:calendar.example.test' \
  run_stack >/dev/null 2>&1); then
  echo "an existing TLS certificate without the MCP SAN must be rejected" >&2
  failures=$((failures + 1))
elif [ -s "$bad_tls_helm_log" ]; then
  echo "invalid existing TLS certificate must fail before invoking Helm" >&2
  failures=$((failures + 1))
fi

rollout_log="$fixture/rollout-kubectl.log"
: >"$rollout_log"
DRY_RUN_OVERRIDE=0 KUBECTL_LOG_OVERRIDE="$rollout_log" run_stack >/dev/null
require_text \
  'rollout status statefulset commoncal --namespace commoncal --timeout=15m' \
  "$rollout_log" \
  "direct deploy must wait for the core StatefulSet rollout"
require_text \
  'rollout status deployment commoncal-mcp --namespace commoncal --timeout=15m' \
  "$rollout_log" \
  "direct deploy must wait for the MCP Deployment rollout"

if [ "$failures" -ne 0 ]; then
  echo "production stack contract failed with $failures error(s)" >&2
  exit 1
fi

echo "production stack deploy contract passed"
