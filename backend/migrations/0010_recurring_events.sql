ALTER TABLE events ADD COLUMN recurrence_rule TEXT;

CREATE TABLE event_recurrence_exceptions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    series_id INTEGER NOT NULL REFERENCES events(id) ON DELETE CASCADE,
    recurrence_id INTEGER,
    recurrence_date TEXT,
    is_deleted INTEGER NOT NULL DEFAULT 0 CHECK (is_deleted IN (0, 1)),
    title TEXT,
    description TEXT,
    location TEXT,
    status TEXT CHECK (status IN ('tentative', 'confirmed', 'cancelled')),
    timed_start_utc INTEGER,
    timed_end_utc INTEGER,
    event_timezone TEXT,
    all_day_start_date TEXT,
    all_day_end_date TEXT,
    last_edited_by_user_id INTEGER NOT NULL REFERENCES users(id),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK (
        (recurrence_id IS NOT NULL AND recurrence_date IS NULL)
        OR
        (recurrence_id IS NULL
            AND recurrence_date IS NOT NULL
            AND length(recurrence_date) = 10)
    ),
    CHECK (
        (is_deleted = 1
            AND title IS NULL
            AND status IS NULL
            AND timed_start_utc IS NULL
            AND timed_end_utc IS NULL
            AND event_timezone IS NULL
            AND all_day_start_date IS NULL
            AND all_day_end_date IS NULL)
        OR
        (is_deleted = 0
            AND title IS NOT NULL
            AND length(trim(title)) > 0
            AND status IS NOT NULL
            AND (
                (timed_start_utc IS NOT NULL
                    AND timed_end_utc IS NOT NULL
                    AND timed_start_utc < timed_end_utc
                    AND event_timezone IS NOT NULL
                    AND length(trim(event_timezone)) > 0
                    AND all_day_start_date IS NULL
                    AND all_day_end_date IS NULL)
                OR
                (timed_start_utc IS NULL
                    AND timed_end_utc IS NULL
                    AND event_timezone IS NULL
                    AND all_day_start_date IS NOT NULL
                    AND all_day_end_date IS NOT NULL
                    AND all_day_start_date < all_day_end_date)
            ))
    )
);

CREATE UNIQUE INDEX event_recurrence_exceptions_timed_idx
ON event_recurrence_exceptions(series_id, recurrence_id);

CREATE UNIQUE INDEX event_recurrence_exceptions_all_day_idx
ON event_recurrence_exceptions(series_id, recurrence_date);
