#!/usr/bin/env bash
# Explicitly rotate only SESSION_SECRET. Invalidates sessions, connection
# passwords, sync tokens and access to feed URLs encrypted with the previous key.
set -euo pipefail
: "${SESSION_SECRET:?Set the new SESSION_SECRET}"
: "${CONFIRM_SESSION_KEY_ROTATION:?Set CONFIRM_SESSION_KEY_ROTATION=rotate after reviewing docs/DEPLOYMENT.md}"
[[ "$CONFIRM_SESSION_KEY_ROTATION" == rotate ]] || exit 1
export SESSION_SECRET
python3 - <<'PY' | kubectl patch secret commoncal-session -n "${NAMESPACE:-commoncal}" --type=merge --patch-file=/dev/stdin
import base64, json, os
print(json.dumps({"data": {"SESSION_SECRET": base64.b64encode(os.environ["SESSION_SECRET"].encode()).decode()}}))
PY
printf '%s\n' 'Key rotated. Restart every core process together; reissue credentials and reconfigure encrypted feeds.'
