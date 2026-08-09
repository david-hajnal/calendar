-- Rollback for migration 0005: passwordless_login
-- Reverses: ALTER TABLE users ADD COLUMN last_login_at
-- Safe to run even if forward migration was not applied.

PRAGMA foreign_keys = OFF;
ALTER TABLE users DROP COLUMN IF EXISTS last_login_at;
PRAGMA foreign_keys = ON;
