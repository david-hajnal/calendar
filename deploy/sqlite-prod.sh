#!/usr/bin/env bash
set -euo pipefail

# Ad-hoc production SQLite console.
# Usage:
#   sudo ./deploy/sqlite-prod.sh          # read-only (default)
#   sudo ./deploy/sqlite-prod.sh --write  # writable, requires typed confirmation
#
# Must run on the production server with:
#   - kubectl installed and configured
#   - Interactive terminal (tty)
#   - Local k3s kubeconfig at /etc/rancher/k3s/k3s.yaml

readonly CONSOLE_POD_NAME="commoncal-sqlite-console"
readonly DB_PATH="/app/data/commoncal.sqlite"
readonly CORE_STATEFULSET="commoncal"
readonly DEFAULT_NAMESPACE="commoncal"
readonly MAX_DURATION=3600

# --- Defaults ---
NAMESPACE="$DEFAULT_NAMESPACE"
WRITE_MODE=0

# --- Argument parsing (before preflight so tests can validate args without root) ---
while [ $# -gt 0 ]; do
  case "$1" in
    --write) WRITE_MODE=1; shift ;;
    -n|--namespace)
      shift
      if [ $# -eq 0 ] || [ -z "$1" ]; then
        echo "ERROR: --namespace requires a value" >&2
        exit 1
      fi
      NAMESPACE="$1"
      shift
      ;;
    -h|--help)
      echo "Usage: $0 [--write] [-n|--namespace <namespace>]"
      echo ""
      echo "Open an interactive SQLite console on the production database."
      echo "  --write    Enable write mode (requires typed confirmation)"
      echo "  -n, --namespace  Namespace (default: $DEFAULT_NAMESPACE)"
      echo ""
      echo "This tool must be run from the production server with a local"
      echo "k3s kubeconfig at /etc/rancher/k3s/k3s.yaml."
      echo "Read-only access is the default."
      exit 0
      ;;
    *)
      echo "ERROR: Unknown argument: $1" >&2
      echo "Run '$0 --help' for usage." >&2
      exit 1
      ;;
  esac
done

# --- Cleanup function ---
CLEANUP_DONE=0
do_cleanup() {
  if [ "$CLEANUP_DONE" -eq 1 ]; then
    return
  fi
  CLEANUP_DONE=1
  echo ""
  echo "Cleaning up console pod..."
  if kubectl delete pod "$CONSOLE_POD_NAME" \
    --namespace="$NAMESPACE" \
    --ignore-not-found \
    --grace-period=0 \
    &>/dev/null; then
    echo "Pod deleted."
  else
    echo "Pod cleanup failed (may already be gone)." >&2
  fi
}

trap do_cleanup EXIT INT TERM

# --- Preflight checks ---

# 1. Require running as root
if [ "$(id -u)" -ne 0 ]; then
  echo "ERROR: This script must be run as root (sudo)." >&2
  exit 1
fi

# 2. Require kubectl
if ! command -v kubectl &>/dev/null; then
  echo "ERROR: kubectl is required but not installed." >&2
  exit 1
fi

# 3. Require local k3s kubeconfig (loopback-only enforcement)
KUBECONFIG="${KUBECONFIG:-/etc/rancher/k3s/k3s.yaml}"
if [ ! -f "$KUBECONFIG" ]; then
  echo "ERROR: Local kubeconfig not found at $KUBECONFIG" >&2
  echo "This tool must be run from the production server." >&2
  exit 1
fi

# Verify the API server is loopback (not a remote address)
API_SERVER=$(kubectl config --kubeconfig="$KUBECONFIG" view --minify --output='jsonpath={.clusters[0].cluster.server}' 2>/dev/null) || {
  echo "ERROR: Failed to read API server URL from $KUBECONFIG" >&2
  exit 1
}

if [ -z "$API_SERVER" ]; then
  echo "ERROR: API server URL is empty in $KUBECONFIG" >&2
  echo "The kubeconfig's current-context has no cluster server defined." >&2
  echo "Inspect it with: kubectl config --kubeconfig=$KUBECONFIG view --minify" >&2
  exit 1
