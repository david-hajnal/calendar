-- Rollback for migration 0015: backup_metadata
-- Reverses: CREATE TABLE backup_metadata
-- Safe to run even if forward migration was not applied.

DROP TABLE IF EXISTS backup_metadata;
