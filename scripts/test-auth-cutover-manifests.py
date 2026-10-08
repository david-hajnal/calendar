#!/usr/bin/env python3
"""Render the dormant candidate and assert runtime contracts across charts."""
import pathlib
import subprocess
import tempfile
import yaml

root = pathlib.Path(__file__).resolve().parents[1]
cutover = root / 'deploy/flux/overlays/auth-cutover'
rendered_overlay = subprocess.check_output(['kubectl', 'kustomize', str(cutover)], text=True)
releases = {doc['metadata']['name']: doc for doc in yaml.safe_load_all(rendered_overlay) if doc and doc['kind'] == 'HelmRelease'}
expected = {'commoncal-auth': set(), 'commoncal': {'commoncal-auth'}, 'commoncal-mcp': {'commoncal', 'commoncal-auth'}}
assert set(releases) == set(expected)
for name, dependencies in expected.items():
    assert {item['name'] for item in releases[name]['spec'].get('dependsOn', [])} == dependencies, name
    assert releases[name]['spec']['targetNamespace'] == 'commoncal'
    assert releases[name]['spec']['suspend'] is True, 'candidate must not activate stale image artifacts'
active = yaml.safe_load((root / 'deploy/flux/overlays/production/kustomization.yaml').read_text())
assert all('auth-cutover' not in resource for resource in active['resources']), 'candidate must stay dormant'

objects = {}
with tempfile.TemporaryDirectory() as directory:
    for name, release in releases.items():
        values = dict(release['spec']['values'])
        # SMTP host/from are operator inputs, not inferred DNS names. Rendering
        # fixtures supply these required values without inventing production mail.
        if name == 'commoncal': values['mail'] = {'host': 'smtp.render-fixture.test', 'from': 'render-fixture@example.test'}
        path = pathlib.Path(directory) / f'{name}.yaml'
        path.write_text(yaml.safe_dump(values))
        chart = root / release['spec']['chart']['spec']['chart']
        output = subprocess.check_output(['helm', 'template', name, str(chart), '--namespace', 'commoncal', '--values', str(path)], text=True)
        objects[name] = [doc for doc in yaml.safe_load_all(output) if doc]

def obj(name, kind, resource=None):
    return next(doc for doc in objects[name] if doc['kind'] == kind and (resource is None or doc['metadata']['name'] == resource))
def container(name, kind): return obj(name, kind)['spec']['template']['spec']['containers'][0]
def envs(container):
    entries = container.get('env', [])
    assert len(entries) == len({entry['name'] for entry in entries}), 'duplicate runtime environment variables'
    return {entry['name']: entry for entry in entries}

