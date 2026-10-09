import assert from 'node:assert/strict';
import { generateKeyPairSync, createHash, randomBytes } from 'node:crypto';
import { mkdtemp, mkdir, writeFile, readdir, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { spawn, execFileSync } from 'node:child_process';
import { once } from 'node:events';
import { openDatabase, migrateSQLite } from '../src/sqlite-storage.mjs';
import { snapshot, encryptedBackup } from '../src/backup.mjs';
import { copyFile } from 'node:fs/promises';

const base = resolve(import.meta.dirname, '..');
const directory = await mkdtemp(resolve(tmpdir(), 'auth-proof-'));
const databasePath=resolve(directory,'auth.sqlite');
let db=openDatabase(databasePath,{create:true});
migrateSQLite(db);
const pool={ async query(sql) { const stmt=db.prepare(sql); if (/^SELECT|RETURNING/i.test(sql)) return {rows:stmt.all()}; stmt.run(); return {rows:[]}; }, async end(){db.close();} };
const issuer = 'https://auth.production-proof.test';
const resource = 'https://mcp.production-proof.test/mcp';
const commoncal = 'https://commoncal.production-proof.test';
const bridge = randomBytes(32).toString('hex');
const cookies = `${randomBytes(32).toString('hex')},${randomBytes(32).toString('hex')}`;
const privateKey = generateKeyPairSync('rsa', { modulusLength: 2048 }).privateKey.export({ format: 'jwk' });
const jwks = { keys: [{ ...privateKey, kid: 'production-current', use: 'sig', alg: 'RS256' }] };
const keyFile = resolve(directory, 'jwks.json');
await writeFile(keyFile, JSON.stringify(jwks));
const port = Number(process.env.AUTH_TEST_PUBLIC_PORT ?? 55440);
const privatePort = port + 1;
let activeKid = 'production-current';
let child;
let logs = '';
async function start() {
  child = spawn(process.execPath, ['src/production.mjs'], { cwd: base, env: { ...process.env, AUTH_SQLITE_PATH: databasePath, AUTH_ISSUER: issuer, AUTH_RESOURCE_URL: resource, AUTH_COMMONCAL_URL: commoncal, AUTH_BRIDGE_KEY: bridge, AUTH_COOKIE_KEYS: cookies, AUTH_JWKS_FILE: keyFile, AUTH_SIGNING_KID: activeKid, AUTH_TRUST_PROXY: 'true', AUTH_PUBLIC_BIND: '127.0.0.1', AUTH_PRIVATE_BIND: '127.0.0.1', AUTH_PUBLIC_PORT: String(port), AUTH_PRIVATE_PORT: String(privatePort), AUTH_DCR_LOOPBACK_HOSTS: '127.0.0.1,[::1]', AUTH_DCR_CALLBACK_PATH: '/mcp/oauth/callback', AUTH_DCR_RATE_LIMIT: '5', AUTH_CLEANUP_INTERVAL_MS: '1000' }, stdio: ['ignore', 'pipe', 'pipe'] });
  child.stdout.on('data', (data) => { logs += data; });
  child.stderr.on('data', (data) => { logs += data; });
  for (let i = 0; i < 100; i++) {
    if (child.exitCode !== null) throw new Error(`production startup failed: ${logs}`);
    try { if ((await request('/ready')).ok) return; } catch {}
    await new Promise((done) => setTimeout(done, 50));
  }
  throw new Error('production startup timed out');
}
async function stop() { if (child && child.exitCode === null) { child.kill('SIGTERM'); await once(child, 'exit'); } }
async function request(path, options = {}) {
  return fetch(`http://127.0.0.1:${port}${path}`, { ...options, redirect: 'manual', headers: { host: new URL(issuer).host, 'x-forwarded-proto': 'https', 'x-forwarded-for': '192.0.2.9', ...options.headers } });
}
async function privateRequest(path, options = {}) { return fetch(`http://127.0.0.1:${privatePort}${path}`, { ...options, headers: { authorization: `Bearer ${bridge}`, ...options.headers } }); }
const redirectUri = 'http://127.0.0.1:19876/mcp/oauth/callback';
async function register(uri = redirectUri) {
  // Exercise whitespace and multibyte text: replay framing must use the new byte length.
  return request('/reg', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ client_name: 'production proof — recovery', redirect_uris: [uri], token_endpoint_auth_method: 'none', grant_types: ['authorization_code', 'refresh_token'], response_types: ['code'] }, null, 2) });
}
async function token(params) {
  const response = await request('/token', { method: 'POST', headers: { 'content-type': 'application/x-www-form-urlencoded' }, body: new URLSearchParams(params) });
  return { status: response.status, body: await response.json() };
}
try {

  await start();
  assert.equal((await request('/health')).status, 200);
  const discovery = await (await request('/.well-known/openid-configuration')).json();
  assert.equal(discovery.issuer, issuer);
  const registration = await register(); assert.equal(registration.status, 201);
  const client = await registration.json();
  assert.equal((await register('https://evil.test/callback')).status, 400);
  assert.equal((await register('http://localhost:19876/mcp/oauth/callback')).status, 400);
  for (const path of ['/internal/audit', '/internal/test/entity-exists', '/internal/test/dcr-rate-limit']) assert.equal((await privateRequest(path)).status, 404);
  const verifier = randomBytes(32).toString('base64url');
  const challenge = createHash('sha256').update(verifier).digest('base64url');
  const auth = { prompt: 'consent', client_id: client.client_id, response_type: 'code', redirect_uri: redirectUri, scope: 'openid offline_access commoncal.event.read.basic', resource, code_challenge: challenge, code_challenge_method: 'S256', state: 'proof-state' };
  const withoutPkce = { ...auth }; delete withoutPkce.code_challenge; delete withoutPkce.code_challenge_method;
  const rejected = await request(`/auth?${new URLSearchParams(withoutPkce)}`);
  assert.ok(rejected.headers.get('location')?.includes('error=invalid_request'));
  const jar = new Map();
  let next = `/auth?${new URLSearchParams(auth)}`;
  let code;
  for (let steps = 0; steps < 20; steps++) {
    const response = await request(next, { headers: { cookie: [...jar].map(([key, value]) => `${key}=${value}`).join('; ') } });
    for (const cookie of response.headers.getSetCookie()) { const pair = cookie.split(';')[0]; const offset = pair.indexOf('='); jar.set(pair.slice(0, offset), pair.slice(offset + 1)); }
    const location = response.headers.get('location');
    assert.ok(location, `OAuth flow must redirect (status ${response.status})`);
    const url = new URL(location, issuer);
    if (url.origin === new URL(redirectUri).origin) { assert.equal(url.searchParams.get('state'), 'proof-state'); code = url.searchParams.get('code'); assert.ok(code); break; }
    if (url.origin === commoncal) {
      const handoff = url.searchParams.get('handoff');
      const view = await (await privateRequest(`/internal/interactions/${handoff}`)).json();
      const decision = { kind: view.prompt === 'login' ? 'login' : 'consent', subject: '42' };
      if (view.prompt === 'login') {
        const missingSubject = await privateRequest(`/internal/interactions/${handoff}`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ kind: 'login' }) });
        assert.equal(missingSubject.status, 400);
      }
      const approved = await privateRequest(`/internal/interactions/${handoff}`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify(decision) });
      assert.equal(approved.status, 200);
      next = new URL((await approved.json()).resumeUrl).pathname + `?handoff=${handoff}`;
    } else next = url.pathname + url.search;
  }
  assert.ok(code, 'authorization code issued');
  const issued = await token({ grant_type: 'authorization_code', client_id: client.client_id, code, code_verifier: verifier, redirect_uri: redirectUri, resource });
  assert.equal(issued.status, 200, JSON.stringify(issued.body)); assert.ok(issued.body.refresh_token); assert.ok(issued.body.access_token);
  const claims = JSON.parse(Buffer.from(issued.body.access_token.split('.')[1], 'base64url'));
  assert.equal(claims.iss, issuer); assert.equal(claims.aud, resource); assert.equal(claims.sub, '42');
  // Take an online snapshot while OAuth state is live, then restore it offline.
  const backupPath=resolve(directory,'recovery.sqlite');
  if (process.env.AUTH_TEST_ENCRYPTED_BACKUP === 'true') {
    const identity=resolve(directory,'backup-identity');
    execFileSync('age-keygen',['-o',identity],{stdio:'ignore'});
    const recipient=execFileSync('age-keygen',['-y',identity],{encoding:'utf8'}).trim();
    const backupDirectory=resolve(directory,'encrypted-backups');
    const archive=await encryptedBackup({AUTH_SQLITE_PATH:databasePath,AUTH_BACKUP_DIR:backupDirectory,AGE_RECIPIENT:recipient,TMPDIR:directory});
    execFileSync('age',['--decrypt','--identity',identity,'--output',backupPath,resolve(backupDirectory,archive)],{stdio:'ignore'});
  } else await snapshot(databasePath,backupPath);
  const restoreInputs = resolve(directory, 'restore-inputs');
  await mkdir(restoreInputs, { mode: 0o700 });
  await copyFile(backupPath, resolve(restoreInputs, 'auth.sqlite'));
  await writeFile(resolve(restoreInputs, 'auth-secret.json'), JSON.stringify({ data: Object.fromEntries(Object.entries({
    AUTH_JWKS: await readFile(keyFile, 'utf8'), AUTH_BRIDGE_KEY: bridge, AUTH_COOKIE_KEYS: cookies, AUTH_SIGNING_KID: activeKid,
  }).map(([key, value]) => [key, Buffer.from(value).toString('base64')])) }), { mode: 0o600 });
  await writeFile(resolve(restoreInputs, 'auth-config.json'), JSON.stringify({ data: {
    AUTH_ISSUER: issuer, AUTH_RESOURCE_URL: resource, AUTH_COMMONCAL_URL: commoncal,
    AUTH_TRUST_PROXY: 'true', AUTH_DCR_LOOPBACK_HOSTS: '127.0.0.1', AUTH_DCR_CALLBACK_PATH: '/mcp/oauth/callback',
  } }), { mode: 0o600 });
  await writeFile(resolve(restoreInputs, 'proof.json'), JSON.stringify({ issuer, resource, client_id: client.client_id, tokens: issued.body }), { mode: 0o600 });
  const restoreOutput = execFileSync(process.execPath, ['tests/restore-live-proof.mjs', restoreInputs], { cwd: base, encoding: 'utf8' });
  assert.match(restoreOutput, /PASS: restored schema/);
  await stop();
  await pool.end();
  await rm(databasePath+'-wal',{force:true}); await rm(databasePath+'-shm',{force:true});
  await copyFile(backupPath,databasePath);
  db=openDatabase(databasePath);
  // Persist the old public key for verification while activating a fresh private key.
  const previous = { ...jwks.keys[0] };
  for (const field of ['d', 'p', 'q', 'dp', 'dq', 'qi']) delete previous[field];
  const nextKey = generateKeyPairSync('rsa', { modulusLength: 2048 }).privateKey.export({ format: 'jwk' });
  activeKid = 'production-next';
  await writeFile(keyFile, JSON.stringify({ keys: [previous, { ...nextKey, kid: activeKid, use: 'sig', alg: 'RS256' }] }));
  await start();
  const publishedKeys = await (await request('/jwks')).json();
  assert.deepEqual(new Set(publishedKeys.keys.map((key) => key.kid)), new Set(['production-current', 'production-next']));
  assert.ok(publishedKeys.keys.every((key) => !key.d));
  const rotated = await token({ grant_type: 'refresh_token', client_id: client.client_id, refresh_token: issued.body.refresh_token, resource });
  assert.equal(rotated.status, 200, JSON.stringify(rotated.body)); assert.ok(rotated.body.refresh_token);
  assert.notEqual(rotated.body.refresh_token, issued.body.refresh_token);
  assert.equal(JSON.parse(Buffer.from(rotated.body.access_token.split('.')[0], 'base64url')).kid, activeKid);
  const revoked = await request('/token/revocation', { method: 'POST', headers: { 'content-type': 'application/x-www-form-urlencoded' }, body: new URLSearchParams({ token: rotated.body.refresh_token, client_id: client.client_id, token_type_hint: 'refresh_token' }) });
  assert.equal(revoked.status, 200);
  const revokedRefresh = await token({ grant_type: 'refresh_token', client_id: client.client_id, refresh_token: rotated.body.refresh_token, resource });
  assert.equal(revokedRefresh.status, 400);
  const reused = await token({ grant_type: 'refresh_token', client_id: client.client_id, refresh_token: issued.body.refresh_token, resource });
  assert.equal(reused.status, 400);
  // DCR counter persists across restart and is shared by every provider process.
  assert.equal((await register()).status, 201); assert.equal((await register()).status, 201); assert.equal((await register()).status, 429);
  const audit = await pool.query('SELECT event, detail FROM authorization_audit');
  assert.ok(audit.rows.some((row) => row.event === 'dcr_rate_limited'));
  assert.ok(audit.rows.some((row) => row.event === 'interaction_login'));
  assert.ok(!JSON.stringify(audit.rows).includes(bridge));
  assert.ok(!JSON.stringify(audit.rows).includes('production proof'));
  assert.ok(!logs.includes(issued.body.access_token));
  await pool.query("INSERT INTO provider_entity (model, id, payload, expires_at) VALUES ('RetentionProof', 'expired', '{}', (unixepoch()*1000 - 86400000)), ('RetentionProof', 'live', '{}', (unixepoch()*1000 + 86400000))");
  await pool.query("INSERT INTO interaction_handoff (token_hash, interaction_uid, view, expires_at) VALUES ('expired-proof', 'retention', '{}', (unixepoch()*1000 - 86400000))");
  await pool.query("INSERT INTO dcr_rate_bucket (bucket_key, window_start, count) VALUES ('expired-proof', (unixepoch()*1000 - 172800000), 1)");
  await pool.query("INSERT INTO authorization_audit (event, detail, created_at) VALUES ('retention-proof', '{}', (unixepoch()*1000 - 7862400000))");
  let expired = true;
  for (let attempt = 0; attempt < 60; attempt++) {
    const result = await pool.query("SELECT (SELECT count(*) FROM provider_entity WHERE model = 'RetentionProof' AND id = 'expired') + (SELECT count(*) FROM interaction_handoff WHERE token_hash = 'expired-proof') + (SELECT count(*) FROM dcr_rate_bucket WHERE bucket_key = 'expired-proof') + (SELECT count(*) FROM authorization_audit WHERE event = 'retention-proof') AS remaining");
    if (Number(result.rows[0].remaining) === 0) { expired = false; break; }
    await new Promise((done) => setTimeout(done, 50));
  }
  assert.equal(expired, false, 'periodic production retention removes expired rows without an internal cleanup caller');
  assert.equal((await pool.query("SELECT count(*) AS count FROM provider_entity WHERE model = 'RetentionProof' AND id = 'live'")).rows[0].count, 1);
  await pool.query('ALTER TABLE dcr_rate_bucket RENAME TO dcr_rate_bucket_offline');
  assert.equal((await request('/ready')).status, 500);
  assert.equal((await request('/health')).status, 200);
  await stop();
  await assert.rejects(start(), /production startup failed/);
  const absentSchema = await pool.query("SELECT name FROM sqlite_master WHERE type='table' AND name='dcr_rate_bucket'");
  assert.equal(absentSchema.rows.length, 0, 'production startup must not create missing tables');
  await pool.query('ALTER TABLE dcr_rate_bucket_offline RENAME TO dcr_rate_bucket');
  await start();
  assert.equal((await request('/ready')).status, 200);
  console.log('production integration passed: discovery, DCR policy, PKCE, bridge login/consent, tokens, online backup/restore refresh continuation, restart persistence, refresh rotation/reuse/revoke, JWKS rotation overlap, durable audit/rate limit, disabled lab endpoints');
} finally {
  await stop(); await pool.end(); await rm(directory, { recursive: true, force: true });
}
