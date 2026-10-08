import { DatabaseSync } from 'node:sqlite';
import { isAbsolute } from 'node:path';
import { existsSync, readFileSync, chmodSync } from 'node:fs';
import { createHash, randomBytes } from 'node:crypto';
const now = () => Date.now();
const hash = (token) => createHash('sha256').update(token).digest('hex');
export function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(',')}]`;
  if (value && typeof value === 'object') return `{${Object.keys(value).sort().map(k => `${JSON.stringify(k)}:${canonical(value[k])}`).join(',')}}`;
  return JSON.stringify(value);
}
export function openDatabase(path, { create = false } = {}) {
  if (!path || !isAbsolute(path) || path === ':memory:') throw new Error('AUTH_SQLITE_PATH must be an absolute file path');
  if (!create && !existsSync(path)) throw new Error('authorization database must be explicitly migrated');
  const db = new DatabaseSync(path);
  chmodSync(path, 0o600);
  db.exec('PRAGMA busy_timeout=5000; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA cache_size=-2048; PRAGMA foreign_keys=ON;');
  return db;
}
export function migrateSQLite(db) {
  db.exec('BEGIN IMMEDIATE');
  try {
    db.exec('CREATE TABLE IF NOT EXISTS schema_migration(version INTEGER PRIMARY KEY)');
    const version = db.prepare('SELECT MAX(version) AS version FROM schema_migration').get().version ?? 0;
    if (version > 1) throw new Error('unsupported authorization schema');
    if (version < 1) {
      db.exec(readFileSync(new URL('../migrations/sqlite/0001.sql', import.meta.url), 'utf8'));
      db.prepare('INSERT INTO schema_migration VALUES (?)').run(1);
    }
    db.exec('COMMIT');
  } catch (error) { db.exec('ROLLBACK'); throw error; }
}
export class SQLiteStorage {
  constructor(path) { this.db = openDatabase(path); try { this.assertReady(); } catch (error) { this.db.close(); throw error; } }
  assertReady() {
    if (this.db.prepare('SELECT MAX(version) AS version FROM schema_migration').get().version !== 1) throw new Error('authorization schema requires migration');
    this.db.prepare('SELECT 1 FROM provider_entity,interaction_handoff,authorization_audit,dcr_rate_bucket LIMIT 0').all();
  }
  audit(event, detail) { this.db.prepare('INSERT INTO authorization_audit(created_at,event,detail) VALUES(?,?,?)').run(now(),event,JSON.stringify(detail)); }
  rateLimit(key, limit) {
    const minute = Math.floor(now()/60000)*60000;
    const row = this.db.prepare(`INSERT INTO dcr_rate_bucket VALUES(?,?,1) ON CONFLICT(bucket_key) DO UPDATE SET window_start=excluded.window_start,count=CASE WHEN dcr_rate_bucket.window_start=excluded.window_start THEN dcr_rate_bucket.count+1 ELSE 1 END RETURNING count`).get(key,minute);
    return row.count <= limit;
  }
  cleanup(batch=1000) {
    const output={};
    for (const [name,table,column,cutoff] of [['provider','provider_entity','expires_at',now()],['handoffs','interaction_handoff','expires_at',now()],['rateBuckets','dcr_rate_bucket','window_start',now()-86400000],['audit','authorization_audit','created_at',now()-90*86400000]]) output[name]=this.db.prepare(`DELETE FROM ${table} WHERE rowid IN (SELECT rowid FROM ${table} WHERE ${column} <= ? ORDER BY ${column} LIMIT ?)`).run(cutoff,batch).changes;
    return output;
  }
  end() { this.db.close(); }
  adapter() {
    const db=this.db;
    return class SQLiteAdapter {
      constructor(model) { this.model=model; }
      async upsert(id,payload,ttl) { db.prepare(`INSERT INTO provider_entity VALUES(?,?,?,?,?,?,?,NULL) ON CONFLICT(model,id) DO UPDATE SET payload=excluded.payload,grant_id=excluded.grant_id,uid=excluded.uid,user_code=excluded.user_code,expires_at=excluded.expires_at,consumed_at=NULL`).run(this.model,id,JSON.stringify(payload),payload.grantId??null,payload.uid??null,payload.userCode??null,typeof ttl==='number'?now()+ttl*1000:null); }
      async lookup(column,value) { const row=db.prepare(`SELECT payload FROM provider_entity WHERE model=? AND ${column}=? AND (expires_at IS NULL OR expires_at>?) LIMIT 1`).get(this.model,value,now()); return row?JSON.parse(row.payload):undefined; }
      async find(id) { return this.lookup('id',id); }
      async findByUid(uid) { return this.lookup('uid',uid); }
      async findByUserCode(code) { return this.lookup('user_code',code); }
      async destroy(id) { db.prepare('DELETE FROM provider_entity WHERE model=? AND id=?').run(this.model,id); }
      async revokeByGrantId(id) { db.prepare('DELETE FROM provider_entity WHERE grant_id=?').run(id); }
      async consume(id) { const time=now(); db.prepare(`UPDATE provider_entity SET payload=json_set(payload,'$.consumed',?),consumed_at=? WHERE model=? AND id=? AND consumed_at IS NULL AND (expires_at IS NULL OR expires_at>?)`).run(Math.floor(time/1000),time,this.model,id,time); }
      async cleanup() { return db.prepare('DELETE FROM provider_entity WHERE model=? AND expires_at<=?').run(this.model,now()).changes; }
    };
  }
  handoffs() {
    const db=this.db;
    return {
      async create(uid,view,ttl=120) { const token=randomBytes(32).toString('base64url'); db.prepare('INSERT INTO interaction_handoff(token_hash,interaction_uid,view,expires_at) VALUES(?,?,?,?)').run(hash(token),uid,canonical(view),now()+ttl*1000); return token; },
      async lookup(token) { const row=db.prepare('SELECT interaction_uid,view,decision FROM interaction_handoff WHERE token_hash=? AND expires_at>? AND consumed_at IS NULL').get(hash(token),now()); return row?{...row,view:JSON.parse(row.view),decision:row.decision?JSON.parse(row.decision):null}:undefined; },
      async decide(token,decision) { const value=canonical(decision); return db.prepare('UPDATE interaction_handoff SET decision=? WHERE token_hash=? AND expires_at>? AND consumed_at IS NULL AND (decision IS NULL OR decision=?) RETURNING interaction_uid').get(value,hash(token),now(),value)?.interaction_uid; },
      async consume(token,uid) { const time=now(); const row=db.prepare('UPDATE interaction_handoff SET consumed_at=? WHERE token_hash=? AND interaction_uid=? AND expires_at>? AND consumed_at IS NULL AND decision IS NOT NULL RETURNING decision').get(time,hash(token),uid,time); return row?JSON.parse(row.decision):undefined; },
      async cleanup() { return db.prepare('DELETE FROM interaction_handoff WHERE expires_at<=?').run(now()).changes; }
    };
  }
}
