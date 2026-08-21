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
: >"$fixture/flux.log"
: >"$fixture/kubectl.log"
: >"$fixture/kubectl-stdin.log"
: >"$fixture/openssl-config.log"
: >"$fixture/cert-manager-state"
: >"$fixture/clusterissuer-state"

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
    if [ "${FLUX_ACTIVE:-0}" = 1 ] || [ "${TLS_EXISTING:-0}" = 1 ]; then
      printf '%s' ZHVtbXk=
    else
      # Force the deploy script to exercise certificate generation.
      exit 1
    fi
    ;;
  "get helmrelease "*)
    case "$*" in
      *'.spec.values.domain}'*)
        if [ "$3" = commoncal ]; then
          printf '%s' calendar.example.test
        else
          printf '%s' mcp.example.test
        fi
        ;;
      *'.spec.values.ingress.tls[0].secretName}'*)
        if [ "$3" = commoncal-mcp ]; then
          printf '%s' "${MCP_TLS_SECRET:-commoncal-stack-tls}"
        else
          printf '%s' commoncal-stack-tls
        fi
        ;;
      *)
        if [ "${FLUX_ACTIVE:-0}" = 1 ] || [ "${FLUX_ACTIVE_RELEASE:-}" = "$3" ]; then
          printf '%s' "$3"
        fi
        ;;
    esac
    ;;
  "get crd certificates.cert-manager.io")
    [ "${CERT_MANAGER_CRD_READY:-${CERT_MANAGER_READY:-1}}" = 1 ] || [ -s "$CERT_MANAGER_STATE" ]
    ;;
  "get clusterissuer letsencrypt-prod")
    state=$(cat "$CLUSTERISSUER_STATE" 2>/dev/null || true)
    if [ -n "$state" ]; then
      case "$state" in
        notready) printf '%s' False ;;
        *) printf '%s' True ;;
      esac
      exit 0
    fi
    if [ "${CLUSTERISSUER_READY:-${CERT_MANAGER_READY:-1}}" = 1 ]; then
      printf '%s' True
      exit 0
    fi
    exit 1
    ;;
  "get --raw /version")
    printf '{"gitVersion":"%s"}\n' "${K8S_SERVER_VERSION:-v1.34.2+k3s2}"
    ;;
  "get certificate "*)
    printf '%s' "${CERTIFICATE_DNS:-calendar.example.test mcp.example.test}"
    ;;
  "create secret generic"|"create secret tls")
    printf '%s\n' 'apiVersion: v1' 'kind: Secret' 'metadata:' "  name: $4"
    ;;
  "apply --dry-run=server"*)
    cat >>"$KUBECTL_STDIN_LOG"
    ;;
  "apply --dry-run=client"*)
    cat >>"$KUBECTL_STDIN_LOG"
    ;;
  "apply -f "*)
    manifest=$(cat)
    printf '%s\n' "$manifest" >>"$KUBECTL_STDIN_LOG"
    if printf '%s\n' "$manifest" | grep -F 'kind: ClusterIssuer' >/dev/null; then
      printf '%s\n' ready >"$CLUSTERISSUER_STATE"
    fi
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

if [ "${1:-}" = upgrade ] && [ "${2:-}" = --install ] && [ "${3:-}" = cert-manager ]; then
  {
    printf 'BEGIN cert-manager-bootstrap\n'
    printf '%s\n' "$@"
    printf '%s\n' END
  } >>"$HELM_LOG"
  if [ "${CERT_MANAGER_INSTALL_FAIL:-0}" = 1 ]; then
    exit 42
  fi
  case " $* " in
    *' --dry-run '*) ;;
    *) printf '%s\n' installed >"$CERT_MANAGER_STATE" ;;
  esac
  exit 0
fi

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

cat >"$fixture/bin/flux" <<'EOF'
#!/bin/sh
set -eu

printf '%s\n' "$*" >>"$FLUX_LOG"
EOF

chmod +x "$fixture/bin/kubectl" "$fixture/bin/openssl" "$fixture/bin/helm" "$fixture/bin/flux"

