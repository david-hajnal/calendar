-- Rollback for migration 0024: caldav_event_resources
-- Reverses: CREATE TABLE caldav_event_resources and its indexes
-- Safe to run even if forward migration was not applied.

DROP INDEX IF EXISTS idx_caldav_event_resources_calendar_id;
DROP INDEX IF EXISTS idx_caldav_resources_calendar_name_live;
DROP INDEX IF EXISTS idx_caldav_resources_calendar_uid_live;
DROP TABLE IF EXISTS caldav_event_resources;
