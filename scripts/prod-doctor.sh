#!/bin/sh
# prod-doctor.sh — Production Configuration Doctor
#
# Diagnose production deployment state from committed files (offline)
# or from live cluster inspection (read-only).
#
# Usage:
#   scripts/prod-doctor.sh          # offline mode (default)
#   scripts/prod-doctor.sh --live   # live cluster inspection
#
# Stdout: diagnostic results
# Stderr: diagnostics, warnings, progress
# Exit:   0 on success; 1 on fatal error

set -eu

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
MODE="offline"
if [ "${PROD_DOCTOR_LIVE:-0}" = "1" ]; then
    MODE="live"
fi
for arg in "$@"; do
    case "$arg" in
        --live) MODE="live" ;;
    esac
done

# ---------------------------------------------------------------------------
# Offline mode: analyze committed files only. No network, no kubectl.
# ---------------------------------------------------------------------------
run_offline() {
    echo "=== Production Configuration Doctor ===" >&2
    echo "" >&2

    # --- Flux HelmReleases (active overlay) ---
    echo "--- Flux HelmReleases (active overlay) ---"
    found_files=0
    for f in deploy/flux/overlays/production/charts/*-helmrelease.yaml; do
        if [ ! -f "$f" ]; then
            continue
        fi
        found_files=$((found_files + 1))
        name=$(basename "$f")
        echo "  ${name}:"
        grep -E '^\s+tag:|^\s+enabled:|^\s+url:' "$f" 2>/dev/null | sed 's/^/    /' || true
        echo ""
    done
    if [ "$found_files" -eq 0 ]; then
        echo "  (no HelmRelease files found)"
        echo ""
    fi

    # --- Secret References ---
    echo "--- Secret References ---"
    echo "  commoncal-session (core): SESSION_SECRET, BACKUP_ENCRYPTION_KEY_HEX"
    echo "  commoncal-mail (core): SMTP_USERNAME, SMTP_PASSWORD"
    echo "  commoncal-mcp-secrets (MCP): mcp-internal-api-key, mcp-session-secret, mcp-oauth-issuer"
    echo "  commoncal-auth-secrets (auth): AUTH_BRIDGE_KEY, AUTH_COOKIE_KEYS, AUTH_SIGNING_KID"
    echo "  commoncal-tls (ingress): tls.crt, tls.key"
    echo ""

    # --- Unresolved Prerequisites ---
    echo "--- Unresolved Prerequisites ---"
    unknown=0
    for check in "helm:helm" "kustomize:kustomize" "kubectl:kubectl"; do
        cmd=$(echo "$check" | cut -d: -f1)
        bin=$(echo "$check" | cut -d: -f2)
        if command -v "$bin" >/dev/null 2>&1; then
            echo "  [OK] ${cmd}"
        else
            echo "  [MISSING] ${cmd} (install ${bin})"
            unknown=$((unknown + 1))
        fi
    done
    echo ""
    if [ "$unknown" -gt 0 ]; then
        echo "  ${unknown} prerequisite(s) missing. Install them for full validation."
    else
        echo "  All prerequisites present. Run 'make prod-check' for full validation."
    fi
    echo ""

    # --- Known External Unknowns ---
    echo "--- Known External Unknowns ---"
    echo "  Traefik controller namespace: traefik (verify in production)"
    echo "  StorageClass: (default) (verify in production)"
    echo "  Cloudflare DNS: cal.hajnal.space, mcal.hajnal.space, auth.hajnal.space"
    echo "  Cloudflare TLS: Full (origin) mode"
    echo "  TLS certificate: commoncal-tls Secret (verify expiry)"
    echo ""

    # --- Deployment Mode ---
    echo "--- Deployment Mode ---"
    echo "  Mode: OFFLINE (live inspection disabled; use 'make prod-doctor-live' to enable)"
    echo ""
    echo "=== Doctor complete ==="
}

# ---------------------------------------------------------------------------
# Live mode: read-only cluster inspection of Flux HelmRelease resources.
#
# Distinguishes confirmed resource absence from failed observations
# (network errors, permission denied, context errors).
# Never infers deployment ownership from connection or authorization failures.
# ---------------------------------------------------------------------------
run_live() {
    echo "=== Production Configuration Doctor (LIVE) ===" >&2
    echo "" >&2

    # Check kubectl availability
    if ! command -v kubectl >/dev/null 2>&1; then
        echo "ERROR: kubectl is not installed or not in PATH" >&2
        echo "  Install kubectl or run 'make prod-doctor' for offline mode." >&2
        exit 1
    fi

    # Verify cluster connectivity
    if ! kubectl cluster-info >/dev/null 2>&1; then
        echo "ERROR: cannot connect to Kubernetes cluster" >&2
        echo "  Check your kubeconfig and network access." >&2
        exit 1
    fi

    # Check flux-system namespace exists
    if ! kubectl get namespace flux-system >/dev/null 2>&1; then
        echo "ERROR: flux-system namespace not found" >&2
        echo "  Ensure Flux is installed in the cluster." >&2
        exit 1
    fi

    # --- Flux HelmReleases (live) ---
    echo "--- Flux HelmReleases (live) ---"

    flux_active=0
    flux_suspended=0
    flux_absent=0
    flux_failed=0

    for hr in commoncal-auth commoncal commoncal-mcp; do
        if kubectl get helmrelease "$hr" -n flux-system >/dev/null 2>&1; then
            # Resource exists — inspect only the suspend field
            suspend=$(kubectl get helmrelease "$hr" -n flux-system \
                -o jsonpath='{.spec.suspend}' 2>/dev/null || true)
            if [ -z "$suspend" ] || [ "$suspend" = "false" ]; then
                flux_active=$((flux_active + 1))
                echo "  ${hr}: ACTIVE"
            else
                flux_suspended=$((flux_suspended + 1))
                echo "  ${hr}: SUSPENDED"
            fi
        else
            # Capture error to distinguish absence from failure
            error_output=$(kubectl get helmrelease "$hr" -n flux-system 2>&1 || true)
            if echo "$error_output" | grep -qi "not found"; then
                flux_absent=$((flux_absent + 1))
                echo "  ${hr}: ABSENT (confirmed resource does not exist)"
            else
                flux_failed=$((flux_failed + 1))
                echo "  ${hr}: FAILED (observation error — network, permission, or context issue)"
            fi
        fi
    done

    echo ""

    # --- Deployment Mode Classification ---
    echo "--- Deployment Mode ---"

    if [ "$flux_failed" -gt 0 ]; then
        echo "  Mode: FAILED (${flux_failed}/3 HelmRelease observations failed)"
        echo "  Note: Cannot determine deployment mode — check kubectl access and permissions."
    elif [ "$flux_absent" -eq 3 ]; then
        echo "  Mode: ABSENT (all 3 HelmReleases confirmed absent)"
    elif [ "$flux_active" -eq 3 ]; then
        echo "  Mode: ACTIVE (all 3 HelmReleases active)"
    elif [ "$flux_suspended" -eq 3 ]; then
        echo "  Mode: SUSPENDED (all 3 HelmReleases suspended)"
    else
        echo "  Mode: MIXED (${flux_active} active, ${flux_suspended} suspended, ${flux_absent} absent)"
    fi
    echo ""
    echo "=== Doctor complete ==="
}

# ---------------------------------------------------------------------------
# Main dispatch
# ---------------------------------------------------------------------------
case "$MODE" in
    offline) run_offline ;;
    live)    run_live ;;
    *)
        echo "ERROR: unknown mode '$MODE'" >&2
        echo "Usage: $0 [--live]" >&2
        exit 1
        ;;
esac
