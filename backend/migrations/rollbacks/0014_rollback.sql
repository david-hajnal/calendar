-- Rollback for migration 0014: notifications
-- Reverses: CREATE TABLE notification_preferences, event_reminder_overrides, notification_jobs, in_app_notifications
-- Safe to run even if forward migration was not applied.

DROP TABLE IF EXISTS in_app_notifications;
DROP TABLE IF EXISTS notification_jobs;
DROP TABLE IF EXISTS event_reminder_overrides;
DROP TABLE IF EXISTS notification_preferences;
