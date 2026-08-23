#!/usr/bin/env bash
set -eu

# Tests for deploy/sqlite-prod.sh using stubbed kubectl/helm.
# Usage: sh scripts/test-sqlite-prod.sh

export repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
test_dir=$(mktemp -d)
trap 'rm -rf "$test_dir"' EXIT

# Create stub kubectl that simulates the production environment
mkdir -p "$test_dir/bin"

cat > "$test_dir/bin/kubectl" <<'STUB'
#!/usr/bin/env bash
set -eu

# Simulated kubectl for testing
ACTION="${1:-}"
NAMESPACE="commoncal"
SELECTOR=""
FIELD_SELECTOR=""
NO_HEADERS=0
OUTPUT=""
POD_NAME=""
STATEFULSET_NAME=""
JSONPATH=""

# Parse arguments
while [ $# -gt 0 ]; do
  case "$1" in
    -n|--namespace) shift; NAMESPACE="$1" ;;
    --selector) shift; SELECTOR="$1" ;;
    --field-selector) shift; FIELD_SELECTOR="$1" ;;
    --no-headers) NO_HEADERS=1 ;;
    -o) shift; OUTPUT="$1" ;;
    jsonpath=*) JSONPATH="${1#jsonpath=}" ;;
    jsonpath) shift; JSONPATH="$1" ;;
    *)
      # Positional args
      if [ -z "$POD_NAME" ]; then
        POD_NAME="$1"
      fi
      ;;
  esac
  shift
done

case "$ACTION" in
  get)
    # Handle "kubectl get pods --selector=... --field-selector=..."
    if [ "$OUTPUT" = "jsonpath=*" ] || [ "${OUTPUT:0:8}" = "jsonpath" ]; then
      case "$JSONPATH" in
        *.spec.nodeName*) echo "prod-node-1" ;;
        *.image*) echo "ghcr.io/david-hajnal/calendar-core:v1.2.3" ;;
        *.volumeClaimTemplates*) echo "commoncal-data" ;;
      esac
    elif [ -n "$POD_NAME" ]; then
      # kubectl get pod <name>
      if [ "$POD_NAME" = "commoncal-sqlite-console" ]; then
        # No existing console pod
        exit 1
      fi
    else
      # List pods
      if echo "$SELECTOR" | grep -q "app.kubernetes.io/name=commoncal" && \
         echo "$FIELD_SELECTOR" | grep -q "status.phase=Running"; then
        echo "commoncal-0   1/1     Running   0          10m"
      fi
    fi
    ;;
  exec)
    # kubectl exec <pod> -- test -f <path>
    if echo "$@" | grep -q "test -f"; then
      echo "yes"
    fi
    ;;
  apply)
    # kubectl apply -f -
    # Read from stdin
    cat > /dev/null
    ;;
  delete)
    # kubectl delete pod <name>
    ;;
  describe)
    # kubectl describe <type> <name>
    ;;
  config)
    # kubectl config view --minify --output='jsonpath=...'
    # Mimic real kubectl: print the server from the kubeconfig, or nothing
    # (exit 0) when the current-context cluster has no server field.
    if echo "$*" | grep -q "cluster.server"; then
      if [ -n "${KUBECONFIG:-}" ] && [ -f "$KUBECONFIG" ]; then
        grep -o 'server: .*' "$KUBECONFIG" | head -1 | sed 's/^server: //'
      fi
    fi
    ;;
  *)
    echo "kubectl stub: unhandled action: $ACTION" >&2
    exit 1
    ;;
esac
STUB
chmod +x "$test_dir/bin/kubectl"

# Stub busybox (for the pod's sleep command - not needed for script tests)
cat > "$test_dir/bin/busybox" <<'STUB'
#!/usr/bin/env bash
exit 0
STUB
chmod +x "$test_dir/bin/busybox"

# Override PATH so the script uses our stubs
export PATH="$test_dir/bin:$PATH"

