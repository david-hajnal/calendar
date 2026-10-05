use commoncal_backend::{
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    http::Readiness,
};
use sqlx::{
    Row,
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::borrow::Cow;
use tempfile::TempDir;
static ALL: Migrator = sqlx::migrate!("./migrations");

#[tokio::test]
async fn upgrade_preserves_identity_dependencies_and_backfills_invitees() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("upgrade.sqlite");
    let old = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true)
                .foreign_keys(true),
        )
        .await
        .unwrap();
    let before = Migrator {
        migrations: Cow::Owned(ALL.iter().filter(|m| m.version < 29).cloned().collect()),
        ..Migrator::DEFAULT
    };
    before.run(&old).await.unwrap();
    // Existing IDs, credentials, ownership and ON DELETE CASCADE rows must survive rebuilding users.
    sqlx::raw_sql("UPDATE users SET last_login_at = 123 WHERE id = 1;
        INSERT INTO users(id, normalized_email, status, created_at) VALUES(2,'inactive@example.test','suspended',1),(3,'deleted@example.test','deleted',1);
        INSERT INTO users(id,normalized_email,status,created_at) VALUES(99,'old-removed@example.test','active',1);
        DELETE FROM users WHERE id=99;
        INSERT INTO user_preferences(user_id,theme,created_at,updated_at) VALUES(1,'dark',1,1);
        INSERT INTO calendars(id,owner_user_id,name,color,default_timezone,default_event_visibility,created_at,updated_at) VALUES(1,1,'Existing','#123456','UTC','default',1,1);
        INSERT INTO calendar_acl(calendar_id,user_id,role,created_at,updated_at) VALUES(1,1,'owner',1,1);
        INSERT INTO sessions(user_id,session_hash,expires_at,created_at,last_seen_at) VALUES(1,X'0102',9000000000,1,1);
        INSERT INTO caldav_accounts(user_id,principal_id,created_at,updated_at) VALUES(1,'existing-principal',1,1);
        INSERT INTO caldav_credentials(user_id,label,token_prefix,token_hash,created_at) VALUES(1,'existing','12345678',zeroblob(32),1);
        INSERT INTO mcp_grant(id,user_id,oauth_client_id,created_at) VALUES('existing',1,'client',1);
        INSERT INTO invitations(normalized_email,token_hash,expires_at,created_by_user_id,created_at) VALUES('waiting@example.test',X'010203',1,1,1);
        INSERT INTO invitations(normalized_email,token_hash,expires_at,created_by_user_id,created_at,revoked_at) VALUES('revoked@example.test',X'010204',1,1,1,1);")
        .execute(&old).await.unwrap();
    let old_hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id=1")
        .fetch_one(&old)
        .await
        .unwrap();
    old.close().await;
    let config =
        AppConfig::with_database_path(Environment::Development, "127.0.0.1:3000", None, path)
            .unwrap();
    let pool = connect_and_migrate(&config, Readiness::new())
        .await
        .unwrap();
    let user = sqlx::query(
        "SELECT status,password_hash,is_superadmin,last_login_at FROM users WHERE id=1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(user.get::<String, _>("status"), "registered");
    assert_eq!(user.get::<String, _>("password_hash"), old_hash);
    assert!(user.get::<bool, _>("is_superadmin"));
    assert_eq!(user.get::<i64, _>("last_login_at"), 123);
    for table in [
        "user_preferences",
        "calendars",
        "calendar_acl",
        "sessions",
        "caldav_accounts",
        "caldav_credentials",
        "mcp_grant",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "{table} must survive");
    }
    let inactive: String = sqlx::query_scalar("SELECT status FROM users WHERE id=2")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(inactive, "inactive");
    let waiting: String = sqlx::query_scalar(
        "SELECT status FROM users WHERE normalized_email='waiting@example.test'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(waiting, "invited");
    let revoked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM users WHERE normalized_email='revoked@example.test'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(revoked, 0);
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .unwrap()
            .is_empty()
    );
    let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(fk, 1);
    pool.close().await;
    let pool = connect_and_migrate(&config, Readiness::new())
        .await
        .unwrap();
    let waiting_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM users WHERE normalized_email='waiting@example.test'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(waiting_count, 1);
    let waiting_id: i64 =
        sqlx::query_scalar("SELECT id FROM users WHERE normalized_email='waiting@example.test'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(waiting_id > 99, "deleted IDs must not be reused");
}

#[tokio::test]
async fn failed_integrity_check_rolls_back_upgrade_and_keeps_readiness_false() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("broken.sqlite");
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true)
                .foreign_keys(false),
        )
        .await
        .unwrap();
    let before = Migrator {
        migrations: Cow::Owned(ALL.iter().filter(|m| m.version < 29).cloned().collect()),
        ..Migrator::DEFAULT
    };
    before.run(&pool).await.unwrap();
    sqlx::query("INSERT INTO sessions(user_id,session_hash,expires_at,created_at,last_seen_at) VALUES(999,X'010203',9000000000,1,1)").execute(&pool).await.unwrap();
    pool.close().await;
    let config = AppConfig::with_database_path(
        Environment::Development,
        "127.0.0.1:3000",
        None,
        path.clone(),
    )
    .unwrap();
    let readiness = Readiness::new();
    assert!(
        connect_and_migrate(&config, readiness.clone())
            .await
            .is_err()
    );
    assert!(!readiness.is_ready());
    let pool = SqlitePoolOptions::new()
        .connect_with(SqliteConnectOptions::new().filename(path))
        .await
        .unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM users WHERE id=1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "active");
    let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations WHERE version=29")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(applied, 0);
}
