use axum::{
    body::Body,
    http::{
        Request, StatusCode,
        header::{CONTENT_TYPE, COOKIE, SET_COOKIE},
    },
};
use commoncal_backend::{
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    http::{Readiness, build_router_with_invitation_consumer},
    identity::{IdentityRepository, NewUser, UserStatus},
    invitations::InvitationConsumer,
    security::{SecretKey, TokenDomain},
};
use http_body_util::BodyExt;
use sqlx::SqlitePool;
use tempfile::TempDir;
use tower::ServiceExt;

const NOW: i64 = 1_000;

struct TestApplication {
    _temp_dir: TempDir,
    pool: SqlitePool,
    secret_key: SecretKey,
}

impl TestApplication {
    async fn new() -> Self {
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
        // Migration 0019 seeds a default admin; remove it for clean test state.
        sqlx::query("DELETE FROM users WHERE normalized_email = 'admin@localhost'")
            .execute(&pool)
            .await
            .unwrap();

        Self {
            _temp_dir: temp_dir,
            pool,
            secret_key: SecretKey::new([91; 32]),
        }
    }

    fn router(&self) -> axum::Router {
        build_router_with_invitation_consumer(
            Readiness::new(),
            InvitationConsumer::new_at(self.pool.clone(), self.secret_key.clone(), NOW),
            None,
            None,
            None,
        )
    }

    async fn invitation(
        &self,
        email: &str,
        expires_at: i64,
        revoked_at: Option<i64>,
    ) -> (i64, String) {
        let token = self.secret_key.generate_token();
        let token_hash = self.secret_key.hash_token(TokenDomain::Invitation, &token);
        let invitation = sqlx::query(
            "INSERT INTO invitations (
                normalized_email, display_name, token_hash, expires_at, revoked_at,
                consumed_at, created_by_user_id, platform_role, created_at
             ) VALUES (?, 'Invitee', ?, ?, ?, NULL, NULL, 'user', ?)",
        )
        .bind(email)
        .bind(token_hash.as_bytes().as_slice())
        .bind(expires_at)
        .bind(revoked_at)
        .bind(NOW - 100)
        .execute(&self.pool)
        .await
        .unwrap();

        (invitation.last_insert_rowid(), token.expose().to_owned())
    }

    async fn consume(&self, token: &str, cookie: Option<&str>) -> axum::response::Response {
        let body = serde_json::json!({"token":token,"password":"new-invite-password-123","password_confirmation":"new-invite-password-123"}).to_string();
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/v1/auth/invitations/consume")
            .header(CONTENT_TYPE, "application/json");
        if let Some(cookie) = cookie {
            request = request.header(COOKIE, cookie);
        }

        self.router()
            .oneshot(request.body(Body::from(body)).unwrap())
            .await
            .unwrap()
    }
}

