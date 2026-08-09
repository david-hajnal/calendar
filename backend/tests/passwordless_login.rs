use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
};

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{
        Request, StatusCode,
        header::{CONTENT_TYPE, COOKIE, SET_COOKIE},
    },
};
use commoncal_backend::{
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    email::{
        AuthenticationLink, DevelopmentEmailSender, EmailError, EmailSender, InvitationEmail,
        LoginLinkEmail, ProductionEmailProvider, ProductionEmailSender, ProviderEmail,
        ProviderError,
    },
    http::{Readiness, build_router_with_login_service},
    identity::{IdentityRepository, NewUser, UserStatus},
    login::{FixedWindowLoginRateLimiter, LoginRateLimitKey, LoginRateLimiter, LoginService},
    security::{SecretKey, TokenDomain},
};
use http_body_util::BodyExt;
use sqlx::{Row, SqlitePool};
use tempfile::TempDir;
use tower::ServiceExt;

const NOW: i64 = 10_000;
const LOGIN_TOKEN_LIFETIME: i64 = 900;
const SESSION_LIFETIME: i64 = 86_400;
const CLIENT_IP: &str = "192.0.2.10";

#[derive(Default)]
struct CapturingEmailSender {
    login_links: Mutex<Vec<(String, String)>>,
}

impl CapturingEmailSender {
    fn links(&self) -> Vec<(String, String)> {
        self.login_links.lock().unwrap().clone()
    }
}

impl EmailSender for CapturingEmailSender {
    async fn send_invitation(&self, _command: InvitationEmail) -> Result<(), EmailError> {
        Ok(())
    }

    async fn send_login_link(&self, command: LoginLinkEmail) -> Result<(), EmailError> {
        self.login_links.lock().unwrap().push((
            command.recipient().to_owned(),
            command.authentication_link().expose().to_owned(),
        ));
        Ok(())
    }
}

#[derive(Default)]
struct RecordingLimiter {
    denied_ip: Mutex<Option<String>>,
    denied_email: Mutex<Option<String>>,
    checks: Mutex<Vec<String>>,
}

impl RecordingLimiter {
    fn deny_email(&self, email: &str) {
        *self.denied_email.lock().unwrap() = Some(email.to_owned());
    }

    fn checks(&self) -> Vec<String> {
        self.checks.lock().unwrap().clone()
    }
}

impl LoginRateLimiter for RecordingLimiter {
    fn allow(&self, key: LoginRateLimitKey<'_>) -> bool {
        match key {
            LoginRateLimitKey::Ip(ip) => {
                self.checks.lock().unwrap().push(format!("ip:{ip}"));
                self.denied_ip.lock().unwrap().as_deref() != Some(ip)
            }
            LoginRateLimitKey::Email(email) => {
                self.checks.lock().unwrap().push(format!("email:{email}"));
                self.denied_email.lock().unwrap().as_deref() != Some(email)
            }
        }
    }
}

struct TestApplication {
    _temp_dir: TempDir,
    pool: SqlitePool,
    secret_key: SecretKey,
    email_sender: Arc<CapturingEmailSender>,
    limiter: Arc<RecordingLimiter>,
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

