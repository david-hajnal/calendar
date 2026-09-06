#!/usr/bin/env sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
work_dir=$(mktemp -d)
trap 'rm -rf "$work_dir"' EXIT

if ! command -v helm >/dev/null 2>&1; then
  echo 'helm is required to verify effective production NetworkPolicies' >&2
  exit 1
fi

render_and_check() {
  release=$1
  chart=$2
  helmrelease=$3
  values="$work_dir/$release-values.yaml"
  rendered="$work_dir/$release-rendered.yaml"

  # Use exactly the values Flux supplies to Helm in production. Checking chart
  # defaults or the HelmRelease YAML alone can miss an incorrect effective
  # selector after the two layers are merged.
  python3 - "$helmrelease" "$values" <<'PY'
import sys

import yaml

with open(sys.argv[1], encoding="utf-8") as stream:
    helmrelease = yaml.safe_load(stream)

with open(sys.argv[2], "w", encoding="utf-8") as stream:
    yaml.safe_dump(helmrelease["spec"]["values"], stream)
PY

  helm template "$release" "$chart" \
    --namespace commoncal \
    --values "$values" > "$rendered"

  python3 - "$release" "$rendered" <<'PY'
import sys

import yaml

release, rendered = sys.argv[1:]
with open(rendered, encoding="utf-8") as stream:
    documents = [document for document in yaml.safe_load_all(stream) if document]

policies = [document for document in documents if document.get("kind") == "NetworkPolicy"]
assert policies, f"{release}: production render has no NetworkPolicy"

selected_namespaces = []
for policy in policies:
    for rule in policy.get("spec", {}).get("ingress", []):
        for peer in rule.get("from", []):
            selector = peer.get("namespaceSelector", {}).get("matchLabels", {})
            namespace = selector.get("kubernetes.io/metadata.name")
            if namespace:
                selected_namespaces.append(namespace)

assert "traefik" in selected_namespaces, (
    f"{release}: public ingress NetworkPolicy must allow the production "
    f"controller namespace 'traefik'; rendered namespace selectors: {selected_namespaces}"
)
PY
}

failures=0

render_and_check \
  commoncal \
  "$repo_root/deploy/helm/commoncal" \
  "$repo_root/deploy/flux/overlays/production/charts/core-helmrelease.yaml" || failures=$((failures + 1))
render_and_check \
  commoncal-mcp \
  "$repo_root/deploy/helm/commoncal-mcp" \
  "$repo_root/deploy/flux/overlays/production/charts/mcp-helmrelease.yaml" || failures=$((failures + 1))

if [ "$failures" -ne 0 ]; then
  echo "$failures production ingress NetworkPolicy assertion(s) failed" >&2
  exit 1
fi

echo 'production ingress NetworkPolicy assertions passed'
