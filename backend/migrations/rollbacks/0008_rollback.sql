-- Rollback for migration 0008: calendars
-- Reverses: CREATE TABLE calendars, calendar_acl
-- Safe to run even if forward migration was not applied.

DROP TABLE IF EXISTS calendar_acl;
DROP TABLE IF EXISTS calendars;
