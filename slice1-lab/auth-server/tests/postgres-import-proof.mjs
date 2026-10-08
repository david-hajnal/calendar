// Optional legacy-source migration proof; never needed by production SQLite.
import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import pg from 'pg';
import { SQLiteStorage } from '../src/sqlite-storage.mjs';

const url = process.env.AUTH_TEST_DATABASE_URL;
assert.ok(url, 'a disposable PostgreSQL fixture is required');
const schema = `import_proof_${randomBytes(8).toString('hex')}`;
const admin = new pg.Client({ connectionString: url });
await admin.connect();
const directory = await mkdtemp(resolve(tmpdir(), 'auth-import-proof-'));
let source, target;
try {
  await admin.query(`CREATE SCHEMA ${schema}`);
  const database = new URL(url);
  database.searchParams.set('options', `-c search_path=${schema}`);
  source = new pg.Client({ connectionString: database.href });
  await source.connect();
  for (const file of ['0001_lab.sql', '0002_production.sql']) {
    await source.query(await readFile(new URL(`../migrations/${file}`, import.meta.url), 'utf8'));
  }
  const timestamp = new Date('2026-10-08T10:00:00.123Z');
  const expires = new Date(Date.now() + 86_400_000);
  const payload = { grantId: 'saved-grant', consumed: 1791453600, accountId: '42' };
  await source.query('INSERT INTO provider_entity VALUES ($1,$2,$3,$4,$5,$6,$7,$8)',
    ['RefreshToken', 'saved-token', payload, 'saved-grant', 'saved-uid', null, expires, timestamp]);
  await source.query('INSERT INTO interaction_handoff VALUES ($1,$2,$3,$4,$5,$6)',
    ['saved-hash', 'saved-interaction', { subject: '42', prompt: 'consent' }, { subject: '42', kind: 'consent' }, expires, timestamp]);
  await source.query('INSERT INTO authorization_audit(created_at,event,detail) VALUES ($1,$2,$3)',
    [timestamp, 'saved-event', { reason: 'saved-proof' }]);
  await source.query('INSERT INTO dcr_rate_bucket VALUES ($1,$2,$3)', ['saved-bucket', timestamp, 3]);
  const path = resolve(directory, 'import.sqlite');
  const result = spawnSync(process.execPath, ['src/import-postgres.mjs'], {
    cwd: resolve(import.meta.dirname, '..'),
    env: { ...process.env, DATABASE_URL: database.href, AUTH_SQLITE_PATH: path },
    encoding: 'utf8',
  });
  assert.equal(result.status, 0, result.stderr);
  assert.ok(!result.stdout.includes(database.href));
  target = new SQLiteStorage(path);
  assert.deepEqual(await new (target.adapter())('RefreshToken').find('saved-token'), payload);
  const provider = target.db.prepare('SELECT expires_at,consumed_at FROM provider_entity').get();
  assert.equal(provider.expires_at, expires.getTime());
  assert.equal(provider.consumed_at, timestamp.getTime());
  const handoff = target.db.prepare('SELECT * FROM interaction_handoff').get();
  assert.deepEqual(JSON.parse(handoff.decision), { subject: '42', kind: 'consent' });
  assert.equal(handoff.consumed_at, timestamp.getTime());
  assert.equal(target.db.prepare('SELECT created_at FROM authorization_audit').get().created_at, timestamp.getTime());
  assert.equal(target.db.prepare('SELECT count FROM dcr_rate_bucket').get().count, 3);
  target.audit('after-import', {});
  assert.equal(target.db.prepare('SELECT COUNT(*) AS count FROM authorization_audit').get().count, 2);
  // Source stays unchanged, and a second import must refuse the destination.
  assert.equal((await source.query('SELECT count(*) AS count FROM authorization_audit')).rows[0].count, '1');
  const repeated = spawnSync(process.execPath, ['src/import-postgres.mjs'], {
    cwd: resolve(import.meta.dirname, '..'),
    env: { ...process.env, DATABASE_URL: database.href, AUTH_SQLITE_PATH: path },
  });
  assert.equal(repeated.status, 1);
  assert.equal(target.db.prepare('SELECT COUNT(*) AS count FROM authorization_audit').get().count, 2);
  console.log('PostgreSQL import proof passed: all stores, timestamps, consumption, source preservation and overwrite refusal');
} finally {
  target?.end();
  await source?.end();
  await admin.query(`DROP SCHEMA IF EXISTS ${schema} CASCADE`);
  await admin.end();
  await rm(directory, { recursive: true, force: true });
}
