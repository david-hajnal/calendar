CREATE TABLE calendars (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    owner_user_id INTEGER NOT NULL REFERENCES users(id),
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    description TEXT,
    color TEXT NOT NULL CHECK (length(trim(color)) > 0),
    default_timezone TEXT NOT NULL CHECK (length(trim(default_timezone)) > 0),
    default_event_visibility TEXT NOT NULL
        CHECK (default_event_visibility IN ('default', 'public', 'private')),
    default_notification_rules_json TEXT,
    archived INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1)),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE calendar_acl (
    calendar_id INTEGER NOT NULL REFERENCES calendars(id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES users(id),
    role TEXT NOT NULL CHECK (
        role IN ('owner', 'manager', 'editor', 'viewer', 'free_busy_viewer')
    ),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (calendar_id, user_id)
);

CREATE UNIQUE INDEX calendar_acl_one_owner_idx
ON calendar_acl(calendar_id)
WHERE role = 'owner';

CREATE INDEX calendar_acl_user_idx ON calendar_acl(user_id);
