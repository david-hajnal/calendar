#!/usr/bin/env bash
# Prepare secrets only. Never generate/rotate credentials implicitly.
set -euo pipefail
set +x
: "${AUTH_POSTGRES_PASSWORD:?Supply a strong PostgreSQL administrator password}"
: "${AUTH_DATABASE_PASSWORD:?Supply a distinct strong application password}"
: "${AUTH_BACKUP_AGE_RECIPIENT:?Supply an age recipient whose identity is kept off-cluster}"
: "${KUBECONFIG:?Configure the intended cluster before provisioning}"
export AUTH_POSTGRES_PASSWORD AUTH_DATABASE_PASSWORD AUTH_BACKUP_AGE_RECIPIENT
namespace=${NAMESPACE:-commoncal}
# An initialized PostgreSQL PVC keeps its original role passwords. Reject a
# changed Secret rather than silently breaking an existing database.
for name in commoncal-auth-postgres-secrets commoncal-auth-postgres-backup; do
  existing=$(kubectl get secret "$name" -n "$namespace" --ignore-not-found -o json)
  if [[ -n "$existing" ]]; then
    EXISTING_SECRET="$existing" python3 - "$name" <<'CHECK'
import base64,json,os,sys
secret=json.loads(os.environ['EXISTING_SECRET'])
expected=({'POSTGRES_PASSWORD':os.environ['AUTH_POSTGRES_PASSWORD'],'AUTH_DATABASE_PASSWORD':os.environ['AUTH_DATABASE_PASSWORD']}
          if sys.argv[1].endswith('-secrets') else {'AGE_RECIPIENT':os.environ['AUTH_BACKUP_AGE_RECIPIENT']})
if any(base64.b64decode(secret.get('data',{}).get(key,'')).decode() != value for key,value in expected.items()):
    raise SystemExit('Existing database credentials/backup recipient differ; use an explicit rotation procedure')
CHECK
  fi
done
# No secret values are passed as shell arguments or written to a local file.
python3 - "$namespace" <<'PY' | kubectl apply -f -
import base64, json, os, sys
namespace = sys.argv[1]
admin = os.environ['AUTH_POSTGRES_PASSWORD']
password = os.environ['AUTH_DATABASE_PASSWORD']
recipient = os.environ['AUTH_BACKUP_AGE_RECIPIENT']
if min(len(admin), len(password)) < 32 or admin == password or not recipient.startswith('age1'):
    raise SystemExit('Distinct 32+ character passwords and an age recipient are required')
def secret(name, values):
    return {'apiVersion':'v1','kind':'Secret','metadata':{'name':name,'namespace':namespace},'type':'Opaque',
            'data':{key:base64.b64encode(value.encode()).decode() for key,value in values.items()}}
# Keep the database's initial credentials stable on subsequent invocations.
print(json.dumps({'apiVersion':'v1','kind':'List','items':[
    secret('commoncal-auth-postgres-secrets',{'POSTGRES_PASSWORD':admin,'AUTH_DATABASE_PASSWORD':password}),
    secret('commoncal-auth-postgres-backup',{'AGE_RECIPIENT':recipient}),
]}))
PY
# DATABASE_URL belongs to the larger auth Secret; print no DSN or credentials.
echo 'Database and encrypted-backup secret references prepared. Set the matching AUTH_DATABASE_URL before auth bootstrap.'