run_stack() {
  PATH="$fixture/bin:$PATH" \
    KUBECONFIG="$fixture/kubeconfig" \
    KUBECTL_LOG="${KUBECTL_LOG_OVERRIDE:-$fixture/kubectl.log}" \
    KUBECTL_STDIN_LOG="${KUBECTL_STDIN_LOG_OVERRIDE:-$fixture/kubectl-stdin.log}" \
    HELM_LOG="${HELM_LOG_OVERRIDE:-$fixture/helm.log}" \
    FLUX_LOG="${FLUX_LOG_OVERRIDE:-$fixture/flux.log}" \
    OPENSSL_CONFIG_LOG="${OPENSSL_CONFIG_LOG_OVERRIDE:-$fixture/openssl-config.log}" \
    SESSION_SECRET=test-session-secret \
    BACKUP_ENCRYPTION_KEY_HEX=00000000000000000000000000000000 \
    IMAGE_TAG="${IMAGE_TAG_OVERRIDE-v9.8.7}" \
    DOMAIN=calendar.example.test \
    MCP_DOMAIN=mcp.example.test \
    MCP_OAUTH_ISSUER=https://issuer.example.test \
    MCP_INTERNAL_API_BASE=https://calendar.example.test \
    MCP_INTERNAL_API_KEY=test-internal-api-key \
    MCP_SESSION_SECRET=test-mcp-session-secret \
    GHCR_TOKEN="${GHCR_TOKEN_OVERRIDE-}" \
    TLS_EXISTING="${TLS_EXISTING_OVERRIDE:-0}" \
    K8S_SERVER_VERSION="${K8S_SERVER_VERSION_OVERRIDE:-v1.34.2+k3s2}" \
    TLS_CERT_SANS="${TLS_CERT_SANS_OVERRIDE:-DNS:calendar.example.test, DNS:mcp.example.test}" \
    TLS_SECRET_NAME=commoncal-stack-tls \
    FLUX_ACTIVE_RELEASE="${FLUX_ACTIVE_RELEASE_OVERRIDE:-}" \
    CERT_MANAGER_READY="${CERT_MANAGER_READY_OVERRIDE:-1}" \
    CERT_MANAGER_CRD_READY="${CERT_MANAGER_CRD_READY_OVERRIDE:-}" \
    CLUSTERISSUER_READY="${CLUSTERISSUER_READY_OVERRIDE:-}" \
    CERT_MANAGER_ACME_EMAIL="${CERT_MANAGER_ACME_EMAIL_OVERRIDE-deployer@example.test}" \
    CERT_MANAGER_INSTALL_FAIL="${CERT_MANAGER_INSTALL_FAIL_OVERRIDE:-0}" \
    CERT_MANAGER_VERSION="${CERT_MANAGER_VERSION_OVERRIDE-v1.21.1}" \
    CERT_MANAGER_STATE="$fixture/cert-manager-state" \
    CLUSTERISSUER_STATE="$fixture/clusterissuer-state" \
    CERTIFICATE_DNS="${CERTIFICATE_DNS_OVERRIDE:-calendar.example.test mcp.example.test}" \
    MCP_TLS_SECRET="${MCP_TLS_SECRET_OVERRIDE:-commoncal-stack-tls}" \
    CORE_HELM_RELEASE_NAME="${CORE_RELEASE_OVERRIDE:-}" \
    DRY_RUN="${DRY_RUN_OVERRIDE:-1}" \
    "$deploy_script"
}

# bash 3.2 POSIX mode leaks "VAR=x func" assignments into the shell, so set
# and clear the overrides explicitly instead of using a command prefix.
GHCR_TOKEN_OVERRIDE=test-ghcr-token
TLS_EXISTING_OVERRIDE=1
run_stack >/dev/null
unset GHCR_TOKEN_OVERRIDE
unset TLS_EXISTING_OVERRIDE

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
  '{{- range .Values.ingress.hosts }}' \
  "$mcp_ingress_template" \
  "MCP ingress must iterate configured host entries"
require_text \
  '{{- range .paths }}' \
  "$mcp_ingress_template" \
  "MCP ingress must iterate each host's nested paths"

guard_helm_log="$fixture/guard-helm.log"
guard_flux_log="$fixture/guard-flux.log"
guard_kubectl_log="$fixture/guard-kubectl.log"
guard_openssl_log="$fixture/guard-openssl.log"
: >"$guard_helm_log"
: >"$guard_flux_log"
: >"$guard_kubectl_log"
: >"$guard_openssl_log"
if ! (HELM_LOG_OVERRIDE="$guard_helm_log" \
  FLUX_LOG_OVERRIDE="$guard_flux_log" \
  KUBECTL_LOG_OVERRIDE="$guard_kubectl_log" \
  OPENSSL_CONFIG_LOG_OVERRIDE="$guard_openssl_log" \
  FLUX_ACTIVE=1 \
  IMAGE_TAG_OVERRIDE= \
  DRY_RUN_OVERRIDE=0 \
  run_stack >/dev/null 2>&1); then
  echo "deploy must reconcile through Flux when active HelmReleases own production" >&2
  failures=$((failures + 1))
fi
if [ -s "$guard_helm_log" ]; then
  echo "Flux-owned deployment must not invoke Helm directly" >&2
  failures=$((failures + 1))
fi
require_line \
  'reconcile kustomization flux-system --namespace flux-system --with-source' \
  "$guard_flux_log" \
  "Flux-owned deployment must load the latest HelmRelease manifests from Git"
require_line \
  'reconcile helmrelease commoncal --namespace flux-system --with-source' \
  "$guard_flux_log" \
  "Flux-owned deployment must reconcile the core HelmRelease"
