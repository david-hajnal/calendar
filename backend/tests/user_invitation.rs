use axum::{
    body::Body,
    http::{Request, StatusCode, header::COOKIE},
};
use commoncal_backend::{
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    email::InMemoryEmailSender,
    http::{
        Readiness,
        build_router_with_auth_flows_sessions_admin_calendars_and_views_and_user_invitations,
        build_router_with_sessions,
    },
    identity::{IdentityRepository, NewUser, UserStatus},
    invitations::InvitationConsumer,
    login::{FixedWindowLoginRateLimiter, LoginService},
    security::SecretKey,
    sessions::SessionManager,
    user_invitation::{UserInvitationError, UserInvitationService},
    user_invitation_rate_limit::{
        USER_INVITATION_MAX_REQUESTS, USER_INVITATION_WINDOW_SECONDS,
        UserInvitationRateLimiterState, check_user_invitation_rate_limit,
        check_user_invitation_resend_rate_limit,
    },
};
use http_body_util::BodyExt;
use sqlx::SqlitePool;
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

const NOW: i64 = 1_000;
const SESSION_LIFETIME: i64 = 86_400;
const ORIGIN: &str = "http://localhost:3000";

struct TestApplication {
    _temp_dir: TempDir,
    pool: SqlitePool,
    secret_key: SecretKey,
    email_sender: Arc<InMemoryEmailSender>,
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
            secret_key: SecretKey::new([92; 32]),
            email_sender: Arc::new(InMemoryEmailSender::new()),
        }
    }

    async fn create_user(&self, email: &str, display_name: Option<&str>) -> i64 {
        let now = NOW;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users (normalized_email, display_name, status, is_superadmin, created_at)
             VALUES (?, ?, 'active', 0, ?)
             RETURNING id",
        )
        .bind(email)
        .bind(display_name)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .unwrap();
        id
    }

    fn session_manager(&self) -> SessionManager {
        SessionManager::new(
            self.pool.clone(),
            self.secret_key.clone(),
            commoncal_backend::sessions::SessionSecurityConfig::new(
                SESSION_LIFETIME,
                5 * 60,
                "http://localhost:3000",
            )
            .unwrap(),
        )
    }

    fn user_invitation_service(&self) -> UserInvitationService {
        UserInvitationService::with_email_sender(
            self.pool.clone(),
            self.secret_key.clone(),
            24 * 60 * 60,
            "http://localhost:3000/invitations/accept",
            self.email_sender.clone(),
        )
    }

    #[allow(dead_code)]
    async fn insert_session(&self, user_id: i64) -> String {
        let token = self.secret_key.generate_token();
        let hash = self
            .secret_key
            .hash_token(commoncal_backend::security::TokenDomain::Session, &token);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        sqlx::query(
            "INSERT INTO sessions (user_id, session_hash, expires_at, revoked_at, created_at, last_seen_at) VALUES (?, ?, ?, NULL, ?, ?)",
        )
        .bind(user_id)
        .bind(hash.as_bytes().as_slice())
        .bind(now + 3600)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .unwrap();
        token.expose().to_owned()
    }

    fn rate_limiter(max_requests: u32, window_seconds: i64) -> UserInvitationRateLimiterState {
        let limiter = commoncal_backend::rate_limiter::FixedWindowRateLimiter::new_at(
            max_requests,
            window_seconds,
            NOW,
        );
        UserInvitationRateLimiterState {
            limiter: Arc::new(limiter),
        }
    }

    #[allow(dead_code)]
    fn router_with_user_invitation(&self, user_id: i64, is_superadmin: bool) -> axum::Router {
        let _session = self.make_session(user_id, is_superadmin);
        let session_manager = self.session_manager();
        let _user_invitation_service = self.user_invitation_service();
        let _limiter = Self::rate_limiter(
            USER_INVITATION_MAX_REQUESTS + 1,
            USER_INVITATION_WINDOW_SECONDS,
        );

        build_router_with_sessions(Readiness::new(), session_manager, None, None, None)
    }

    fn router_with_user_invitation_and_service(
        &self,
        user_id: i64,
        is_superadmin: bool,
    ) -> axum::Router {
        let _session = self.make_session(user_id, is_superadmin);
        let session_manager = self.session_manager();
        let user_invitation_service = self.user_invitation_service();
        let login_service = LoginService::new_at(
            self.pool.clone(),
            self.secret_key.clone(),
            15 * 60,
            SESSION_LIFETIME,
            "/login",
            self.email_sender.clone(),
            Arc::new(FixedWindowLoginRateLimiter::new(5, 15 * 60)),
            NOW,
            false,
        );
        let limiter = Self::rate_limiter(
            USER_INVITATION_MAX_REQUESTS + 1,
            USER_INVITATION_WINDOW_SECONDS,
        );

        build_router_with_auth_flows_sessions_admin_calendars_and_views_and_user_invitations(
            Readiness::new(),
            InvitationConsumer::new_at(
                self.pool.clone(),
                self.secret_key.clone(),
                SESSION_LIFETIME,
                NOW,
            ),
            login_service,
            session_manager,
            commoncal_backend::admin::AdminService::new_for_test(),
            commoncal_backend::calendar::CalendarService::new(self.pool.clone()),
            commoncal_backend::event::EventService::new(self.pool.clone()),
            commoncal_backend::shared_view::SharedViewService::new(self.pool.clone()),
            user_invitation_service,
            Some(limiter),
        )
    }

    fn make_session(
        &self,
        user_id: i64,
        is_superadmin: bool,
    ) -> commoncal_backend::sessions::AuthenticatedSession {
        let token = self.secret_key.generate_token();
        let csrf_token = self
            .secret_key
            .generate_csrf_token(&token)
            .expose()
            .to_owned();
        commoncal_backend::sessions::AuthenticatedSession::new_for_test(
            user_id,
            token,
            csrf_token,
            commoncal_backend::invitations::ActiveUser {
                id: user_id,
                email: "test@example.com".into(),
                display_name: Some("Test User".into()),
                status: "active",
                is_superadmin,
            },
            NOW,
            NOW,
            NOW + SESSION_LIFETIME,
        )
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

// --- UserInvitationService unit tests ---

#[tokio::test]
async fn service_creates_invitation_successfully() {
    let app = TestApplication::new().await;
    let service = app.user_invitation_service();
    let actor_id = app.create_user("actor@example.com", Some("Actor")).await;

    let result = service
        .create_invitation(actor_id, "invitee@example.com".to_owned(), None)
        .await;

    assert!(result.is_ok());
    let invitation = result.unwrap();
    assert!(invitation.invitation_id > 0);

    // Verify email was sent
    let messages = app.email_sender.messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].recipient(), "invitee@example.com");
    assert_eq!(messages[0].subject(), "You are invited to CommonCal");
}

