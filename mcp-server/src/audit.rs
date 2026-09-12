// Audit logging module.
//
// Records MCP tool invocations in the mcp_audit table.
// Never logs credentials, tokens, or sensitive data.

use sqlx::SqlitePool;

/// One completed MCP tool invocation, ready for the audit table.
#[derive(Debug)]
pub struct AuditRecord<'a> {
    pub request_id: &'a str,
    pub user_id: i64,
    pub client_id: &'a str,
    pub grant_id: Option<&'a str>,
    pub tool: &'a str,
    pub resource_ids: Option<&'a str>,
    pub auth_result: &'a str,
    pub scope: Option<&'a str>,
    pub auth_strength: &'a str,
    pub latency_ms: i64,
    pub result_type: &'a str,
    pub operation_id: Option<&'a str>,
}

/// Append one invocation record to the audit table.
pub async fn log_invocation(
    pool: &SqlitePool,
    record: &AuditRecord<'_>,
) -> Result<(), AuditError> {
    let now = chrono::Utc::now().timestamp();

    sqlx::query(
        "INSERT INTO mcp_audit (timestamp, request_id, user_id, oauth_client_id,
         mcp_grant_id, tool, resource_ids, auth_result, scope, auth_strength,
         latency_ms, result_type, operation_id)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(now)
    .bind(record.request_id)
    .bind(record.user_id)
    .bind(record.client_id)
    .bind(record.grant_id)
    .bind(record.tool)
    .bind(record.resource_ids)
    .bind(record.auth_result)
    .bind(record.scope)
    .bind(record.auth_strength)
    .bind(record.latency_ms)
    .bind(record.result_type)
    .bind(record.operation_id)
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
    use crate::db::connect_and_migrate;

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

    #[tokio::test]
    async fn log_invocation_appends_a_row_with_integer_timestamp() {
        let database_path = std::env::temp_dir().join(format!(
            "commoncal-mcp-audit-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let pool = connect_and_migrate(&database_path)
            .await
            .expect("a fresh database should be created and migrated");

        let record = AuditRecord {
            request_id: "req-1",
            user_id: 42,
            client_id: "client-1",
            grant_id: Some("grant-1"),
            tool: "calendar_list",
            resource_ids: None,
            auth_result: "allowed",
            scope: None,
            auth_strength: "passwordless",
            latency_ms: 7,
            result_type: "success",
            operation_id: None,
        };

        log_invocation(&pool, &record)
            .await
            .expect("audit insert should succeed");

        let row: (i64, String, i64, String, String, String, i64, String) =
            sqlx::query_as(
                "SELECT timestamp, request_id, user_id, oauth_client_id, tool,
                 auth_result, latency_ms, result_type
                 FROM mcp_audit",
            )
            .fetch_one(&pool)
            .await
            .expect("the audit row should be readable");

        pool.close().await;
        let _ = std::fs::remove_file(&database_path);

        assert!(row.0 > 0, "timestamp should be stored as unix seconds");
        assert_eq!(row.1, "req-1");
        assert_eq!(row.2, 42);
        assert_eq!(row.3, "client-1");
        assert_eq!(row.4, "calendar_list");
        assert_eq!(row.5, "allowed");
        assert_eq!(row.6, 7);
        assert_eq!(row.7, "success");
    }

    #[tokio::test]
    async fn log_invocation_reports_failure_when_pool_is_closed() {
        let database_path = std::env::temp_dir().join(format!(
            "commoncal-mcp-audit-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let pool = connect_and_migrate(&database_path)
            .await
            .expect("a fresh database should be created and migrated");
        pool.close().await;
        let _ = std::fs::remove_file(&database_path);

        let record = AuditRecord {
            request_id: "req-2",
            user_id: 42,
            client_id: "client-1",
            grant_id: None,
            tool: "calendar_list",
            resource_ids: None,
            auth_result: "denied",
            scope: None,
            auth_strength: "passwordless",
            latency_ms: 0,
            result_type: "denied",
            operation_id: None,
        };

        let error = log_invocation(&pool, &record)
            .await
            .expect_err("a closed pool should reject the audit insert");

        assert!(error.message.contains("pool"), "error should name the pool failure");
    }
}