fi

case "$API_SERVER" in
  http://127.0.0.1:*|http://[::1]:*|https://127.0.0.1:*|https://[::1]:*)
    # Loopback -- OK
    ;;
  *)
    echo "ERROR: API server is not local: $API_SERVER" >&2
    echo "This tool must be run from the production server with a local kubeconfig." >&2
    echo "Expected a 127.0.0.1 or ::1 address." >&2
    exit 1
    ;;
esac

# 4. Require interactive terminal (after kubeconfig check so tests can validate other errors)
if [ ! -t 0 ]; then
  echo "ERROR: An interactive terminal is required. Run from a tty." >&2
  exit 1
fi

# 5. Discover exactly one Ready core StatefulSet pod
echo "Discovering core StatefulSet pod..."

# Derive the pod selector from the StatefulSet itself so this works
# regardless of the Helm release name.
CORE_SELECTOR=$(kubectl get statefulset "$CORE_STATEFULSET" \
  --namespace="$NAMESPACE" \
  -o go-template='{{range $k, $v := .spec.selector.matchLabels}}{{$k}}={{$v}},{{end}}' \
  2>/dev/null | sed 's/,$//') || {
  echo "ERROR: Failed to read selector from StatefulSet $CORE_STATEFULSET" >&2
  exit 1
}

if [ -z "$CORE_SELECTOR" ]; then
  echo "ERROR: StatefulSet $CORE_STATEFULSET has no selector matchLabels" >&2
  exit 1
fi

CORE_PODS=$(kubectl get pods \
  --namespace="$NAMESPACE" \
  --selector="$CORE_SELECTOR" \
  --field-selector="status.phase=Running" \
  --no-headers 2>/dev/null) || {
  echo "ERROR: Failed to query pods in namespace $NAMESPACE" >&2
  exit 1
}

CORE_POD_COUNT=$(echo "$CORE_PODS" | wc -l | tr -d ' ')
if [ "$CORE_POD_COUNT" -ne 1 ]; then
  echo "ERROR: Expected exactly 1 running core pod, found $CORE_POD_COUNT" >&2
  echo "Ensure the StatefulSet $CORE_STATEFULSET is ready in namespace $NAMESPACE." >&2
  exit 1
fi

CORE_POD_NAME=$(echo "$CORE_PODS" | awk '{print $1}')
CORE_POD_NODE=$(kubectl get pod "$CORE_POD_NAME" \
  --namespace="$NAMESPACE" \
  -o jsonpath='{.spec.nodeName}' 2>/dev/null) || {
  echo "ERROR: Failed to read node for pod $CORE_POD_NAME" >&2
  exit 1
}

if [ -z "$CORE_POD_NODE" ]; then
  echo "ERROR: Pod $CORE_POD_NAME has no node assigned" >&2
  exit 1
fi

# 6. Resolve the running image and PVC
CORE_IMAGE=$(kubectl get pod "$CORE_POD_NAME" \
  --namespace="$NAMESPACE" \
  -o jsonpath='{.spec.containers[0].image}' 2>/dev/null) || {
  echo "ERROR: Failed to read image from pod $CORE_POD_NAME" >&2
  exit 1
}

# Read the data PVC from the running pod spec. Works for both pre-created
# PVCs (claimName in pod volumes) and volumeClaimTemplates, since both end
# up in the pod's volume list. Require exactly one PVC-backed volume.
DATA_PVC=$(kubectl get pod "$CORE_POD_NAME" \
  --namespace="$NAMESPACE" \
  -o jsonpath='{.spec.volumes[*].persistentVolumeClaim.claimName}' 2>/dev/null | tr -s '[:space:]' '\n' | grep -v '^$' || true)

DATA_PVC_COUNT=$(printf '%s\n' "$DATA_PVC" | grep -c . || true)
if [ "$DATA_PVC_COUNT" -ne 1 ]; then
  echo "ERROR: Expected exactly 1 PVC-backed volume on pod $CORE_POD_NAME, found $DATA_PVC_COUNT" >&2
  echo "The console must mount the same volume that holds the database." >&2
  exit 1
