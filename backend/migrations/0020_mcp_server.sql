-- MCP Server tables.

CREATE TABLE IF NOT EXISTS mcp_grant (
    id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL,
    oauth_client_id TEXT NOT NULL,
    allowed_calendar_ids TEXT NOT NULL DEFAULT '[]',
    allow_availability INTEGER NOT NULL DEFAULT 0,
    allow_event_titles INTEGER NOT NULL DEFAULT 0,
    allow_event_details INTEGER NOT NULL DEFAULT 0,
    allow_create INTEGER NOT NULL DEFAULT 0,
    allow_update INTEGER NOT NULL DEFAULT 0,
    allow_delete INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER,
    expires_at INTEGER,
    revoked_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_mcp_grant_user_client ON mcp_grant(user_id, oauth_client_id);

CREATE TABLE IF NOT EXISTS delete_intent (
    id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL,
    oauth_client_id TEXT NOT NULL,
    event_id INTEGER NOT NULL,
    calendar_id INTEGER NOT NULL,
    event_version INTEGER NOT NULL,
    confirmation_state TEXT NOT NULL DEFAULT 'pending',
    expires_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_delete_intent_id ON delete_intent(id);
CREATE INDEX IF NOT EXISTS idx_delete_intent_expires ON delete_intent(expires_at);

CREATE TABLE IF NOT EXISTS idempotency_key (
    key TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL,
    response_status INTEGER NOT NULL,
    response_headers TEXT NOT NULL,
    response_body TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_idempotency_expires ON idempotency_key(created_at);

CREATE TABLE IF NOT EXISTS mcp_audit (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp TEXT NOT NULL,
    request_id TEXT NOT NULL DEFAULT '',
    user_id INTEGER NOT NULL,
    oauth_client_id TEXT NOT NULL,
    mcp_grant_id TEXT,
    tool TEXT NOT NULL,
    resource_ids TEXT,
    auth_result TEXT NOT NULL,
    scope TEXT,
    auth_strength TEXT NOT NULL,
    latency_ms INTEGER NOT NULL,
    result_type TEXT NOT NULL,
    operation_id TEXT
);

CREATE INDEX IF NOT EXISTS idx_mcp_audit_user ON mcp_audit(user_id, timestamp);
CREATE INDEX IF NOT EXISTS idx_mcp_audit_tool ON mcp_audit(tool, timestamp);
