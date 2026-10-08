#!/usr/bin/env bash
# Shared stdin-based auth Secret application; no credential process arguments.
apply_auth_secret() {
  local namespace=$1
  shift
  : "${AUTH_BRIDGE_KEY:?}" "${AUTH_COOKIE_KEYS:?}" "${AUTH_SIGNING_KID:?}" "${AUTH_JWKS_FILE:?}"
  export AUTH_BRIDGE_KEY AUTH_COOKIE_KEYS AUTH_SIGNING_KID AUTH_JWKS_FILE
  python3 - "$namespace" <<'PY' | kubectl "$@"
import base64, json, os, pathlib, sys
values={'AUTH_BRIDGE_KEY':os.environ['AUTH_BRIDGE_KEY'],
        'AUTH_COOKIE_KEYS':os.environ['AUTH_COOKIE_KEYS'],'AUTH_SIGNING_KID':os.environ['AUTH_SIGNING_KID']}
values['AUTH_JWKS']=pathlib.Path(os.environ['AUTH_JWKS_FILE']).read_text()
json.loads(values['AUTH_JWKS'])
print(json.dumps({'apiVersion':'v1','kind':'Secret','metadata':{'name':'commoncal-auth-secrets','namespace':sys.argv[1]},
                 'type':'Opaque','data':{key:base64.b64encode(value.encode()).decode() for key,value in values.items()}}))
PY
}

validate_auth_backup_recipient() {
  : "${AUTH_BACKUP_AGE_RECIPIENT:?Supply the public age backup recipient}"
  export AUTH_BACKUP_AGE_RECIPIENT
  command -v age >/dev/null 2>&1 || { echo 'age is required to validate the backup recipient' >&2; return 1; }
  local recipient_check
  recipient_check=$(mktemp -d)
  # Encrypt an empty stream to validate the actual key/checksum, not just its prefix.
  if ! age --encrypt --recipient "$AUTH_BACKUP_AGE_RECIPIENT" --output "$recipient_check/proof.age" </dev/null >/dev/null 2>&1; then
    rm -rf "$recipient_check"
    echo 'The backup age recipient is invalid' >&2
    return 1
  fi
  rm -rf "$recipient_check"
}

apply_auth_backup_secret() {
  local namespace=$1
  shift
  validate_auth_backup_recipient || return
  local existing
  existing=$(kubectl get secret commoncal-auth-backup -n "$namespace" --ignore-not-found -o json) || return
  EXISTING_SECRET="$existing" python3 - "$namespace" <<'PY' | kubectl "$@"
import base64,json,os,sys
recipient=os.environ['AUTH_BACKUP_AGE_RECIPIENT']
if not recipient.startswith('age1'):
    raise SystemExit('An age encryption recipient is required')
existing=os.environ['EXISTING_SECRET']
if existing and base64.b64decode(json.loads(existing).get('data',{}).get('AGE_RECIPIENT','')).decode()!=recipient:
    raise SystemExit('Existing backup recipient differs; use an explicit rotation procedure')
print(json.dumps({'apiVersion':'v1','kind':'Secret','metadata':{'name':'commoncal-auth-backup','namespace':sys.argv[1]},
                 'type':'Opaque','data':{'AGE_RECIPIENT':base64.b64encode(recipient.encode()).decode()}}))
PY
}