fi

# 7. Check that the database file exists on the live pod
DB_EXISTS=$(kubectl exec "$CORE_POD_NAME" \
  --namespace="$NAMESPACE" \
  -- test -f "$DB_PATH" 2>/dev/null && echo "yes" || echo "no")

if [ "$DB_EXISTS" != "yes" ]; then
  echo "ERROR: Database file $DB_PATH not found on pod $CORE_POD_NAME" >&2
  echo "The database must exist before opening a console." >&2
  exit 1
fi

# 8. Check console pod does not already exist (one session at a time)
EXISTING_CONSOLE=$(kubectl get pod "$CONSOLE_POD_NAME" \
  --namespace="$NAMESPACE" \
  --no-headers 2>/dev/null || true)

if [ -n "$EXISTING_CONSOLE" ]; then
  echo "ERROR: Console pod $CONSOLE_POD_NAME already exists" >&2
  echo "Another operator session may be active. Delete it first:" >&2
  echo "  kubectl delete pod $CONSOLE_POD_NAME -n $NAMESPACE" >&2
  exit 1
fi

# 9. Verify deny-all NetworkPolicy exists
POLICY_EXISTS=$(kubectl get networkpolicy sqlite-console-deny-all \
  --namespace="$NAMESPACE" \
  --no-headers 2>/dev/null || true)

if [ -z "$POLICY_EXISTS" ]; then
  echo "ERROR: NetworkPolicy sqlite-console-deny-all not found in namespace $NAMESPACE" >&2
  echo "Deploy the Helm chart or create the policy before opening a console." >&2
  exit 1
fi

# --- Preflight summary ---
if [ "$WRITE_MODE" -eq 1 ]; then
  echo ""
  echo "=== WRITE MODE ==="
  echo "Cluster API : $API_SERVER"
  echo "Namespace   : $NAMESPACE"
  echo "Core pod    : $CORE_POD_NAME"
  echo "Node        : $CORE_POD_NODE"
  echo "Image       : $CORE_IMAGE"
  echo "PVC         : $DATA_PVC"
  echo "Database    : $DB_PATH"
  echo ""
  echo "WARNING: Direct SQL bypasses application authorization and domain invariants."
  echo "Use BEGIN IMMEDIATE for writes, verify changes, and COMMIT or ROLLBACK explicitly."
  echo "Schema changes, VACUUM, and journal-mode changes require a maintenance window."
  echo ""
  echo "To confirm write mode, type the pod name exactly: $CONSOLE_POD_NAME"
  read -r confirmation
  if [ "$confirmation" != "$CONSOLE_POD_NAME" ]; then
    echo "Confirmation mismatch. Aborting." >&2
    exit 1
  fi
  echo ""
else
  echo ""
  echo "=== Read-only SQLite console ==="
  echo "Cluster API : $API_SERVER"
  echo "Namespace   : $NAMESPACE"
  echo "Core pod    : $CORE_POD_NAME"
  echo "Node        : $CORE_POD_NODE"
  echo "Image       : $CORE_IMAGE"
  echo "PVC         : $DATA_PVC"
  echo "Database    : $DB_PATH"
  echo ""
fi

# --- Build the Pod spec ---
PVC_MOUNT_READ_ONLY=""
SQLITE_READONLY_FLAG=""

if [ "$WRITE_MODE" -eq 0 ]; then
  # 10 spaces: must be a field of the "data" mount item, aligned with
  # name:/mountPath: in the heredoc below.
  PVC_MOUNT_READ_ONLY="          readOnly: true"
  SQLITE_READONLY_FLAG="-readonly"
fi

# Note: the app runs SQLite in WAL mode (backend/src/database.rs). A
# read-only connection requires the -shm/-wal files to already exist; they
# do, because the core pod is running and holds them open (verified above).

