use commoncal_backend::{
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    http::Readiness,
    identity::{
        AuditEntry, IdentityRepository, InvitationHash, LoginTokenHash, NewAuditEntry,
        NewInvitation, NewLoginToken, NewSession, NewUser, SessionHash, UserStatus,
    },
};
use sqlx::SqlitePool;
use tempfile::TempDir;

async fn repository() -> (TempDir, SqlitePool, IdentityRepository) {
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

    (temp_dir, pool.clone(), IdentityRepository::new(pool))
}

fn user(email: &str, status: UserStatus) -> NewUser {
    NewUser {
        normalized_email: email.to_owned(),
        display_name: Some("Test User".to_owned()),
        status,
        created_at: 100,
    }
}

#[tokio::test]
async fn duplicate_normalized_emails_fail() {
    let (_temp_dir, _pool, repository) = repository().await;
    repository
        .create_user(user(" Person@Example.com ", UserStatus::Active))
        .await
        .unwrap();

    let result = repository
        .create_user(user("person@example.com", UserStatus::Active))
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn invalid_user_status_fails_at_the_schema_boundary() {
    let (_temp_dir, pool, _repository) = repository().await;

    let result = sqlx::query(
        "INSERT INTO users (normalized_email, display_name, status, created_at)
         VALUES (?, ?, ?, ?)",
    )
    .bind("person@example.com")
    .bind("Test User")
    .bind("archived")
    .bind(100_i64)
    .execute(&pool)
    .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn deleted_user_status_is_schema_valid_and_round_trips() {
    let (_temp_dir, _pool, repository) = repository().await;

    let deleted = repository
        .create_user(user("deleted@example.com", UserStatus::Deleted))
        .await
        .unwrap();

    assert_eq!(
        repository
            .user_by_normalized_email("deleted@example.com")
            .await
            .unwrap(),
        Some(deleted)
    );
}

#[tokio::test]
async fn expired_and_revoked_records_can_be_queried_correctly() {
    let (_temp_dir, _pool, repository) = repository().await;
    let account = repository
        .create_user(user("person@example.com", UserStatus::Active))
        .await
        .unwrap();

    repository
        .create_invitation(NewInvitation {
            normalized_email: "invitee@example.com".to_owned(),
            display_name: None,
            token_hash: InvitationHash::new(vec![1; 32]),
            expires_at: 99,
            revoked_at: None,
            consumed_at: None,
            created_by_user_id: account.id,
            created_at: 10,
        })
        .await
        .unwrap();
    repository
        .create_invitation(NewInvitation {
            normalized_email: "revoked@example.com".to_owned(),
            display_name: None,
            token_hash: InvitationHash::new(vec![2; 32]),
            expires_at: 200,
            revoked_at: Some(50),
            consumed_at: None,
            created_by_user_id: account.id,
            created_at: 10,
        })
        .await
        .unwrap();
    repository
        .create_login_token(NewLoginToken {
            user_id: account.id,
            token_hash: LoginTokenHash::new(vec![3; 32]),
            expires_at: 99,
            revoked_at: None,
            consumed_at: None,
            created_at: 10,
        })
        .await
        .unwrap();
    repository
        .create_login_token(NewLoginToken {
            user_id: account.id,
            token_hash: LoginTokenHash::new(vec![4; 32]),
            expires_at: 200,
            revoked_at: Some(50),
            consumed_at: None,
            created_at: 10,
        })
        .await
        .unwrap();
    repository
        .create_session(NewSession {
            user_id: account.id,
            session_hash: SessionHash::new(vec![5; 32]),
            expires_at: 99,
            revoked_at: None,
            created_at: 10,
        })
        .await
        .unwrap();
    repository
        .create_session(NewSession {
            user_id: account.id,
            session_hash: SessionHash::new(vec![6; 32]),
            expires_at: 200,
            revoked_at: Some(50),
            created_at: 10,
        })
        .await
        .unwrap();

    assert_eq!(repository.expired_invitations(100).await.unwrap().len(), 1);
    assert_eq!(repository.revoked_invitations().await.unwrap().len(), 1);
    assert_eq!(repository.expired_login_tokens(100).await.unwrap().len(), 1);
    assert_eq!(repository.revoked_login_tokens().await.unwrap().len(), 1);
    assert_eq!(repository.expired_sessions(100).await.unwrap().len(), 1);
    assert_eq!(repository.revoked_sessions().await.unwrap().len(), 1);
}

#[tokio::test]
async fn identity_records_round_trip() {
    let (_temp_dir, pool, repository) = repository().await;
    let account = repository
        .create_user(user("person@example.com", UserStatus::Suspended))
        .await
        .unwrap();
    assert_eq!(
        repository
            .user_by_normalized_email("person@example.com")
            .await
            .unwrap(),
        Some(account.clone())
    );

    let invitation = repository
        .create_invitation(NewInvitation {
            normalized_email: "invitee@example.com".to_owned(),
            display_name: Some("Invitee".to_owned()),
            token_hash: InvitationHash::new(vec![7; 32]),
            expires_at: 200,
            revoked_at: None,
            consumed_at: None,
            created_by_user_id: account.id,
            created_at: 10,
        })
        .await
        .unwrap();
    assert_eq!(
        repository.invitation(invitation.id).await.unwrap(),
        Some(invitation)
    );

    let login_token = repository
        .create_login_token(NewLoginToken {
            user_id: account.id,
            token_hash: LoginTokenHash::new(vec![8; 32]),
            expires_at: 200,
            revoked_at: None,
            consumed_at: None,
            created_at: 10,
        })
        .await
        .unwrap();
    assert_eq!(
        repository.login_token(login_token.id).await.unwrap(),
        Some(login_token)
    );

    let session = repository
        .create_session(NewSession {
            user_id: account.id,
            session_hash: SessionHash::new(vec![9; 32]),
            expires_at: 200,
            revoked_at: None,
            created_at: 10,
        })
        .await
        .unwrap();
    assert_eq!(repository.session(session.id).await.unwrap(), Some(session));

    let audit = repository
        .append_audit_entry(NewAuditEntry {
            actor_user_id: Some(account.id),
            action: "user.suspended".to_owned(),
            target_type: "user".to_owned(),
            target_id: Some(account.id.to_string()),
            metadata_json: Some(r#"{"reason":"test"}"#.to_owned()),
            created_at: 300,
        })
        .await
        .unwrap();
    let audit_id = audit.id;
    let loaded: Option<AuditEntry> = repository.audit_entry(audit.id).await.unwrap();
    assert_eq!(loaded, Some(audit));

    let update_result = sqlx::query("UPDATE audit_log SET action = ? WHERE id = ?")
        .bind("user.reactivated")
        .bind(audit_id)
        .execute(&pool)
        .await;
    assert!(update_result.is_err());
}

#[test]
fn repository_api_stores_only_explicit_hash_types() {
    assert_ne!(
        std::any::TypeId::of::<InvitationHash>(),
        std::any::TypeId::of::<LoginTokenHash>()
    );
    assert_ne!(
        std::any::TypeId::of::<LoginTokenHash>(),
        std::any::TypeId::of::<SessionHash>()
    );
}
