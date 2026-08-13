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
    identity::{IdentityRepository, NewUser, SessionHash, UserStatus},
    invitations::InvitationConsumer,
    security::{SecretKey, TokenDomain},
};
use http_body_util::BodyExt;
use sqlx::{Row, SqlitePool};
use tempfile::TempDir;
use tower::ServiceExt;

const NOW: i64 = 1_000;
const SESSION_LIFETIME: i64 = 86_400;

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
            InvitationConsumer::new_at(
                self.pool.clone(),
                self.secret_key.clone(),
                SESSION_LIFETIME,
                NOW,
            ),
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
        let body = format!(r#"{{"token":"{token}"}}"#);
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
    let cookie = response
        .headers()
        .get(SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(cookie.starts_with("__Host-commoncal_session="));
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Lax"));
    let body = response_body(response).await;
    assert!(body.contains(r#""email":"invitee@example.com""#));
    assert!(body.contains(r#""status":"active""#));
    assert!(body.contains(r#""csrf_token":""#));

    let user_status: String =
        sqlx::query_scalar("SELECT status FROM users WHERE normalized_email = ?")
            .bind("invitee@example.com")
            .fetch_one(&application.pool)
            .await
            .unwrap();
    assert_eq!(user_status, "active");
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

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response_body(response).await,
        r#"{"error":{"code":"invalid_invitation","message":"Invitation is invalid or expired"}}"#
    );
    let session_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&application.pool)
        .await
        .unwrap();
    assert_eq!(session_count, 1);
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

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
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

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
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
async fn email_collision_resolves_without_duplicate_user_creation() {
    let application = TestApplication::new().await;
    let existing = IdentityRepository::new(application.pool.clone())
        .create_user(NewUser {
            normalized_email: "invitee@example.com".to_owned(),
            display_name: Some("Existing Name".to_owned()),
            status: UserStatus::Active,
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
    let session_user_id: i64 = sqlx::query_scalar("SELECT user_id FROM sessions")
        .fetch_one(&application.pool)
        .await
        .unwrap();
    assert_eq!(session_user_id, existing.id);
}

#[tokio::test]
async fn database_rollback_occurs_when_session_creation_fails() {
    let application = TestApplication::new().await;
    let (invitation_id, token) = application
        .invitation("rollback@example.com", NOW + 100, None)
        .await;
    sqlx::query(
        "CREATE TRIGGER fail_session_creation
         BEFORE INSERT ON sessions
         BEGIN
             SELECT RAISE(ABORT, 'injected session failure');
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
async fn session_fixation_is_not_possible() {
    let application = TestApplication::new().await;
    let existing_user = IdentityRepository::new(application.pool.clone())
        .create_user(NewUser {
            normalized_email: "invitee@example.com".to_owned(),
            display_name: None,
            status: UserStatus::Active,
            created_at: NOW - 500,
        })
        .await
        .unwrap();
    let fixed_token = application.secret_key.generate_token();
    let fixed_hash = application
        .secret_key
        .hash_token(TokenDomain::Session, &fixed_token);
    sqlx::query(
        "INSERT INTO sessions (user_id, session_hash, expires_at, revoked_at, created_at)
         VALUES (?, ?, ?, NULL, ?)",
    )
    .bind(existing_user.id)
    .bind(fixed_hash.as_bytes().as_slice())
    .bind(NOW + SESSION_LIFETIME)
    .bind(NOW - 100)
    .execute(&application.pool)
    .await
    .unwrap();
    let (_invitation_id, invitation_token) = application
        .invitation("invitee@example.com", NOW + 100, None)
        .await;
    let cookie = format!("__Host-commoncal_session={}", fixed_token.expose());

    let response = application.consume(&invitation_token, Some(&cookie)).await;

    assert_eq!(response.status(), StatusCode::OK);
    let set_cookie = response
        .headers()
        .get(SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(!set_cookie.contains(fixed_token.expose()));
    let old_session_revoked_at: Option<i64> =
        sqlx::query("SELECT revoked_at FROM sessions WHERE session_hash = ?")
            .bind(SessionHash::new(fixed_hash.as_bytes().to_vec()).as_bytes())
            .fetch_one(&application.pool)
            .await
            .unwrap()
            .get("revoked_at");
    assert_eq!(old_session_revoked_at, Some(NOW));
    let session_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&application.pool)
        .await
        .unwrap();
    assert_eq!(session_count, 2);
}
