#!/usr/bin/env sh
set -eu

chart_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
repo_root=$(CDPATH= cd -- "$chart_dir/../../.." && pwd)
helmrelease="$repo_root/deploy/flux/overlays/production/charts/auth-helmrelease.yaml"
values=$(mktemp)
rendered=$(mktemp)
trap 'rm -f "$values" "$rendered"' EXIT

# Render the chart with exactly the values Flux passes from the production
# HelmRelease. Helm silently accepts unknown keys, so a successful render alone
# does not prove that production configuration reaches the workload.
python3 - "$helmrelease" "$values" <<'PY'
import sys

import yaml

with open(sys.argv[1], encoding="utf-8") as stream:
    helmrelease = yaml.safe_load(stream)
values = helmrelease["spec"]["values"]
assert "issuer" not in values, "obsolete top-level issuer key bypasses chart config"
expected_secret_keys = {
    "name",
    "databaseUrlKey",
    "bridgeKeyKey",
    "cookieKeysKey",
    "jwksKey",
    "signingKidKey",
}
actual_secret_keys = set(values.get("secrets", {}))
unknown_secret_keys = actual_secret_keys - expected_secret_keys
assert not unknown_secret_keys, (
    "production secrets contain values ignored by the chart: "
    f"{sorted(unknown_secret_keys)!r}"
)
with open(sys.argv[2], "w", encoding="utf-8") as stream:
    yaml.safe_dump(values, stream)
PY

helm template commoncal-auth "$chart_dir" \
  --namespace commoncal \
  --values "$values" > "$rendered"

# Helm otherwise accepts unknown values silently. Keep the obsolete production
# shape invalid so a future issuer/config mismatch fails before reconciliation.
if helm template commoncal-auth "$chart_dir" \
  --namespace commoncal \
  --set-string issuer.url=https://invalid.example >/dev/null 2>&1; then
  echo 'obsolete top-level issuer values must be rejected by the chart schema' >&2
  exit 1
fi

# Secret references must use the exact names consumed by the templates. Helm
# otherwise accepts a typo while retaining the chart defaults unnoticed.
if helm template commoncal-auth "$chart_dir" \
  --namespace commoncal \
  --set-string secrets.bridgeKey=LAB_BRIDGE_KEY >/dev/null 2>&1; then
  echo 'obsolete secrets.bridgeKey must be rejected by the chart schema' >&2
  exit 1
fi

python3 - "$rendered" <<'PY'
import sys

import yaml

with open(sys.argv[1], encoding="utf-8") as stream:
    documents = [document for document in yaml.safe_load_all(stream) if document]

config_map = next(
    document
    for document in documents
    if document.get("kind") == "ConfigMap"
    and document.get("metadata", {}).get("name") == "commoncal-auth"
)

expected = {
    "LAB_ISSUER": "https://cal.hajnal.space",
    "LAB_RESOURCE_URL": "https://cal.hajnal.space",
    "LAB_COMMONCAL_URL": "https://cal.hajnal.space",
}
for key, value in expected.items():
    actual = config_map["data"].get(key)
    assert actual == value, f"{key}: expected {value!r}, rendered {actual!r}"
PY

echo 'commoncal-auth production values assertions passed'
