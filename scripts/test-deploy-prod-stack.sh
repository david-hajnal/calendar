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
: >"$fixture/openssl-cmd.log"
: >"$fixture/cmd-seq.log"

cat >"$fixture/bin/kubectl" <<'EOF'
#!/bin/sh
set -eu

printf '%s\n' "$*" >>"$KUBECTL_LOG"
if [ -n "${CMD_SEQ_LOG:-}" ]; then
  printf 'CMDSEQ kubectl %s\n' "$*" >>"$CMD_SEQ_LOG"
fi

case "${1:-} ${2:-} ${3:-}" in
  "config current-context ")
    printf '%s\n' test-context
    ;;
  "get namespace "*)
    ;;
  "get secret "*)
    # Existence precedence:
    #   1. The Secret was created earlier in this run (state file) — models the
    #      absent -> generated -> present transition.
    #   2. TLS_SECRET_PRESENT set explicitly by a scenario.
    #   3. Legacy FLUX_ACTIVE/TLS_EXISTING signals for backward compatibility.
    if [ -n "${TLS_SECRET_STATE:-}" ] && [ -s "$TLS_SECRET_STATE" ]; then
      present=1
    elif [ -n "${TLS_SECRET_PRESENT:-}" ]; then
      present="$TLS_SECRET_PRESENT"
    elif [ "${FLUX_ACTIVE:-0}" = 1 ] || [ "${TLS_EXISTING:-0}" = 1 ]; then
      present=1
    else
      present=0
    fi
    if [ "$present" != 1 ]; then
      exit 1
    fi
    case "$*" in
      *'jsonpath={.type}'*)
        printf '%s' "${TLS_SECRET_TYPE:-kubernetes.io/tls}"
        ;;
      *'jsonpath={.data.tls\.crt}'*)
        printf '%s' "${TLS_SECRET_CRT_B64:-ZHVtbXk=}"
        ;;
      *'jsonpath={.data.tls\.key}'*)
        if [ "${TLS_SECRET_HAS_KEY:-1}" = 0 ]; then
          exit 0
        fi
        printf '%s' "${TLS_SECRET_KEY_B64:-ZHVtbXk=}"
        ;;
      *)
        printf '%s' ZHVtbXk=
        ;;
    esac
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
          printf '%s' "${MCP_TLS_SECRET:-commoncal-tls}"
        else
          printf '%s' commoncal-tls
        fi
        ;;
      *)
        if [ "${FLUX_ACTIVE:-0}" = 1 ] || [ "${FLUX_ACTIVE_RELEASE:-}" = "$3" ]; then
          printf '%s' "$3"
        fi
        ;;
    esac
    ;;
  "create secret tls")
    # Record the arguments (the top-level log already captured the command
    # line) and consume the certificate/key file paths without ever reading or
    # logging their contents.
    name=$4
    cert_file=
    key_file=
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --cert=*) cert_file=${1#--cert=} ;;
        --key=*) key_file=${1#--key=} ;;
      esac
      shift
    done
    if [ -n "$cert_file" ] && [ ! -f "$cert_file" ]; then
      echo "mock kubectl: certificate file not found: $cert_file" >&2
      exit 1
    fi
    if [ -n "$key_file" ] && [ ! -f "$key_file" ]; then
      echo "mock kubectl: key file not found: $key_file" >&2
      exit 1
    fi
    # Record that the Secret now exists so later `get secret` calls in the same
    # run observe the absent -> created transition.
    if [ -n "${TLS_SECRET_STATE:-}" ]; then
      printf '%s\n' "$name" >"$TLS_SECRET_STATE"
    fi
    printf '%s\n' 'apiVersion: v1' 'kind: Secret' 'metadata:' "  name: $name" 'type: kubernetes.io/tls'
    ;;
  "create secret generic")
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
    ;;
esac
EOF

cat >"$fixture/bin/openssl" <<'EOF'
#!/bin/sh
set -eu

cmd="${1:-}"
if [ -n "${OPENSSL_CMD_LOG:-}" ]; then
  printf '%s\n' "$*" >>"$OPENSSL_CMD_LOG"
fi
if [ -n "${CMD_SEQ_LOG:-}" ]; then
  printf 'CMDSEQ openssl %s\n' "$*" >>"$CMD_SEQ_LOG"
