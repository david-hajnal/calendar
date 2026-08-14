use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};

pub async fn connect_and_migrate(database_path: &str) -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::new()
        .filename(database_path)
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new().connect_with(options).await?;

    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::connect_and_migrate;
    use std::path::PathBuf;

    #[tokio::test]
    async fn creates_and_migrates_a_new_database_file() {
        let database_path = unique_database_path();

        let pool = connect_and_migrate(database_path.to_str().unwrap())
            .await
            .expect("a fresh database should be created and migrated");
        let migration_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await
            .expect("migration history should exist");
        let grant_table_exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'mcp_grant'",
        )
        .fetch_one(&pool)
        .await
        .expect("schema should be queryable");

        pool.close().await;
        let _ = std::fs::remove_file(&database_path);

        assert_eq!(migration_count, 1);
        assert_eq!(grant_table_exists, 1);
    }

    fn unique_database_path() -> PathBuf {
        std::env::temp_dir().join(format!("commoncal-mcp-{}.sqlite", uuid::Uuid::new_v4()))
    }
}
