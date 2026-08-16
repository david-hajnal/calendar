// Audit logging module.
//
// Records every MCP tool invocation to the mcp_audit table.
// Never logs credentials, tokens, or sensitive data.

use sqlx::SqlitePool;

/// Log a tool invocation to the audit table.
pub async fn log_invocation(
    pool: &SqlitePool,
    user_id: i64,
    client_id: String,
    grant_id: Option<String>,
    tool: &str,
    resource_ids: Option<String>,
    auth_result: &str,
    scope: Option<String>,
    auth_strength: &str,
    latency_ms: i64,
    result_type: &str,
    operation_id: Option<String>,
) -> Result<(), AuditError> {
    let now = chrono::Utc::now().to_rfc3339();

    sqlx::query(
        "INSERT INTO mcp_audit (timestamp, request_id, user_id, oauth_client_id,
         mcp_grant_id, tool, resource_ids, auth_result, scope, auth_strength,
         latency_ms, result_type, operation_id)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(now)
    .bind(
        operation_id
            .as_ref()
            .map(|s| s.as_str())
            .unwrap_or_default(),
    )
    .bind(user_id)
    .bind(client_id)
    .bind(grant_id)
    .bind(tool)
    .bind(resource_ids)
    .bind(auth_result)
    .bind(scope)
    .bind(auth_strength)
    .bind(latency_ms)
    .bind(result_type)
    .bind(operation_id.as_ref().map(|s| s.clone()))
    .execute(pool)
    .await?;

    Ok(())
}

/// Log a deletion operation.
pub async fn log_deletion(
    pool: &SqlitePool,
    user_id: i64,
    client_id: String,
    event_id: i64,
    _event_version: i64,
    confirmation_method: &str,
) -> Result<(), AuditError> {
    let now = chrono::Utc::now().to_rfc3339();

    sqlx::query(
        "INSERT INTO mcp_audit (timestamp, request_id, user_id, oauth_client_id,
         mcp_grant_id, tool, resource_ids, auth_result, scope, auth_strength,
         latency_ms, result_type, operation_id)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(now)
    .bind("")
    .bind(user_id)
    .bind(client_id)
    .bind(None::<String>)
    .bind("deletion")
    .bind(format!("event:{}", event_id))
    .bind(confirmation_method)
    .bind(None::<String>)
    .bind("strong")
    .bind(0)
    .bind("deletion")
    .bind(None::<String>)
    .execute(pool)
    .await?;

    Ok(())
}

#[derive(Debug)]
pub struct AuditError {
    pub message: String,
}

impl std::fmt::Display for AuditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "audit error: {}", self.message)
    }
}

impl std::error::Error for AuditError {}

impl From<sqlx::Error> for AuditError {
    fn from(error: sqlx::Error) -> Self {
        Self {
            message: error.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_error_display() {
        let err = AuditError {
            message: "test error".to_string(),
        };
        assert_eq!(format!("{}", err), "audit error: test error");
    }

    #[test]
    fn audit_error_is_error() {
        let err: AuditError = AuditError {
            message: "test".to_string(),
        };
        let _: &dyn std::error::Error = &err;
    }

    #[test]
    fn audit_error_from_sqlx_error() {
        let sqlx_err = sqlx::Error::PoolClosed;
        let audit_err: AuditError = sqlx_err.into();
        assert!(!audit_err.message.is_empty());
    }
}
