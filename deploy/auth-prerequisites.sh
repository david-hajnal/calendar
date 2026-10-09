#!/usr/bin/env bash
# Read-only checks. Success never enables auth or changes the MCP issuer.
set -euo pipefail
set +x
: "${KUBECONFIG:?Set KUBECONFIG to the intended production cluster}"
namespace=${NAMESPACE:-commoncal}
source "$(dirname "${BASH_SOURCE[0]}")/auth-tls.sh"
auth_tls_secret "$namespace" validate
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
for name in commoncal-auth-secrets commoncal-auth-backup; do
  kubectl get secret "$name" -n "$namespace" -o json > "$work_dir/$name.json"
done
python3 - "$work_dir" "$namespace" <<'PY'
import base64, json, pathlib, sys
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
recipient = data('commoncal-auth-backup','AGE_RECIPIENT').decode()
if not recipient.startswith('age1'):
    raise SystemExit('Encrypted backup recipient is missing')
(root/'backup-recipient').write_text(recipient)
PY
command -v age >/dev/null 2>&1 || { echo "age is required to validate the backup recipient" >&2; exit 1; }
age --encrypt --recipient "$(cat "$work_dir/backup-recipient")" --output "$work_dir/recipient-proof.age" </dev/null
kubectl rollout status deployment/commoncal-auth -n "$namespace" --timeout=30s
kubectl get deployment commoncal-auth -n "$namespace" -o json > "$work_dir/deployment.json"
kubectl get pvc commoncal-auth-data -n "$namespace" -o json > "$work_dir/pvc.json"
python3 - "$work_dir" <<'SQLITE'
import json,pathlib,sys
root=pathlib.Path(sys.argv[1]); spec=json.loads((root/'deployment.json').read_text())['spec']
if spec['replicas']!=1 or spec['strategy']['type']!='Recreate':
    raise SystemExit('SQLite auth requires one replica and Recreate rollouts')
if json.loads((root/'pvc.json').read_text()).get('status',{}).get('phase')!='Bound':
    raise SystemExit('Authorization persistent volume is not Bound')
SQLITE
kubectl exec -n "$namespace" deployment/commoncal-auth -- node --input-type=module -e '
import { DatabaseSync } from "node:sqlite";
const db = new DatabaseSync(process.env.AUTH_SQLITE_PATH, {readOnly:true});
try {
  if (db.prepare("PRAGMA integrity_check").get().integrity_check !== "ok") process.exitCode=1;
  if (db.prepare("SELECT MAX(version) AS version FROM schema_migration").get().version !== 1) process.exitCode=1;
} finally { db.close(); }
'
# Successful encryption alone does not prove recovery. Require the separate
# restore drill evidence, whose operator procedure is documented in Phase 5.
job_success=$(kubectl get jobs -n "$namespace" -l app.kubernetes.io/name=commoncal-auth-backup -o json)
JOB_SUCCESS="$job_success" python3 - <<'PY'
import json,os
if not any(job.get('status',{}).get('succeeded',0)>0 for job in json.loads(os.environ['JOB_SUCCESS']).get('items',[])):
    raise SystemExit('A completed encrypted database backup is required')
PY
kubectl get configmap commoncal-auth-restore-proof -n "$namespace" -o jsonpath='{.data.verified}' | python3 -c 'import sys; sys.exit(0 if sys.stdin.read()=="true" else 1)'
echo 'Auth prerequisites passed; issuer activation and MCP cutover remain separate steps.'
