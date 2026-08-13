-- McpGrant: per-user per-OAuth-client permission grant
CREATE TABLE IF NOT EXISTS mcp_grant (
    grant_id TEXT PRIMARY KEY,
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

-- Delete intent: two-phase deletion
CREATE TABLE IF NOT EXISTS delete_intent (
    intent_id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL,
    oauth_client_id TEXT NOT NULL,
    event_id INTEGER NOT NULL,
    calendar_id INTEGER NOT NULL,
    event_version INTEGER NOT NULL,
    confirmation_state TEXT NOT NULL DEFAULT 'pending',
    expires_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);

-- Idempotency: replay protection
CREATE TABLE IF NOT EXISTS idempotency_key (
    operation_id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL,
    oauth_client_id TEXT NOT NULL,
    tool TEXT NOT NULL,
    arg_hash TEXT NOT NULL,
    result TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

-- Audit log: every MCP tool invocation
CREATE TABLE IF NOT EXISTS mcp_audit (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp INTEGER NOT NULL,
    request_id TEXT NOT NULL,
    user_id INTEGER NOT NULL,
    oauth_client_id TEXT NOT NULL,
    mcp_grant_id TEXT,
    tool TEXT NOT NULL,
    resource_ids TEXT,
    auth_result TEXT NOT NULL,
    scope TEXT,
    auth_strength TEXT,
    latency_ms INTEGER,
    result_type TEXT NOT NULL,
    operation_id TEXT
);