# Create a fake kubeconfig pointing to loopback
mkdir -p "$test_dir/etc/rancher/k3s"
cat > "$test_dir/etc/rancher/k3s/k3s.yaml" <<'EOF'
apiVersion: v1
kind: Config
clusters:
  - cluster:
      server: https://127.0.0.1:6443
      insecure-skip-tls-verify: true
    name: k3s
current-context: k3s
contexts:
  - context:
      cluster: k3s
      user: admin
    name: k3s
users:
  - name: admin
    user:
      token: fake-token
EOF

export KUBECONFIG="$test_dir/etc/rancher/k3s/k3s.yaml"

# --- Test helpers ---
PASSED=0
FAILED=0

assert_pass() {
  local desc="$1"
  shift
  if "$@" &>/dev/null; then
    echo "PASS: $desc"
    PASSED=$((PASSED + 1))
  else
    echo "FAIL: $desc (expected to pass)" >&2
    FAILED=$((FAILED + 1))
  fi
}

assert_fail() {
  local desc="$1"
  shift
  if "$@" &>/dev/null; then
    echo "FAIL: $desc (expected to fail)" >&2
    FAILED=$((FAILED + 1))
  else
    echo "PASS: $desc"
    PASSED=$((PASSED + 1))
  fi
}

# --- Tests ---
echo "=== Testing deploy/sqlite-prod.sh ==="

# 1. Default mode is read-only (no --write flag)
#    We can't fully test this without a real tty, but we can verify the script parses args correctly
# 2. --write flag enables write mode
# 3. Remote kubeconfig is rejected
# 4. Missing kubeconfig is rejected
# 5. Non-interactive use is rejected
# 6. Missing database/PVC/image fails
# 7. Ambiguous workloads fails
# 8. Existing console pod fails
# 9. Missing deny-all policy fails

# Test: missing kubeconfig
assert_fail "missing kubeconfig exits non-zero" \
  bash -c 'unset KUBECONFIG; export KUBECONFIG="/nonexistent/kubeconfig"; '"$repo_root/deploy/sqlite-prod.sh"' >/dev/null 2>&1'

# Test: kubeconfig whose current-context cluster has no server field
# (kubectl exits 0 with empty output; script must reject with a clear error)
noserver_dir=$(mktemp -d)
trap 'rm -rf "$test_dir" "$noserver_dir"' EXIT
cat > "$noserver_dir/k3s.yaml" <<'EOF'
apiVersion: v1
kind: Config
clusters:
  - cluster:
      insecure-skip-tls-verify: true
    name: k3s
current-context: k3s
contexts:
  - context:
      cluster: k3s
      user: admin
    name: k3s
users:
  - name: admin
    user:
      token: fake-token
EOF
assert_fail "kubeconfig without server field exits non-zero" \
  bash -c "KUBECONFIG=$noserver_dir/k3s.yaml $repo_root/deploy/sqlite-prod.sh >/dev/null 2>&1"

# Test: help flag works
assert_pass "help flag exits 0" \
  bash -c "KUBECONFIG=/etc/rancher/k3s/k3s.yaml $repo_root/deploy/sqlite-prod.sh --help"

# Test: unknown argument fails
assert_fail "unknown argument exits non-zero" \
  bash -c "KUBECONFIG=$KUBECONFIG $repo_root/deploy/sqlite-prod.sh --unknown"

# Test: --write flag is accepted
assert_pass "--write flag accepted" \
  bash -c "KUBECONFIG=$KUBECONFIG $repo_root/deploy/sqlite-prod.sh --write --help"

# Test: --namespace flag is accepted
assert_pass "--namespace flag accepted" \
  bash -c "KUBECONFIG=$KUBECONFIG $repo_root/deploy/sqlite-prod.sh -n testns --help"

# Test: --namespace without value fails
assert_fail "--namespace without value exits non-zero" \
  bash -c "KUBECONFIG=$KUBECONFIG $repo_root/deploy/sqlite-prod.sh -n"

echo ""
echo "Results: $PASSED passed, $FAILED failed"

if [ "$FAILED" -gt 0 ]; then
  exit 1
fi
