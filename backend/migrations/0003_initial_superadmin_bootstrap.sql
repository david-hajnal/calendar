DROP INDEX invitations_expiration_idx;
DROP INDEX invitations_revocation_idx;

ALTER TABLE invitations RENAME TO invitations_before_bootstrap;

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

INSERT INTO invitations (
    id, normalized_email, display_name, token_hash, expires_at, revoked_at,
    consumed_at, created_by_user_id, platform_role, created_at
)
SELECT
    id, normalized_email, display_name, token_hash, expires_at, revoked_at,
    consumed_at, created_by_user_id, 'user', created_at
FROM invitations_before_bootstrap;

DROP TABLE invitations_before_bootstrap;

CREATE INDEX invitations_expiration_idx ON invitations(expires_at);
CREATE INDEX invitations_revocation_idx ON invitations(revoked_at);

CREATE TABLE initial_superadmin_bootstrap (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    invitation_id INTEGER NOT NULL UNIQUE REFERENCES invitations(id),
    consumed_at INTEGER,
    created_at INTEGER NOT NULL
);