fi

case "$cmd" in
  base64)
    # openssl base64 -d -A : base64-decode stdin to stdout.
    base64 -d
    exit 0
    ;;

  req)
    # Self-signed generation:
    #   openssl req -x509 -newkey rsa:2048 -sha256 -days 365 -nodes \
    #     -keyout <key> -out <cert> -config <config>
    config=
    keyout=
    certout=
    while [ "$#" -gt 0 ]; do
      case "$1" in
        -config) shift; config=$1 ;;
        -keyout) shift; keyout=$1 ;;
        -out) shift; certout=$1 ;;
      esac
      shift
    done
    if [ -n "${OPENSSL_CONFIG_LOG:-}" ] && [ -n "$config" ] && [ -f "$config" ]; then
      cp "$config" "$OPENSSL_CONFIG_LOG"
    fi
    : >"$keyout"
    : >"$certout"
    exit 0
    ;;

  x509)
    infile=
    mode=
    host=
    seconds=
    while [ "$#" -gt 0 ]; do
      case "$1" in
        -in) shift; infile=$1 ;;
        -checkhost) shift; host=$1; mode=checkhost ;;
        -checkend) shift; seconds=$1; mode=checkend ;;
        -pubkey) mode=pubkey ;;
        -text) mode=text ;;
        -noout) ;;
      esac
      shift
    done
    case "$mode" in
      checkhost)
        sans=$(printf '%s' "${TLS_CERT_SANS:-}" | tr -d '[:space:]')
        case ",$sans," in
          *",DNS:$host,"*) exit 0 ;;
          *) exit 1 ;;
        esac
        ;;
      checkend)
        days="${TLS_EXPIRY_DAYS:-365}"
        if [ "$days" -lt "$((seconds / 86400))" ]; then
          exit 1
        fi
        exit 0
        ;;
      pubkey)
        printf '%s\n' \
          '-----BEGIN PUBLIC KEY-----' \
          "MOCK-PUBLIC-KEY ${TLS_KEY_ID:-origin}" \
          '-----END PUBLIC KEY-----'
        ;;
      text)
        if [ "${TLS_CERT_VALID:-1}" = 0 ]; then
          exit 1
        fi
        printf '%s\n' \
          'X509v3 Subject Alternative Name:' \
          "    ${TLS_CERT_SANS:-DNS:calendar.example.test, DNS:mcp.example.test}"
        ;;
      *)
        # Parse / inspection: succeed.
        exit 0
        ;;
    esac
    exit 0
    ;;

  pkey)
    infile=
    pubout=0
    while [ "$#" -gt 0 ]; do
      case "$1" in
        -in) shift; infile=$1 ;;
        -pubout) pubout=1 ;;
      esac
      shift
    done
    if [ "$pubout" = 1 ]; then
      # A missing/empty private key cannot produce a public key.
      if [ -n "$infile" ] && [ ! -s "$infile" ]; then
        echo "mock openssl: unable to load private key" >&2
        exit 1
      fi
      if [ "${TLS_KEY_MATCH:-1}" = 1 ]; then
        printf '%s\n' \
          '-----BEGIN PUBLIC KEY-----' \
          "MOCK-PUBLIC-KEY ${TLS_KEY_ID:-origin}" \
          '-----END PUBLIC KEY-----'
      else
        printf '%s\n' \
          '-----BEGIN PUBLIC KEY-----' \
          'MOCK-PUBLIC-KEY MISMATCHED' \
          '-----END PUBLIC KEY-----'
      fi
    fi
    exit 0
    ;;
esac

exit 0
EOF

cat >"$fixture/bin/helm" <<'EOF'
#!/bin/sh
set -eu

if [ -n "${CMD_SEQ_LOG:-}" ]; then
  printf 'CMDSEQ helm %s\n' "$*" >>"$CMD_SEQ_LOG"
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
if [ -n "${CMD_SEQ_LOG:-}" ]; then
  printf 'CMDSEQ flux %s\n' "$*" >>"$CMD_SEQ_LOG"
