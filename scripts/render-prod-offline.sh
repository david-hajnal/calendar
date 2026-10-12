#!/usr/bin/env bash
set -euo pipefail

# render-prod-offline.sh — Render production Helm charts from committed
# HelmRelease YAML files without kubeconfig, live cluster, or .env files.
#
# Usage: scripts/render-prod-offline.sh
#
# Stdout: rendered Kubernetes manifests (one per chart, separated by ---)
# Stderr: diagnostics, warnings, and progress messages
# Exit:   0 on success; nonzero on missing tools or render failure

# ---------------------------------------------------------------------------
# Tool checks
# ---------------------------------------------------------------------------
for cmd in helm python3; do
    if ! command -v "$cmd" &>/dev/null; then
        echo "ERROR: required tool '$cmd' not found in PATH" >&2
        exit 1
    fi
done

# ---------------------------------------------------------------------------
# Resolve paths relative to repo root
# ---------------------------------------------------------------------------
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CHARTS_DIR="${REPO_ROOT}/deploy/flux/overlays/production/charts"
HELM_DIR="${REPO_ROOT}/deploy/helm"

# Chart definitions: name|chart_path|release_file
# Render order respects dependsOn (auth → core → mcp)
CHART_DEFS=(
    "auth|${HELM_DIR}/commoncal-auth|auth-helmrelease.yaml"
    "core|${HELM_DIR}/commoncal|core-helmrelease.yaml"
    "mcp|${HELM_DIR}/commoncal-mcp|mcp-helmrelease.yaml"
)

# ---------------------------------------------------------------------------
# Extract HelmRelease spec.values as YAML via python3 (no yq needed)
# ---------------------------------------------------------------------------
extract_values() {
    local release_file="$1"
    python3 -c "
import sys, yaml

with open(sys.argv[1]) as f:
    doc = yaml.safe_load(f)

values = doc.get('spec', {}).get('values', None)
if values is None:
    print('WARNING: no spec.values found in ' + sys.argv[1], file=sys.stderr)
    sys.exit(0)

yaml.dump(values, sys.stdout, default_flow_style=False, sort_keys=False)
" "$release_file"
}

# ---------------------------------------------------------------------------
# Render a single chart
# ---------------------------------------------------------------------------
render_chart() {
    local chart_name="$1"
    local chart_path="$2"
    local release_file="$3"

    local release_path="${CHARTS_DIR}/${release_file}"
    if [[ ! -f "$release_path" ]]; then
        echo "ERROR: HelmRelease file not found: ${release_path}" >&2
        return 1
    fi

    # Resolve chart path (handle relative paths in HelmRelease)
    local chart_dir
    if [[ "$chart_path" == ./* ]]; then
        local release_dir
        release_dir="$(dirname "$release_path")"
        chart_dir="$(cd "${release_dir}/${chart_path}" 2>/dev/null && pwd)" || {
            echo "ERROR: cannot resolve chart path ${chart_path} relative to ${release_dir}" >&2
            return 1
        }
    else
        chart_dir="$chart_path"
    fi

    if [[ ! -d "$chart_dir" ]]; then
        echo "ERROR: chart directory not found: ${chart_dir}" >&2
        return 1
    fi

    # Extract values from the HelmRelease
    local values_yaml
    values_yaml="$(extract_values "$release_path")"

    if [[ -z "$values_yaml" ]]; then
        echo "WARNING: empty values extracted for '${chart_name}'; using chart defaults" >&2
    fi

    # Write values to a temp file
    local tmpfile
    tmpfile="$(mktemp)"
    trap "rm -f '$tmpfile'" RETURN

    if [[ -n "$values_yaml" ]]; then
        echo "$values_yaml" > "$tmpfile"
    fi

    # Render with helm template
    echo "==> Rendering chart '${chart_name}' from ${release_path} ..." >&2

    if ! helm template "commoncal-${chart_name}" "$chart_dir" \
        --namespace commoncal \
        --values "$tmpfile" \
        2>&1; then
        echo "ERROR: helm template failed for chart '${chart_name}'" >&2
        echo "  Chart:   ${chart_dir}" >&2
        echo "  Values:  ${release_path}" >&2
        echo "  Fix the chart or values and re-run." >&2
        return 1
    fi

    return 0
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
echo "==> Rendering production HelmRelease manifests (offline)..." >&2
echo "==> Source: ${CHARTS_DIR}" >&2
echo "" >&2

first=true
for def in "${CHART_DEFS[@]}"; do
    IFS='|' read -r chart_name chart_path release_file <<< "$def"
    if [[ "$first" == true ]]; then
        first=false
    else
        echo "---"
    fi
    render_chart "$chart_name" "$chart_path" "$release_file" || exit 1
    echo "" >&2
done

echo "==> Done. All charts rendered successfully." >&2
exit 0