auth = container('commoncal-auth', 'Deployment')
core = container('commoncal', 'StatefulSet')
mcp = container('commoncal-mcp', 'Deployment')
auth_env, core_env, mcp_env = map(envs, (auth, core, mcp))
auth_config = obj('commoncal-auth', 'ConfigMap')['data']
core_config = obj('commoncal', 'ConfigMap')['data']
issuer, resource, origin = 'https://auth.hajnal.space', 'https://mcal.hajnal.space/mcp', 'https://cal.hajnal.space'
assert auth_config['AUTH_ISSUER'] == mcp_env['MCP_OAUTH_ISSUER']['value'] == issuer
assert auth_config['AUTH_RESOURCE_URL'] == mcp_env['MCP_PUBLIC_RESOURCE_URL']['value'] == resource
assert auth_config['AUTH_COMMONCAL_URL'] == core_config['APP_ORIGIN'] == mcp_env['MCP_INTERNAL_API_BASE']['value'] == origin
assert auth_config['AUTH_TRUST_PROXY'] == 'true'
assert auth_config['AUTH_DCR_CALLBACK_PATH'] == '/mcp/oauth/callback'
assert 'MCP_OAUTH_ISSUER_HOLD' not in mcp_env
assert 'valueFrom' not in mcp_env['MCP_OAUTH_ISSUER']
bridge_key = {'name': 'commoncal-auth-secrets', 'key': 'AUTH_BRIDGE_KEY'}
assert auth_env['AUTH_BRIDGE_KEY']['valueFrom']['secretKeyRef'] == core_env['AUTH_BRIDGE_SECRET']['valueFrom']['secretKeyRef'] == bridge_key
assert 'AUTH_BRIDGE_KEY' not in core_env
assert core_config['AUTH_BRIDGE_TIMEOUT_SECS'] == '5'
assert 'AUTH_BRIDGE_TIMEOUT_MS' not in core_config
assert core_config['AUTH_BRIDGE_URL'] == 'http://commoncal-auth-internal.commoncal.svc:80'
assert auth['command'] == ['node', 'src/production.mjs']
assert auth['readinessProbe']['httpGet']['path'] == '/ready'
assert 'DATABASE_URL' not in auth_env
assert auth_config['AUTH_SQLITE_PATH'] == '/app/data/auth.sqlite'
workload = obj('commoncal-auth', 'Deployment')
assert workload['spec']['replicas'] == 1
assert workload['spec']['strategy'] == {'type': 'Recreate'}
assert workload['spec']['template']['spec']['initContainers'][0]['command'] == ['node', 'src/migrate.mjs']
volume = next(v for v in workload['spec']['template']['spec']['volumes'] if v['name'] == 'data')
assert volume['persistentVolumeClaim']['claimName'] == 'commoncal-auth-data'
assert not any(v['name'] == 'database-ca' for v in workload['spec']['template']['spec']['volumes'])
assert obj('commoncal-auth', 'PersistentVolumeClaim')['metadata']['annotations']['helm.sh/resource-policy'] == 'keep'
for name, host, secret in [('commoncal-auth', 'auth.hajnal.space', 'commoncal-auth-tls'), ('commoncal', 'cal.hajnal.space', 'commoncal-tls'), ('commoncal-mcp', 'mcal.hajnal.space', 'commoncal-tls')]:
    ingress = obj(name, 'Ingress')['spec']
    assert host in [rule['host'] for rule in ingress['rules']]
    assert any(tls['secretName'] == secret and host in tls['hosts'] for tls in ingress['tls'])
    for rule in ingress['rules']:
        for path in rule['http']['paths']:
            assert path['backend']['service']['name'] != 'commoncal-auth-internal'
private = obj('commoncal-auth', 'Service', 'commoncal-auth-internal')['spec']
assert private.get('type', 'ClusterIP') == 'ClusterIP'
assert private['ports'][0]['targetPort'] == 'private'
assert next(port for port in auth['ports'] if port['name'] == 'private')['containerPort'] == 4001
policy = obj('commoncal-auth', 'NetworkPolicy', 'commoncal-auth')['spec']
assert policy['egress'] == []
bridge = next(rule for rule in policy['ingress'] if rule['ports'][0]['port'] == 4001)
assert bridge['from'][0]['namespaceSelector']['matchLabels']['kubernetes.io/metadata.name'] == 'commoncal'
assert bridge['from'][0]['podSelector']['matchLabels']['app.kubernetes.io/name'] == 'commoncal'
core_policy = obj('commoncal', 'NetworkPolicy', 'commoncal')['spec']
assert any(any(peer.get('podSelector', {}).get('matchLabels', {}).get('app.kubernetes.io/component') == 'authorization' for peer in rule.get('to', [])) and {80, 4001} <= {port['port'] for port in rule['ports']} for rule in core_policy['egress'])
duplicate = subprocess.run(['helm', 'template', 'invalid', str(root / 'deploy/helm/commoncal-mcp'), '--set-string', 'env.MCP_OAUTH_ISSUER=https://auth.hajnal.space'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
assert duplicate.returncode != 0, 'schema must reject duplicate secret and literal primary issuer sources'
print('dormant auth cutover manifest contracts passed')
