// SQLite storage setup for the MCP service.
//
// Centralizes connection options, embedded migrations, and readiness probing.
// The MCP service owns a single SQLite file; these options make that file safe
// for one writer: foreign keys enforced, WAL journaling, a busy timeout, and a
// bounded connection pool.

use std::path::Path;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

/// Open or create the configured SQLite file, apply embedded migrations, and
/// return a bounded pool.
///
/// Connection contract:
/// - `create_if_missing(true)`
/// - `foreign_keys(true)`
/// - `journal_mode(Wal)`
/// - `busy_timeout(5s)`
/// - `max_connections(5)`
pub async fn connect_and_migrate(database_path: &Path) -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::new()
        .filename(database_path)
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    guard_duplicate_tables(&pool).await?;

    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}

/// Abort startup if the duplicate local state tables still hold rows.
///
/// The `0002_local_audit_only` migration drops `mcp_grant`, `delete_intent`,
/// and `idempotency_key`. Those records have authoritative equivalents in
/// CommonCal core, but discarding data is an operator decision, not a
/// migration side effect: any row forces a human to reconcile first.
async fn guard_duplicate_tables(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    for table in ["mcp_grant", "delete_intent", "idempotency_key"] {
        let exists = sqlx::query_scalar::<_, String>(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?",
        )
        .bind(table)
        .fetch_optional(pool)
        .await?
        .is_some();

        if !exists {
            continue;
        }

        let rows: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(pool)
            .await?;

        if rows > 0 {
            return Err(sqlx::Error::Configuration(Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!(
                    "refusing to migrate: table {table} holds {rows} row(s); \
                     reconcile with CommonCal core and remove the data before upgrading"
                ),
            ))));
        }
    }

    Ok(())
}

/// Report whether the pool can serve a trivial query.
///
/// Used by storage-aware readiness: a queryable file is ready, an unavailable
/// or closed pool is not.
pub async fn is_ready(pool: &SqlitePool) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(pool)
        .await
        .is_ok_and(|value| value == 1)
}

#[cfg(test)]
mod tests {
    use super::{connect_and_migrate, is_ready};
    use std::path::PathBuf;

    use sqlx::SqlitePool;
    use sqlx::sqlite::SqliteConnectOptions;

    #[tokio::test]
    async fn creates_and_migrates_a_new_database_file() {
        let database_path = unique_database_path();

        let pool = connect_and_migrate(&database_path)
            .await
            .expect("a fresh database should be created and migrated");
        let migration_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await
            .expect("migration history should exist");

        pool.close().await;
        let _ = std::fs::remove_file(&database_path);

        assert_eq!(migration_count, 2);
    }

    #[tokio::test]
    async fn migration_keeps_audit_and_drops_duplicate_tables() {
        let database_path = unique_database_path();

        // Build a pre-upgrade database: initial schema only, with an audit row.
        {
            let options = SqliteConnectOptions::new()
                .filename(&database_path)
                .create_if_missing(true);
            let pool = SqlitePool::connect_with(options)
                .await
                .expect("pool should open");
            let migrator = sqlx::migrate!("./migrations");
            let initial = migrator
                .iter()
                .next()
                .expect("the initial migration should exist");
            sqlx::raw_sql(initial.sql.as_ref())
                .execute(&pool)
                .await
                .expect("initial schema should apply");
            sqlx::query(
                "INSERT INTO mcp_audit (timestamp, request_id, user_id, oauth_client_id,
                 tool, auth_result, result_type)
                 VALUES (1700000000, 'req-1', 42, 'client-1', 'calendar_list', 'allowed', 'success')",
            )
            .execute(&pool)
            .await
            .expect("audit row should insert");
            pool.close().await;
        }

        let pool = connect_and_migrate(&database_path)
            .await
            .expect("the upgrade should complete");

