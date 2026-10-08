import { DatabaseSync, backup } from 'node:sqlite';
import { mkdtemp, mkdir, rename, rm, readdir, stat, chmod } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { isAbsolute, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { pathToFileURL } from 'node:url';
export async function snapshot(source, destination) {
  if (!isAbsolute(source ?? '') || !existsSync(source)) throw new Error('existing absolute authorization database required');
  const db = new DatabaseSync(source, { readOnly: true });
  try { await backup(db, destination); } finally { db.close(); }
  await chmod(destination, 0o600);
  const copy = new DatabaseSync(destination, { readOnly: true });
  try {
    const result = copy.prepare('PRAGMA integrity_check').all();
    if (result.length !== 1 || result[0].integrity_check !== 'ok') throw new Error('snapshot integrity failure');
    if (copy.prepare('SELECT MAX(version) AS version FROM schema_migration').get().version !== 1) throw new Error('snapshot schema invalid');
  } finally { copy.close(); }
}
export async function encryptedBackup(env = process.env) {
  const source=env.AUTH_SQLITE_PATH;
  const directory=env.AUTH_BACKUP_DIR ?? '/app/data/backups';
  const days=Number(env.AUTH_BACKUP_RETENTION_DAYS ?? 14);
  const recipient=env.AGE_RECIPIENT;
  if (!isAbsolute(directory) || !Number.isInteger(days) || days<1 || !/^age1[a-z0-9]+$/.test(recipient ?? '')) throw new Error('invalid backup configuration');
  await mkdir(directory,{recursive:true,mode:0o700});
  const work=await mkdtemp(resolve(env.TMPDIR ?? tmpdir(),'auth-backup-'));
  const name=`auth-${new Date().toISOString().replaceAll(':','-')}-${randomUUID()}.sqlite.age`;
  const partial=resolve(directory,`.${name}.partial`);
  try {
    const plain=resolve(work,'snapshot.sqlite');
    await snapshot(source,plain);
    await new Promise((done,reject)=>{
      const child=spawn('age',['--encrypt','--recipient',recipient,'--output',partial,plain],{stdio:'ignore'});
      child.on('error',reject); child.on('exit',code=>code===0?done():reject(new Error('backup encryption failed')));
    });
    await chmod(partial,0o600);
    await rename(partial,resolve(directory,name));
    const cutoff=Date.now()-days*86400000;
    for (const file of await readdir(directory)) if (/^auth-.*\.sqlite\.age$/.test(file)) { const path=resolve(directory,file); if ((await stat(path)).mtimeMs<cutoff) await rm(path); }
    return name;
  } finally { await rm(work,{recursive:true,force:true}); await rm(partial,{force:true}); }
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try { console.log(await encryptedBackup()); } catch { console.error('authorization backup failed'); process.exitCode=1; }
}
