CREATE TABLE external_feeds (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    calendar_id INTEGER NOT NULL REFERENCES calendars(id) ON DELETE CASCADE,
    source_url_encrypted BLOB NOT NULL,
    source_url_display TEXT NOT NULL,
    refresh_interval_seconds INTEGER NOT NULL CHECK (refresh_interval_seconds >= 60),
    etag TEXT,
    last_modified TEXT,
    last_attempt_at INTEGER,
    last_success_at INTEGER,
    next_refresh_at INTEGER NOT NULL,
    last_error_code TEXT,
    consecutive_failures INTEGER NOT NULL DEFAULT 0 CHECK (consecutive_failures >= 0),
    disabled_at INTEGER,
    created_by_user_id INTEGER NOT NULL REFERENCES users(id),
    created_at INTEGER NOT NULL
);

CREATE INDEX external_feeds_calendar_id_idx ON external_feeds(calendar_id);
CREATE INDEX external_feeds_next_refresh_idx ON external_feeds(next_refresh_at) WHERE disabled_at IS NULL;

CREATE TABLE external_event_mapping (
    feed_id INTEGER NOT NULL REFERENCES external_feeds(id) ON DELETE CASCADE,
    external_uid TEXT NOT NULL,
    recurrence_id TEXT NOT NULL DEFAULT '',
    event_id INTEGER NOT NULL REFERENCES events(id) ON DELETE CASCADE,
    external_sequence INTEGER,
    external_modified_at INTEGER,
    content_hash BLOB,
    last_seen_sync_id INTEGER NOT NULL,
    PRIMARY KEY (feed_id, external_uid, recurrence_id),
    UNIQUE (event_id)
);