        let audit_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mcp_audit")
            .fetch_one(&pool)
            .await
            .expect("mcp_audit should survive the upgrade");
        let audit_request: String = sqlx::query_scalar("SELECT request_id FROM mcp_audit")
            .fetch_one(&pool)
            .await
            .expect("the audit row should be readable");
        let duplicates: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name IN ('mcp_grant', 'delete_intent', 'idempotency_key')",
        )
        .fetch_one(&pool)
        .await
        .expect("schema query should succeed");

        pool.close().await;
        let _ = std::fs::remove_file(&database_path);

        assert_eq!(audit_rows, 1, "audit rows must survive the upgrade");
        assert_eq!(audit_request, "req-1");
        assert_eq!(duplicates, 0, "duplicate local state tables must be dropped");
    }

    #[tokio::test]
    async fn migration_aborts_when_duplicate_tables_hold_rows() {
        let database_path = unique_database_path();

        // Build a pre-upgrade database with a row in a duplicate table.
        {
            let options = SqliteConnectOptions::new()
                .filename(&database_path)
                .create_if_missing(true);
            let pool = SqlitePool::connect_with(options)
                .await
                .expect("pool should open");
            let migrator = sqlx::migrate!("./migrations");
            let initial = migrator
                .iter()
                .next()
                .expect("the initial migration should exist");
            sqlx::raw_sql(initial.sql.as_ref())
                .execute(&pool)
                .await
                .expect("initial schema should apply");
            sqlx::query(
                "INSERT INTO mcp_grant (grant_id, user_id, oauth_client_id,
                 allowed_calendar_ids, allow_availability, allow_event_titles,
                 allow_event_details, allow_create, allow_update, allow_delete, created_at)
                 VALUES ('g1', 42, 'client-1', '[]', 0, 0, 0, 0, 0, 0, 1700000000)",
            )
            .execute(&pool)
            .await
            .expect("duplicate row should insert");
            pool.close().await;
        }

        let error = connect_and_migrate(&database_path)
            .await
            .expect_err("rows in duplicate tables must block the upgrade");

        let _ = std::fs::remove_file(&database_path);

        assert!(
            error.to_string().contains("mcp_grant"),
            "the error should name the blocking table"
        );
    }

    #[tokio::test]
    async fn creates_database_with_required_pragmas() {
        let database_path = unique_database_path();
        let pool = connect_and_migrate(&database_path)
            .await
            .expect("a fresh database should be created and migrated");

        let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap();
        let busy_timeout: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
            .fetch_one(&pool)
            .await
            .unwrap();

        pool.close().await;
        let _ = std::fs::remove_file(&database_path);

        assert_eq!(foreign_keys, 1, "foreign keys should be enforced");
        assert_eq!(journal_mode, "wal", "WAL journaling should be enabled");
        assert_eq!(busy_timeout, 5000, "busy timeout should be 5 seconds");
    }

    #[tokio::test]
    async fn connection_pool_tolerates_concurrent_audit_writes() {
        let database_path = unique_database_path();
        let pool = connect_and_migrate(&database_path)
            .await
            .expect("a fresh database should be created and migrated");

        let mut handles = Vec::new();
        for i in 0..20 {
            let pool = pool.clone();
            handles.push(tokio::spawn(async move {
                sqlx::query(
                    "INSERT INTO mcp_audit (timestamp, request_id, user_id, oauth_client_id,
                     tool, auth_result, result_type)
                     VALUES (?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(1_700_000_000i64)
                .bind(format!("req-{i}"))
                .bind(42i64)
                .bind("client-1")
                .bind("calendar_list")
                .bind("allowed")
                .bind("success")
                .execute(&pool)
                .await
                .expect("concurrent audit writes should not fail on locks")
            }));
        }

        for handle in handles {
            handle.await.expect("task should not panic");
        }

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mcp_audit")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 20, "all concurrent audit rows should be present");

        pool.close().await;
        let _ = std::fs::remove_file(&database_path);
    }

    #[tokio::test]
    async fn readiness_succeeds_for_open_database() {
        let database_path = unique_database_path();
        let pool = connect_and_migrate(&database_path)
            .await
            .expect("a fresh database should be created and migrated");

        assert!(
            is_ready(&pool).await,
            "an open, queryable pool should be ready"
        );

        pool.close().await;
        let _ = std::fs::remove_file(&database_path);
    }

    #[tokio::test]
    async fn readiness_fails_for_closed_pool() {
        let database_path = unique_database_path();
        let pool = connect_and_migrate(&database_path)
            .await
            .expect("a fresh database should be created and migrated");

        pool.close().await;
        let _ = std::fs::remove_file(&database_path);

        assert!(!is_ready(&pool).await, "a closed pool should not be ready");
    }

    fn unique_database_path() -> PathBuf {
        std::env::temp_dir().join(format!("commoncal-mcp-{}.sqlite", uuid::Uuid::new_v4()))
    }
}
