CREATE TABLE notification_preferences (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    calendar_id INTEGER REFERENCES calendars(id) ON DELETE CASCADE,
    event_id INTEGER REFERENCES events(id) ON DELETE CASCADE,
    reminder_minutes INTEGER NOT NULL CHECK (reminder_minutes >= 0 AND reminder_minutes <= 10080),
    timezone TEXT NOT NULL CHECK (length(trim(timezone)) > 0),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    updated_at INTEGER NOT NULL,
    CHECK ((calendar_id IS NULL AND event_id IS NULL) OR (calendar_id IS NOT NULL AND event_id IS NULL) OR (calendar_id IS NULL AND event_id IS NOT NULL))
);
CREATE UNIQUE INDEX notification_preferences_account_idx ON notification_preferences(user_id) WHERE calendar_id IS NULL AND event_id IS NULL;
CREATE UNIQUE INDEX notification_preferences_calendar_idx ON notification_preferences(user_id, calendar_id) WHERE calendar_id IS NOT NULL;
CREATE UNIQUE INDEX notification_preferences_event_idx ON notification_preferences(user_id, event_id) WHERE event_id IS NOT NULL;

CREATE TABLE event_reminder_overrides (
    event_id INTEGER NOT NULL REFERENCES events(id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    reminder_minutes INTEGER NOT NULL CHECK (reminder_minutes >= 0 AND reminder_minutes <= 10080),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (event_id, user_id)
);

CREATE TABLE notification_jobs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id),
    calendar_id INTEGER NOT NULL REFERENCES calendars(id),
    event_id INTEGER NOT NULL,
    occurrence_key TEXT NOT NULL,
    scheduled_at INTEGER NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending', 'claimed', 'delivered', 'cancelled', 'failed')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    delivered_at INTEGER,
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at INTEGER,
    last_error_code TEXT,
    claim_token TEXT,
    claim_expires_at INTEGER,
    UNIQUE (user_id, event_id, occurrence_key, scheduled_at)
);
CREATE INDEX notification_jobs_pending_idx ON notification_jobs(state, scheduled_at);
CREATE INDEX notification_jobs_claim_expiry_idx ON notification_jobs(state, claim_expires_at);
CREATE INDEX notification_jobs_event_user_idx ON notification_jobs(event_id, user_id, state);

CREATE TABLE in_app_notifications (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id),
    notification_job_id INTEGER NOT NULL UNIQUE REFERENCES notification_jobs(id),
    created_at INTEGER NOT NULL,
    read_at INTEGER
);
