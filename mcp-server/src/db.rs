use sqlx::SqlitePool;

pub async fn connect_and_migrate(database_path: &str) -> Result<SqlitePool, sqlx::Error> {
    let connection_string = format!("sqlite:{}", database_path);
    let pool = SqlitePool::connect(&connection_string).await?;

    // Tracer bullet: skip migrations.
    // Slice 4 will run real migrations (mcp_grant, delete_intent, idempotency_key, mcp_audit).
    Ok(pool)
}
