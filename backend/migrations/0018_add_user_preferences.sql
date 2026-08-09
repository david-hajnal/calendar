-- Migration: add_user_preferences
-- Created: 2026-08-08
-- Adds user_preferences table for per-user settings.

CREATE TABLE IF NOT EXISTS user_preferences (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
    default_timezone TEXT NOT NULL DEFAULT 'UTC' CHECK (length(trim(default_timezone)) > 0),
    first_day_of_week INTEGER NOT NULL DEFAULT 1 CHECK (first_day_of_week BETWEEN 0 AND 6),
    theme TEXT NOT NULL DEFAULT 'light' CHECK (theme IN ('light', 'dark', 'system')),
    event_view_default TEXT NOT NULL DEFAULT 'week' CHECK (event_view_default IN ('day', 'week', 'month', 'composite')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
