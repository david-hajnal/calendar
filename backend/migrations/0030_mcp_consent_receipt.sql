-- Durable retry identity; retain receipts so an old approval cannot recreate a
-- revoked grant. Store only a digest of the bearer handoff token.
CREATE TABLE mcp_consent_receipt (
    handoff_hash TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id),
    decision TEXT NOT NULL CHECK (decision IN ('approve', 'deny')),
    grant_id TEXT REFERENCES mcp_grant(id),
    resume_url TEXT,
    expires_at INTEGER NOT NULL
);
