-- Rollback for migration 0013: external_feeds
-- Reverses: CREATE TABLE external_feeds, external_event_mapping
-- Safe to run even if forward migration was not applied.

DROP TABLE IF EXISTS external_event_mapping;
DROP TABLE IF EXISTS external_feeds;
