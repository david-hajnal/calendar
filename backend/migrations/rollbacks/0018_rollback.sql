-- Rollback for migration 0018: add_user_preferences
-- Reverses: CREATE TABLE user_preferences
-- Safe to run even if forward migration was not applied.

DROP TABLE IF EXISTS user_preferences;