require_line \
  'reconcile helmrelease commoncal-mcp --namespace flux-system --with-source' \
  "$guard_flux_log" \
  "Flux-owned deployment must reconcile the MCP HelmRelease"
require_text \
  'create secret generic commoncal-session' \
  "$guard_kubectl_log" \
  "Flux-owned deployment must apply the core runtime Secret"
require_text \
  'create secret generic commoncal-mcp-secrets' \
  "$guard_kubectl_log" \
  "Flux-owned deployment must apply the MCP runtime Secret"
require_text \
  'wait --for=create certificate commoncal-stack-tls --namespace commoncal --timeout=2m' \
  "$guard_kubectl_log" \
  "Flux-owned deployment must wait for ingress-shim to create the Certificate"
require_text \
  'wait certificate commoncal-stack-tls --namespace commoncal --for=condition=Ready --timeout=5m' \
  "$guard_kubectl_log" \
  "Flux-owned deployment must wait for the shared Certificate to become Ready"
certificate_create_wait_line=$(grep -n -F -- 'wait --for=create certificate commoncal-stack-tls' "$guard_kubectl_log" | head -1 | cut -d: -f1 || true)
certificate_ready_wait_line=$(grep -n -F -- 'wait certificate commoncal-stack-tls' "$guard_kubectl_log" | head -1 | cut -d: -f1 || true)
if [ -z "$certificate_create_wait_line" ] || [ -z "$certificate_ready_wait_line" ] || \
  [ "$certificate_create_wait_line" -ge "$certificate_ready_wait_line" ]; then
  echo "Certificate creation must be awaited before its Ready condition" >&2
  failures=$((failures + 1))
fi
require_text \
  'rollout restart statefulset commoncal --namespace commoncal' \
  "$guard_kubectl_log" \
  "Flux-owned deployment must restart core after applying external Secrets"
require_text \
  'rollout status deployment commoncal-mcp --namespace commoncal --timeout=15m' \
  "$guard_kubectl_log" \
  "Flux-owned deployment must wait for the MCP rollout"
if grep -F -- 'create secret tls' "$guard_kubectl_log" >/dev/null || [ -s "$guard_openssl_log" ]; then
  echo "Flux-owned deployment must leave TLS issuance to cert-manager" >&2
  failures=$((failures + 1))
fi

override_helm_log="$fixture/override-helm.log"
override_flux_log="$fixture/override-flux.log"
override_kubectl_log="$fixture/override-kubectl.log"
: >"$override_helm_log"
: >"$override_flux_log"
: >"$override_kubectl_log"
if (HELM_LOG_OVERRIDE="$override_helm_log" \
  FLUX_LOG_OVERRIDE="$override_flux_log" \
  KUBECTL_LOG_OVERRIDE="$override_kubectl_log" \
  FLUX_ACTIVE=1 \
  CORE_RELEASE_OVERRIDE=legacy-core \
  DRY_RUN_OVERRIDE=0 \
  run_stack >/dev/null 2>&1); then
  echo "Flux-owned deployment must reject legacy release-name overrides" >&2
  failures=$((failures + 1))
fi
if [ -s "$override_helm_log" ] || [ -s "$override_flux_log" ] || \
  grep -F -- 'create secret' "$override_kubectl_log" >/dev/null; then
  echo "invalid Flux name overrides must fail before mutations" >&2
  failures=$((failures + 1))
fi

missing_cert_flux_log="$fixture/missing-cert-flux.log"
missing_cert_kubectl_log="$fixture/missing-cert-kubectl.log"
missing_cert_helm_log="$fixture/missing-cert-helm.log"
missing_cert_stdin_log="$fixture/missing-cert-stdin.log"
: >"$missing_cert_flux_log"
: >"$missing_cert_kubectl_log"
: >"$missing_cert_helm_log"
: >"$missing_cert_stdin_log"
: >"$fixture/cert-manager-state"
: >"$fixture/clusterissuer-state"
if ! (FLUX_LOG_OVERRIDE="$missing_cert_flux_log" \
  HELM_LOG_OVERRIDE="$missing_cert_helm_log" \
  KUBECTL_LOG_OVERRIDE="$missing_cert_kubectl_log" \
  KUBECTL_STDIN_LOG_OVERRIDE="$missing_cert_stdin_log" \
  FLUX_ACTIVE=1 \
  CERT_MANAGER_READY_OVERRIDE=0 \
  DRY_RUN_OVERRIDE=0 \
  run_stack >/dev/null 2>&1); then
  echo "Flux-owned deployment must bootstrap missing cert-manager and its production issuer" >&2
  failures=$((failures + 1))
fi
require_line \
  'BEGIN cert-manager-bootstrap' \
  "$missing_cert_helm_log" \
  "missing cert-manager must be installed through the pinned Helm chart"