fi
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
    OPENSSL_CMD_LOG="${OPENSSL_CMD_LOG_OVERRIDE:-$fixture/openssl-cmd.log}" \
    CMD_SEQ_LOG="${CMD_SEQ_LOG_OVERRIDE:-$fixture/cmd-seq.log}" \
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
    TLS_CERT_SANS="${TLS_CERT_SANS_OVERRIDE:-DNS:calendar.example.test, DNS:mcp.example.test}" \
    TLS_SECRET_PRESENT="${TLS_SECRET_PRESENT_OVERRIDE:-}" \
    TLS_SECRET_TYPE="${TLS_SECRET_TYPE_OVERRIDE:-kubernetes.io/tls}" \
    TLS_SECRET_CRT_B64="${TLS_SECRET_CRT_B64_OVERRIDE:-ZHVtbXk=}" \
    TLS_SECRET_KEY_B64="${TLS_SECRET_KEY_B64_OVERRIDE:-ZHVtbXk=}" \
    TLS_KEY_MATCH="${TLS_KEY_MATCH_OVERRIDE:-1}" \
    TLS_EXPIRY_DAYS="${TLS_EXPIRY_DAYS_OVERRIDE:-365}" \
    TLS_KEY_ID="${TLS_KEY_ID_OVERRIDE:-origin}" \
    TLS_SECRET_HAS_KEY="${TLS_SECRET_HAS_KEY_OVERRIDE:-1}" \
    TLS_CERT_VALID="${TLS_CERT_VALID_OVERRIDE:-1}" \
    TLS_WORKDIR="${TLS_WORKDIR_OVERRIDE:-}" \
    TLS_SECRET_STATE="${TLS_SECRET_STATE_OVERRIDE:-$fixture/tls-secret-state}" \
    TLS_SECRET_NAME=commoncal-tls \
    FLUX_ACTIVE_RELEASE="${FLUX_ACTIVE_RELEASE_OVERRIDE:-}" \
    MCP_TLS_SECRET="${MCP_TLS_SECRET_OVERRIDE:-commoncal-tls}" \
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
  'ingress.tls[0].secretName=commoncal-tls' \
  "$fixture/helm.log" \
  "both ingresses must reference the managed TLS secret"
tls_reference_count=$(grep -F -x -c -- 'ingress.tls[0].secretName=commoncal-tls' "$fixture/helm.log" || true)
if [ "$tls_reference_count" -ne 2 ]; then
  echo "expected core and MCP to each reference commoncal-tls; found $tls_reference_count reference(s)" >&2
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
  'rollout restart statefulset commoncal --namespace commoncal' \
  "$guard_kubectl_log" \
  "Flux-owned deployment must restart core after applying external Secrets"
require_text \
  'rollout status deployment commoncal-mcp --namespace commoncal --timeout=15m' \
  "$guard_kubectl_log" \
  "Flux-owned deployment must wait for the MCP rollout"
# A present, valid origin TLS Secret must be validated and reused, not regenerated.
if grep -F -- 'create secret tls' "$guard_kubectl_log" >/dev/null; then
  echo "a present valid origin TLS Secret must not be regenerated under Flux ownership" >&2
  failures=$((failures + 1))
fi
if [ -s "$guard_openssl_log" ]; then
  echo "a present valid origin TLS Secret must not trigger certificate generation" >&2
  failures=$((failures + 1))
fi

# TLS-02: first-run self-signed Secret creation (Flux mode, secret absent).
first_run_helm_log="$fixture/first-run-helm.log"
first_run_flux_log="$fixture/first-run-flux.log"
first_run_kubectl_log="$fixture/first-run-kubectl.log"
first_run_stdin_log="$fixture/first-run-stdin.log"
first_run_openssl_config_log="$fixture/first-run-openssl-config.log"
first_run_openssl_cmd_log="$fixture/first-run-openssl-cmd.log"
first_run_seq_log="$fixture/first-run-seq.log"
: >"$first_run_helm_log"
: >"$first_run_flux_log"
: >"$first_run_kubectl_log"
: >"$first_run_stdin_log"
: >"$first_run_openssl_config_log"
: >"$first_run_openssl_cmd_log"
: >"$first_run_seq_log"
rm -f "$fixture/tls-secret-state"
if ! (HELM_LOG_OVERRIDE="$first_run_helm_log" \
  FLUX_LOG_OVERRIDE="$first_run_flux_log" \
  KUBECTL_LOG_OVERRIDE="$first_run_kubectl_log" \
  KUBECTL_STDIN_LOG_OVERRIDE="$first_run_stdin_log" \
  OPENSSL_CONFIG_LOG_OVERRIDE="$first_run_openssl_config_log" \
  OPENSSL_CMD_LOG_OVERRIDE="$first_run_openssl_cmd_log" \
  CMD_SEQ_LOG_OVERRIDE="$first_run_seq_log" \
  FLUX_ACTIVE=1 \
  TLS_SECRET_PRESENT_OVERRIDE=0 \
  DRY_RUN_OVERRIDE=0 \
  run_stack >/dev/null 2>&1); then
  echo "Flux-owned first run must create a self-signed TLS Secret when absent" >&2
  failures=$((failures + 1))
