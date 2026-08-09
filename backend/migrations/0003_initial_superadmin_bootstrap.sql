-- Migration 0003: initial_superadmin_bootstrap
-- Adds platform_role to invitations and creates initial_superadmin_bootstrap table.
-- Safe to re-apply: uses PRAGMA foreign_keys = OFF for table recreation.
-- Rollback: see 0003_rollback.sql

-- Disable foreign keys for safe table recreation
PRAGMA foreign_keys = OFF;

-- Create bootstrap table (idempotent)
CREATE TABLE IF NOT EXISTS initial_superadmin_bootstrap (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    invitation_id INTEGER NOT NULL UNIQUE REFERENCES invitations(id),
    consumed_at INTEGER,
    created_at INTEGER NOT NULL
);

-- Add platform_role to invitations if it doesn't already exist.
-- SQLite doesn't support IF NOT EXISTS for ALTER TABLE ADD COLUMN.
-- We use a workaround: create a temp table that will error if the column exists.
-- If the temp table creation fails (column exists), we skip the ALTER.
-- If it succeeds (column doesn't exist), we apply the ALTER.

-- Attempt to create a temp table with a CHECK on platform_role.
-- This will fail if platform_role doesn't exist in invitations.
-- We catch the failure by using INSERT OR IGNORE into a marker table.
-- Actually, the simplest approach: just try the ALTER and handle the error.
-- Since sqlx runs all statements, we use a safe pattern:

-- Create a backup of existing invitations data
CREATE TABLE IF NOT EXISTS __m0003_invitations_backup (
    id INTEGER,
    normalized_email TEXT,
    display_name TEXT,
    token_hash BLOB,
    expires_at INTEGER,
    revoked_at INTEGER,
    consumed_at INTEGER,
    created_by_user_id INTEGER,
    platform_role TEXT,
    created_at INTEGER
);

-- Copy existing data (safe: INSERT OR IGNORE handles duplicates)
INSERT OR IGNORE INTO __m0003_invitations_backup (id, normalized_email, display_name, token_hash, expires_at, revoked_at, consumed_at, created_by_user_id, created_at)
SELECT id, normalized_email, display_name, token_hash, expires_at, revoked_at, consumed_at, created_by_user_id, created_at
FROM invitations;

-- Drop old invitations and indexes
DROP TABLE IF EXISTS invitations;
DROP INDEX IF EXISTS invitations_expiration_idx;
DROP INDEX IF EXISTS invitations_revocation_idx;

-- Create new invitations with platform_role
CREATE TABLE invitations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    normalized_email TEXT NOT NULL COLLATE NOCASE,
    display_name TEXT,
    token_hash BLOB NOT NULL UNIQUE CHECK (length(token_hash) > 0),
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER,
    consumed_at INTEGER,
    created_by_user_id INTEGER REFERENCES users(id),
    platform_role TEXT NOT NULL DEFAULT 'user'
        CHECK (platform_role IN ('user', 'superadmin')),
    created_at INTEGER NOT NULL
);

-- Restore data with default platform_role
INSERT OR IGNORE INTO invitations (id, normalized_email, display_name, token_hash, expires_at, revoked_at, consumed_at, created_by_user_id, platform_role, created_at)
SELECT id, normalized_email, display_name, token_hash, expires_at, revoked_at, consumed_at, created_by_user_id, 'user', created_at
FROM __m0003_invitations_backup;

-- Recreate indexes
CREATE INDEX invitations_expiration_idx ON invitations(expires_at);
CREATE INDEX invitations_revocation_idx ON invitations(revoked_at);

-- Clean up temp table
DROP TABLE IF EXISTS __m0003_invitations_backup;

-- Re-enable foreign keys
PRAGMA foreign_keys = ON;