require_line \
  'oci://quay.io/jetstack/charts/cert-manager' \
  "$missing_cert_helm_log" \
  "cert-manager bootstrap must use the official OCI chart"
for cert_manager_arg in '--version' 'v1.21.1' '--namespace' 'cert-manager' '--create-namespace' '--set' 'crds.enabled=true' 'crds.keep=true' '--wait' '--timeout=10m'; do
  require_line \
    "$cert_manager_arg" \
    "$missing_cert_helm_log" \
    "cert-manager bootstrap is missing required Helm argument: $cert_manager_arg"
done
require_text \
  'kind: ClusterIssuer' \
  "$missing_cert_stdin_log" \
  "missing issuer must be created declaratively"
require_text \
  'name: letsencrypt-prod' \
  "$missing_cert_stdin_log" \
  "cert-manager bootstrap must create the letsencrypt-prod ClusterIssuer"
require_text \
  'email: deployer@example.test' \
  "$missing_cert_stdin_log" \
  "the ClusterIssuer must use CERT_MANAGER_ACME_EMAIL"
require_text \
  'server: https://acme-v02.api.letsencrypt.org/directory' \
  "$missing_cert_stdin_log" \
  "the production issuer must use the Let's Encrypt production endpoint"
require_text \
  'name: letsencrypt-prod-account-key' \
  "$missing_cert_stdin_log" \
  "the production issuer must persist its ACME account key"
require_text \
  'ingressClassName: traefik' \
  "$missing_cert_stdin_log" \
  "HTTP-01 challenges must use the k3s Traefik ingress class"
require_text \
  'wait clusterissuer letsencrypt-prod --for=condition=Ready --timeout=5m' \
  "$missing_cert_kubectl_log" \
  "deployment must wait for the bootstrapped ClusterIssuer to become Ready"
require_line \
  'reconcile helmrelease commoncal-mcp --namespace flux-system --with-source' \
  "$missing_cert_flux_log" \
  "Flux reconciliation must continue after TLS prerequisites become Ready"

missing_cert_dry_helm_log="$fixture/missing-cert-dry-helm.log"
missing_cert_dry_flux_log="$fixture/missing-cert-dry-flux.log"
missing_cert_dry_kubectl_log="$fixture/missing-cert-dry-kubectl.log"
: >"$missing_cert_dry_helm_log"
: >"$missing_cert_dry_flux_log"
: >"$missing_cert_dry_kubectl_log"
: >"$fixture/cert-manager-state"
: >"$fixture/clusterissuer-state"
if ! (HELM_LOG_OVERRIDE="$missing_cert_dry_helm_log" \
  FLUX_LOG_OVERRIDE="$missing_cert_dry_flux_log" \
  KUBECTL_LOG_OVERRIDE="$missing_cert_dry_kubectl_log" \
  FLUX_ACTIVE=1 \
  CERT_MANAGER_READY_OVERRIDE=0 \
  DRY_RUN_OVERRIDE=1 \
  run_stack >/dev/null 2>&1); then
  echo "Flux-owned dry-run must validate a missing cert-manager bootstrap" >&2
  failures=$((failures + 1))
fi
require_line \
  '--dry-run' \
  "$missing_cert_dry_helm_log" \
  "missing cert-manager dry-run must render the pinned Helm install without installing it"
if [ -s "$missing_cert_dry_flux_log" ]; then
  echo "missing cert-manager dry-run must not reconcile Flux" >&2
  failures=$((failures + 1))
fi
if grep -F -x -- 'apply -f -' "$missing_cert_dry_kubectl_log" >/dev/null || \
  grep -F -- 'rollout restart' "$missing_cert_dry_kubectl_log" >/dev/null || \
  [ -s "$fixture/cert-manager-state" ] || [ -s "$fixture/clusterissuer-state" ]; then
  echo "missing cert-manager dry-run must not persist cert-manager, issuer, or workload changes" >&2
  failures=$((failures + 1))
fi

missing_email_helm_log="$fixture/missing-email-helm.log"
missing_email_flux_log="$fixture/missing-email-flux.log"
missing_email_kubectl_log="$fixture/missing-email-kubectl.log"
: >"$missing_email_helm_log"
: >"$missing_email_flux_log"
: >"$missing_email_kubectl_log"
: >"$fixture/cert-manager-state"
: >"$fixture/clusterissuer-state"
if (HELM_LOG_OVERRIDE="$missing_email_helm_log" \
  FLUX_LOG_OVERRIDE="$missing_email_flux_log" \
  KUBECTL_LOG_OVERRIDE="$missing_email_kubectl_log" \
  FLUX_ACTIVE=1 \
  CERT_MANAGER_READY_OVERRIDE=0 \
  CERT_MANAGER_ACME_EMAIL_OVERRIDE= \
  DRY_RUN_OVERRIDE=0 \
  run_stack >/dev/null 2>&1); then
  echo "cert-manager bootstrap must require CERT_MANAGER_ACME_EMAIL" >&2
  failures=$((failures + 1))
