#!/usr/bin/env bash
# Provision SQLite auth secrets. No database passwords, certificate authority or server.
set -euo pipefail
set +x
: "${KUBECONFIG:?Configure the intended cluster}"
: "${AUTH_BACKUP_AGE_RECIPIENT:?Supply the age recipient; keep its identity off-cluster}"
export AUTH_BACKUP_AGE_RECIPIENT
namespace=${NAMESPACE:-commoncal}
deploy_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
source "$deploy_dir/auth-secret.sh"
apply_auth_backup_secret "$namespace" apply -f -
apply_auth_secret "$namespace" apply -f -
echo 'SQLite auth and encrypted-backup secrets prepared.'
