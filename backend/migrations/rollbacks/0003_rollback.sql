-- Rollback for migration 0003: initial_superadmin_bootstrap
-- Reverses: adds platform_role to invitations, creates initial_superadmin_bootstrap
-- Safe to run even if forward migration was not applied.
-- IMPORTANT: This rollback drops and recreates the invitations table.
-- It must be applied in reverse migration order (after all later rollbacks).

PRAGMA foreign_keys = OFF;

-- Drop bootstrap table
DROP TABLE IF EXISTS initial_superadmin_bootstrap;

-- Backup current invitations (with platform_role)
CREATE TABLE IF NOT EXISTS __m0003_restore (
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

-- Copy data (only if invitations has platform_role)
INSERT OR IGNORE INTO __m0003_restore (id, normalized_email, display_name, token_hash, expires_at, revoked_at, consumed_at, created_by_user_id, platform_role, created_at)
SELECT id, normalized_email, display_name, token_hash, expires_at, revoked_at, consumed_at, created_by_user_id, platform_role, created_at
FROM invitations;

-- Drop invitations and indexes
DROP TABLE IF EXISTS invitations;
DROP INDEX IF EXISTS invitations_expiration_idx;
DROP INDEX IF EXISTS invitations_revocation_idx;
DROP INDEX IF EXISTS invitations_one_pending_per_email_idx;

-- Recreate invitations without platform_role
CREATE TABLE invitations (
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

-- Restore data
INSERT OR IGNORE INTO invitations (id, normalized_email, display_name, token_hash, expires_at, revoked_at, consumed_at, created_by_user_id, created_at)
SELECT id, normalized_email, display_name, token_hash, expires_at, revoked_at, consumed_at, created_by_user_id, created_at
FROM __m0003_restore;

-- Recreate indexes
CREATE INDEX invitations_expiration_idx ON invitations(expires_at);
CREATE INDEX invitations_revocation_idx ON invitations(revoked_at);

-- Clean up
DROP TABLE IF EXISTS __m0003_restore;

PRAGMA foreign_keys = ON;