fi
require_text \
  'req -x509 -newkey rsa:2048 -sha256 -days 365' \
  "$first_run_openssl_cmd_log" \
  "first run must generate an RSA 2048-bit, SHA-256, 365-day self-signed certificate"
require_text \
  'DNS:calendar.example.test' \
  "$first_run_openssl_config_log" \
  "the generated certificate SANs must include the configured DOMAIN"
require_text \
  'DNS:mcp.example.test' \
  "$first_run_openssl_config_log" \
  "the generated certificate SANs must include the configured MCP_DOMAIN"
if ! grep -F -- 'create secret tls commoncal-tls' "$first_run_kubectl_log" \
  | grep -F -- '-n commoncal' >/dev/null; then
  echo "first run must create the TLS Secret in NAMESPACE using TLS_SECRET_NAME" >&2
  failures=$((failures + 1))
fi
require_text \
  'type: kubernetes.io/tls' \
  "$first_run_stdin_log" \
  "the applied TLS Secret must be of type kubernetes.io/tls"
first_run_create_line=$(grep -n -F -- 'CMDSEQ kubectl create secret tls commoncal-tls' "$first_run_seq_log" | head -1 | cut -d: -f1 || true)
first_run_reconcile_line=$(grep -n -F -- 'CMDSEQ flux reconcile helmrelease commoncal' "$first_run_seq_log" | head -1 | cut -d: -f1 || true)
if [ -z "$first_run_create_line" ] || [ -z "$first_run_reconcile_line" ] || \
  [ "$first_run_create_line" -ge "$first_run_reconcile_line" ]; then
  echo "TLS Secret generation must happen before Flux reconciliation" >&2
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

# TLS-07: direct Helm mode with an absent origin TLS Secret must generate a
# self-signed certificate and proceed (the old "never generate" contract is gone).
direct_gen_helm_log="$fixture/direct-gen-helm.log"
direct_gen_kubectl_log="$fixture/direct-gen-kubectl.log"
direct_gen_openssl_cmd_log="$fixture/direct-gen-openssl-cmd.log"
direct_gen_out="$fixture/direct-gen.out"
: >"$direct_gen_helm_log"
: >"$direct_gen_kubectl_log"
: >"$direct_gen_openssl_cmd_log"
rm -f "$fixture/tls-secret-state"
if ! (HELM_LOG_OVERRIDE="$direct_gen_helm_log" \
  KUBECTL_LOG_OVERRIDE="$direct_gen_kubectl_log" \
  OPENSSL_CMD_LOG_OVERRIDE="$direct_gen_openssl_cmd_log" \
  TLS_EXISTING_OVERRIDE=0 \
  DRY_RUN_OVERRIDE=0 \
  run_stack >"$direct_gen_out" 2>&1); then
  echo "direct deployment must generate a self-signed TLS Secret when absent" >&2
  failures=$((failures + 1))
fi
require_text \
  'req -x509 -newkey rsa:2048 -sha256 -days 365' \
  "$direct_gen_openssl_cmd_log" \
  "direct deployment must generate an RSA 2048-bit, SHA-256, 365-day certificate"
require_text \
  'create secret tls commoncal-tls' \
  "$direct_gen_kubectl_log" \
  "direct deployment must create the shared origin TLS Secret"
require_text \
  'BEGIN release=commoncal chart=commoncal resource=commoncal' \
  "$direct_gen_helm_log" \
  "direct deployment must proceed to the core Helm release after generating TLS"

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

