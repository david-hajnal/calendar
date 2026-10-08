import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm, writeFile, readFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { openDatabase, migrateSQLite, SQLiteStorage } from '../src/sqlite-storage.mjs';
import { snapshot, encryptedBackup } from '../src/backup.mjs';
test('SQLite durable state, expiry, canonical decisions and grant revocation',async()=>{
  const dir=await mkdtemp(resolve(tmpdir(),'sqlite-unit-'));let storage;
  try {
    const path=resolve(dir,'auth.sqlite');assert.throws(()=>new SQLiteStorage(path));
    const db=openDatabase(path,{create:true});migrateSQLite(db);migrateSQLite(db);db.close();
    storage=new SQLiteStorage(path);const Adapter=storage.adapter();const adapter=new Adapter('RefreshToken');
    await adapter.upsert('live',{grantId:'grant',uid:'uid',userCode:'code'},60);await adapter.consume('live');const consumed=(await adapter.find('live')).consumed;await adapter.consume('live');assert.equal((await adapter.findByUid('uid')).consumed,consumed);
    await adapter.upsert('expired',{},-1);assert.equal(await adapter.find('expired'),undefined);
    const handoffs=storage.handoffs();const token=await handoffs.create('uid',{});
    assert.equal(await handoffs.decide(token,{kind:'consent',subject:'1'}),'uid');assert.equal(await handoffs.decide(token,{subject:'1',kind:'consent'}),'uid');assert.equal(await handoffs.decide(token,{subject:'2',kind:'consent'}),undefined);assert.equal(await handoffs.consume(token,'wrong'),undefined);assert.ok(await handoffs.consume(token,'uid'));assert.equal(await handoffs.consume(token,'uid'),undefined);
    assert.equal(storage.rateLimit('rate',1),true);storage.audit('proof',{});
    await snapshot(path,resolve(dir,'copy.sqlite'));const copy=new SQLiteStorage(resolve(dir,'copy.sqlite'));assert.equal((await new (copy.adapter())('RefreshToken').find('live')).consumed,consumed);copy.end();
    await adapter.revokeByGrantId('grant');assert.equal(await adapter.find('live'),undefined);
    storage.end();storage=new SQLiteStorage(path);assert.equal(storage.rateLimit('rate',1),false);
    await assert.rejects(encryptedBackup({AUTH_SQLITE_PATH:path,AUTH_BACKUP_DIR:resolve(dir,'backup'),AGE_RECIPIENT:'invalid'}));
  } finally {storage?.end();await rm(dir,{recursive:true,force:true});}
});
test('offline PostgreSQL importer refuses existing destination without changing it',async()=>{
  const dir=await mkdtemp(resolve(tmpdir(),'sqlite-import-'));
  try {
    const path=resolve(dir,'existing.sqlite');const content=Buffer.from('existing destination must remain untouched');await writeFile(path,content);
    const result=spawnSync(process.execPath,[new URL('../src/import-postgres.mjs',import.meta.url).pathname],{env:{...process.env,AUTH_SQLITE_PATH:path,DATABASE_URL:'postgresql://invalid-source.invalid/auth'},encoding:'utf8'});
    assert.equal(result.status,1);assert.match(result.stderr,/source remains unchanged/);assert.deepEqual(await readFile(path),content);
  } finally {await rm(dir,{recursive:true,force:true});}
});
test('independent SQLite connections share atomic decisions, consumption and durable audit',async()=>{
  const dir=await mkdtemp(resolve(tmpdir(),'sqlite-shared-'));let first,second;
  try {
    const path=resolve(dir,'auth.sqlite');const db=openDatabase(path,{create:true});migrateSQLite(db);db.close();
    first=new SQLiteStorage(path);second=new SQLiteStorage(path);
    const token=await first.handoffs().create('uid',{subject:'42'});
    assert.equal(await second.handoffs().decide(token,{kind:'login',subject:'42'}),'uid');
    assert.equal(await first.handoffs().decide(token,{subject:'43',kind:'login'}),undefined);
    assert.deepEqual(await first.handoffs().consume(token,'uid'),{kind:'login',subject:'42'});
    assert.equal(await second.handoffs().consume(token,'uid'),undefined);
    first.audit('shared-proof',{subject:'42'});
    assert.equal(second.db.prepare('SELECT event FROM authorization_audit').get().event,'shared-proof');
    assert.equal(first.rateLimit('shared-ip',1),true);assert.equal(second.rateLimit('shared-ip',1),false);
    const Adapter=first.adapter();await new Adapter('RefreshToken').upsert('token',{grantId:'shared-grant'},60);
    const Other=second.adapter();await new Other('Grant').revokeByGrantId('shared-grant');assert.equal(await new Adapter('RefreshToken').find('token'),undefined);
  } finally {first?.end();second?.end();await rm(dir,{recursive:true,force:true});}
});