fi
if [ -s "$missing_email_helm_log" ] || [ -s "$missing_email_flux_log" ] || \
  grep -F -x -- 'apply -f -' "$missing_email_kubectl_log" >/dev/null; then
  echo "missing ACME email must fail before persistent deployment mutations" >&2
  failures=$((failures + 1))
fi

issuer_only_helm_log="$fixture/issuer-only-helm.log"
issuer_only_flux_log="$fixture/issuer-only-flux.log"
issuer_only_kubectl_log="$fixture/issuer-only-kubectl.log"
issuer_only_stdin_log="$fixture/issuer-only-stdin.log"
: >"$issuer_only_helm_log"
: >"$issuer_only_flux_log"
: >"$issuer_only_kubectl_log"
: >"$issuer_only_stdin_log"
: >"$fixture/cert-manager-state"
: >"$fixture/clusterissuer-state"
if ! (HELM_LOG_OVERRIDE="$issuer_only_helm_log" \
  FLUX_LOG_OVERRIDE="$issuer_only_flux_log" \
  KUBECTL_LOG_OVERRIDE="$issuer_only_kubectl_log" \
  KUBECTL_STDIN_LOG_OVERRIDE="$issuer_only_stdin_log" \
  FLUX_ACTIVE=1 \
  CERT_MANAGER_CRD_READY_OVERRIDE=1 \
  CLUSTERISSUER_READY_OVERRIDE=0 \
  DRY_RUN_OVERRIDE=0 \
  run_stack >/dev/null 2>&1); then
  echo "Flux-owned deployment must create a missing issuer when cert-manager is installed" >&2
  failures=$((failures + 1))
fi
if [ -s "$issuer_only_helm_log" ]; then
  echo "an installed cert-manager must not be reinstalled just because its issuer is absent" >&2
  failures=$((failures + 1))
fi
require_text \
  'kind: ClusterIssuer' \
  "$issuer_only_stdin_log" \
  "an absent letsencrypt-prod issuer must be applied without reinstalling cert-manager"
require_text \
  'wait clusterissuer letsencrypt-prod --for=condition=Ready --timeout=5m' \
  "$issuer_only_kubectl_log" \
  "a newly applied issuer must become Ready before application reconciliation"
require_line \
  'reconcile helmrelease commoncal --namespace flux-system --with-source' \
  "$issuer_only_flux_log" \
  "Flux reconciliation must continue after creating the missing issuer"

install_failure_helm_log="$fixture/install-failure-helm.log"
install_failure_flux_log="$fixture/install-failure-flux.log"
install_failure_kubectl_log="$fixture/install-failure-kubectl.log"
: >"$install_failure_helm_log"
: >"$install_failure_flux_log"
: >"$install_failure_kubectl_log"
: >"$fixture/cert-manager-state"
: >"$fixture/clusterissuer-state"
if (HELM_LOG_OVERRIDE="$install_failure_helm_log" \
  FLUX_LOG_OVERRIDE="$install_failure_flux_log" \
  KUBECTL_LOG_OVERRIDE="$install_failure_kubectl_log" \
  FLUX_ACTIVE=1 \
  CERT_MANAGER_READY_OVERRIDE=0 \
  CERT_MANAGER_INSTALL_FAIL_OVERRIDE=1 \
  DRY_RUN_OVERRIDE=0 \
  run_stack >/dev/null 2>&1); then
  echo "a failed cert-manager installation must abort deployment" >&2
  failures=$((failures + 1))
fi
require_line \
  'BEGIN cert-manager-bootstrap' \
  "$install_failure_helm_log" \
  "the installation-failure scenario must reach the cert-manager bootstrap"
if [ -s "$install_failure_flux_log" ] || \
  grep -F -- 'create secret' "$install_failure_kubectl_log" >/dev/null || \
  grep -F -x -- 'apply -f -' "$install_failure_kubectl_log" >/dev/null; then
  echo "a failed cert-manager installation must abort before app secrets, issuer, or Flux mutations" >&2
  failures=$((failures + 1))
fi