#[tokio::test]
async fn service_rejects_duplicate_invitation_for_same_email() {
    let app = TestApplication::new().await;
    let service = app.user_invitation_service();
    let actor_id = app.create_user("actor@example.com", Some("Actor")).await;

    service
        .create_invitation(actor_id, "invitee@example.com".to_owned(), None)
        .await
        .unwrap();

    let result = service
        .create_invitation(actor_id, "invitee@example.com".to_owned(), None)
        .await;

    assert_eq!(result, Err(UserInvitationError::Conflict));
}

#[tokio::test]
async fn service_rejects_invitation_for_existing_user() {
    let app = TestApplication::new().await;
    let service = app.user_invitation_service();
    let actor_id = app.create_user("actor@example.com", Some("Actor")).await;

    // Create existing user
    IdentityRepository::new(app.pool.clone())
        .create_user(NewUser {
            normalized_email: "existing@example.com".to_owned(),
            display_name: Some("Existing".to_owned()),
            status: UserStatus::Active,
            created_at: NOW - 100,
        })
        .await
        .unwrap();

    let result = service
        .create_invitation(actor_id, "existing@example.com".to_owned(), None)
        .await;

    assert_eq!(result, Err(UserInvitationError::Conflict));
}

#[tokio::test]
async fn service_resends_invitation_by_email() {
    let app = TestApplication::new().await;
    let service = app.user_invitation_service();
    let actor_id = app.create_user("actor@example.com", Some("Actor")).await;

    // Create initial invitation
    service
        .create_invitation(actor_id, "invitee@example.com".to_owned(), None)
        .await
        .unwrap();

    // Resend
    let result = service
        .resend_invitation_by_email("invitee@example.com".to_owned())
        .await;

    assert!(result.is_ok());
    let new_invitation = result.unwrap();

    // Verify old invitation is revoked
    let old_revoked: Option<i64> =
        sqlx::query_scalar("SELECT revoked_at FROM invitations ORDER BY id ASC LIMIT 1")
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert!(old_revoked.is_some());

    // Verify new invitation exists and is not revoked
    let new_invitation_id = new_invitation.invitation_id;
    let new_revoked: Option<i64> =
        sqlx::query_scalar("SELECT revoked_at FROM invitations WHERE id = ?")
            .bind(new_invitation_id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert!(new_revoked.is_none());

    // Verify new email was sent
    let messages = app.email_sender.messages();
    assert_eq!(messages.len(), 2);
}

#[tokio::test]
async fn service_resend_returns_not_found_for_nonexistent_email() {
    let app = TestApplication::new().await;
    let service = app.user_invitation_service();

    let result = service
        .resend_invitation_by_email("nobody@example.com".to_owned())
        .await;

    assert_eq!(result, Err(UserInvitationError::NotFound));
}

// --- Rate limiter tests ---

#[tokio::test]
async fn rate_limit_allows_under_limit() {
    let app = TestApplication::new().await;
    let actor_id = app.create_user("actor@example.com", Some("Actor")).await;
    let _service = app.user_invitation_service();
    let limiter = TestApplication::rate_limiter(
        USER_INVITATION_MAX_REQUESTS + 1,
        USER_INVITATION_WINDOW_SECONDS,
    );

    // All requests should succeed
    for i in 0..=USER_INVITATION_MAX_REQUESTS {
        let result = check_user_invitation_rate_limit(&limiter, actor_id);
        assert!(result.is_ok(), "request {} should be allowed", i + 1);
    }
}

#[tokio::test]
async fn rate_limit_blocks_over_limit() {
    let app = TestApplication::new().await;
    let actor_id = app.create_user("actor@example.com", Some("Actor")).await;
    let limiter =
        TestApplication::rate_limiter(USER_INVITATION_MAX_REQUESTS, USER_INVITATION_WINDOW_SECONDS);

    // Exhaust the limit
    for _ in 0..USER_INVITATION_MAX_REQUESTS {
        check_user_invitation_rate_limit(&limiter, actor_id).unwrap();
    }

    // Next request should be blocked
    let result = check_user_invitation_rate_limit(&limiter, actor_id);
    assert!(result.is_err());
}

#[tokio::test]
async fn resend_rate_limit_by_email() {
    let _app = TestApplication::new().await;
    let limiter = TestApplication::rate_limiter(3, USER_INVITATION_WINDOW_SECONDS);

    // First 3 requests should succeed
    assert!(check_user_invitation_resend_rate_limit(&limiter, "test@example.com").is_ok());
    assert!(check_user_invitation_resend_rate_limit(&limiter, "test@example.com").is_ok());
    assert!(check_user_invitation_resend_rate_limit(&limiter, "test@example.com").is_ok());

    // 4th request should be blocked
    assert!(
        check_user_invitation_resend_rate_limit(&limiter, "test@example.com").is_err(),
        "4th request for same email should be rate limited"
    );

    // Different email should be independent
    assert!(
        check_user_invitation_resend_rate_limit(&limiter, "other@example.com").is_ok(),
        "different email should be independent"
    );
}

// --- HTTP endpoint tests ---

#[tokio::test]
async fn http_endpoint_creates_invitation() {
    let app = TestApplication::new().await;
    let actor_id = app.create_user("actor@example.com", Some("Actor")).await;
    let token = app.secret_key.generate_token();
    let hash = app
        .secret_key
        .hash_token(commoncal_backend::security::TokenDomain::Session, &token);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    sqlx::query(
        "INSERT INTO sessions (user_id, session_hash, expires_at, revoked_at, created_at, last_seen_at) VALUES (?, ?, ?, NULL, ?, ?)",
    )
    .bind(actor_id)
    .bind(hash.as_bytes().as_slice())
    .bind(now + 3600)
    .bind(now)
    .bind(now)
    .execute(&app.pool)
    .await
    .unwrap();
    let csrf = app.secret_key.generate_csrf_token(&token);
    let router = app.router_with_user_invitation_and_service(actor_id, false);

    let body = r#"{"email":"invitee@example.com","display_name":"Invitee"}"#;
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/invitations")
        .header("content-type", "application/json")
        .header("origin", ORIGIN)
        .header("sec-fetch-site", "same-origin")
        .header(
            COOKIE,
            format!("__Host-commoncal_session={}", token.expose()),
        )
        .header("x-csrf-token", csrf.expose())
        .body(Body::from(body))
        .unwrap();

    let response = router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
    let body_str = response_body(response).await;
    assert!(body_str.contains(r#""id":"#));

    // Verify invitation was created in DB
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM invitations WHERE normalized_email = ?")
            .bind("invitee@example.com")
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn http_endpoint_rejects_duplicate_invitation() {
    let app = TestApplication::new().await;
    let actor_id = app.create_user("actor@example.com", Some("Actor")).await;
    let service = app.user_invitation_service();
    let token = app.secret_key.generate_token();
    let hash = app
        .secret_key
        .hash_token(commoncal_backend::security::TokenDomain::Session, &token);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    sqlx::query(
        "INSERT INTO sessions (user_id, session_hash, expires_at, revoked_at, created_at, last_seen_at) VALUES (?, ?, ?, NULL, ?, ?)",
    )
    .bind(actor_id)
    .bind(hash.as_bytes().as_slice())
    .bind(now + 3600)
    .bind(now)
    .bind(now)
    .execute(&app.pool)
    .await
    .unwrap();
    let csrf = app.secret_key.generate_csrf_token(&token);

    // Create first invitation
    service
        .create_invitation(actor_id, "invitee@example.com".to_owned(), None)
        .await
        .unwrap();

    let router = app.router_with_user_invitation_and_service(actor_id, false);

    let body = r#"{"email":"invitee@example.com"}"#;
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/invitations")
        .header("content-type", "application/json")
        .header("origin", ORIGIN)
        .header("sec-fetch-site", "same-origin")
        .header(
            COOKIE,
            format!("__Host-commoncal_session={}", token.expose()),
        )
        .header("x-csrf-token", csrf.expose())
        .body(Body::from(body))
        .unwrap();

    let response = router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn http_endpoint_resends_invitation() {
    let app = TestApplication::new().await;
    let actor_id = app.create_user("actor@example.com", Some("Actor")).await;
    let service = app.user_invitation_service();
    let token = app.secret_key.generate_token();
    let hash = app
        .secret_key
        .hash_token(commoncal_backend::security::TokenDomain::Session, &token);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    sqlx::query(
        "INSERT INTO sessions (user_id, session_hash, expires_at, revoked_at, created_at, last_seen_at) VALUES (?, ?, ?, NULL, ?, ?)",
    )
    .bind(actor_id)
    .bind(hash.as_bytes().as_slice())
    .bind(now + 3600)
    .bind(now)
    .bind(now)
    .execute(&app.pool)
    .await
    .unwrap();
    let csrf = app.secret_key.generate_csrf_token(&token);

    // Create initial invitation
    service
        .create_invitation(actor_id, "invitee@example.com".to_owned(), None)
        .await
        .unwrap();

    let router = app.router_with_user_invitation_and_service(actor_id, false);

    let body = r#"{"email":"invitee@example.com"}"#;
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/invitations/resend")
        .header("content-type", "application/json")
        .header("origin", ORIGIN)
        .header("sec-fetch-site", "same-origin")
        .header(
            COOKIE,
            format!("__Host-commoncal_session={}", token.expose()),
        )
        .header("x-csrf-token", csrf.expose())
        .body(Body::from(body))
        .unwrap();

    let response = router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body_str = response_body(response).await;
    assert!(body_str.contains(r#""id":"#));

    // Verify new invitation was created
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM invitations WHERE normalized_email = ? AND revoked_at IS NULL AND consumed_at IS NULL",
    )
    .bind("invitee@example.com")
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn http_endpoint_resend_returns_not_found() {
    let app = TestApplication::new().await;
    let actor_id = app.create_user("actor@example.com", Some("Actor")).await;
    let token = app.secret_key.generate_token();
    let hash = app
        .secret_key
        .hash_token(commoncal_backend::security::TokenDomain::Session, &token);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    sqlx::query(
        "INSERT INTO sessions (user_id, session_hash, expires_at, revoked_at, created_at, last_seen_at) VALUES (?, ?, ?, NULL, ?, ?)",
    )
    .bind(actor_id)
    .bind(hash.as_bytes().as_slice())
    .bind(now + 3600)
    .bind(now)
    .bind(now)
    .execute(&app.pool)
    .await
    .unwrap();
    let csrf = app.secret_key.generate_csrf_token(&token);
    let router = app.router_with_user_invitation_and_service(actor_id, false);

    let body = r#"{"email":"nobody@example.com"}"#;
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/invitations/resend")
        .header("content-type", "application/json")
        .header("origin", ORIGIN)
        .header("sec-fetch-site", "same-origin")
        .header(
            COOKIE,
            format!("__Host-commoncal_session={}", token.expose()),
        )
        .header("x-csrf-token", csrf.expose())
        .body(Body::from(body))
        .unwrap();

    let response = router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn http_endpoint_normalizes_email_to_lowercase() {
    let app = TestApplication::new().await;
    let actor_id = app.create_user("actor@example.com", Some("Actor")).await;
    let service = app.user_invitation_service();

    // Create invitation with uppercase email
    service
        .create_invitation(actor_id, "INVITEE@EXAMPLE.COM".to_owned(), None)
        .await
        .unwrap();

    // Try to create another with different case should fail (conflict)
    let result = service
        .create_invitation(actor_id, "invitee@example.com".to_owned(), None)
        .await;

    assert_eq!(result, Err(UserInvitationError::Conflict));
}
