#!/usr/bin/env bash
# Shared stdin-based auth Secret application; no credential process arguments.
apply_auth_secret() {
  local namespace=$1
  shift
  : "${AUTH_DATABASE_URL:?}" "${AUTH_BRIDGE_KEY:?}" "${AUTH_COOKIE_KEYS:?}" "${AUTH_SIGNING_KID:?}" "${AUTH_JWKS_FILE:?}"
  export AUTH_DATABASE_URL AUTH_BRIDGE_KEY AUTH_COOKIE_KEYS AUTH_SIGNING_KID AUTH_JWKS_FILE
  python3 - "$namespace" <<'PY' | kubectl "$@"
import base64, json, os, pathlib, sys
values={'DATABASE_URL':os.environ['AUTH_DATABASE_URL'],'AUTH_BRIDGE_KEY':os.environ['AUTH_BRIDGE_KEY'],
        'AUTH_COOKIE_KEYS':os.environ['AUTH_COOKIE_KEYS'],'AUTH_SIGNING_KID':os.environ['AUTH_SIGNING_KID']}
values['AUTH_JWKS']=pathlib.Path(os.environ['AUTH_JWKS_FILE']).read_text()
json.loads(values['AUTH_JWKS'])
print(json.dumps({'apiVersion':'v1','kind':'Secret','metadata':{'name':'commoncal-auth-secrets','namespace':sys.argv[1]},
                 'type':'Opaque','data':{key:base64.b64encode(value.encode()).decode() for key,value in values.items()}}))
PY
}
