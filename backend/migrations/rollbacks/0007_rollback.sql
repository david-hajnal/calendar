-- Rollback for migration 0007: superadmin_user_management
-- Reverses: CREATE UNIQUE INDEX invitations_one_pending_per_email_idx
-- Safe to run even if forward migration was not applied.

DROP INDEX IF EXISTS invitations_one_pending_per_email_idx;
