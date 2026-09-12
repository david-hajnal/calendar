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

    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
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

        assert_eq!(migration_count, 1);
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
