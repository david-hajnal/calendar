-- Run with foreign keys disabled on the dedicated migration connection.
-- Preserve stable user IDs and every dependent table while updating status names.
CREATE TABLE users_account_management (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    normalized_email TEXT NOT NULL UNIQUE COLLATE NOCASE,
    display_name TEXT,
    status TEXT NOT NULL CHECK(status IN ('invited', 'registered', 'pending', 'inactive', 'deleted')),
    created_at INTEGER NOT NULL,
    is_superadmin INTEGER NOT NULL DEFAULT 0 CHECK(is_superadmin IN (0, 1)),
    last_login_at INTEGER,
    password_hash TEXT
);
INSERT INTO users_account_management
SELECT id, normalized_email, display_name,
       CASE status WHEN 'active' THEN 'registered' WHEN 'suspended' THEN 'inactive' ELSE status END,
       created_at, is_superadmin, last_login_at, password_hash
FROM users;
UPDATE sqlite_sequence SET seq = MAX(seq, COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'users'), 0))
WHERE name = 'users_account_management';
DROP TABLE users;
ALTER TABLE users_account_management RENAME TO users;

INSERT INTO users(normalized_email, display_name, status, created_at)
SELECT normalized_email, display_name, 'invited', created_at FROM invitations i
WHERE revoked_at IS NULL AND consumed_at IS NULL
  AND NOT EXISTS(SELECT 1 FROM users u WHERE u.normalized_email = i.normalized_email);

CREATE TABLE password_reset_tokens (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id),
    token_hash BLOB NOT NULL UNIQUE,
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER,
    consumed_at INTEGER,
    created_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX password_reset_one_pending_idx ON password_reset_tokens(user_id)
WHERE revoked_at IS NULL AND consumed_at IS NULL;
CREATE TABLE email_change_requests (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id),
    normalized_new_email TEXT NOT NULL COLLATE NOCASE,
    token_hash BLOB NOT NULL UNIQUE,
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER,
    consumed_at INTEGER,
    actor_user_id INTEGER NOT NULL REFERENCES users(id),
    created_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX email_change_one_pending_user_idx ON email_change_requests(user_id)
WHERE revoked_at IS NULL AND consumed_at IS NULL;
CREATE UNIQUE INDEX email_change_one_pending_email_idx ON email_change_requests(normalized_new_email)
WHERE revoked_at IS NULL AND consumed_at IS NULL;

-- Abort this migration before commit if any dependency was damaged.
CREATE TEMP TABLE account_migration_integrity(valid INTEGER NOT NULL CHECK(valid = 1));
INSERT INTO account_migration_integrity
SELECT CASE WHEN EXISTS(SELECT 1 FROM pragma_foreign_key_check) THEN 0 ELSE 1 END;
DROP TABLE account_migration_integrity;