# TLS-03 focused failure paths: an existing but invalid TLS Secret must be
# rejected before any Helm mutation. Each scenario runs in direct Helm mode.
# Run a direct-Helm-mode scenario against an existing-but-invalid TLS Secret and
# assert the deployment is rejected before any Helm mutation. The per-scenario
# override is set as a regular shell variable (not a command prefix) because
# bash 3.2 POSIX mode mis-handles expanded words in the prefix position.
assert_invalid_tls_rejected() {
  local label="$1"
  local helm_log="$fixture/invalid-tls-$label-helm.log"
  local kubectl_log="$fixture/invalid-tls-$label-kubectl.log"
  local out="$fixture/invalid-tls-$label.out"
  local workdir="$fixture/invalid-tls-$label-workdir"
  : >"$helm_log"
  : >"$kubectl_log"
  : >"$out"
  mkdir -p "$workdir"
  rm -f "$fixture/tls-secret-state"
  case "$label" in
    wrong_type)     TLS_SECRET_TYPE_OVERRIDE=Opaque ;;
    missing_key)    TLS_SECRET_HAS_KEY_OVERRIDE=0 ;;
    malformed_cert) TLS_CERT_VALID_OVERRIDE=0 ;;
    key_mismatch)   TLS_KEY_MATCH_OVERRIDE=0 ;;
    near_expiry)    TLS_EXPIRY_DAYS_OVERRIDE=10 ;;
  esac
  if (HELM_LOG_OVERRIDE="$helm_log" \
    KUBECTL_LOG_OVERRIDE="$kubectl_log" \
    TLS_WORKDIR_OVERRIDE="$workdir" \
    TLS_EXISTING_OVERRIDE=1 \
    run_stack >"$out" 2>&1); then
    echo "invalid TLS secret ($label) must be rejected" >&2
    failures=$((failures + 1))
  fi
  case "$label" in
    wrong_type)     unset TLS_SECRET_TYPE_OVERRIDE ;;
    missing_key)    unset TLS_SECRET_HAS_KEY_OVERRIDE ;;
    malformed_cert) unset TLS_CERT_VALID_OVERRIDE ;;
    key_mismatch)   unset TLS_KEY_MATCH_OVERRIDE ;;
    near_expiry)    unset TLS_EXPIRY_DAYS_OVERRIDE ;;
  esac
  if [ -s "$helm_log" ]; then
    echo "invalid TLS secret ($label) must fail before invoking Helm" >&2
    failures=$((failures + 1))
  fi
  if grep -F -- 'create secret' "$kubectl_log" >/dev/null || \
    grep -F -x -- 'apply -f -' "$kubectl_log" >/dev/null; then
    echo "invalid TLS secret ($label) must fail before mutating secrets" >&2
    failures=$((failures + 1))
  fi
  if [ -e "$workdir" ]; then
    echo "invalid TLS secret ($label) must leave no temporary TLS files behind" >&2
    failures=$((failures + 1))
  fi
}

assert_invalid_tls_rejected wrong_type
if ! grep -F -- 'expected kubernetes.io/tls' "$fixture/invalid-tls-wrong_type.out" >/dev/null; then
  echo "a wrong Secret type must be reported as such" >&2
  failures=$((failures + 1))
fi

assert_invalid_tls_rejected missing_key
if ! grep -F -- 'does not match its certificate' "$fixture/invalid-tls-missing_key.out" >/dev/null; then
  echo "a missing private key must be reported as a key/certificate mismatch" >&2
  failures=$((failures + 1))
fi

assert_invalid_tls_rejected malformed_cert
if ! grep -F -- 'does not contain a valid TLS certificate' "$fixture/invalid-tls-malformed_cert.out" >/dev/null; then
  echo "a malformed certificate must be reported as such" >&2
  failures=$((failures + 1))
fi

assert_invalid_tls_rejected key_mismatch
if ! grep -F -- 'does not match its certificate' "$fixture/invalid-tls-key_mismatch.out" >/dev/null; then
  echo "a mismatched private key must be reported as such" >&2
  failures=$((failures + 1))
fi

