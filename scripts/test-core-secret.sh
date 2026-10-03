#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/../deploy/core-secret.sh"
NAMESPACE=test
SESSION_SECRET=test-key
BACKUP_ENCRYPTION_KEY_HEX=00000000000000000000000000000000
core_secret_create_args=()
created=0
create_args=()
kubectl() {
  if [[ "$1" == create ]]; then created=$((created+1)); create_args=("$@"); return 0; fi
  [[ "$scenario" != error ]] || return 1
  case "${*: -1}" in
    'jsonpath={.data.SESSION_SECRET}')
      case "$scenario" in equal) printf '%s' "$encoded" ;; different) printf '%s' ZGlmZmVyZW50 ;; esac ;;
    'jsonpath={.metadata.name}') [[ "$scenario" == empty ]] && printf '%s' commoncal-session; return 0 ;;
  esac
}
encoded=$(printf '%s' "$SESSION_SECRET" | openssl base64 -A)
scenario=equal; ensure_core_secret; [[ "$created" == 0 ]]
scenario=different; if ensure_core_secret; then exit 1; fi; [[ "$created" == 0 ]]
scenario=empty; if ensure_core_secret; then exit 1; fi; [[ "$created" == 0 ]]
scenario=error; if ensure_core_secret; then exit 1; fi; [[ "$created" == 0 ]]
scenario=absent; ensure_core_secret; [[ "$created" == 1 ]]
core_secret_create_args=(--dry-run=server)
ensure_core_secret; [[ "$created" == 2 ]]
[[ "${create_args[*]}" == *"--dry-run=server"* ]]
printf '%s\n' 'Core secret guard tests passed.'

# Exercise the explicit rotation entry point without touching a cluster.
fixture_dir=$(mktemp -d)
trap 'rm -rf "$fixture_dir"' EXIT
cat > "$fixture_dir/kubectl" <<'MOCK'
#!/usr/bin/env bash
printf '%s\n' "$@" > "$ROTATION_FIXTURE/args"
cat > "$ROTATION_FIXTURE/patch"
MOCK
chmod +x "$fixture_dir/kubectl"
rotation_script="$(dirname "$0")/../deploy/rotate-session-secret.sh"
if env -u CONFIRM_SESSION_KEY_ROTATION PATH="$fixture_dir:$PATH" SESSION_SECRET=fixture-rotation-key ROTATION_FIXTURE="$fixture_dir" bash "$rotation_script" > /dev/null 2>&1; then
  echo 'Unconfirmed rotation unexpectedly succeeded' >&2
  exit 1
fi
[[ ! -e "$fixture_dir/args" ]]
if env CONFIRM_SESSION_KEY_ROTATION=no PATH="$fixture_dir:$PATH" SESSION_SECRET=fixture-rotation-key ROTATION_FIXTURE="$fixture_dir" bash "$rotation_script" > /dev/null 2>&1; then
  echo 'Incorrect confirmation unexpectedly succeeded' >&2
  exit 1
fi
[[ ! -e "$fixture_dir/args" ]]
env CONFIRM_SESSION_KEY_ROTATION=rotate PATH="$fixture_dir:$PATH" SESSION_SECRET=fixture-rotation-key ROTATION_FIXTURE="$fixture_dir" NAMESPACE=fixture bash "$rotation_script" > /dev/null
python3 - "$fixture_dir" <<'PY'
import base64, json, pathlib, sys
fixture = pathlib.Path(sys.argv[1])
assert (fixture / 'args').read_text().splitlines() == ['patch', 'secret', 'commoncal-session', '-n', 'fixture', '--type=merge', '--patch-file=/dev/stdin']
patch = json.loads((fixture / 'patch').read_text())
assert patch == {'data': {'SESSION_SECRET': base64.b64encode(b'fixture-rotation-key').decode()}}
PY
printf '%s\n' 'Explicit rotation guard tests passed.'
