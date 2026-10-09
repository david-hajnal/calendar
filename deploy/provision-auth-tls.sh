#!/usr/bin/env bash
# Provision only the auth origin TLS Secret; no workloads or Flux changes.
set -euo pipefail
set +x
: "${KUBECONFIG:?Configure the intended cluster}"
source "$(dirname "${BASH_SOURCE[0]}")/auth-tls.sh"
auth_tls_args=()
case "${DRY_RUN:-0}" in
  0|'') ;;
  1) auth_tls_args=(--dry-run=server) ;;
  *) echo 'ERROR: DRY_RUN must be 0 or 1' >&2; exit 1 ;;
esac
auth_tls_secret "${NAMESPACE:-commoncal}" provision ${auth_tls_args[@]+"${auth_tls_args[@]}"}
