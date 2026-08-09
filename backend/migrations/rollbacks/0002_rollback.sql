-- Rollback for migration 0002: core_identity
-- Reverses: CREATE TABLE users, invitations, login_tokens, sessions, audit_log
-- Safe to run even if forward migration was not applied.
-- WARNING: This drops all core tables. Must be applied as part of a full rollback chain.

DROP TABLE IF EXISTS audit_log;
DROP TABLE IF EXISTS sessions;
DROP TABLE IF EXISTS login_tokens;
DROP TABLE IF EXISTS invitations;
DROP TABLE IF EXISTS users;
