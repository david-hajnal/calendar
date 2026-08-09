-- Rollback for migration 0016: backup_encryption_upload
-- Reverses: ALTER TABLE backup_metadata ADD COLUMN encryption_algorithm/encrypted_sha256/encrypted_bytes/upload_status
-- Safe to run even if forward migration was not applied.

PRAGMA foreign_keys = OFF;
ALTER TABLE backup_metadata DROP COLUMN IF EXISTS upload_status;
ALTER TABLE backup_metadata DROP COLUMN IF EXISTS encrypted_bytes;
ALTER TABLE backup_metadata DROP COLUMN IF EXISTS encrypted_sha256;
ALTER TABLE backup_metadata DROP COLUMN IF EXISTS encryption_algorithm;
PRAGMA foreign_keys = ON;
