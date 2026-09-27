-- Migration 0023: caldav_accounts
-- Creates CalDAV account principals and revocable connection credentials.

CREATE TABLE caldav_accounts (
    user_id INTEGER PRIMARY KEY REFERENCES users(id),
    principal_id TEXT NOT NULL UNIQUE CHECK (length(principal_id) > 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE caldav_credentials (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id),
    label TEXT NOT NULL CHECK (length(trim(label)) > 0),
    token_prefix TEXT NOT NULL CHECK (length(token_prefix) = 8),
    token_hash BLOB NOT NULL CHECK (length(token_hash) = 32),
    created_at INTEGER NOT NULL,
    last_used_at INTEGER,
    revoked_at INTEGER
);

CREATE INDEX idx_caldav_credentials_token_prefix
    ON caldav_credentials(token_prefix);

CREATE INDEX idx_caldav_credentials_user_id
    ON caldav_credentials(user_id);
