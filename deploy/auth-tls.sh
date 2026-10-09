#!/usr/bin/env bash
# Auth origin TLS only. Functions run in subshells so traps/umask stay local.
validate_auth_tls_files() {
  local cert_pub key_pub
  openssl x509 -in "$auth_tls_dir/tls.crt" -noout >/dev/null 2>&1 || { echo 'ERROR: auth TLS certificate is malformed or missing.' >&2; return 1; }
  openssl x509 -in "$auth_tls_dir/tls.crt" -noout -checkhost auth.hajnal.space >/dev/null 2>&1 || { echo 'ERROR: auth TLS certificate does not cover auth.hajnal.space.' >&2; return 1; }
  cert_pub=$(openssl x509 -in "$auth_tls_dir/tls.crt" -noout -pubkey 2>/dev/null) || return
  key_pub=$(openssl pkey -in "$auth_tls_dir/tls.key" -pubout 2>/dev/null) || { echo 'ERROR: auth TLS private key is malformed or missing.' >&2; return 1; }
  [[ -n "$key_pub" && "$cert_pub" == "$key_pub" ]] || { echo 'ERROR: auth TLS certificate and private key do not match.' >&2; return 1; }
  # checkend checks notAfter; also reject certificates that are not yet valid.
  openssl verify -no-CAfile -no-CApath -partial_chain -trusted "$auth_tls_dir/tls.crt" "$auth_tls_dir/tls.crt" >/dev/null 2>&1 || { echo 'ERROR: auth TLS certificate is expired, not yet valid, or otherwise invalid.' >&2; return 1; }
  openssl x509 -in "$auth_tls_dir/tls.crt" -noout -checkend 2592000 >/dev/null 2>&1 || { echo 'ERROR: auth TLS certificate expires within 30 days.' >&2; return 1; }
}

auth_tls_secret() (
  set -euo pipefail
  set +x
  umask 077
  local namespace=$1 mode=$2
  shift 2
  auth_tls_dir=$(mktemp -d)
  trap 'rm -rf "$auth_tls_dir"' EXIT
  trap 'exit 129' HUP
  trap 'exit 130' INT
  trap 'exit 143' TERM
  # ignore-not-found distinguishes absence from permissions/network failures.
  kubectl get secret commoncal-auth-tls -n "$namespace" --ignore-not-found -o json >"$auth_tls_dir/secret.json"
  if [[ -s "$auth_tls_dir/secret.json" ]]; then
    if python3 - "$auth_tls_dir" <<'PY'
import base64,json,pathlib,sys
root=pathlib.Path(sys.argv[1])
try:
    secret=json.loads((root/'secret.json').read_text())
    if secret.get('type')!='kubernetes.io/tls':
        raise ValueError('expected Secret type kubernetes.io/tls')
    for field in ('crt','key'):
        value=base64.b64decode(secret.get('data',{}).get('tls.'+field,''),validate=True)
        if not value: raise ValueError('missing tls.'+field)
        (root/('tls.'+field)).write_bytes(value)
except (ValueError,TypeError) as error:
    raise SystemExit('ERROR: invalid auth TLS Secret: '+str(error))
PY
    then
      if validate_auth_tls_files; then
        echo "Auth origin TLS '$namespace/commoncal-auth-tls' is valid; reusing it."
        exit 0
      fi
    fi
    echo 'Existing commoncal-auth-tls was not overwritten. See docs/AUTH-PRODUCTION.md, Origin TLS rotation: securely back up the Secret, explicitly delete it, then rerun deploy/provision-auth-tls.sh.' >&2
    exit 1
  fi
  [[ "$mode" == provision ]] || { echo 'ERROR: commoncal-auth-tls is missing; run bash deploy/provision-auth-tls.sh.' >&2; exit 1; }
  cat >"$auth_tls_dir/openssl.cnf" <<'CONFIG'
[req]
distinguished_name = req_dn
x509_extensions = v3_req
prompt = no
[req_dn]
CN = auth.hajnal.space
[v3_req]
subjectAltName = DNS:auth.hajnal.space
CONFIG
  openssl req -x509 -newkey rsa:2048 -sha256 -days 365 -nodes \
    -keyout "$auth_tls_dir/tls.key" -out "$auth_tls_dir/tls.crt" -config "$auth_tls_dir/openssl.cnf" >/dev/null 2>&1
  validate_auth_tls_files
  # Atomic create (not apply): never replace a Secret created concurrently.
  kubectl create secret tls commoncal-auth-tls -n "$namespace" \
    --cert="$auth_tls_dir/tls.crt" --key="$auth_tls_dir/tls.key" "$@" >/dev/null
  echo "Auth origin TLS '$namespace/commoncal-auth-tls' creation succeeded."
)
