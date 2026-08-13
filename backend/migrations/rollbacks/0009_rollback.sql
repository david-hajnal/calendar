-- Rollback for migration 0009: events
-- Reverses: CREATE TABLE events
-- Safe to run even if forward migration was not applied.

DROP TABLE IF EXISTS events;
