-- Rollback for migration 0011: shared_views
-- Reverses: CREATE TABLE shared_views, shared_view_calendars
-- Safe to run even if forward migration was not applied.

DROP TABLE IF EXISTS shared_view_calendars;
DROP TABLE IF EXISTS shared_views;
