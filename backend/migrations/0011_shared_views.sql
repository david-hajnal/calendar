-- Migration 0011: shared_views
-- Creates shared_views and shared_view_calendars tables.
-- Idempotent: uses CREATE TABLE IF NOT EXISTS and CREATE INDEX IF NOT EXISTS.

CREATE TABLE IF NOT EXISTS shared_views (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    owner_user_id INTEGER NOT NULL REFERENCES users(id),
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    version INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS shared_views_owner_idx ON shared_views(owner_user_id);

CREATE TABLE IF NOT EXISTS shared_view_calendars (
    view_id INTEGER NOT NULL REFERENCES shared_views(id) ON DELETE CASCADE,
    calendar_id INTEGER NOT NULL REFERENCES calendars(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK (position >= 0),
    color TEXT NOT NULL CHECK (length(trim(color)) > 0),
    PRIMARY KEY (view_id, calendar_id),
    UNIQUE (view_id, position)
);
