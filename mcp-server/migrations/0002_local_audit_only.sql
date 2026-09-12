-- Forward migration: MCP-local state is audit-only.
--
-- mcp_grant, delete_intent, and idempotency_key duplicate authoritative
-- CommonCal core state. The pre-migration guard in db.rs aborts startup if
-- any of these tables still hold rows, so dropping them here cannot discard
-- unreconciled data. mcp_audit is MCP-owned and stays.
DROP TABLE IF EXISTS mcp_grant;
DROP TABLE IF EXISTS delete_intent;
DROP TABLE IF EXISTS idempotency_key;
