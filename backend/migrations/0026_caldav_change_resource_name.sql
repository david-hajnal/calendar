-- Migration 0026: add resource_name to caldav_event_changes
-- T11: sync-collection needs the resource name to emit the correct href for
-- deleted changes. The tombstone clears event_id (FK to events), so the
-- change log must carry the resource name directly for deleted entries.
-- The column is nullable: created/updated changes resolve the name from the
-- live mapping; deleted changes carry it here.

ALTER TABLE caldav_event_changes ADD COLUMN resource_name TEXT;
