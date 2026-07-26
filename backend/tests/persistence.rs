use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use commoncal_backend::{
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    http::{Readiness, build_router_with_readiness},
};
use sqlx::Row;
use tempfile::TempDir;
use tower::ServiceExt;

fn test_config(temp_dir: &TempDir) -> AppConfig {
    AppConfig::with_database_path(
        Environment::Development,
        "127.0.0.1:3000",
        None,
        temp_dir.path().join("commoncal.sqlite"),
    )
    .expect("temporary database path should be valid")
}

#[tokio::test]
async fn migrations_run_on_a_fresh_database() {
    let temp_dir = TempDir::new().unwrap();
    let readiness = Readiness::new();

    let pool = connect_and_migrate(&test_config(&temp_dir), readiness.clone())
        .await
        .expect("fresh database should migrate");

    let migration_table_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(migration_table_count, 1);
    assert!(readiness.is_ready());
}

#[tokio::test]
async fn rerunning_migrations_is_safe() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config(&temp_dir);

    let first_pool = connect_and_migrate(&config, Readiness::new())
        .await
        .expect("first migration run should succeed");
    let initially_applied_migrations: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(&first_pool)
            .await
            .unwrap();
    first_pool.close().await;

    let second_pool = connect_and_migrate(&config, Readiness::new())
        .await
        .expect("second migration run should succeed");
    let applied_migrations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(&second_pool)
        .await
        .unwrap();

    assert_eq!(applied_migrations, initially_applied_migrations);
}

#[tokio::test]
async fn foreign_key_violations_fail() {
    let temp_dir = TempDir::new().unwrap();
    let pool = connect_and_migrate(&test_config(&temp_dir), Readiness::new())
        .await
        .unwrap();
    sqlx::query("CREATE TABLE test_parents (id INTEGER PRIMARY KEY)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE test_children (
            id INTEGER PRIMARY KEY,
            parent_id INTEGER NOT NULL REFERENCES test_parents(id)
        )",
    )
    .execute(&pool)
    .await
    .unwrap();

    let result = sqlx::query("INSERT INTO test_children (id, parent_id) VALUES (?, ?)")
        .bind(1_i64)
        .bind(999_i64)
        .execute(&pool)
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn readiness_remains_false_when_migration_fails() {
    let temp_dir = TempDir::new().unwrap();
    let config = test_config(&temp_dir);
    let initial_pool = connect_and_migrate(&config, Readiness::new())
        .await
        .unwrap();
    sqlx::query("UPDATE _sqlx_migrations SET checksum = X'00'")
        .execute(&initial_pool)
        .await
        .unwrap();
    initial_pool.close().await;
    let readiness = Readiness::new();

    let result = connect_and_migrate(&config, readiness.clone()).await;

    assert!(result.is_err());
    assert!(!readiness.is_ready());
    let response = build_router_with_readiness(readiness)
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn wal_mode_is_active_in_the_integration_database() {
    let temp_dir = TempDir::new().unwrap();
    let pool = connect_and_migrate(&test_config(&temp_dir), Readiness::new())
        .await
        .unwrap();

    let row = sqlx::query("PRAGMA journal_mode")
        .fetch_one(&pool)
        .await
        .unwrap();
    let journal_mode: String = row.get(0);

    assert_eq!(journal_mode.to_lowercase(), "wal");
}

#[test]
fn database_path_must_not_be_empty() {
    let result =
        AppConfig::with_database_path(Environment::Development, "127.0.0.1:3000", None, "");

    assert_eq!(
        result.unwrap_err().to_string(),
        "DATABASE_PATH must not be empty"
    );
}
