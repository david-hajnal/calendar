# Backup and restore

Backups are compressed SQLite snapshots. When encryption is enabled through the
backup encryption interface, the recovery artifact has the `.sqlite.gz.enc`
suffix and must be restored with the same 32-byte AES-256-GCM key.

## Clean-environment restore drill

Never restore over the database used by a running production application. The
`restore` command rejects the configured production database path. Stop the
production workload before any planned recovery, and restore its artifact into
a separate, clean environment first.

1. Create an empty directory and choose a new database path in it.
2. Set `APP_ENV=development`, set `DATABASE_PATH` to that new path, and set
   `BACKUP_ENCRYPTION_KEY_HEX` to the 64-character hexadecimal recovery key.
   Do not place the key in shell history, command arguments, logs, or tickets.
3. Create the encrypted backup, using the same key:

   ```sh
   cargo run --manifest-path backend/Cargo.toml -- backup /secure/backups
   ```

4. Restore it into the clean database:

   ```sh
   cargo run --manifest-path backend/Cargo.toml -- restore \
     /secure/backups/backup.sqlite.gz.enc /clean-room/commoncal.sqlite
   ```

5. Start the clean environment using the restored database and verify a
   representative record (for example, a known user, calendar, and event).
   The command decrypts, decompresses, and runs SQLite `PRAGMA integrity_check`
   before replacing the target database. A corrupt artifact leaves an existing
   destination untouched.

Keep the restore drill’s record identifiers and results in the operational
runbook; never record encryption keys or access tokens there.
