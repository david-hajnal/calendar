-- Rollback for migration 0017: password_login
-- Reverses: ALTER TABLE users ADD COLUMN password_hash TEXT
-- Safe to run even if forward migration was not applied.

PRAGMA foreign_keys = OFF;
ALTER TABLE users DROP COLUMN IF EXISTS password_hash;
PRAGMA foreign_keys = ON;
