-- Rollback for migration 0010: recurring_events
-- Reverses: ALTER TABLE events ADD COLUMN recurrence_rule, CREATE TABLE event_recurrence_exceptions
-- Safe to run even if forward migration was not applied.

PRAGMA foreign_keys = OFF;
DROP TABLE IF EXISTS event_recurrence_exceptions;
ALTER TABLE events DROP COLUMN IF EXISTS recurrence_rule;
PRAGMA foreign_keys = ON;
