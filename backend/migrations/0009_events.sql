CREATE TABLE events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    calendar_id INTEGER NOT NULL REFERENCES calendars(id) ON DELETE CASCADE,
    title TEXT NOT NULL CHECK (length(trim(title)) > 0),
    description TEXT,
    location TEXT,
    status TEXT NOT NULL CHECK (status IN ('tentative', 'confirmed', 'cancelled')),
    event_kind TEXT NOT NULL CHECK (event_kind IN ('timed', 'all_day')),
    timed_start_utc INTEGER,
    timed_end_utc INTEGER,
    event_timezone TEXT,
    all_day_start_date TEXT,
    all_day_end_date TEXT,
    created_by_user_id INTEGER NOT NULL REFERENCES users(id),
    last_edited_by_user_id INTEGER NOT NULL REFERENCES users(id),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK (
        (
            event_kind = 'timed'
            AND timed_start_utc IS NOT NULL
            AND timed_end_utc IS NOT NULL
            AND timed_start_utc < timed_end_utc
            AND event_timezone IS NOT NULL
            AND length(trim(event_timezone)) > 0
            AND all_day_start_date IS NULL
            AND all_day_end_date IS NULL
        )
        OR
        (
            event_kind = 'all_day'
            AND timed_start_utc IS NULL
            AND timed_end_utc IS NULL
            AND event_timezone IS NULL
            AND all_day_start_date IS NOT NULL
            AND all_day_end_date IS NOT NULL
            AND all_day_start_date < all_day_end_date
        )
    )
);

CREATE INDEX events_calendar_timed_range_idx
ON events(calendar_id, timed_start_utc, timed_end_utc)
WHERE event_kind = 'timed';

CREATE INDEX events_calendar_all_day_range_idx
ON events(calendar_id, all_day_start_date, all_day_end_date)
WHERE event_kind = 'all_day';
