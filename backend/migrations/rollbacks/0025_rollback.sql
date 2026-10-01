-- Rollback for migration 0025: caldav_event_changes
-- Reverses: CREATE TABLE caldav_event_changes and its index
-- Safe to run even if forward migration was not applied.

DROP INDEX IF EXISTS idx_caldav_event_changes_calendar_id;
DROP TABLE IF EXISTS caldav_event_changes;
