-- Rollback for migration 0004: invitation_activation (is_superadmin)
-- Reverses: ALTER TABLE users ADD COLUMN is_superadmin
-- Safe to run even if forward migration was not applied.

PRAGMA foreign_keys = OFF;
ALTER TABLE users DROP COLUMN IF EXISTS is_superadmin;
PRAGMA foreign_keys = ON;
