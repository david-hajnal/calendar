import assert from 'node:assert/strict';
import { mkdtemp, rm, readdir, readFile, utimes } from 'node:fs/promises';
import { resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { execFileSync, spawnSync } from 'node:child_process';
import { openDatabase, migrateSQLite, SQLiteStorage } from '../src/sqlite-storage.mjs';
import { encryptedBackup } from '../src/backup.mjs';
const directory=await mkdtemp(resolve(tmpdir(),'sqlite-proof-'));
let storage;
try {
  const path=resolve(directory,'auth.sqlite');
  assert.throws(()=>new SQLiteStorage(path));
  const db=openDatabase(path,{create:true});migrateSQLite(db);migrateSQLite(db);db.close();
  storage=new SQLiteStorage(path);const Adapter=storage.adapter();
  const refresh=new Adapter('RefreshToken');await refresh.upsert('live',{grantId:'grant',uid:'uid',userCode:'code'},60);
  assert.ok(await refresh.findByUid('uid'));assert.ok(await refresh.findByUserCode('code'));
  await refresh.consume('live');const consumed=(await refresh.find('live')).consumed;await refresh.consume('live');assert.equal((await refresh.find('live')).consumed,consumed);
  await refresh.upsert('expired',{},-1);assert.equal(await refresh.find('expired'),undefined);await refresh.consume('expired');assert.equal(storage.db.prepare("SELECT consumed_at FROM provider_entity WHERE id='expired'").get().consumed_at,null);
  const handoffs=storage.handoffs();const handoff=await handoffs.create('interaction',{prompt:'consent'});
  assert.equal(await handoffs.decide(handoff,{subject:'1',kind:'consent'}),'interaction');assert.equal(await handoffs.decide(handoff,{kind:'consent',subject:'1'}),'interaction');assert.equal(await handoffs.decide(handoff,{kind:'consent',subject:'2'}),undefined);
  assert.equal(await handoffs.consume(handoff,'wrong'),undefined);assert.deepEqual(await handoffs.consume(handoff,'interaction'),{kind:'consent',subject:'1'});assert.equal(await handoffs.consume(handoff,'interaction'),undefined);
  const expired=await handoffs.create('expired',{},-1);assert.equal(await handoffs.lookup(expired),undefined);assert.equal(await handoffs.decide(expired,{}),undefined);
  assert.equal(storage.rateLimit('rate',1),true);assert.equal(storage.rateLimit('rate',1),false);storage.audit('proof',{});
  const identity=resolve(directory,'identity');execFileSync('age-keygen',['-o',identity],{stdio:'ignore'});const recipient=execFileSync('age-keygen',['-y',identity],{encoding:'utf8'}).trim();
  const backups=resolve(directory,'backups');const env={AUTH_SQLITE_PATH:path,AUTH_BACKUP_DIR:backups,AGE_RECIPIENT:recipient,TMPDIR:directory};
  const name=await encryptedBackup(env);assert.deepEqual(await readdir(backups),[name]);
  const restored=resolve(directory,'restored.sqlite');execFileSync('age',['--decrypt','--identity',identity,'--output',restored,resolve(backups,name)],{stdio:'ignore'});
  const recovery=new SQLiteStorage(restored);assert.equal((await new (recovery.adapter())('RefreshToken').find('live')).consumed,consumed);assert.equal(recovery.db.prepare('SELECT count FROM dcr_rate_bucket').get().count,2);recovery.end();
  const raw=await readFile(resolve(backups,name));assert.ok(!raw.subarray(0,16).equals(Buffer.from('SQLite format 3\0')));
  // A failed backup must preserve even archives old enough for retention.
  const old=new Date(Date.now()-30*86400000);await utimes(resolve(backups,name),old,old);
  await assert.rejects(encryptedBackup({...env,AGE_RECIPIENT:'age1invalid'}));assert.deepEqual(await readdir(backups),[name]);assert.ok(!(await readdir(directory)).some(name=>name.startsWith('auth-backup-')));
  const fresh=await encryptedBackup(env);assert.deepEqual(await readdir(backups),[fresh]);
  if (process.env.AUTH_SECRET_HELPER) {
    const provision=(recipient)=>spawnSync('/bin/bash',['-c','set -euo pipefail; source "$AUTH_SECRET_HELPER"; validate_auth_backup_recipient'],{
      env:{...process.env,AUTH_BACKUP_AGE_RECIPIENT:recipient},encoding:'utf8',
    });
    const valid=provision(recipient);assert.equal(valid.status,0,valid.stderr);
    assert.equal(valid.stdout,'');
    const invalid=provision('age1invalid');assert.equal(invalid.status,1);assert.equal(invalid.stdout,'');
  }
  await refresh.revokeByGrantId('grant');assert.equal(await refresh.find('live'),undefined);
  storage.end();storage=null;const restarted=new SQLiteStorage(path);assert.equal(restarted.rateLimit('rate',1),false);restarted.end();
  console.log('SQLite storage proof passed: atomic consume/revoke, expiry, canonical handoff retries, durable audit/rate, online encrypted backup, isolated restore, failure cleanup');
} finally { storage?.end();await rm(directory,{recursive:true,force:true}); }
