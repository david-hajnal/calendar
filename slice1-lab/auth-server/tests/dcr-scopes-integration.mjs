import assert from 'node:assert/strict';
import { createHash, createPublicKey, generateKeyPairSync, randomBytes, verify } from 'node:crypto';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { openDatabase, migrateSQLite } from '../src/sqlite-storage.mjs';

// Exercise the deployed runtime, including DCR ingress and the trusted consent
// bridge, without retaining identities, keys, tokens, or provider state.
const base = resolve(import.meta.dirname, '..');
const directory = await mkdtemp(resolve(tmpdir(), 'auth-dcr-scopes-'));
const databasePath = resolve(directory, 'auth.sqlite');
const db = openDatabase(databasePath, { create: true });
migrateSQLite(db);
db.close();
const issuer = 'https://auth.dcr-proof.test';
const resource = 'https://mcal.hajnal.space/mcp';
const commoncal = 'https://commoncal.dcr-proof.test';
const scope = 'commoncal.calendar.metadata.read';
const redirectUri = 'http://127.0.0.1:19877/mcp/oauth/callback';
const bridge = randomBytes(32).toString('hex');
const privateKey = generateKeyPairSync('rsa', { modulusLength: 2048 }).privateKey.export({ format: 'jwk' });
const keyFile = resolve(directory, 'jwks.json');
await writeFile(keyFile, JSON.stringify({ keys: [{ ...privateKey, kid: 'dcr-proof', use: 'sig', alg: 'RS256' }] }), { mode: 0o600 });
const port = Number(process.env.AUTH_TEST_DCR_PORT ?? 55444);
let child;
async function request(path, options = {}) {
  return fetch(`http://127.0.0.1:${port}${path}`, { ...options, redirect: 'manual', headers: { host: new URL(issuer).host, 'x-forwarded-proto': 'https', 'x-forwarded-for': '192.0.2.11', ...options.headers } });
}
async function internal(path, options = {}) {
  return fetch(`http://127.0.0.1:${port + 1}${path}`, { ...options, headers: { authorization: `Bearer ${bridge}`, ...options.headers } });
}
async function register(scopes) {
  return request('/reg', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ client_name: 'Codex DCR scope regression', redirect_uris: [redirectUri], token_endpoint_auth_method: 'none', grant_types: ['authorization_code', 'refresh_token'], response_types: ['code'], scope: scopes }) });
}
async function token(params) {
  const response = await request('/token', { method: 'POST', headers: { 'content-type': 'application/x-www-form-urlencoded' }, body: new URLSearchParams(params) });
  return { status: response.status, body: await response.json() };
}
async function authorize(client, { target = resource, deny = false, requestedScope = scope } = {}) {
  const verifier = randomBytes(32).toString('base64url');
  const params = { prompt: 'consent', client_id: client.client_id, response_type: 'code', redirect_uri: redirectUri, scope: requestedScope, code_challenge: createHash('sha256').update(verifier).digest('base64url'), code_challenge_method: 'S256', state: 'dcr-proof-state' };
  if (target !== null) params.resource = target;
  const jar = new Map();
  let next = `/auth?${new URLSearchParams(params)}`;
  let consentSeen = false;
  for (let steps = 0; steps < 20; steps++) {
    const headers = { cookie: [...jar].map(([key, value]) => `${key}=${value}`).join('; ') };
    const response = await request(next, { headers });
    for (const cookie of response.headers.getSetCookie()) {
      const pair = cookie.split(';')[0]; const offset = pair.indexOf('=');
      jar.set(pair.slice(0, offset), pair.slice(offset + 1));
    }
    const location = response.headers.get('location');
    assert.ok(location, `authorization must redirect (HTTP ${response.status})`);
    const url = new URL(location, issuer);
    if (url.origin === new URL(redirectUri).origin) {
      assert.equal(url.searchParams.get('state'), params.state);
      return { code: url.searchParams.get('code'), error: url.searchParams.get('error'), verifier, consentSeen };
    }
    if (url.origin === commoncal) {
      const handoff = url.searchParams.get('handoff');
      const viewResponse = await internal(`/internal/interactions/${handoff}`);
      assert.equal(viewResponse.status, 200);
      const view = await viewResponse.json();
      assert.ok(view.requestedScopes.includes(scope));
      if (view.prompt === 'consent') {
        consentSeen = true;
        // A browser cannot resume an undecided consent and obtain a grant.
        const undecided = await request(`${next}?handoff=${encodeURIComponent(handoff)}`, { headers });
        assert.equal(undecided.status, 400, 'undecided consent must not issue a code');
      }
      const decision = { kind: view.prompt === 'login' ? 'login' : deny ? 'deny' : 'consent', subject: '42' };
      const approved = await internal(`/internal/interactions/${handoff}`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify(decision) });
      assert.equal(approved.status, 200);
      const resume = new URL((await approved.json()).resumeUrl);
      next = resume.pathname + resume.search;
    } else next = url.pathname + url.search;
  }
  assert.fail('authorization exceeded redirect limit');
}

