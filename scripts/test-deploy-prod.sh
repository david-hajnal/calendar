#!/bin/sh
set -eu

repository_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
deploy_script="$repository_root/deploy/deploy-prod.sh"
mcp_deploy_script="$repository_root/deploy/deploy-mcp-prod.sh"
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT HUP INT TERM

mkdir "$fixture/bin"

cat >"$fixture/bin/kubectl" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$KUBECTL_LOG"
if [ "${1:-}" = create ]; then
  printf '%s\n' 'apiVersion: v1' 'kind: Secret'
else
  cat >/dev/null
fi
EOF

cat >"$fixture/bin/helm" <<'EOF'
#!/bin/sh
printf '%s\n' "$@" >"$HELM_LOG"
EOF

cat >"$fixture/bin/docker" <<'EOF'
#!/bin/sh
echo 'deploy scripts must not invoke docker' >&2
exit 99
EOF

chmod +x "$fixture/bin/kubectl" "$fixture/bin/helm" "$fixture/bin/docker"

run_deploy() {
  PATH="$fixture/bin:$PATH" \
    SESSION_SECRET=test-session-secret \
    BACKUP_ENCRYPTION_KEY_HEX="${BACKUP_ENCRYPTION_KEY_HEX:-0000000000000000000000000000000000000000000000000000000000000000}" \
    IMAGE_TAG="${IMAGE_TAG:-test-image-tag}" \
    KUBECTL_LOG="$fixture/kubectl.log" \
    HELM_LOG="$fixture/helm.log" \
    "$deploy_script"
}

assert_helm_argument() {
  if ! grep -Fx -- "$1" "$fixture/helm.log" >/dev/null; then
    echo "expected Helm argument: $1" >&2
    exit 1
  fi
}

assert_no_helm_argument() {
  if grep -Fx -- "$1" "$fixture/helm.log" >/dev/null; then
    echo "unexpected Helm argument: $1" >&2
    exit 1
  fi
}

run_deploy >/dev/null

if grep -Fx '' "$fixture/helm.log" >/dev/null; then
  echo "Helm received an empty argument" >&2
  exit 1
fi

assert_helm_argument upgrade
assert_helm_argument commoncal
assert_helm_argument "$repository_root/deploy/helm/commoncal"
assert_helm_argument "$repository_root/deploy/values-production.yaml"
assert_helm_argument --reset-values
assert_no_helm_argument --wait

if ! grep -F -- 'rollout status statefulset --selector app.kubernetes.io/instance=commoncal --namespace production --timeout=15m' "$fixture/kubectl.log" >/dev/null; then
  echo "core deploy should wait for the StatefulSet rollout" >&2
  exit 1
fi

DRY_RUN=0 run_deploy >/dev/null
if grep -Fx -- --dry-run "$fixture/helm.log" >/dev/null; then
  echo "DRY_RUN=0 should not enable Helm dry-run" >&2
  exit 1
fi

: >"$fixture/kubectl.log"
DOMAIN=calendar.example.test \
TLS_SECRET_NAME=calendar-example-tls \
IMAGE_TAG=2026.08.14 \
DRY_RUN=1 \
run_deploy >/dev/null

assert_helm_argument image.tag=2026.08.14
assert_helm_argument domain=calendar.example.test
assert_helm_argument config.appOrigin=https://calendar.example.test
assert_helm_argument 'ingress.hosts[0].host=calendar.example.test'
assert_helm_argument 'ingress.tls[0].secretName=calendar-example-tls'
assert_helm_argument 'ingress.tls[0].hosts[0]=calendar.example.test'
assert_helm_argument --dry-run

if ! grep -F -- 'apply --dry-run=server -f -' "$fixture/kubectl.log" >/dev/null; then
  echo "DRY_RUN=1 should not apply the Secret" >&2
  exit 1
fi

if DRY_RUN=yes run_deploy >/dev/null 2>&1; then
  echo "expected invalid DRY_RUN value to fail" >&2
  exit 1
fi

if BACKUP_ENCRYPTION_KEY_HEX=not-a-valid-key run_deploy >/dev/null 2>&1; then
  echo "expected invalid backup encryption key to fail" >&2
  exit 1
fi

unset DRY_RUN
env_deploy_dir="$fixture/deploy"
cp -R "$repository_root/deploy" "$env_deploy_dir"
cat >"$env_deploy_dir/.env" <<'EOF'
SESSION_SECRET=env-session-secret
BACKUP_ENCRYPTION_KEY_HEX=1111111111111111111111111111111111111111111111111111111111111111
CALENDAR_API_URL=http://commoncal:3000/api
IMAGE_TAG=from-dot-env
EOF

PATH="$fixture/bin:$PATH" \
  KUBECTL_LOG="$fixture/kubectl.log" \
  HELM_LOG="$fixture/helm.log" \
  "$env_deploy_dir/deploy-prod.sh" >/dev/null
assert_helm_argument image.tag=from-dot-env

PATH="$fixture/bin:$PATH" \
  DOMAIN=calendar.example.test \
  KUBECTL_LOG="$fixture/kubectl.log" \
  HELM_LOG="$fixture/helm.log" \
  "$env_deploy_dir/deploy-mcp-prod.sh" >/dev/null
assert_helm_argument image.tag=from-dot-env
assert_helm_argument domain=calendar.example.test
assert_helm_argument --reset-values
assert_helm_argument --wait
assert_no_helm_argument 'ingress.hosts[0].host=calendar.example.test'
