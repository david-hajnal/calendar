-- Rollback for migration 0006: session_last_seen
-- Reverses: ALTER TABLE sessions ADD COLUMN last_seen_at, CREATE INDEX sessions_user_revocation_idx
-- Safe to run even if forward migration was not applied.

PRAGMA foreign_keys = OFF;
DROP INDEX IF EXISTS sessions_user_revocation_idx;
ALTER TABLE sessions DROP COLUMN IF EXISTS last_seen_at;
PRAGMA foreign_keys = ON;