assert_invalid_tls_rejected near_expiry
if ! grep -F -- 'expires within 30 days' "$fixture/invalid-tls-near_expiry.out" >/dev/null; then
  echo "a certificate expiring within 30 days must be reported as such" >&2
  failures=$((failures + 1))
fi

# TLS-05: a valid existing TLS Secret must be reused without any generation or
# apply, and must leave no temporary TLS files behind.
reuse_flux_log="$fixture/reuse-flux.log"
reuse_kubectl_log="$fixture/reuse-kubectl.log"
reuse_openssl_cmd_log="$fixture/reuse-openssl-cmd.log"
reuse_workdir="$fixture/reuse-workdir"
: >"$reuse_flux_log"
: >"$reuse_kubectl_log"
: >"$reuse_openssl_cmd_log"
mkdir -p "$reuse_workdir"
rm -f "$fixture/tls-secret-state"
if ! (FLUX_LOG_OVERRIDE="$reuse_flux_log" \
  KUBECTL_LOG_OVERRIDE="$reuse_kubectl_log" \
  OPENSSL_CMD_LOG_OVERRIDE="$reuse_openssl_cmd_log" \
  FLUX_ACTIVE=1 \
  TLS_SECRET_PRESENT_OVERRIDE=1 \
  TLS_WORKDIR_OVERRIDE="$reuse_workdir" \
  DRY_RUN_OVERRIDE=0 \
  run_stack >/dev/null 2>&1); then
  echo "a valid existing TLS Secret must be reused and the deployment must proceed" >&2
  failures=$((failures + 1))
fi
if grep -F -- 'create secret tls' "$reuse_kubectl_log" >/dev/null; then
  echo "a valid existing TLS Secret must not be regenerated or re-applied" >&2
  failures=$((failures + 1))
fi
if grep -F -- 'req -x509' "$reuse_openssl_cmd_log" >/dev/null; then
  echo "a valid existing TLS Secret must not trigger certificate generation" >&2
  failures=$((failures + 1))
fi
if [ -e "$reuse_workdir" ]; then
  echo "a reuse scenario must leave no temporary TLS files behind" >&2
  failures=$((failures + 1))
fi

# TLS-05: a dry-run first run must generate material and server-dry-run the
# Secret without persisting any TLS Secret state.
dryrun_kubectl_log="$fixture/dryrun-kubectl.log"
dryrun_stdin_log="$fixture/dryrun-stdin.log"
dryrun_openssl_cmd_log="$fixture/dryrun-openssl-cmd.log"
dryrun_workdir="$fixture/dryrun-workdir"
: >"$dryrun_kubectl_log"
: >"$dryrun_stdin_log"
: >"$dryrun_openssl_cmd_log"
mkdir -p "$dryrun_workdir"
rm -f "$fixture/tls-secret-state"
if ! (KUBECTL_LOG_OVERRIDE="$dryrun_kubectl_log" \
  KUBECTL_STDIN_LOG_OVERRIDE="$dryrun_stdin_log" \
  OPENSSL_CMD_LOG_OVERRIDE="$dryrun_openssl_cmd_log" \
  TLS_SECRET_PRESENT_OVERRIDE=0 \
  TLS_WORKDIR_OVERRIDE="$dryrun_workdir" \
  DRY_RUN_OVERRIDE=1 \
  run_stack >/dev/null 2>&1); then
  echo "a dry-run first run must validate the self-signed Secret without persisting it" >&2
  failures=$((failures + 1))
fi
require_text \
  'req -x509 -newkey rsa:2048 -sha256 -days 365' \
  "$dryrun_openssl_cmd_log" \
  "a dry-run first run must still generate the self-signed certificate material"
require_text \
  'create secret tls commoncal-tls' \
  "$dryrun_kubectl_log" \
  "a dry-run first run must render the TLS Secret"
require_text \
  'apply --dry-run=server -f -' \
  "$dryrun_kubectl_log" \
  "a dry-run first run must server-dry-run the TLS Secret apply"
if grep -F -x -- 'apply -f -' "$dryrun_kubectl_log" >/dev/null; then
  echo "a dry-run first run must not persist the TLS Secret" >&2
  failures=$((failures + 1))
fi
if [ -e "$dryrun_workdir" ]; then
  echo "a dry-run first run must leave no temporary TLS files behind" >&2
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