ready_tls_helm_log="$fixture/ready-tls-helm.log"
ready_tls_flux_log="$fixture/ready-tls-flux.log"
ready_tls_kubectl_log="$fixture/ready-tls-kubectl.log"
ready_tls_stdin_log="$fixture/ready-tls-stdin.log"
: >"$ready_tls_helm_log"
: >"$ready_tls_flux_log"
: >"$ready_tls_kubectl_log"
: >"$ready_tls_stdin_log"
: >"$fixture/cert-manager-state"
: >"$fixture/clusterissuer-state"
if ! (HELM_LOG_OVERRIDE="$ready_tls_helm_log" \
  FLUX_LOG_OVERRIDE="$ready_tls_flux_log" \
  KUBECTL_LOG_OVERRIDE="$ready_tls_kubectl_log" \
  KUBECTL_STDIN_LOG_OVERRIDE="$ready_tls_stdin_log" \
  FLUX_ACTIVE=1 \
  CERT_MANAGER_CRD_READY_OVERRIDE=1 \
  CLUSTERISSUER_READY_OVERRIDE=1 \
  CERT_MANAGER_ACME_EMAIL_OVERRIDE= \
  DRY_RUN_OVERRIDE=0 \
  run_stack >/dev/null 2>&1); then
  echo "an existing Ready cert-manager and issuer must deploy without an ACME email" >&2
  failures=$((failures + 1))
fi
if [ -s "$ready_tls_helm_log" ] || grep -F -- 'kind: ClusterIssuer' "$ready_tls_stdin_log" >/dev/null; then
  echo "existing Ready TLS prerequisites must not be reinstalled or overwritten" >&2
  failures=$((failures + 1))
fi
require_line \
  'reconcile helmrelease commoncal-mcp --namespace flux-system --with-source' \
  "$ready_tls_flux_log" \
  "existing Ready TLS prerequisites must proceed directly to Flux deployment"

issuer_notready_helm_log="$fixture/issuer-notready-helm.log"
issuer_notready_flux_log="$fixture/issuer-notready-flux.log"
issuer_notready_kubectl_log="$fixture/issuer-notready-kubectl.log"
issuer_notready_out="$fixture/issuer-notready.out"
: >"$issuer_notready_helm_log"
: >"$issuer_notready_flux_log"
: >"$issuer_notready_kubectl_log"
printf 'notready\n' >"$fixture/clusterissuer-state"
if (HELM_LOG_OVERRIDE="$issuer_notready_helm_log" \
  FLUX_LOG_OVERRIDE="$issuer_notready_flux_log" \
  KUBECTL_LOG_OVERRIDE="$issuer_notready_kubectl_log" \
  FLUX_ACTIVE=1 \
  CERT_MANAGER_CRD_READY_OVERRIDE=1 \
  DRY_RUN_OVERRIDE=0 \
  run_stack >"$issuer_notready_out" 2>&1); then
  echo "an existing NotReady issuer must abort the Flux deployment" >&2
  failures=$((failures + 1))
fi
if [ -s "$issuer_notready_helm_log" ] || [ -s "$issuer_notready_flux_log" ] || \
  grep -F -- 'create secret' "$issuer_notready_kubectl_log" >/dev/null || \
  grep -F -x -- 'apply -f -' "$issuer_notready_kubectl_log" >/dev/null; then
  echo "an existing NotReady issuer must fail before any deployment mutation" >&2
  failures=$((failures + 1))
fi
if ! grep -F -- 'exists but is not Ready' "$issuer_notready_out" >/dev/null; then
  echo "an existing NotReady issuer must be reported as not Ready" >&2
  failures=$((failures + 1))
fi

k8s_old_helm_log="$fixture/k8s-old-helm.log"
k8s_old_flux_log="$fixture/k8s-old-flux.log"
k8s_old_kubectl_log="$fixture/k8s-old-kubectl.log"
: >"$k8s_old_helm_log"
: >"$k8s_old_flux_log"
: >"$k8s_old_kubectl_log"
: >"$fixture/cert-manager-state"
: >"$fixture/clusterissuer-state"
if (HELM_LOG_OVERRIDE="$k8s_old_helm_log" \
  FLUX_LOG_OVERRIDE="$k8s_old_flux_log" \
  KUBECTL_LOG_OVERRIDE="$k8s_old_kubectl_log" \
  FLUX_ACTIVE=1 \
  CERT_MANAGER_READY_OVERRIDE=0 \
  K8S_SERVER_VERSION_OVERRIDE=v1.31.4+k3s1 \
  DRY_RUN_OVERRIDE=0 \
  run_stack >/dev/null 2>&1); then
  echo "cert-manager bootstrap must refuse an unsupported Kubernetes version" >&2
  failures=$((failures + 1))
fi
if [ -s "$k8s_old_helm_log" ] || [ -s "$k8s_old_flux_log" ] || \
  grep -F -- 'create secret' "$k8s_old_kubectl_log" >/dev/null || \
  grep -F -x -- 'apply -f -' "$k8s_old_kubectl_log" >/dev/null; then
  echo "an unsupported Kubernetes version must fail before cert-manager bootstrap" >&2
  failures=$((failures + 1))
fi

