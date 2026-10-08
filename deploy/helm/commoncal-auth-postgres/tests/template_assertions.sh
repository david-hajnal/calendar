#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../../../.." && pwd)
render=$(mktemp)
trap 'rm -f "$render"' EXIT
helm template commoncal-auth-postgres "$root/deploy/helm/commoncal-auth-postgres" \
  --namespace commoncal --values "$root/deploy/values-auth-postgres-production.yaml" \
  --set-string image.tag=sha-0123456789012345678901234567890123456789 > "$render"
python3 - "$render" <<'PY'
import sys,yaml
objects=list(yaml.safe_load_all(open(sys.argv[1])))
def obj(kind): return next(o for o in objects if o['kind']==kind)
state=obj('StatefulSet')['spec']['template']['spec']
assert state['automountServiceAccountToken'] is False
assert state['securityContext']['runAsNonRoot'] is True
container=state['containers'][0]
assert 'ssl=on' in container['args']
assert 'ssl_key_file=/etc/postgres-tls/tls.key' in container['args']
assert container['securityContext']['allowPrivilegeEscalation'] is False
job=obj('CronJob')['spec']
assert job['concurrencyPolicy']=='Forbid'
assert job['jobTemplate']['metadata']['labels']['app.kubernetes.io/name']=='commoncal-auth-postgres-backup'
pod=job['jobTemplate']['spec']['template']['spec']
assert pod['automountServiceAccountToken'] is False
backup=pod['containers'][0]
env={e['name']:e for e in backup['env']}
assert env['PGSSLMODE']['value']=='verify-full'
assert env['PGPASSWORD']['valueFrom']['secretKeyRef']['key']=='AUTH_DATABASE_PASSWORD'
assert env['AGE_RECIPIENT']['valueFrom']['secretKeyRef']['key']=='AGE_RECIPIENT'
assert backup['securityContext']['readOnlyRootFilesystem'] is True
assert next(v for v in pod['volumes'] if v['name']=='work')['emptyDir']['medium']=='Memory'
assert all(o['metadata']['annotations']['helm.sh/resource-policy']=='keep' for o in objects if o['kind']=='PersistentVolumeClaim')
policies=[o['spec'] for o in objects if o['kind']=='NetworkPolicy']
assert len(policies)==2
assert next(p for p in policies if p['podSelector']['matchLabels']['app.kubernetes.io/name']=='commoncal-auth-postgres')['egress']==[]
for policy in policies:
    for rule in policy.get('egress',[]):
        assert all('ipBlock' not in peer for peer in rule.get('to',[]))
print('PostgreSQL chart assertions passed')
PY
