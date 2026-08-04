#!/bin/sh
set -eu

python3 - <<'PY'
import json
from pathlib import Path

report_path = Path("docs/authorization-coverage.json")
report = json.loads(report_path.read_text())

assert report["schemaVersion"] == 1
assert report["suite"] == "backend/tests/authorization_regression.rs"
assert set(report["principals"]) == {
    "owner", "manager", "editor", "viewer", "free_busy_viewer", "unrelated",
    "suspended", "superadmin_without_acl",
}
assert set(report["endpointFamilies"]) == {
    "calendar", "event", "acl", "view", "feed", "notification",
}
assert all(report["endpointFamilies"].values())
assert set(report["adversarialCases"]) == {
    "calendar identifier substitution", "event-to-calendar mismatch",
    "public token on authenticated endpoints", "non-leaking denial responses",
}
assert report["missingAuthorizationRegression"]["test"] == (
    "missing_calendar_acl_is_denied_before_any_endpoint_can_authorize"
)
PY