        Self {
            _temp_dir: temp_dir,
            pool,
            secret_key: SecretKey::new([37; 32]),
            email_sender: Arc::new(CapturingEmailSender::default()),
            limiter: Arc::new(RecordingLimiter::default()),
        }
    }

    fn router(&self) -> axum::Router {
        build_router_with_login_service(
            Readiness::new(),
            LoginService::new_at(
                self.pool.clone(),
                self.secret_key.clone(),
                LOGIN_TOKEN_LIFETIME,
                SESSION_LIFETIME,
                "https://commoncal.test/login",
                self.email_sender.clone(),
                self.limiter.clone(),
                NOW,
                false,
            ),
        )
    }

    async fn user(&self, email: &str, status: UserStatus) -> i64 {
        IdentityRepository::new(self.pool.clone())
            .create_user(NewUser {
                normalized_email: email.to_owned(),
                display_name: Some("Test User".to_owned()),
                status,
                created_at: NOW - 500,
            })
            .await
            .unwrap()
            .id
    }

    async fn request_link(&self, email: &str) -> axum::response::Response {
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/v1/auth/login-links")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(format!(r#"{{"email":"{email}"}}"#)))
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)),
            54321,
        )));
        self.router().oneshot(request).await.unwrap()
    }

    async fn consume(&self, token: &str, cookie: Option<&str>) -> axum::response::Response {
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/v1/auth/login-links/consume")
            .header(CONTENT_TYPE, "application/json");
        if let Some(cookie) = cookie {
            request = request.header(COOKIE, cookie);
        }
        self.router()
            .oneshot(
                request
                    .body(Body::from(format!(r#"{{"token":"{token}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn issued_token(&self) -> String {
        self.email_sender
            .links()
            .last()
            .unwrap()
            .1
            .split("token=")
            .nth(1)
            .unwrap()
            .to_owned()
    }
}

async fn body(response: axum::response::Response) -> String {
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
async fn registered_and_unknown_requests_are_indistinguishable() {
    let registered = TestApplication::new().await;
    registered
        .user("known@example.com", UserStatus::Active)
        .await;
    let registered_response = registered.request_link(" Known@Example.com ").await;
    let registered_status = registered_response.status();
    let registered_content_type = registered_response.headers()[CONTENT_TYPE].clone();
    let registered_body = body(registered_response).await;

    let unknown = TestApplication::new().await;
    let unknown_response = unknown.request_link("unknown@example.com").await;

    assert_eq!(unknown_response.status(), registered_status);
    assert_eq!(
        unknown_response.headers()[CONTENT_TYPE],
        registered_content_type
    );
    assert_eq!(body(unknown_response).await, registered_body);
}

#[tokio::test]
async fn active_user_receives_a_link() {
    let application = TestApplication::new().await;
    application
        .user("active@example.com", UserStatus::Active)
        .await;

    let response = application.request_link("ACTIVE@example.com").await;

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let links = application.email_sender.links();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].0, "active@example.com");
    assert!(
        links[0]
            .1
            .starts_with("https://commoncal.test/login?token=")
    );
}

#[tokio::test]
async fn unknown_user_does_not_create_a_token() {
    let application = TestApplication::new().await;

    let response = application.request_link("missing@example.com").await;

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert!(application.email_sender.links().is_empty());
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM login_tokens")
        .fetch_one(&application.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn delivery_failure_keeps_the_generic_response_and_revokes_the_token() {
    let application = TestApplication::new().await;
    application
        .user("active@example.com", UserStatus::Active)
        .await;
    let service = LoginService::new_at(
        application.pool.clone(),
        application.secret_key.clone(),
        LOGIN_TOKEN_LIFETIME,
        SESSION_LIFETIME,
        "https://commoncal.test/login",
        Arc::new(ProductionEmailSender::new(RejectingProvider)),
        application.limiter.clone(),
        NOW,
        false,
    );
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/login-links")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"email":"active@example.com"}"#))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)),
        54321,
    )));

    let response = build_router_with_login_service(Readiness::new(), service)
        .oneshot(request)
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let revoked_at: Option<i64> =
        sqlx::query_scalar("SELECT revoked_at FROM login_tokens ORDER BY id DESC LIMIT 1")
            .fetch_one(&application.pool)
            .await
            .unwrap();
    assert_eq!(revoked_at, Some(NOW));
}

#[tokio::test]
async fn suspended_user_cannot_log_in() {
    let application = TestApplication::new().await;
    let user_id = application
        .user("suspended@example.com", UserStatus::Active)
        .await;
    application.request_link("suspended@example.com").await;
    let token = application.issued_token().await;
    sqlx::query("UPDATE users SET status = 'suspended' WHERE id = ?")
        .bind(user_id)
        .execute(&application.pool)
        .await
        .unwrap();

    let response = application.consume(&token, None).await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(&application.pool)
        .await
        .unwrap();
    assert_eq!(sessions, 0);
}

#[tokio::test]
async fn expired_and_reused_links_fail() {
    let application = TestApplication::new().await;
    application
        .user("active@example.com", UserStatus::Active)
        .await;
    application.request_link("active@example.com").await;
    let expired = application.issued_token().await;
    sqlx::query("UPDATE login_tokens SET expires_at = ?")
        .bind(NOW)
        .execute(&application.pool)
        .await
        .unwrap();
    assert_eq!(
        application.consume(&expired, None).await.status(),
        StatusCode::UNAUTHORIZED
    );

    application.request_link("active@example.com").await;
    let usable = application.issued_token().await;
    assert_eq!(
        application.consume(&usable, None).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        application.consume(&usable, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn rate_limit_checks_ip_and_normalized_email() {
    let application = TestApplication::new().await;
    application
        .user("active@example.com", UserStatus::Active)
        .await;
    application.limiter.deny_email("active@example.com");

    let response = application.request_link(" ACTIVE@EXAMPLE.COM ").await;

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(application.email_sender.links().is_empty());
    assert_eq!(
        application.limiter.checks(),
        vec![
            format!("ip:{CLIENT_IP}"),
            "email:active@example.com".to_owned()
        ]
    );
}

#[test]
fn fixed_window_limiter_enforces_each_key_independently() {
    let limiter = FixedWindowLoginRateLimiter::new_at(2, 60, NOW);

    assert!(limiter.allow(LoginRateLimitKey::Ip(CLIENT_IP)));
    assert!(limiter.allow(LoginRateLimitKey::Ip(CLIENT_IP)));
    assert!(!limiter.allow(LoginRateLimitKey::Ip(CLIENT_IP)));
    assert!(limiter.allow(LoginRateLimitKey::Email("active@example.com")));
}

#[tokio::test]
async fn successful_login_rotates_session_and_updates_last_login() {
    let application = TestApplication::new().await;
    let user_id = application
        .user("active@example.com", UserStatus::Active)
        .await;
    let old_session = application.secret_key.generate_token();
    let old_hash = application
        .secret_key
        .hash_token(TokenDomain::Session, &old_session);
    sqlx::query(
        "INSERT INTO sessions (user_id, session_hash, expires_at, revoked_at, created_at)
         VALUES (?, ?, ?, NULL, ?)",
    )
    .bind(user_id)
    .bind(old_hash.as_bytes().as_slice())
    .bind(NOW + SESSION_LIFETIME)
    .bind(NOW - 100)
    .execute(&application.pool)
    .await
    .unwrap();
    application.request_link("active@example.com").await;
    let login_token = application.issued_token().await;

    let response = application
        .consume(
            &login_token,
            Some(&format!(
                "__Host-commoncal_session={}",
                old_session.expose()
            )),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);
    let set_cookie = response.headers()[SET_COOKIE].to_str().unwrap();
    assert!(!set_cookie.contains(old_session.expose()));
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("SameSite=Lax"));
    let old_revoked: Option<i64> =
        sqlx::query("SELECT revoked_at FROM sessions WHERE session_hash = ?")
            .bind(old_hash.as_bytes().as_slice())
            .fetch_one(&application.pool)
            .await
            .unwrap()
            .get("revoked_at");
    assert_eq!(old_revoked, Some(NOW));
    let last_login_at: Option<i64> =
        sqlx::query_scalar("SELECT last_login_at FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_one(&application.pool)
            .await
            .unwrap();
    assert_eq!(last_login_at, Some(NOW));
    let response_body = body(response).await;
    assert!(response_body.contains(r#""csrf_token":""#));
    assert!(!response_body.contains("hash"));
    assert!(!response_body.contains(&login_token));
}

#[tokio::test(flavor = "current_thread")]
async fn logs_and_audits_do_not_reveal_account_existence_or_tokens() {
    let application = TestApplication::new().await;
    application
        .user("secret@example.com", UserStatus::Active)
        .await;
    let captured = Arc::new(Mutex::new(Vec::<u8>::new()));
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || TestWriter(writer.clone()))
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    application.request_link("secret@example.com").await;
    let token = application.issued_token().await;
    application.consume(&token, None).await;
    DevelopmentEmailSender::new()
        .send_login_link(LoginLinkEmail::new(
            "secret@example.com",
            AuthenticationLink::new(format!("https://commoncal.test/login?token={token}")),
        ))
        .await
        .unwrap();

    let logs = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    assert!(!logs.contains("secret@example.com"));
    assert!(!logs.contains(&token));
    assert!(!logs.contains("account_exists"));
    let audit_data: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT action, metadata_json FROM audit_log ORDER BY id")
            .fetch_all(&application.pool)
            .await
            .unwrap();
    let audit_text = format!("{audit_data:?}");
    assert!(!audit_text.contains("secret@example.com"));
    assert!(!audit_text.contains(&token));
}

#[derive(Clone)]
struct TestWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for TestWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct RejectingProvider;

impl ProductionEmailProvider for RejectingProvider {
    async fn send(&self, _email: ProviderEmail) -> Result<(), ProviderError> {
        Err(ProviderError::new())
    }
}
