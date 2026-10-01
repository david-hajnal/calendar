-- Migration 0027: caldav_event_properties
-- T16: Safe client-property preservation.
--
-- Stores the allowlisted, client-owned metadata for a CalDAV event resource:
-- categories, URL, transparency, alarms, and selected X-properties. The
-- incoming PUT body is the source of truth: present properties are stored,
-- absent ones are removed (the row is replaced wholesale). The row is
-- cascade-deleted with the event so no orphaned metadata survives.
--
-- properties_json is a canonical JSON document; the serialized ICS (and thus
-- the ETag) is a deterministic function of it, so identical content always
-- yields the same ETag.

CREATE TABLE caldav_event_properties (
    event_id INTEGER PRIMARY KEY REFERENCES events(id) ON DELETE CASCADE,
    properties_json TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
