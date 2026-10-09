// Inputs are private local files. The auth child has only loopback listeners.
import { readFile, writeFile, copyFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn } from 'node:child_process';
import { DatabaseSync } from 'node:sqlite';
import { createPublicKey, verify } from 'node:crypto';

const input = process.argv[2] ?? '/inputs';
const work = await mkdtemp(join(tmpdir(), 'auth-restore-proof-'));
let child;
let stage = 'private input files';
try {
  const secret = JSON.parse(await readFile(join(input, 'auth-secret.json'), 'utf8'));
  const config = JSON.parse(await readFile(join(input, 'auth-config.json'), 'utf8')).data;
  const proof = JSON.parse(await readFile(join(input, 'proof.json'), 'utf8'));
  const decode = key => Buffer.from(secret.data[key], 'base64').toString();
  const jwks = JSON.parse(decode('AUTH_JWKS'));
  await writeFile(join(work, 'jwks.json'), JSON.stringify(jwks), { mode: 0o600 });
  await copyFile(join(input, 'auth.sqlite'), join(work, 'auth.sqlite'));
  stage = 'restored database integrity and schema';
  const db = new DatabaseSync(join(work, 'auth.sqlite'), { readOnly: true });
  try {
    if (db.prepare('PRAGMA integrity_check').get().integrity_check !== 'ok'
        || db.prepare('SELECT MAX(version) AS version FROM schema_migration').get().version !== 1)
      throw new Error('invalid restore');
    for (const table of ['provider_entity', 'interaction_handoff', 'authorization_audit', 'dcr_rate_bucket'])
      db.prepare(`SELECT COUNT(*) FROM ${table}`).get();
    const refreshRows = db.prepare("SELECT expires_at FROM provider_entity WHERE model='RefreshToken' AND json_extract(payload, '$.clientId')=?").all(proof.client_id);
    if (!refreshRows.length) {
      console.error('Snapshot contains no refresh-token records for this proof client. Check that the proof and backup belong to the same run.');
      throw new Error('missing proof records');
    }
    const latestExpiry = Math.max(...refreshRows.map(row => row.expires_at ?? Infinity));
    if (latestExpiry <= Date.now()) {
      console.error(`Snapshot refresh tokens for this client expired by ${new Date(latestExpiry).toISOString()}. Repeat login, backup and restore within the one-hour lifetime.`);
      throw new Error('expired snapshot tokens');
    }
  } finally { db.close(); }
  if (proof.issuer !== config.AUTH_ISSUER || proof.resource !== config.AUTH_RESOURCE_URL) throw new Error('configuration mismatch');
  stage = 'isolated auth startup';
  child = spawn(process.execPath, ['src/production.mjs'], {
    env: { ...process.env, ...config, AUTH_SQLITE_PATH: join(work, 'auth.sqlite'),
      AUTH_JWKS_FILE: join(work, 'jwks.json'), AUTH_BRIDGE_KEY: decode('AUTH_BRIDGE_KEY'),
      AUTH_COOKIE_KEYS: decode('AUTH_COOKIE_KEYS'), AUTH_SIGNING_KID: decode('AUTH_SIGNING_KID'),
      AUTH_PUBLIC_BIND: '127.0.0.1', AUTH_PRIVATE_BIND: '127.0.0.1',
      AUTH_PUBLIC_PORT: '4500', AUTH_PRIVATE_PORT: '4501' }, stdio: 'ignore',
  });
  const headers = { host: new URL(proof.issuer).host, 'x-forwarded-proto': 'https' };
  let ready = false;
  for (let i = 0; i < 100 && child.exitCode === null; i++) {
    try { ready = (await fetch('http://127.0.0.1:4500/ready', { headers })).ok; } catch {}
    if (ready) break;
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  if (!ready) throw new Error('restored instance unavailable');
  stage = 'restored refresh token';
  const response = await fetch('http://127.0.0.1:4500/token', {
    method: 'POST', headers: { ...headers, 'content-type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams({ grant_type: 'refresh_token', client_id: proof.client_id,
      refresh_token: proof.tokens.refresh_token, resource: proof.resource }),
  });
  if (!response.ok) {
    console.error(`Restored token endpoint HTTP ${response.status}`);
    try {
      const rejection = await response.json();
      const allowed = ['invalid_grant', 'invalid_client', 'invalid_request', 'invalid_scope', 'invalid_target', 'unauthorized_client', 'unsupported_grant_type'];
      if (allowed.includes(rejection.error)) console.error(`OAuth error: ${rejection.error}`);
    } catch {}
    throw new Error('restored refresh failed');
  }
  const tokens = await response.json();
  if (!tokens.access_token || !tokens.refresh_token) throw new Error('incomplete refresh');
  stage = 'restored signing key and token claims';
  const [head, body, signature] = tokens.access_token.split('.');
  const kid = JSON.parse(Buffer.from(head, 'base64url')).kid;
  const key = jwks.keys.find(key => key.kid === kid);
  if (!key || !verify('RSA-SHA256', Buffer.from(`${head}.${body}`), createPublicKey({ key, format: 'jwk' }), Buffer.from(signature, 'base64url')))
    throw new Error('invalid restored signing key');
  const claims = JSON.parse(Buffer.from(body, 'base64url'));
  const original = JSON.parse(Buffer.from(proof.tokens.access_token.split('.')[1], 'base64url'));
  if (claims.iss !== proof.issuer || claims.aud !== proof.resource || claims.sub !== original.sub)
    throw new Error('restored token claims mismatch');
  console.log('PASS: restored schema, all four tables, refresh continuation, signing key, issuer, audience and subject.');
} catch {
  console.error(`FAIL: ${stage}. No credentials printed.`);
  process.exitCode = 1;
} finally {
  if (child && child.exitCode === null) {
    const stopped = new Promise(resolve => child.once('exit', resolve));
    child.kill('SIGTERM');
    const timeout = setTimeout(() => child.kill('SIGKILL'), 5000);
    await stopped;
    clearTimeout(timeout);
  }
  await rm(work, { recursive: true, force: true });
}