k8s_unknown_helm_log="$fixture/k8s-unknown-helm.log"
k8s_unknown_kubectl_log="$fixture/k8s-unknown-kubectl.log"
: >"$k8s_unknown_helm_log"
: >"$k8s_unknown_kubectl_log"
: >"$fixture/cert-manager-state"
: >"$fixture/clusterissuer-state"
if (HELM_LOG_OVERRIDE="$k8s_unknown_helm_log" \
  KUBECTL_LOG_OVERRIDE="$k8s_unknown_kubectl_log" \
  FLUX_ACTIVE=1 \
  CERT_MANAGER_READY_OVERRIDE=0 \
  K8S_SERVER_VERSION_OVERRIDE=unparseable \
  DRY_RUN_OVERRIDE=0 \
  run_stack >/dev/null 2>&1); then
  echo "cert-manager bootstrap must fail when the Kubernetes version is undetectable" >&2
  failures=$((failures + 1))
fi
if [ -s "$k8s_unknown_helm_log" ] || grep -F -x -- 'apply -f -' "$k8s_unknown_kubectl_log" >/dev/null; then
  echo "an undetectable Kubernetes version must fail before cert-manager bootstrap" >&2
  failures=$((failures + 1))
fi

k8s_override_helm_log="$fixture/k8s-override-helm.log"
k8s_override_kubectl_log="$fixture/k8s-override-kubectl.log"
: >"$k8s_override_helm_log"
: >"$k8s_override_kubectl_log"
: >"$fixture/cert-manager-state"
: >"$fixture/clusterissuer-state"
if ! (HELM_LOG_OVERRIDE="$k8s_override_helm_log" \
  KUBECTL_LOG_OVERRIDE="$k8s_override_kubectl_log" \
  FLUX_ACTIVE=1 \
  CERT_MANAGER_READY_OVERRIDE=0 \
  K8S_SERVER_VERSION_OVERRIDE=v1.31.4+k3s1 \
  CERT_MANAGER_VERSION_OVERRIDE=v1.15.10 \
  DRY_RUN_OVERRIDE=0 \
  run_stack >/dev/null 2>&1); then
  echo "an explicit CERT_MANAGER_VERSION override must be allowed on an older cluster" >&2
  failures=$((failures + 1))
fi
require_line \
  'BEGIN cert-manager-bootstrap' \
  "$k8s_override_helm_log" \
  "an explicit CERT_MANAGER_VERSION override must reach the cert-manager bootstrap"
require_line \
  'v1.15.10' \
  "$k8s_override_helm_log" \
  "the bootstrap must install the overridden cert-manager version"

direct_no_tls_helm_log="$fixture/direct-no-tls-helm.log"
direct_no_tls_kubectl_log="$fixture/direct-no-tls-kubectl.log"
direct_no_tls_openssl_log="$fixture/direct-no-tls-openssl.log"
direct_no_tls_out="$fixture/direct-no-tls.out"
: >"$direct_no_tls_helm_log"
: >"$direct_no_tls_kubectl_log"
: >"$direct_no_tls_openssl_log"
if (HELM_LOG_OVERRIDE="$direct_no_tls_helm_log" \
  KUBECTL_LOG_OVERRIDE="$direct_no_tls_kubectl_log" \
  OPENSSL_CONFIG_LOG_OVERRIDE="$direct_no_tls_openssl_log" \
  TLS_EXISTING_OVERRIDE=0 \
  DRY_RUN_OVERRIDE=0 \
  run_stack >"$direct_no_tls_out" 2>&1); then
  echo "direct deployment must require an existing trusted TLS secret" >&2
  failures=$((failures + 1))
fi
if [ -s "$direct_no_tls_helm_log" ] || [ -s "$direct_no_tls_openssl_log" ] || \
  grep -F -- 'create' "$direct_no_tls_kubectl_log" >/dev/null || \
  grep -F -- 'apply' "$direct_no_tls_kubectl_log" >/dev/null; then
  echo "a missing TLS secret must fail before any namespace, secret, or Helm mutation" >&2
  failures=$((failures + 1))
fi
if ! grep -F -- 'will not generate a self-signed' "$direct_no_tls_out" >/dev/null; then
  echo "the missing-TLS failure must state that no self-signed certificate is generated" >&2
  failures=$((failures + 1))
fi

ghcr_flux_flux_log="$fixture/ghcr-flux-flux.log"
ghcr_flux_helm_log="$fixture/ghcr-flux-helm.log"
ghcr_flux_kubectl_log="$fixture/ghcr-flux-kubectl.log"
ghcr_flux_out="$fixture/ghcr-flux.out"
: >"$ghcr_flux_flux_log"
: >"$ghcr_flux_helm_log"
: >"$ghcr_flux_kubectl_log"
if (HELM_LOG_OVERRIDE="$ghcr_flux_helm_log" \
  FLUX_LOG_OVERRIDE="$ghcr_flux_flux_log" \
  KUBECTL_LOG_OVERRIDE="$ghcr_flux_kubectl_log" \
  FLUX_ACTIVE=1 \
  GHCR_TOKEN_OVERRIDE=test-ghcr-token \
  DRY_RUN_OVERRIDE=0 \
  run_stack >"$ghcr_flux_out" 2>&1); then
  echo "GHCR_TOKEN must be rejected under Flux ownership" >&2
  failures=$((failures + 1))