POD_SPEC=$(cat <<EOF
apiVersion: v1
kind: Pod
metadata:
  name: $CONSOLE_POD_NAME
  namespace: $NAMESPACE
  labels:
    commoncal.io/role: sqlite-console
spec:
  nodeName: $CORE_POD_NODE
  activeDeadlineSeconds: $MAX_DURATION
  automountServiceAccountToken: false
  containers:
    - name: sqlite-console
      image: $CORE_IMAGE
      command:
        - busybox
        - sleep
        - "${MAX_DURATION}"
      volumeMounts:
        - name: data
          mountPath: /app/data
$PVC_MOUNT_READ_ONLY
        - name: tmp
          mountPath: /app/tmp
      securityContext:
        runAsNonRoot: true
        runAsUser: 1000
        runAsGroup: 1000
        allowPrivilegeEscalation: false
        readOnlyRootFilesystem: true
        capabilities:
          drop:
            - ALL
      resources:
        requests:
          cpu: 50m
          memory: 64Mi
        limits:
          cpu: 100m
          memory: 128Mi
  restartPolicy: Never
  securityContext:
    runAsNonRoot: true
    runAsUser: 1000
    runAsGroup: 1000
    seccompProfile:
      type: RuntimeDefault
  volumes:
    - name: data
      persistentVolumeClaim:
        claimName: $DATA_PVC
    - name: tmp
      emptyDir: {}
EOF
)

# --- Create the pod ---
echo "Creating console pod..."
CREATE_ERR=$(echo "$POD_SPEC" | kubectl create -f - 2>&1 >/dev/null) || {
  echo "ERROR: Failed to create console pod" >&2
  if [ -n "$CREATE_ERR" ]; then
    echo "$CREATE_ERR" >&2
  fi
  exit 1
}

# --- Wait for pod to be Ready ---
echo "Waiting for console pod to become Ready..."
READY=0
for i in $(seq 1 60); do
  PHASE=$(kubectl get pod "$CONSOLE_POD_NAME" \
    --namespace="$NAMESPACE" \
    -o jsonpath='{.status.phase}' 2>/dev/null || true)

  if [ "$PHASE" = "Running" ]; then
    READY_PHASE=$(kubectl get pod "$CONSOLE_POD_NAME" \
      --namespace="$NAMESPACE" \
      -o jsonpath='{.status.containerStatuses[0].ready}' 2>/dev/null || true)
    if [ "$READY_PHASE" = "true" ]; then
      READY=1
      break
    fi
  fi

  # Check for permanent failure
  if [ "$PHASE" = "Failed" ] || [ "$PHASE" = "Succeeded" ]; then
    echo "ERROR: Console pod failed: $PHASE" >&2
    kubectl describe pod "$CONSOLE_POD_NAME" --namespace="$NAMESPACE" >&2
    exit 1
  fi

  sleep 1
done

if [ "$READY" -ne 1 ]; then
  echo "ERROR: Console pod did not become Ready within 60 seconds" >&2
  echo "Check PVC attachment and node resources:" >&2
  echo "  kubectl describe pvc $DATA_PVC -n $NAMESPACE" >&2
  echo "  kubectl describe node $CORE_POD_NODE" >&2
  exit 1
fi

echo "Console pod is Ready. Opening SQLite session..."
echo ""

# --- Run sqlite3 interactively ---
# No `exec`: the shell must survive the session so the EXIT trap deletes
# the console pod immediately. activeDeadlineSeconds is the backstop for
# hard disconnects (e.g. SIGHUP) where the trap cannot run.
# busy_timeout is a SQL pragma, not a sqlite3 CLI flag (there is no
# -timeout option). -cmd runs the pragma after the DB opens, before
# the interactive prompt.
if [ "$WRITE_MODE" -eq 1 ]; then
  kubectl exec -it "$CONSOLE_POD_NAME" \
    --namespace="$NAMESPACE" \
    -- sqlite3 \
      -cmd "PRAGMA busy_timeout = 5000;" \
      "$DB_PATH"
else
  kubectl exec -it "$CONSOLE_POD_NAME" \
    --namespace="$NAMESPACE" \
    -- sqlite3 \
      -cmd "PRAGMA busy_timeout = 5000;" \
      $SQLITE_READONLY_FLAG \
      "$DB_PATH"
fi