try {
  child = spawn(process.execPath, ['src/production.mjs'], { cwd: base, env: { ...process.env, AUTH_SQLITE_PATH: databasePath, AUTH_ISSUER: issuer, AUTH_RESOURCE_URL: resource, AUTH_COMMONCAL_URL: commoncal, AUTH_BRIDGE_KEY: bridge, AUTH_COOKIE_KEYS: `${randomBytes(32).toString('hex')},${randomBytes(32).toString('hex')}`, AUTH_JWKS_FILE: keyFile, AUTH_SIGNING_KID: 'dcr-proof', AUTH_TRUST_PROXY: 'true', AUTH_PUBLIC_BIND: '127.0.0.1', AUTH_PRIVATE_BIND: '127.0.0.1', AUTH_PUBLIC_PORT: String(port), AUTH_PRIVATE_PORT: String(port + 1), AUTH_DCR_LOOPBACK_HOSTS: '127.0.0.1', AUTH_DCR_CALLBACK_PATH: '/mcp/oauth/callback', AUTH_DCR_RATE_LIMIT: '30' }, stdio: 'ignore' });
  let ready = false;
  for (let attempt = 0; attempt < 100; attempt++) {
    assert.equal(child.exitCode, null, 'production fixture unexpectedly exited');
    try { if ((await request('/ready')).ok) { ready = true; break; } } catch {}
    await new Promise((done) => setTimeout(done, 50));
  }
  assert.ok(ready, 'production fixture starts');
  const registered = await register(scope);
  const client = await registered.json();
  assert.equal(registered.status, 201, `known CommonCal DCR scope rejected: ${client.error ?? ''}: ${client.error_description ?? ''}`);
  assert.equal(client.scope, scope);
  const unknown = await register(`${scope} commoncal.unknown.read`);
  assert.equal(unknown.status, 400);
  assert.equal((await unknown.json()).error, 'invalid_client_metadata');
  const excessive = await authorize(client, { requestedScope: 'commoncal.event.create' });
  assert.equal(excessive.code, null, 'DCR scope restricts the client to its registered permission');
  assert.equal(excessive.error, 'invalid_scope');
  const authorization = await authorize(client);
  assert.ok(authorization.code, 'approved resource consent issues a code');
  assert.equal(authorization.error, null);
  assert.equal(authorization.consentSeen, true);
  const issued = await token({ grant_type: 'authorization_code', client_id: client.client_id, code: authorization.code, code_verifier: authorization.verifier, redirect_uri: redirectUri, resource });
  assert.equal(issued.status, 200, `resource token exchange failed: ${issued.body.error ?? ''}`);
  const parts = issued.body.access_token.split('.');
  const published = await (await request('/jwks')).json();
  const header = JSON.parse(Buffer.from(parts[0], 'base64url'));
  assert.equal(header.alg, 'RS256');
  const key = createPublicKey({ key: published.keys.find((candidate) => candidate.kid === header.kid), format: 'jwk' });
  assert.ok(verify('RSA-SHA256', Buffer.from(`${parts[0]}.${parts[1]}`), key, Buffer.from(parts[2], 'base64url')), 'resource JWT signature verifies');
  const claims = JSON.parse(Buffer.from(parts[1], 'base64url'));
  assert.equal(claims.iss, issuer); assert.equal(claims.aud, resource); assert.equal(claims.sub, '42');
  assert.equal(claims.scope, scope); assert.ok(claims.exp > Date.now() / 1000);
  const inspection = openDatabase(databasePath);
  try {
    const grants = inspection.prepare("SELECT payload FROM provider_entity WHERE model = 'Grant'").all();
    assert.ok(grants.length > 0);
    for (const row of grants) {
      const grant = JSON.parse(row.payload);
      assert.equal(grant.resources?.[resource], scope, 'CommonCal permission belongs to the resource grant');
      assert.ok(!grant.openid?.scope?.split(' ').includes(scope), 'CommonCal permission never becomes an OIDC grant');
      assert.ok(grant.rejected?.openid?.scope?.split(' ').includes(scope), 'CommonCal OIDC interpretation is explicitly rejected');
    }
  } finally { inspection.close(); }
  for (const target of [null, 'https://unapproved-resource.test/mcp']) {
    const refused = await authorize(client, { target });
    assert.equal(refused.code, null, 'CommonCal authorization requires the exact resource');
    assert.ok(refused.error, 'missing or wrong resource fails authorization');
  }
  const denied = await authorize(client, { deny: true });
  assert.equal(denied.consentSeen, true);
  assert.equal(denied.code, null); assert.equal(denied.error, 'access_denied');
  console.log('DCR scope integration passed: known/unknown registration, client scope allowlist, PKCE resource JWT signature/issuer/audience/scope, resource-only grants, resource enforcement, undecided and denied consent');
} finally {
  if (child && child.exitCode === null) { child.kill('SIGTERM'); await once(child, 'exit'); }
  await rm(directory, { recursive: true, force: true });
}
