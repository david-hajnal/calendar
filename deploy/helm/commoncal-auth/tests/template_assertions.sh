#!/usr/bin/env sh
set -eu
chart_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
rendered=$(mktemp)
trap 'rm -f "$rendered"' EXIT
helm template commoncal-auth "$chart_dir" --namespace commoncal --set-string image.tag=proof > "$rendered"
python3 - "$rendered" <<'ASSERT'
import sys,yaml
objects=[doc for doc in yaml.safe_load_all(open(sys.argv[1])) if doc]
def obj(kind,name=None):
    return next(doc for doc in objects if doc['kind']==kind and (name is None or doc['metadata']['name']==name))
deployment=obj('Deployment'); spec=deployment['spec']; pod=spec['template']['spec']
assert spec['replicas']==1 and spec['strategy']=={'type':'Recreate'}
assert pod['automountServiceAccountToken'] is False
assert pod['securityContext']['fsGroup']==65534
app=pod['containers'][0]; migration=pod['initContainers'][0]
assert app['command']==['node','src/production.mjs']
assert migration['command']==['node','src/migrate.mjs']
assert app['securityContext']['readOnlyRootFilesystem'] is True
for container in [app,migration]:
    assert container['securityContext']['allowPrivilegeEscalation'] is False
    assert container['securityContext']['capabilities']['drop']==['ALL']
    assert any(mount['name']=='data' and mount['mountPath']=='/app/data' for mount in container['volumeMounts'])
env={entry['name']:entry for entry in app['env']}
assert not {'DATABASE_URL','NODE_EXTRA_CA_CERTS'} & env.keys()
assert env['AUTH_BRIDGE_KEY']['valueFrom']['secretKeyRef']=={'name':'commoncal-auth-secrets','key':'AUTH_BRIDGE_KEY'}
assert env['AUTH_COOKIE_KEYS']['valueFrom']['secretKeyRef']['key']=='AUTH_COOKIE_KEYS'
assert obj('ConfigMap')['data']['AUTH_SQLITE_PATH']=='/app/data/auth.sqlite'
assert migration['env'][0]=={'name':'AUTH_SQLITE_PATH','value':'/app/data/auth.sqlite'}
assert not any(v['name']=='database-ca' for v in pod['volumes'])
pvc=obj('PersistentVolumeClaim')
assert pvc['metadata']['name']=='commoncal-auth-data'
assert pvc['metadata']['annotations']['helm.sh/resource-policy']=='keep'
assert pvc['spec']['accessModes']==['ReadWriteOnce']
assert not any(doc['kind']=='Job' for doc in objects), 'no migration hooks competing for SQLite PVC'
policy=obj('NetworkPolicy','commoncal-auth')['spec']
assert policy['egress']==[]
private=next(rule for rule in policy['ingress'] if rule['ports'][0]['port']==4001)
assert private['from'][0]['podSelector']['matchLabels']=={'app.kubernetes.io/name':'commoncal'}
assert private['from'][0]['namespaceSelector']['matchLabels']['kubernetes.io/metadata.name']=='commoncal'
for rule in obj('Ingress')['spec']['rules']:
    for path in rule['http']['paths']:
        assert path['backend']['service']['name']=='commoncal-auth-public'
backup=obj('CronJob')['spec']
assert backup['concurrencyPolicy']=='Forbid'
job=backup['jobTemplate']; assert job['metadata']['labels']['app.kubernetes.io/name']=='commoncal-auth-backup'
bpod=job['spec']['template']['spec']; bapp=bpod['containers'][0]
assert bpod['automountServiceAccountToken'] is False
assert bapp['command']==['node','src/backup.mjs']
assert bapp['securityContext']['readOnlyRootFilesystem'] is True
assert bpod['affinity']['podAffinity']['requiredDuringSchedulingIgnoredDuringExecution'][0]['topologyKey']=='kubernetes.io/hostname'
assert bpod['volumes'][0]['persistentVolumeClaim']['claimName']==pvc['metadata']['name']
assert bpod['volumes'][1]['emptyDir']['medium']=='Memory'
benv={entry['name']:entry for entry in bapp['env']}
assert benv['AGE_RECIPIENT']['valueFrom']['secretKeyRef']=={'name':'commoncal-auth-backup','key':'AGE_RECIPIENT'}
assert obj('NetworkPolicy','commoncal-auth-backup')['spec']['egress']==[]
ASSERT
for invalid in 'replicaCount=2' 'migration.enabled=false' 'secrets.databaseUrlKey=DATABASE_URL' 'networkPolicy.postgres.port=5432' 'persistence.databasePath=:memory:'; do
  if helm template commoncal-auth "$chart_dir" --set "$invalid" >/dev/null 2>&1; then
    echo "Invalid SQLite configuration accepted: $invalid" >&2
    exit 1
  fi
done
# Custom PVC placement and private-image pull credentials must reach both workloads.
helm template commoncal-auth "$chart_dir" --namespace commoncal \
  --set-string persistence.storageClass=local-path \
  --set-string imagePullSecrets[0].name=registry-proof > "$rendered"
python3 - "$rendered" <<'ASSERT'
import sys,yaml
objects=[doc for doc in yaml.safe_load_all(open(sys.argv[1])) if doc]
for doc in objects:
    if doc['kind']=='Deployment': pod=doc['spec']['template']['spec']
    elif doc['kind']=='CronJob': pod=doc['spec']['jobTemplate']['spec']['template']['spec']
    else: continue
    assert pod['imagePullSecrets']==[{'name':'registry-proof'}]
assert next(doc for doc in objects if doc['kind']=='PersistentVolumeClaim')['spec']['storageClassName']=='local-path'
ASSERT
echo 'commoncal-auth SQLite template assertions passed'
