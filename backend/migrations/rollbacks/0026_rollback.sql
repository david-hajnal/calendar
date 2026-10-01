-- Rollback for migration 0026: add resource_name to caldav_event_changes
-- Reverses: ALTER TABLE caldav_event_changes ADD COLUMN resource_name TEXT
-- Requires SQLite 3.35.0+ for DROP COLUMN. Safe to run even if the forward
-- migration was not applied (the column simply will not exist).

ALTER TABLE caldav_event_changes DROP COLUMN resource_name;
