#!/usr/bin/env bash
# Read-only checks. Success never enables auth or changes the MCP issuer.
set -euo pipefail
set +x
: "${KUBECONFIG:?Set KUBECONFIG to the intended production cluster}"
namespace=${NAMESPACE:-commoncal}
work_dir=$(mktemp -d)
trap 'rm -rf "$work_dir"' EXIT
umask 077
kubectl config current-context >/dev/null
controller_namespace=${AUTH_INGRESS_NAMESPACE:-traefik}
kubectl get pods -n "$controller_namespace" -l app.kubernetes.io/name=traefik -o json > "$work_dir/ingress.json"
python3 - "$work_dir/ingress.json" <<'PYCONTROLLER'
import json,sys
pods=json.load(open(sys.argv[1])).get('items',[])
if not any(any(c.get('type')=='Ready' and c.get('status')=='True' for c in p.get('status',{}).get('conditions',[])) for p in pods):
    raise SystemExit('No Ready Traefik controller in the configured ingress namespace')
PYCONTROLLER
kubectl wait certificate/commoncal-auth-postgres-ca certificate/commoncal-auth-postgres-tls -n "$namespace" --for=condition=Ready --timeout=30s
for name in commoncal-auth-secrets commoncal-auth-postgres-secrets commoncal-auth-postgres-backup commoncal-auth-tls commoncal-auth-postgres-tls; do
  kubectl get secret "$name" -n "$namespace" -o json > "$work_dir/$name.json"
done
python3 - "$work_dir" "$namespace" <<'PY'
import base64, json, pathlib, sys
from urllib.parse import urlparse, unquote, parse_qs
root=pathlib.Path(sys.argv[1]); namespace=sys.argv[2]
def data(name,key):
    value=json.loads((root/(name+'.json')).read_text()).get('data',{}).get(key)
    if not value: raise SystemExit(f'{name}: missing {key}')
    return base64.b64decode(value,validate=True)
auth='commoncal-auth-secrets'
bridge=data(auth,'AUTH_BRIDGE_KEY').decode()
keys=data(auth,'AUTH_COOKIE_KEYS').decode().split(',')
if len(bridge)<32 or len(keys)<2 or len(set(keys))!=len(keys) or any(len(key.strip())<32 for key in keys):
    raise SystemExit('Auth bridge/cookie key requirements are not met')
jwks=json.loads(data(auth,'AUTH_JWKS'));kid=data(auth,'AUTH_SIGNING_KID').decode()
if not any(k.get('kid')==kid and k.get('d') and k.get('alg')=='RS256' for k in jwks.get('keys',[])):
    raise SystemExit('Active private signing key is missing')
u=urlparse(data(auth,'DATABASE_URL').decode())
if u.scheme not in ('postgres','postgresql') or u.hostname != f'commoncal-auth-postgres.{namespace}.svc.cluster.local' or u.username!='commoncal_auth' or u.path!='/commoncal_auth' or parse_qs(u.query).get('sslmode')!=['verify-full']:
    raise SystemExit('Auth database DSN does not match the private TLS service')
if unquote(u.password or '').encode()!=data('commoncal-auth-postgres-secrets','AUTH_DATABASE_PASSWORD'):
    raise SystemExit('Auth and PostgreSQL application credentials do not match')
if len(data('commoncal-auth-postgres-secrets','POSTGRES_PASSWORD'))<32:
    raise SystemExit('PostgreSQL administrator password is too short')
if not data('commoncal-auth-postgres-backup','AGE_RECIPIENT').decode().startswith('age1'):
    raise SystemExit('Encrypted backup recipient is missing')
for name,host in [('commoncal-auth-tls','auth.hajnal.space'),('commoncal-auth-postgres-tls',f'commoncal-auth-postgres.{namespace}.svc.cluster.local')]:
    (root/(name+'.crt')).write_bytes(data(name,'tls.crt'))
    data(name,'tls.key')
(root/'database-ca.crt').write_bytes(data('commoncal-auth-postgres-tls','ca.crt'))
PY
openssl x509 -in "$work_dir/commoncal-auth-tls.crt" -noout -checkhost auth.hajnal.space -checkend 86400 >/dev/null
openssl x509 -in "$work_dir/commoncal-auth-postgres-tls.crt" -noout -checkhost "commoncal-auth-postgres.$namespace.svc.cluster.local" -checkend 86400 >/dev/null
openssl verify -CAfile "$work_dir/database-ca.crt" "$work_dir/commoncal-auth-postgres-tls.crt" >/dev/null
kubectl rollout status statefulset/commoncal-auth-postgres -n "$namespace" --timeout=30s
# Successful encryption alone does not prove recovery. Require the separate
# restore drill evidence, whose operator procedure is documented in Phase 5.
job_success=$(kubectl get jobs -n "$namespace" -l app.kubernetes.io/name=commoncal-auth-postgres-backup -o json)
JOB_SUCCESS="$job_success" python3 - <<'PY'
import json,os
if not any(job.get('status',{}).get('succeeded',0)>0 for job in json.loads(os.environ['JOB_SUCCESS']).get('items',[])):
    raise SystemExit('A completed encrypted database backup is required')
PY
kubectl get configmap commoncal-auth-restore-proof -n "$namespace" -o jsonpath='{.data.verified}' | python3 -c 'import sys; sys.exit(0 if sys.stdin.read()=="true" else 1)'
echo 'Auth prerequisites passed; issuer activation and MCP cutover remain separate steps.'