fi
if [ -s "$ghcr_flux_flux_log" ] || [ -s "$ghcr_flux_helm_log" ] || \
  grep -F -- 'create secret' "$ghcr_flux_kubectl_log" >/dev/null; then
  echo "GHCR_TOKEN under Flux ownership must fail before any mutation" >&2
  failures=$((failures + 1))
fi
if ! grep -F -- 'GHCR_TOKEN is ignored under Flux ownership' "$ghcr_flux_out" >/dev/null; then
  echo "the GHCR_TOKEN rejection must explain how Flux pulls images" >&2
  failures=$((failures + 1))
fi

mismatched_tls_kubectl_log="$fixture/mismatched-tls-kubectl.log"
: >"$mismatched_tls_kubectl_log"
if (KUBECTL_LOG_OVERRIDE="$mismatched_tls_kubectl_log" \
  FLUX_ACTIVE=1 \
  MCP_TLS_SECRET_OVERRIDE=wrong-mcp-tls \
  DRY_RUN_OVERRIDE=0 \
  run_stack >/dev/null 2>&1); then
  echo "Flux-owned deployment must reject different core and MCP TLS Secrets" >&2
  failures=$((failures + 1))
fi
if grep -F -- 'rollout restart' "$mismatched_tls_kubectl_log" >/dev/null; then
  echo "mismatched Flux TLS Secrets must fail before workload restarts" >&2
  failures=$((failures + 1))
fi

mixed_helm_log="$fixture/mixed-helm.log"
mixed_flux_log="$fixture/mixed-flux.log"
mixed_kubectl_log="$fixture/mixed-kubectl.log"
: >"$mixed_helm_log"
: >"$mixed_flux_log"
: >"$mixed_kubectl_log"
if (HELM_LOG_OVERRIDE="$mixed_helm_log" \
  FLUX_LOG_OVERRIDE="$mixed_flux_log" \
  KUBECTL_LOG_OVERRIDE="$mixed_kubectl_log" \
  FLUX_ACTIVE_RELEASE_OVERRIDE=commoncal \
  DRY_RUN_OVERRIDE=0 \
  run_stack >/dev/null 2>&1); then
  echo "deploy must reject mixed Flux/direct ownership" >&2
  failures=$((failures + 1))
fi
if [ -s "$mixed_helm_log" ] || [ -s "$mixed_flux_log" ]; then
  echo "mixed ownership must fail before invoking Helm or Flux" >&2
  failures=$((failures + 1))
fi
if grep -F -- 'create secret' "$mixed_kubectl_log" >/dev/null; then
  echo "mixed ownership must fail before mutating runtime secrets" >&2
  failures=$((failures + 1))
fi

flux_dry_run_helm_log="$fixture/flux-dry-run-helm.log"
flux_dry_run_flux_log="$fixture/flux-dry-run-flux.log"
flux_dry_run_kubectl_log="$fixture/flux-dry-run-kubectl.log"
: >"$flux_dry_run_helm_log"
: >"$flux_dry_run_flux_log"
: >"$flux_dry_run_kubectl_log"
if ! (HELM_LOG_OVERRIDE="$flux_dry_run_helm_log" \
  FLUX_LOG_OVERRIDE="$flux_dry_run_flux_log" \
  KUBECTL_LOG_OVERRIDE="$flux_dry_run_kubectl_log" \
  FLUX_ACTIVE=1 \
  DRY_RUN_OVERRIDE=1 \
  run_stack >/dev/null 2>&1); then
  echo "Flux-owned dry-run must validate successfully" >&2
  failures=$((failures + 1))
fi
if [ -s "$flux_dry_run_helm_log" ] || [ -s "$flux_dry_run_flux_log" ]; then
  echo "Flux-owned dry-run must not invoke Helm or reconcile Flux" >&2
  failures=$((failures + 1))
fi
require_text \
  'apply --dry-run=server -f -' \
  "$flux_dry_run_kubectl_log" \
  "Flux-owned dry-run must server-validate runtime secret manifests"
if grep -F -- 'rollout restart' "$flux_dry_run_kubectl_log" >/dev/null; then
  echo "Flux-owned dry-run must not restart workloads" >&2
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
if ! (DRY_RUN_OVERRIDE=0 \
  TLS_EXISTING_OVERRIDE=1 \
  KUBECTL_LOG_OVERRIDE="$rollout_log" \
  run_stack >/dev/null 2>&1); then
  echo "direct deployment must wait for the core and MCP rollouts" >&2
  failures=$((failures + 1))
fi
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
