ALTER TABLE sessions ADD COLUMN last_seen_at INTEGER;

UPDATE sessions SET last_seen_at = created_at WHERE last_seen_at IS NULL;

CREATE INDEX sessions_user_revocation_idx ON sessions(user_id, revoked_at);
