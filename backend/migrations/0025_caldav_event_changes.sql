-- Migration 0025: caldav_event_changes
-- Transactional change log for CalDAV incremental sync.
--
-- Every committed base event mutation (create/update/delete) records exactly
-- one row here, inside the same transaction as the domain write. A rolled-back
-- mutation leaves no row. The autoincrement id is a monotonically increasing
-- revision that sync tokens (T11) page over in order.
--
-- event_id carries no foreign key: a 'deleted' change references an event that
-- no longer exists, and the durable DAV mapping (caldav_event_resources)
-- retains the resource identity for tombstones.

CREATE TABLE caldav_event_changes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    calendar_id INTEGER NOT NULL REFERENCES calendars(id),
    event_id INTEGER,
    change_type TEXT NOT NULL CHECK (change_type IN ('created', 'updated', 'deleted')),
    created_at INTEGER NOT NULL
);

CREATE INDEX idx_caldav_event_changes_calendar_id
    ON caldav_event_changes(calendar_id, id);
