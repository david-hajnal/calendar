# Architecture: MCP Server

## Fit
- **New service**: `mcp-server` (Rust, separate from `commoncal-backend`)
- **Existing backend** (`commoncal-backend`): authority for calendar ACL, event state, user status, McpGrant persistence
- **No changes to existing database** — new tables live in the MCP service's own SQLite instance (in-memory for v1, file-backed for production)
- **MCP service does not access CommonCal's SQLite** — communicates exclusively via internal HTTP API

## Endpoints

### MCP service (new — `mcp-server`)
| Route | Verb | Purpose |
|-------|------|---------|
| `/mcp` | POST | MCP Streamable HTTP transport |
| `/.well-known/oauth-protected-resource` | GET | OAuth Protected Resource Metadata (RFC 9749) |

### Internal API on existing backend (new — `/internal/mcp/*`)
| Route | Verb | Purpose |
|-------|------|---------|
| `/internal/mcp/users/:user_id/status` | GET | Check user is active |
| `/internal/mcp/users/:user_id/calendars` | GET | List user's calendars + their ACL role |
| `/internal/mcp/calendars/:calendar_id/role/:user_id` | GET | Get user's ACL role on a specific calendar |
| `/internal/mcp/events/:calendar_id/:event_id` | GET | Get event by ID (with access level control) |
| `/internal/mcp/events/:calendar_id/search` | GET | Search events in time range |
| `/internal/mcp/events/:calendar_id` | POST | Create event |
| `/internal/mcp/events/:calendar_id/:event_id` | PATCH | Update event |
| `/internal/mcp/delete-intents` | POST | Create deletion intent |
| `/internal/mcp/delete-intents/:id` | GET | Get deletion intent |
| `/internal/mcp/delete-intents/:id/commit` | POST | Commit deletion |
| `/internal/mcp/mcp-grants` | GET | Get all McpGrants for a user+client |
| `/internal/mcp/mcp-grants` | POST | Create/update McpGrant |
| `/internal/mcp/mcp-grants/revoke` | POST | Revoke McpGrant |
| `/internal/mcp/idempotency` | POST | Record idempotency key |

### Frontend changes (existing backend)
| Route | Verb | Purpose |
|-------|------|---------|
| `/api/v1/settings/mcp-grants` | GET | List user's connected MCP clients |
| `/api/v1/settings/mcp-grants/:grant_id` | PATCH | Edit McpGrant permissions |
| `/api/v1/settings/mcp-grants/:grant_id` | DELETE | Revoke McpGrant |
| `/api/v1/settings/mcp-grants` | POST | Authorize new OAuth connection flow |
| `/mcp/confirm/:intent_id` | GET | Deletion confirmation page |

## Data

### MCP service SQLite (new database)
```sql
-- McpGrant: per-user per-OAuth-client permission grant
CREATE TABLE mcp_grant (
    grant_id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL,
    oauth_client_id TEXT NOT NULL,
    allowed_calendar_ids TEXT NOT NULL,  -- JSON array of calendar IDs
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
CREATE TABLE delete_intent (
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
CREATE TABLE idempotency_key (
    operation_id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL,
    oauth_client_id TEXT NOT NULL,
    tool TEXT NOT NULL,
    arg_hash TEXT NOT NULL,
    result TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

-- Audit log: every MCP tool invocation
CREATE TABLE mcp_audit (
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
```

### Backend database changes (new migration)
```sql
-- McpGrant stored in CommonCal DB as source of truth
CREATE TABLE mcp_grant (
    grant_id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id),
    oauth_client_id TEXT NOT NULL,
    allowed_calendar_ids TEXT NOT NULL,
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

-- Index for quick lookup by user + client
CREATE INDEX idx_mcp_grant_user_client ON mcp_grant(user_id, oauth_client_id);
```

## Flow

```
MCP client (Claude Desktop)
    │
    │ OAuth 2.1 + PKCE + DPoP
    ▼
Authorization Server (IdP)
    │
    │ access_token (aud=mcp.commoncal.tld, DPoP bound)
    ▼
MCP Server (gateway)
    │ 1. validate DPoP proof
    │ 2. validate access token (signature, issuer, audience, expiry)
    │ 3. load McpGrant from CommonCal DB
    │ 4. check tool scope against McpGrant
    │ 5. classify risk tier
    │ 6. check rate limit
    │ 7. check idempotency key (mutations)
    │ 8. check authentication strength / step-up (Tier 3)
    ▼
Internal API call (mTLS + short-lived token)
    │ aud=commoncal-internal-api, lifetime=30-60s
    ▼
CommonCal Backend
    │ 1. validate delegated token
    │ 2. load user + ACL from SQLite
    │ 3. authorize action against live ACL
    │ 4. execute transaction
    ▼
SQLite (CommonCal)
```

## External

| Name | Purpose |
|------|---------|
| `MCP_OAUTH_ISSUER` | Authorization server URL for token validation |
| `MCP_INTERNAL_API_BASE` | Base URL of CommonCal internal API |
| `MCP_INTERNAL_API_KEY` | mTLS client certificate / key for internal auth |
| `MCP_SESSION_SECRET` | Session signing key for MCP service |
| `MCP_DATABASE_PATH` | Path to MCP service SQLite |
| `MCP_RATE_LIMIT_ENABLED` | Enable rate limiting (production only) |
| `KAFKA_MCP_AUDIT_TOPIC` | Kafka topic for audit log export (optional) |
| `DPOP_KEY_PATH` | Path to DPoP key pair PEM file |
| `APP_ENV` | `development` or `production` (inherited from commoncal) |
| `BIND_ADDRESS` | Address to bind on (inherited from commoncal) |
| `TRACING_LEVEL` | Log level for MCP service |

No third-party APIs. No webhooks.
