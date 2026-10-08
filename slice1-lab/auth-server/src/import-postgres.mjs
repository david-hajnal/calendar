// Optional offline migration: stop the issuer before importing and preserve keys.
import pg from 'pg';
import { openDatabase, migrateSQLite, canonical } from './sqlite-storage.mjs';
import { open, rm, chmod } from 'node:fs/promises';
import { isAbsolute } from 'node:path';
const destination=process.env.AUTH_SQLITE_PATH;
let created=false,db,source;
try {
  if (!destination || !isAbsolute(destination) || !process.env.DATABASE_URL) throw new Error('source and destination required');
  const file=await open(destination,'wx',0o600); created=true; await file.close();
  db=openDatabase(destination,{create:true}); migrateSQLite(db);
  source=new pg.Client({connectionString:process.env.DATABASE_URL}); await source.connect();
  await source.query('BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY');
  db.exec('BEGIN IMMEDIATE');
  const specs={provider_entity:['model','id','payload','grant_id','uid','user_code','expires_at','consumed_at'],interaction_handoff:['token_hash','interaction_uid','view','decision','expires_at','consumed_at'],authorization_audit:['id','created_at','event','detail'],dcr_rate_bucket:['bucket_key','window_start','count']};
  for (const [table,columns] of Object.entries(specs)) {
    const insert=db.prepare(`INSERT INTO ${table}(${columns.join(',')}) VALUES(${columns.map(()=>'?').join(',')})`);
    // Cursor bounds memory independently of the size of persisted OAuth state.
    await source.query(`DECLARE import_rows NO SCROLL CURSOR FOR SELECT ${columns.join(',')} FROM ${table}`);
    while (true) {
      const {rows}=await source.query('FETCH FORWARD 250 FROM import_rows'); if (!rows.length) break;
      for (const row of rows) insert.run(...columns.map(column=>{
        const value=row[column]; if (value==null) return null;
        if (['payload','view','decision','detail'].includes(column)) return column==='decision'||column==='view'?canonical(value):JSON.stringify(value);
        if (['expires_at','consumed_at','created_at','window_start'].includes(column)) return new Date(value).getTime();
        return value;
      }));
    }
    await source.query('CLOSE import_rows');
  }
  if (db.prepare('PRAGMA integrity_check').get().integrity_check!=='ok') throw new Error('import integrity failure');
  db.exec('COMMIT'); await source.query('COMMIT'); db.close();db=null; await source.end();source=null;
  await chmod(destination,0o600);
  console.log('authorization PostgreSQL import completed');
} catch {
  try { db?.close(); } catch {} try { await source?.end(); } catch {}
  if (created) for (const suffix of ['', '-wal','-shm']) await rm(destination+suffix,{force:true});
  console.error('authorization PostgreSQL import failed; source remains unchanged'); process.exitCode=1;
}