async fn response_body(response: axum::response::Response) -> String {
    String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
async fn valid_invitation_activates_user() {
    let application = TestApplication::new().await;
    let (invitation_id, token) = application
        .invitation("invitee@example.com", NOW + 100, None)
        .await;

    let response = application.consume(&token, None).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get(SET_COOKIE).is_none());
    let body = response_body(response).await;
    assert!(body.contains(r#""email":"invitee@example.com""#));
    assert!(body.contains(r#""status":"registered""#));
    assert!(!body.contains("csrf_token"));
    let hash: String = sqlx::query_scalar(
        "SELECT password_hash FROM users WHERE normalized_email = 'invitee@example.com'",
    )
    .fetch_one(&application.pool)
    .await
    .unwrap();
    assert!(
        commoncal_backend::password::verify_password("new-invite-password-123", &hash).unwrap()
    );

    let user_status: String =
        sqlx::query_scalar("SELECT status FROM users WHERE normalized_email = ?")
            .bind("invitee@example.com")
            .fetch_one(&application.pool)
            .await
            .unwrap();
    assert_eq!(user_status, "registered");
    let consumed_at: Option<i64> =
        sqlx::query_scalar("SELECT consumed_at FROM invitations WHERE id = ?")
            .bind(invitation_id)
            .fetch_one(&application.pool)
            .await
            .unwrap();
    assert_eq!(consumed_at, Some(NOW));
    let audit_metadata: String = sqlx::query_scalar(
        "SELECT metadata_json FROM audit_log
         WHERE action = 'auth.invitation.consume.succeeded'",
    )
    .fetch_one(&application.pool)
    .await
    .unwrap();
    assert_eq!(audit_metadata, r#"{"result":"activated"}"#);
}

#[tokio::test]
async fn reused_invitation_fails() {
    let application = TestApplication::new().await;
    let (_invitation_id, token) = application
        .invitation("invitee@example.com", NOW + 100, None)
        .await;
    assert_eq!(
        application.consume(&token, None).await.status(),
        StatusCode::OK
    );

    let response = application.consume(&token, None).await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_body(response).await,
        r#"{"error":{"code":"invalid_invitation","message":"Invitation is invalid or expired"}}"#
    );
    let session_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&application.pool)
        .await
        .unwrap();
    assert_eq!(session_count, 0);
    let failure_reason: String = sqlx::query_scalar(
        "SELECT metadata_json FROM audit_log
         WHERE action = 'auth.invitation.consume.failed'
         ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(&application.pool)
    .await
    .unwrap();
    assert_eq!(failure_reason, r#"{"reason":"already_consumed"}"#);
}

#[tokio::test]
async fn expired_invitation_fails() {
    let application = TestApplication::new().await;
    let (_invitation_id, token) = application
        .invitation("invitee@example.com", NOW, None)
        .await;

    let response = application.consume(&token, None).await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let reason: String = sqlx::query_scalar(
        "SELECT metadata_json FROM audit_log
         WHERE action = 'auth.invitation.consume.failed'",
    )
    .fetch_one(&application.pool)
    .await
    .unwrap();
    assert_eq!(reason, r#"{"reason":"expired"}"#);
}

#[tokio::test]
async fn revoked_invitation_fails() {
    let application = TestApplication::new().await;
    let (_invitation_id, token) = application
        .invitation("invitee@example.com", NOW + 100, Some(NOW - 1))
        .await;

    let response = application.consume(&token, None).await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let reason: String = sqlx::query_scalar(
        "SELECT metadata_json FROM audit_log
         WHERE action = 'auth.invitation.consume.failed'",
    )
    .fetch_one(&application.pool)
    .await
    .unwrap();
    assert_eq!(reason, r#"{"reason":"revoked"}"#);
}

#[tokio::test]
async fn invited_email_collision_preserves_identity() {
    let application = TestApplication::new().await;
    let existing = IdentityRepository::new(application.pool.clone())
        .create_user(NewUser {
            normalized_email: "invitee@example.com".to_owned(),
            display_name: Some("Existing Name".to_owned()),
            status: UserStatus::Invited,
            created_at: NOW - 500,
        })
        .await
        .unwrap();
    let (_invitation_id, token) = application
        .invitation("INVITEE@example.com", NOW + 100, None)
        .await;

    let response = application.consume(&token, None).await;

    assert_eq!(response.status(), StatusCode::OK);
    let user_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&application.pool)
        .await
        .unwrap();
    assert_eq!(user_count, 1);
    let registered_user_id: i64 =
        sqlx::query_scalar("SELECT id FROM users WHERE status = 'registered'")
            .fetch_one(&application.pool)
            .await
            .unwrap();
    assert_eq!(registered_user_id, existing.id);
}

#[tokio::test]
async fn database_rollback_occurs_when_registration_fails() {
    let application = TestApplication::new().await;
    let (invitation_id, token) = application
        .invitation("rollback@example.com", NOW + 100, None)
        .await;
    sqlx::query(
        "CREATE TRIGGER fail_registration
         BEFORE INSERT ON users
         BEGIN
             SELECT RAISE(ABORT, 'injected registration failure');
         END",
    )
    .execute(&application.pool)
    .await
    .unwrap();

    let response = application.consume(&token, None).await;

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let consumed_at: Option<i64> =
        sqlx::query_scalar("SELECT consumed_at FROM invitations WHERE id = ?")
            .bind(invitation_id)
            .fetch_one(&application.pool)
            .await
            .unwrap();
    assert_eq!(consumed_at, None);
    let user_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE normalized_email = ?")
            .bind("rollback@example.com")
            .fetch_one(&application.pool)
            .await
            .unwrap();
    assert_eq!(user_count, 0);
}

#[tokio::test]
async fn response_does_not_expose_token_hashes() {
    let application = TestApplication::new().await;
    let (_invitation_id, token) = application
        .invitation("invitee@example.com", NOW + 100, None)
        .await;

    let response = application.consume(&token, None).await;
    let body = response_body(response).await;

    assert!(!body.contains("hash"));
    assert!(!body.contains(&token));
}

#[tokio::test]
async fn registered_account_cannot_be_overwritten_by_invitation() {
    let application = TestApplication::new().await;
    let user = IdentityRepository::new(application.pool.clone())
        .create_user(NewUser {
            normalized_email: "invitee@example.com".to_owned(),
            display_name: None,
            status: UserStatus::Registered,
            created_at: NOW - 500,
        })
        .await
        .unwrap();
    let (_, token) = application
        .invitation("invitee@example.com", NOW + 100, None)
        .await;
    let response = application.consume(&token, None).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let password: Option<String> =
        sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?")
            .bind(user.id)
            .fetch_one(&application.pool)
            .await
            .unwrap();
    assert!(password.is_none());
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&application.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn token_only_acceptance_cannot_bypass_password_creation() {
    let application = TestApplication::new().await;
    let (_, token) = application
        .invitation("password-required@example.com", NOW + 100, None)
        .await;
    let response = application
        .router()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/invitations/consume")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::json!({"token": token}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(response.status().is_client_error());
    let consumed: Option<i64> = sqlx::query_scalar("SELECT consumed_at FROM invitations WHERE normalized_email = 'password-required@example.com'")
        .fetch_one(&application.pool).await.unwrap();
    assert_eq!(consumed, None);
}

#[tokio::test]
async fn preview_never_consumes_invitation_or_creates_session() {
    let application = TestApplication::new().await;
    let (_, token) = application
        .invitation("preview@example.com", NOW + 100, None)
        .await;
    for _ in 0..2 {
        let response = application
            .router()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/auth/invitations/preview?token={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
        assert!(
            response_body(response)
                .await
                .contains("preview@example.com")
        );
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&application.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn invalid_password_does_not_consume_and_bootstrap_role_is_preserved() {
    let application = TestApplication::new().await;
    let (id, token) = application
        .invitation("bootstrap@example.com", NOW + 100, None)
        .await;
    sqlx::query("UPDATE invitations SET platform_role = 'superadmin' WHERE id = ?")
        .bind(id)
        .execute(&application.pool)
        .await
        .unwrap();
    for (password, confirmation) in [
        ("short", "short"),
        ("long-enough-password", "mismatched-password"),
    ] {
        let response = application.router().oneshot(Request::builder().method("POST")
            .uri("/api/v1/auth/invitations/consume").header(CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::json!({"token":token,"password":password,"password_confirmation":confirmation}).to_string())).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    assert_eq!(
        application.consume(&token, None).await.status(),
        StatusCode::OK
    );
    let admin: bool = sqlx::query_scalar(
        "SELECT is_superadmin FROM users WHERE normalized_email = 'bootstrap@example.com'",
    )
    .fetch_one(&application.pool)
    .await
    .unwrap();
    assert!(admin);
}

#[tokio::test]
async fn concurrent_acceptance_has_one_winner() {
    let application = TestApplication::new().await;
    let (_, token) = application
        .invitation("race@example.com", NOW + 100, None)
        .await;
    let (first, second) = tokio::join!(
        application.consume(&token, None),
        application.consume(&token, None)
    );
    let statuses = [first.status(), second.status()];
    assert_eq!(
        statuses
            .iter()
            .filter(|&&status| status == StatusCode::OK)
            .count(),
        1
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|&&status| status == StatusCode::BAD_REQUEST)
            .count(),
        1
    );
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE normalized_email='race@example.com'")
            .fetch_one(&application.pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
}
