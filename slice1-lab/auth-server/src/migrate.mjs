import { openDatabase, migrateSQLite } from './sqlite-storage.mjs';
// Explicit schema migration entrypoint. Production uses a versioned SQLite
// transaction; PostgreSQL remains available for the disposable OAuth lab.
// The production server never creates or migrates schema during startup.

import { readdir, readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';


const here = dirname(fileURLToPath(import.meta.url));
const migrationsDir = resolve(here, '../migrations');

if (process.env.AUTH_SQLITE_PATH || process.env.NODE_ENV === 'production') {
  try { const db=openDatabase(process.env.AUTH_SQLITE_PATH,{create:true}); try { migrateSQLite(db); } finally { db.close(); } console.log('migrate: SQLite schema ready'); process.exit(0); } catch { console.error('migrate: SQLite migration failed'); process.exit(1); }
}
const { Pool } = (await import('pg')).default;
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
  const client = await pool.connect();
  try {
  await client.query('BEGIN');
  await client.query("SELECT pg_advisory_xact_lock(hashtext('commoncal-auth-migrations'))");
  for (const file of files) {
    const sql = await readFile(resolve(migrationsDir, file), 'utf8');
    await client.query(sql);
    console.log(`migrate: applied ${file}`);
  }
  await client.query('COMMIT');
  } catch (error) {
    await client.query('ROLLBACK');
    throw error;
  } finally { client.release(); }
  console.log(`migrate: applied ${files.length} migration(s)`);
}

try {
  await main();
  await pool.end();
  process.exit(0);
} catch (error) {
  console.error('migrate: failed');
  await pool.end().catch(() => {});
  process.exit(1);
}
