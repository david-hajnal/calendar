-- Migration 0002: core_identity
-- Creates core tables: users, invitations, login_tokens, sessions, audit_log.
-- Idempotent: uses CREATE TABLE IF NOT EXISTS and CREATE INDEX IF NOT EXISTS.

CREATE TABLE IF NOT EXISTS users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    normalized_email TEXT NOT NULL UNIQUE COLLATE NOCASE,
    display_name TEXT,
    status TEXT NOT NULL CHECK (status IN ('invited', 'active', 'suspended', 'deleted')),
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS invitations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    normalized_email TEXT NOT NULL COLLATE NOCASE,
    display_name TEXT,
    token_hash BLOB NOT NULL UNIQUE CHECK (length(token_hash) > 0),
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER,
    consumed_at INTEGER,
    created_by_user_id INTEGER NOT NULL REFERENCES users(id),
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS invitations_expiration_idx ON invitations(expires_at);
CREATE INDEX IF NOT EXISTS invitations_revocation_idx ON invitations(revoked_at);

CREATE TABLE IF NOT EXISTS login_tokens (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id),
    token_hash BLOB NOT NULL UNIQUE CHECK (length(token_hash) > 0),
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER,
    consumed_at INTEGER,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS login_tokens_expiration_idx ON login_tokens(expires_at);
CREATE INDEX IF NOT EXISTS login_tokens_revocation_idx ON login_tokens(revoked_at);

CREATE TABLE IF NOT EXISTS sessions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id),
    session_hash BLOB NOT NULL UNIQUE CHECK (length(session_hash) > 0),
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS sessions_expiration_idx ON sessions(expires_at);
CREATE INDEX IF NOT EXISTS sessions_revocation_idx ON sessions(revoked_at);

CREATE TABLE IF NOT EXISTS audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    actor_user_id INTEGER REFERENCES users(id),
    action TEXT NOT NULL,
    target_type TEXT NOT NULL,
    target_id TEXT,
    metadata_json TEXT,
    created_at INTEGER NOT NULL
);

CREATE TRIGGER IF NOT EXISTS audit_log_prevent_update
BEFORE UPDATE ON audit_log
BEGIN
    SELECT RAISE(ABORT, 'audit_log entries are immutable');
END;

CREATE TRIGGER IF NOT EXISTS audit_log_prevent_delete
BEFORE DELETE ON audit_log
BEGIN
    SELECT RAISE(ABORT, 'audit_log entries are immutable');
END;
