// Migration entrypoint for the deployment migration job.
//
// Connects to the managed PostgreSQL instance using DATABASE_URL and applies
// every migration in ../migrations/ in lexicographic order. Idempotent: each
// migration uses CREATE TABLE IF NOT EXISTS / CREATE INDEX IF NOT EXISTS, so
// re-running is safe. Exits 0 on success, non-zero on the first failure.
//
// This is the immutable entrypoint used by the Helm migration Job. The long-
// lived server (server.mjs) also runs the same migrations inline so a fresh
// pod can start without a separate migration step; the Job exists to gate
// rollout on a clean schema before the Deployment becomes ready.

import { readdir, readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import pg from 'pg';

const { Pool } = pg;
const here = dirname(fileURLToPath(import.meta.url));
const migrationsDir = resolve(here, '../migrations');

const connectionString = process.env.DATABASE_URL;
if (!connectionString) {
  console.error('migrate: DATABASE_URL is required');
  process.exit(1);
}

const pool = new Pool({ connectionString, max: 1 });

async function main() {
  const files = (await readdir(migrationsDir))
    .filter((f) => f.endsWith('.sql'))
    .sort();
  if (files.length === 0) {
    console.error('migrate: no migration files found in', migrationsDir);
    process.exit(1);
  }
  for (const file of files) {
    const sql = await readFile(resolve(migrationsDir, file), 'utf8');
    await pool.query(sql);
    console.log(`migrate: applied ${file}`);
  }
  console.log(`migrate: applied ${files.length} migration(s)`);
}

try {
  await main();
  await pool.end();
  process.exit(0);
} catch (error) {
  console.error('migrate: failed:', error?.message ?? 'unknown error');
  await pool.end().catch(() => {});
  process.exit(1);
}
