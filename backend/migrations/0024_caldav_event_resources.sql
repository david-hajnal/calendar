-- Migration 0024: caldav_event_resources
-- Stable mapping between Happening events and CalDAV event resources.
--
-- The mapping survives event deletion as a durable tombstone: when an event is
-- removed, the row is retained with `event_id` set to NULL and `deleted_at`
-- stamped, so other clients can observe the deletion through sync.

CREATE TABLE caldav_event_resources (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id INTEGER UNIQUE REFERENCES events(id),
    calendar_id INTEGER NOT NULL REFERENCES calendars(id),
    uid TEXT NOT NULL CHECK (length(trim(uid)) > 0),
    resource_name TEXT NOT NULL CHECK (length(trim(resource_name)) > 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    deleted_at INTEGER
);

-- Live resources keep unique (calendar, uid) and (calendar, resource_name)
-- identity. Tombstones (deleted_at IS NOT NULL) are excluded so a deleted
-- resource's name and uid can be reused by a later create.
CREATE UNIQUE INDEX idx_caldav_resources_calendar_uid_live
    ON caldav_event_resources(calendar_id, uid) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX idx_caldav_resources_calendar_name_live
    ON caldav_event_resources(calendar_id, resource_name) WHERE deleted_at IS NULL;

CREATE INDEX idx_caldav_event_resources_calendar_id
    ON caldav_event_resources(calendar_id);
