use commoncal_backend::{
    bootstrap::{BootstrapCommand, BootstrapError, InitialSuperadminBootstrap},
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    http::Readiness,
    identity::{IdentityRepository, InvitationPlatformRole, NewUser, UserStatus},
    security::{SecretKey, TokenDomain},
};
use sqlx::{Row, SqlitePool};
use tempfile::TempDir;

async fn application() -> (TempDir, SqlitePool, InitialSuperadminBootstrap) {
    let temp_dir = TempDir::new().unwrap();
    let config = AppConfig::with_database_path(
        Environment::Development,
        "127.0.0.1:3000",
        None,
        temp_dir.path().join("commoncal.sqlite"),
    )
    .unwrap();
    let pool = connect_and_migrate(&config, Readiness::new())
        .await
        .unwrap();
    // Migration 0019 seeds a default admin; remove it so bootstrap tests
    // see an empty database as they expect.
    sqlx::query("DELETE FROM users WHERE normalized_email = 'admin@localhost'")
        .execute(&pool)
        .await
        .unwrap();
    let application = InitialSuperadminBootstrap::new(pool.clone(), SecretKey::new([42; 32]));

    (temp_dir, pool, application)
}

fn command() -> BootstrapCommand {
    BootstrapCommand {
        normalized_email: " Initial.Admin@Example.com ".to_owned(),
        display_name: Some("Initial Admin".to_owned()),
        created_at: 1_000,
        expires_at: 2_000,
    }
}

#[tokio::test]
async fn empty_database_permits_bootstrap() {
    let (_temp_dir, pool, application) = application().await;

    let result = application.execute(command()).await.unwrap();

    let invitation_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM invitations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(invitation_count, 1);
    assert!(result.invitation_id > 0);
}

#[tokio::test]
async fn second_bootstrap_attempt_is_rejected() {
    let (_temp_dir, pool, application) = application().await;
    application.execute(command()).await.unwrap();

    let result = application.execute(command()).await;

    assert!(matches!(result, Err(BootstrapError::AlreadyInitialized)));
    let rejected_audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_log
         WHERE action = ? AND metadata_json = ?",
    )
    .bind("bootstrap.initial_superadmin.rejected")
    .bind(r#"{"reason":"already_initialized"}"#)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rejected_audits, 1);
}

#[tokio::test]
async fn existing_user_blocks_bootstrap() {
    let (_temp_dir, pool, application) = application().await;
    IdentityRepository::new(pool.clone())
        .create_user(NewUser {
            normalized_email: "existing@example.com".to_owned(),
            display_name: None,
            status: UserStatus::Active,
            created_at: 500,
        })
        .await
        .unwrap();

    let result = application.execute(command()).await;

    assert!(matches!(result, Err(BootstrapError::UsersExist)));
    let rejected_audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_log
         WHERE action = ? AND metadata_json = ?",
    )
    .bind("bootstrap.initial_superadmin.rejected")
    .bind(r#"{"reason":"users_exist"}"#)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rejected_audits, 1);
}

#[tokio::test]
async fn created_invitation_is_superadmin_bound() {
    let (_temp_dir, pool, application) = application().await;

    let result = application.execute(command()).await.unwrap();

    let invitation = IdentityRepository::new(pool)
        .invitation(result.invitation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(invitation.normalized_email, "initial.admin@example.com");
    assert_eq!(invitation.platform_role, InvitationPlatformRole::Superadmin);
    assert_eq!(invitation.created_by_user_id, None);
}

#[tokio::test]
async fn raw_token_is_returned_only_to_caller_and_is_not_stored() {
    let (_temp_dir, pool, application) = application().await;

    let result = application.execute(command()).await.unwrap();

    let stored_hash: Vec<u8> =
        sqlx::query_scalar("SELECT token_hash FROM invitations WHERE id = ?")
            .bind(result.invitation_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_ne!(stored_hash, result.token.expose().as_bytes());
    let expected_hash = SecretKey::new([42; 32]).hash_token(TokenDomain::Invitation, &result.token);
    assert_eq!(stored_hash, expected_hash.as_bytes());
}

#[tokio::test]
async fn audit_entry_contains_no_token() {
    let (_temp_dir, pool, application) = application().await;

    let result = application.execute(command()).await.unwrap();

    let audit = sqlx::query(
        "SELECT action, target_type, target_id, metadata_json
         FROM audit_log WHERE action = ?",
    )
    .bind("bootstrap.initial_superadmin.created")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        audit.get::<String, _>("target_id"),
        result.invitation_id.to_string()
    );
    let serialized = format!(
        "{}:{}:{}:{}",
        audit.get::<String, _>("action"),
        audit.get::<String, _>("target_type"),
        audit.get::<String, _>("target_id"),
        audit
            .get::<Option<String>, _>("metadata_json")
            .unwrap_or_default()
    );
    assert!(!serialized.contains(result.token.expose()));
}
