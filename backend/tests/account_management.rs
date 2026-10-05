use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use commoncal_backend::{
    account::{AccountService, ConsumePasswordReset, RequestPasswordReset},
    account_rate_limit::AccountRateLimiter,
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    email::{
        EmailChangedEmail, EmailConfirmationEmail, EmailError, EmailSender, InvitationEmail,
        LoginLinkEmail, PasswordResetEmail,
    },
    http::{Readiness, build_account_recovery_router},
    password::verify_password,
    security::{SecretKey, TokenDomain},
};
use http_body_util::BodyExt;
use sqlx::SqlitePool;
use std::{
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tempfile::TempDir;
use tower::ServiceExt;
const NOW: i64 = 20_000;
const ORIGIN: &str = "https://commoncal.test";
const PASSWORD: &str = "new-password-for-recovery";
#[derive(Default)]
struct Sender {
    invitations: Mutex<Vec<InvitationEmail>>,
    messages: Mutex<Vec<PasswordResetEmail>>,
    confirmations: Mutex<Vec<EmailConfirmationEmail>>,
    notices: Mutex<Vec<EmailChangedEmail>>,
    fail_notice: AtomicBool,
    fail: AtomicBool,
}
impl EmailSender for Sender {
    async fn send_invitation(&self, message: InvitationEmail) -> Result<(), EmailError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(EmailError::transient());
        }
        self.invitations.lock().unwrap().push(message);
        Ok(())
    }
    async fn send_login_link(&self, _: LoginLinkEmail) -> Result<(), EmailError> {
        Ok(())
    }
    async fn send_password_reset(&self, message: PasswordResetEmail) -> Result<(), EmailError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(EmailError::transient());
        }
        self.messages.lock().unwrap().push(message);
        Ok(())
    }
    async fn send_email_confirmation(
        &self,
        message: EmailConfirmationEmail,
    ) -> Result<(), EmailError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(EmailError::transient());
        }
        self.confirmations.lock().unwrap().push(message);
        Ok(())
    }
    async fn send_email_changed(&self, message: EmailChangedEmail) -> Result<(), EmailError> {
        if self.fail_notice.load(Ordering::SeqCst) {
            return Err(EmailError::transient());
        }
        self.notices.lock().unwrap().push(message);
        Ok(())
    }
}
struct App {
    _dir: TempDir,
    pool: SqlitePool,
    key: SecretKey,
    sender: Arc<Sender>,
    user_id: i64,
}
impl App {
    async fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let config = AppConfig::with_database_path(
            Environment::Development,
            "127.0.0.1:3000",
            None,
            dir.path().join("accounts.sqlite"),
        )
        .unwrap();
        let pool = connect_and_migrate(&config, Readiness::new())
            .await
            .unwrap();
        let user_id = sqlx::query("INSERT INTO users(normalized_email, status, created_at) VALUES ('member@example.test', 'registered', ?)").bind(NOW).execute(&pool).await.unwrap().last_insert_rowid();
        Self {
            _dir: dir,
            pool,
            key: SecretKey::new([38; 32]),
            sender: Arc::new(Sender::default()),
            user_id,
        }
    }
    fn service_at(&self, now: i64) -> AccountService {
        AccountService::new_at(
            self.pool.clone(),
            self.key.clone(),
            ORIGIN,
            self.sender.clone(),
            now,
        )
    }
    fn service(&self) -> AccountService {
        self.service_at(NOW)
    }
    fn router(&self) -> Router {
        build_account_recovery_router(self.service(), ORIGIN, AccountRateLimiter::new_at(NOW))
    }
    fn token(&self) -> String {
        let messages = self.sender.messages.lock().unwrap();
        let url = url::Url::parse(messages.last().unwrap().authentication_link().expose()).unwrap();
        url.query_pairs()
            .find(|(key, _)| key == "token")
            .unwrap()
            .1
            .into_owned()
    }
    async fn request(&self) -> String {
        self.service()
            .request_password_reset(RequestPasswordReset {
                email: " MEMBER@example.test ".into(),
            })
            .await
            .unwrap();
        self.token()
    }
}
fn consume(token: String) -> ConsumePasswordReset {
    ConsumePasswordReset {
        token,
        password: PASSWORD.into(),
        password_confirmation: PASSWORD.into(),
    }
}
async fn post(
    router: Router,
    path: &str,
    body: String,
    origin: &str,
    ip: &str,
) -> (StatusCode, String) {
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .header("origin", origin)
                .header("x-forwarded-for", ip)
                .extension(ConnectInfo("127.0.0.1:2000".parse::<SocketAddr>().unwrap()))
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
    assert!(response.headers().get("set-cookie").is_none());
    (
        response.status(),
        String::from_utf8(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap(),
    )
}

#[tokio::test]
async fn recovery_request_has_a_generic_public_response() {
    let app = App::new().await;
    let router = app.router();
    let expected = post(
        router.clone(),
        "/api/v1/auth/password-resets",
        r#"{"email":"unknown@example.test"}"#.into(),
        ORIGIN,
        "evil-proxy",
    )
    .await;
    assert_eq!(expected.0, StatusCode::ACCEPTED);
    for status in ["invited", "pending", "inactive", "deleted", "registered"] {
        sqlx::query("UPDATE users SET status = ? WHERE id = ?")
            .bind(status)
            .bind(app.user_id)
            .execute(&app.pool)
            .await
            .unwrap();
        assert_eq!(
            post(
                app.router(),
                "/api/v1/auth/password-resets",
                r#"{"email":"member@example.test"}"#.into(),
                ORIGIN,
                "evil-proxy"
            )
            .await,
            expected
        );
    }
    assert_eq!(app.sender.messages.lock().unwrap().len(), 1);
    app.sender.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        post(
            app.router(),
            "/api/v1/auth/password-resets",
            r#"{"email":"member@example.test"}"#.into(),
            ORIGIN,
            "evil-proxy"
        )
        .await,
        expected
    );
    let live: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM password_reset_tokens WHERE revoked_at IS NULL AND consumed_at IS NULL").fetch_one(&app.pool).await.unwrap();
    assert_eq!(live, 0);
    let audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_log WHERE action = 'account.password_reset.delivery_failed'",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(audits, 1);
    // A storage failure must not reveal the account, either.
    sqlx::query("DROP TABLE password_reset_tokens")
        .execute(&app.pool)
        .await
        .unwrap();
    assert_eq!(
        post(
            app.router(),
            "/api/v1/auth/password-resets",
            r#"{"email":"member@example.test"}"#.into(),
            ORIGIN,
            "evil-proxy"
        )
        .await,
        expected
    );
}

#[tokio::test]
async fn reset_is_single_use_and_ends_sessions_and_login_links_but_keeps_integrations() {
    let app = App::new().await;
    let original = commoncal_backend::password::hash_password("original-account-password").unwrap();
    sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(&original)
        .bind(app.user_id)
        .execute(&app.pool)
        .await
        .unwrap();
    let session = app.key.generate_token();
    let session_hash = app.key.hash_token(TokenDomain::Session, &session);
    sqlx::query("INSERT INTO sessions(user_id, session_hash, expires_at, created_at, last_seen_at) VALUES (?, ?, ?, ?, ?)").bind(app.user_id).bind(session_hash.as_bytes().as_slice()).bind(NOW+1000).bind(NOW).bind(NOW).execute(&app.pool).await.unwrap();
    let login_token = app.key.generate_token();
    let login_hash = app.key.hash_token(TokenDomain::Login, &login_token);
    sqlx::query(
        "INSERT INTO login_tokens(user_id, token_hash, expires_at, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(app.user_id)
    .bind(login_hash.as_bytes().as_slice())
    .bind(NOW + 1000)
    .bind(NOW)
    .execute(&app.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO mcp_grant(id, user_id, oauth_client_id, created_at) VALUES ('reset-grant', ?, 'client', ?)").bind(app.user_id).bind(NOW).execute(&app.pool).await.unwrap();
    let caldav = commoncal_backend::caldav::auth::CaldavAccountService::new_at(
        app.pool.clone(),
        app.key.clone(),
        ORIGIN.parse().unwrap(),
        NOW,
    );
    let credential = caldav
        .issue_credential(app.user_id, "Keep this calendar client".into())
        .await
        .unwrap();
    let token = app.request().await;
    app.service()
        .consume_password_reset(consume(token.clone()))
        .await
        .unwrap();
    assert!(
        app.service()
            .consume_password_reset(consume(token))
            .await
            .is_err()
    );
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?")
        .bind(app.user_id)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert!(verify_password(PASSWORD, &hash).unwrap());
    assert!(!verify_password("original-account-password", &hash).unwrap());
    for table in ["sessions", "login_tokens", "password_reset_tokens"] {
        let query = format!(
            "SELECT COUNT(*) FROM {table} WHERE user_id = ? AND revoked_at IS NULL AND {}",
            if table == "sessions" {
                "1=1"
            } else {
                "consumed_at IS NULL"
            }
        );
        let count: i64 = sqlx::query_scalar(&query)
            .bind(app.user_id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
        assert_eq!(count, 0, "{table}");
    }
    let sessions = commoncal_backend::sessions::SessionManager::new_at(
        app.pool.clone(),
        app.key.clone(),
        commoncal_backend::sessions::SessionSecurityConfig::new(3600, 300, ORIGIN).unwrap(),
        NOW,
    );
    assert!(sessions.authenticate(Some(session.expose())).await.is_err());
    use commoncal_backend::login::{
        ConsumeLoginLink, FixedWindowLoginRateLimiter, LoginFlow, LoginService,
    };
    let login = LoginService::new_at(
        app.pool.clone(),
        app.key.clone(),
        900,
        3600,
        "/login",
        Arc::new(commoncal_backend::email::InMemoryEmailSender::new()),
        Arc::new(FixedWindowLoginRateLimiter::new_at(5, 900, NOW)),
        NOW,
        true,
    );
    assert!(
        login
            .consume_link(ConsumeLoginLink {
                token: login_token.expose().into(),
                prior_session_token: None
            })
            .await
            .is_err()
    );
    use base64::{Engine, engine::general_purpose::STANDARD};
    let authorization = axum::http::HeaderValue::from_str(&format!(
        "Basic {}",
        STANDARD.encode(format!(
            "member@example.test:{}",
            credential.password.expose()
        ))
    ))
    .unwrap();
    assert!(caldav.authenticate(&authorization).await.is_ok());
    let revoked: Option<i64> =
        sqlx::query_scalar("SELECT revoked_at FROM mcp_grant WHERE id = 'reset-grant'")
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(revoked, None);
}

#[tokio::test]
async fn reset_rotation_expiry_status_and_password_rules_are_enforced() {
    let app = App::new().await;
    let old = app.request().await;
    let current = app.request().await;
    assert!(
        app.service()
            .consume_password_reset(consume(old))
            .await
            .is_err()
    );
    assert!(
        app.service_at(NOW + 900)
            .consume_password_reset(consume(current.clone()))
            .await
            .is_err()
    );
    for (password, confirmation) in [
        ("short".into(), "short".into()),
        (PASSWORD.into(), "different-password".into()),
        ("é".repeat(37), "é".repeat(37)),
    ] {
        assert!(
            app.service()
                .consume_password_reset(ConsumePasswordReset {
                    token: current.clone(),
                    password,
                    password_confirmation: confirmation
                })
                .await
                .is_err()
        );
    }
    for status in ["invited", "pending", "inactive", "deleted"] {
        sqlx::query("UPDATE users SET status = ? WHERE id = ?")
            .bind(status)
            .bind(app.user_id)
            .execute(&app.pool)
            .await
            .unwrap();
        assert!(
            app.service()
                .consume_password_reset(consume(current.clone()))
                .await
                .is_err()
        );
    }
    sqlx::query("UPDATE users SET status = 'registered' WHERE id = ?")
        .bind(app.user_id)
        .execute(&app.pool)
        .await
        .unwrap();
    app.service()
        .consume_password_reset(consume(current))
        .await
        .unwrap(); // Existing passwordless account gains a password.
}

#[tokio::test]
async fn concurrent_reset_consumption_has_one_winner_and_failures_roll_back() {
    let app = App::new().await;
    let token = app.request().await;
    let service = app.service();
    let (a, b) = tokio::join!(
        service.consume_password_reset(consume(token.clone())),
        service.consume_password_reset(consume(token))
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let token = app.request().await;
    let before: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?")
        .bind(app.user_id)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER fail_reset_audit BEFORE INSERT ON audit_log WHEN NEW.action = 'account.password_reset.consume' BEGIN SELECT RAISE(ABORT, 'test audit failure'); END").execute(&app.pool).await.unwrap();
    assert!(
        service
            .consume_password_reset(ConsumePasswordReset {
                token: token.clone(),
                password: "another-reset-password".into(),
                password_confirmation: "another-reset-password".into()
            })
            .await
            .is_err()
    );
    let after: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?")
        .bind(app.user_id)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(before, after);
    let consumed: Option<i64> = sqlx::query_scalar(
        "SELECT consumed_at FROM password_reset_tokens ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(consumed, None);
}

#[tokio::test]
async fn public_origin_and_email_ip_token_limits_ignore_untrusted_forwarding() {
    let app = App::new().await;
    let router = app.router();
    let body = r#"{"email":"member@example.test"}"#.to_owned();
    assert_eq!(
        post(
            router.clone(),
            "/api/v1/auth/password-resets",
            body.clone(),
            "https://evil.test",
            "one"
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert!(app.sender.messages.lock().unwrap().is_empty());
    for _ in 0..5 {
        assert_eq!(
            post(
                router.clone(),
                "/api/v1/auth/password-resets",
                body.clone(),
                ORIGIN,
                "one"
            )
            .await
            .0,
            StatusCode::ACCEPTED
        );
    }
    assert_eq!(
        post(
            router.clone(),
            "/api/v1/auth/password-resets",
            r#"{"email":" MEMBER@example.test "}"#.into(),
            ORIGIN,
            "two"
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    let router = app.router();
    for index in 0..20 {
        assert_eq!(
            post(
                router.clone(),
                "/api/v1/auth/password-resets",
                format!(r#"{{"email":"unknown-{index}@example.test"}}"#),
                ORIGIN,
                &index.to_string()
            )
            .await
            .0,
            StatusCode::ACCEPTED
        );
    }
    assert_eq!(
        post(
            router.clone(),
            "/api/v1/auth/password-resets",
            r#"{"email":"different@example.test"}"#.into(),
            ORIGIN,
            "new"
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    let router = app.router();
    let token_body = r#"{"token":"bad-token","password":"a-long-enough-password","password_confirmation":"a-long-enough-password"}"#.to_owned();
    for index in 0..30 {
        assert_eq!(
            post(
                router.clone(),
                "/api/v1/auth/password-resets/consume",
                token_body.clone(),
                ORIGIN,
                &index.to_string()
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        post(
            router,
            "/api/v1/auth/password-resets/consume",
            token_body,
            ORIGIN,
            "new"
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn purpose_scoped_tokens_cannot_cross_invitation_and_reset_flows() {
    let app = App::new().await;
    let reset = app.request().await;
    let invitations = commoncal_backend::invitations::InvitationConsumer::new_at(
        app.pool.clone(),
        app.key.clone(),
        NOW,
    );
    assert!(invitations.preview(reset).await.is_err());
    let token = app.key.generate_token();
    let hash = app.key.hash_token(TokenDomain::Invitation, &token);
    sqlx::query("UPDATE password_reset_tokens SET revoked_at = ?")
        .bind(NOW)
        .execute(&app.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO password_reset_tokens(user_id, token_hash, expires_at, created_at) VALUES (?, ?, ?, ?)").bind(app.user_id).bind(hash.as_bytes().as_slice()).bind(NOW+900).bind(NOW).execute(&app.pool).await.unwrap();
    assert!(
        app.service()
            .consume_password_reset(consume(token.expose().into()))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn public_reset_requires_password_and_returns_no_session() {
    let app = App::new().await;
    let token = app.request().await;
    let request = serde_json::json!({ "token": token }).to_string();
    assert_eq!(
        post(
            app.router(),
            "/api/v1/auth/password-resets/consume",
            request,
            ORIGIN,
            "one"
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let request = serde_json::json!({ "token": token, "password": PASSWORD, "password_confirmation": PASSWORD }).to_string();
    let success = post(
        app.router(),
        "/api/v1/auth/password-resets/consume",
        request,
        ORIGIN,
        "one",
    )
    .await;
    assert_eq!(success, (StatusCode::NO_CONTENT, String::new()));
}

#[tokio::test]
async fn reset_page_has_no_cache_and_referrer_headers() {
    let router = commoncal_backend::http::secure_responses(
        Router::new().route(
            "/password-reset",
            axum::routing::get(|| async { "reset page" }),
        ),
        commoncal_backend::http::ResponseSecurityConfig::local_http(),
    );
    let response = router
        .oneshot(
            Request::builder()
                .uri("/password-reset?token=secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
}

#[tokio::test]
async fn email_confirmation_requires_an_explicit_valid_token() {
    let app = App::new().await;
    let (status, _) = post(
        app.router(),
        "/api/v1/auth/email-changes/consume",
        r#"{"token":"invalid"}"#.into(),
        ORIGIN,
        "127.0.0.1",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

impl App {
    async fn set_password(&self) {
        let hash = commoncal_backend::password::hash_new_password(PASSWORD.into(), PASSWORD.into())
            .await
            .unwrap();
        sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
            .bind(hash)
            .bind(self.user_id)
            .execute(&self.pool)
            .await
            .unwrap();
    }
    async fn change_email(
        &self,
        email: &str,
    ) -> Result<(), commoncal_backend::account::AccountError> {
        self.service()
            .request_email_change(
                self.user_id,
                commoncal_backend::account::RequestEmailChange {
                    email: email.into(),
                    current_password: PASSWORD.into(),
                },
            )
            .await
    }
    fn email_token(&self) -> String {
        let messages = self.sender.confirmations.lock().unwrap();
        url::Url::parse(messages.last().unwrap().authentication_link().expose())
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "token")
            .unwrap()
            .1
            .into_owned()
    }
}
#[tokio::test]
async fn email_change_keeps_old_address_until_confirmation_and_preserves_identity() {
    let app = App::new().await;
    app.set_password().await;
    app.change_email(" NEW@example.test ").await.unwrap();
    let summary = app.service().summary(app.user_id).await.unwrap();
    assert_eq!(summary.email, "member@example.test");
    assert_eq!(
        summary.pending_email_change.unwrap().email,
        "new@example.test"
    );
    let token = app.email_token();
    app.service()
        .confirm_email_change(token.clone())
        .await
        .unwrap();
    assert_eq!(
        app.service().summary(app.user_id).await.unwrap().email,
        "new@example.test"
    );
    assert!(
        app.service()
            .summary(app.user_id)
            .await
            .unwrap()
            .pending_email_change
            .is_none()
    );
    assert!(app.service().confirm_email_change(token).await.is_err());
    let notices = app.sender.notices.lock().unwrap();
    assert_eq!(notices[0].recipient(), "member@example.test");
    assert_eq!(notices[0].new_email(), "new@example.test");
}
#[tokio::test]
async fn email_change_requires_password_and_current_registered_status() {
    use commoncal_backend::account::{AccountError, RequestEmailChange};
    let app = App::new().await;
    assert!(matches!(
        app.change_email("new@example.test").await,
        Err(AccountError::PasswordRequired)
    ));
    app.set_password().await;
    assert!(matches!(
        app.service()
            .request_email_change(
                app.user_id,
                RequestEmailChange {
                    email: "new@example.test".into(),
                    current_password: "incorrect".into()
                }
            )
            .await,
        Err(AccountError::WrongPassword)
    ));
    assert!(matches!(
        app.change_email("member@example.test").await,
        Err(AccountError::InvalidInput)
    ));
    assert!(matches!(
        app.change_email("invalid").await,
        Err(AccountError::InvalidInput)
    ));
    for status in ["invited", "pending", "inactive", "deleted"] {
        sqlx::query("UPDATE users SET status = ? WHERE id = ?")
            .bind(status)
            .bind(app.user_id)
            .execute(&app.pool)
            .await
            .unwrap();
        assert!(matches!(
            app.change_email("new@example.test").await,
            Err(AccountError::Ineligible)
        ));
    }
    assert!(app.sender.confirmations.lock().unwrap().is_empty());
}
#[tokio::test]
async fn email_reservations_conflicts_expiry_rotation_and_delivery_failure() {
    use commoncal_backend::account::AccountError;
    let app = App::new().await;
    app.set_password().await;
    sqlx::query("INSERT INTO users(normalized_email, status, created_at) VALUES ('occupied@example.test', 'invited', ?)").bind(NOW).execute(&app.pool).await.unwrap();
    assert!(matches!(
        app.change_email("occupied@example.test").await,
        Err(AccountError::EmailConflict)
    ));
    app.change_email("new@example.test").await.unwrap();
    let old = app.email_token();
    app.change_email("replacement@example.test").await.unwrap();
    let current = app.email_token();
    assert!(app.service().confirm_email_change(old).await.is_err());
    assert!(
        app.service_at(NOW + 86400)
            .confirm_email_change(current.clone())
            .await
            .is_err()
    );
    app.sender.fail.store(true, Ordering::SeqCst);
    assert!(matches!(
        app.change_email("failed@example.test").await,
        Err(AccountError::DeliveryFailed)
    ));
    let live: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM email_change_requests WHERE revoked_at IS NULL AND consumed_at IS NULL").fetch_one(&app.pool).await.unwrap();
    assert_eq!(live, 0);
    app.sender.fail.store(false, Ordering::SeqCst);
    app.change_email("failed@example.test").await.unwrap();
    assert!(app.service().confirm_email_change(current).await.is_err());
}
#[tokio::test]
async fn email_confirmation_disable_conflict_and_audit_rollback_are_atomic() {
    let app = App::new().await;
    app.set_password().await;
    app.change_email("new@example.test").await.unwrap();
    let token = app.email_token();
    sqlx::query("UPDATE users SET status = 'inactive' WHERE id = ?")
        .bind(app.user_id)
        .execute(&app.pool)
        .await
        .unwrap();
    assert!(
        app.service()
            .confirm_email_change(token.clone())
            .await
            .is_err()
    );
    sqlx::query("UPDATE users SET status = 'registered' WHERE id = ?")
        .bind(app.user_id)
        .execute(&app.pool)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER fail_email_audit BEFORE INSERT ON audit_log WHEN NEW.action = 'account.email_change.consume' BEGIN SELECT RAISE(ABORT, 'test'); END").execute(&app.pool).await.unwrap();
    assert!(
        app.service()
            .confirm_email_change(token.clone())
            .await
            .is_err()
    );
    assert_eq!(
        app.service().summary(app.user_id).await.unwrap().email,
        "member@example.test"
    );
    assert!(
        app.service()
            .summary(app.user_id)
            .await
            .unwrap()
            .pending_email_change
            .is_some()
    );
    sqlx::query("DROP TRIGGER fail_email_audit")
        .execute(&app.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users(normalized_email,status,created_at) VALUES ('new@example.test','registered', ?)").bind(NOW).execute(&app.pool).await.unwrap();
    assert!(
        app.service()
            .confirm_email_change(token.clone())
            .await
            .is_err()
    );
    sqlx::query("DELETE FROM users WHERE normalized_email = 'new@example.test'")
        .execute(&app.pool)
        .await
        .unwrap();
    app.sender.fail_notice.store(true, Ordering::SeqCst);
    app.service().confirm_email_change(token).await.unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_log WHERE action = 'account.email_change.notice_failed'",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        app.service().summary(app.user_id).await.unwrap().email,
        "new@example.test"
    );
}
#[tokio::test]
async fn email_confirmation_has_one_concurrent_winner_and_is_domain_scoped() {
    let app = App::new().await;
    app.set_password().await;
    let reset = app.request().await;
    assert!(app.service().confirm_email_change(reset).await.is_err());
    app.change_email("new@example.test").await.unwrap();
    let token = app.email_token();
    assert!(
        app.service()
            .consume_password_reset(consume(token.clone()))
            .await
            .is_err()
    );
    let a = app.service();
    let b = app.service();
    let (one, two) = tokio::join!(
        a.confirm_email_change(token.clone()),
        b.confirm_email_change(token)
    );
    assert_eq!(usize::from(one.is_ok()) + usize::from(two.is_ok()), 1);
}

#[tokio::test]
async fn account_settings_requires_session_csrf_and_actor_rate_limit() {
    use commoncal_backend::sessions::{SessionManager, SessionSecurityConfig};
    let app = App::new().await;
    app.set_password().await;
    let token = app.key.generate_token();
    let hash = app.key.hash_token(TokenDomain::Session, &token);
    sqlx::query("INSERT INTO sessions(user_id,session_hash,expires_at,created_at,last_seen_at) VALUES (?, ?, ?, ?, ?)").bind(app.user_id).bind(hash.as_bytes().as_slice()).bind(NOW+3600).bind(NOW).bind(NOW).execute(&app.pool).await.unwrap();
    let manager = SessionManager::new_at(
        app.pool.clone(),
        app.key.clone(),
        SessionSecurityConfig::new(3600, 300, ORIGIN).unwrap(),
        NOW,
    );
    let router = commoncal_backend::http::build_account_settings_router(
        app.service(),
        manager.clone(),
        AccountRateLimiter::new_at(NOW),
    );
    let no_session = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/account")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(no_session.status(), StatusCode::UNAUTHORIZED);
    let cookie = format!("__Host-commoncal_session={}", token.expose());
    let csrf = app.key.generate_csrf_token(&token);
    let summary = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/account")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(summary.status(), StatusCode::OK);
    assert_eq!(summary.headers()["cache-control"], "no-store");
    for n in 0..7 {
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/v1/account/email-changes")
            .header("cookie", &cookie)
            .header("sec-fetch-site", "same-origin")
            .header("origin", ORIGIN)
            .header("content-type", "application/json");
        if n > 0 {
            request = request.header("x-csrf-token", csrf.expose());
        }
        let response = router
            .clone()
            .oneshot(
                request
                    .body(Body::from(format!(
                        r#"{{"email":"new@example.test","current_password":"{PASSWORD}"}}"#
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if n == 0 {
                StatusCode::FORBIDDEN
            } else if n == 6 {
                StatusCode::TOO_MANY_REQUESTS
            } else {
                StatusCode::NO_CONTENT
            }
        );
    }
    app.service()
        .confirm_email_change(app.email_token())
        .await
        .unwrap();
    assert!(manager.authenticate(Some(token.expose())).await.is_err());
}
#[tokio::test]
async fn competing_email_requests_reserve_one_address_and_expired_reservation_releases() {
    use commoncal_backend::account::{AccountError, RequestEmailChange};
    let app = App::new().await;
    app.set_password().await;
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?")
        .bind(app.user_id)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    let other = sqlx::query("INSERT INTO users(normalized_email,status,password_hash,created_at) VALUES ('other@example.test','registered',?,?)").bind(hash).bind(NOW).execute(&app.pool).await.unwrap().last_insert_rowid();
    let a = app.service();
    let b = app.service();
    let command = || RequestEmailChange {
        email: "reserved@example.test".into(),
        current_password: PASSWORD.into(),
    };
    let (first, second) = tokio::join!(
        a.request_email_change(app.user_id, command()),
        b.request_email_change(other, command())
    );
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    assert!(matches!(
        if first.is_ok() { second } else { first },
        Err(AccountError::EmailConflict)
    ));
    let later = app.service_at(NOW + 86400);
    later.request_email_change(other, command()).await.unwrap();
    assert_eq!(
        later
            .summary(other)
            .await
            .unwrap()
            .pending_email_change
            .unwrap()
            .email,
        "reserved@example.test"
    );
}
#[tokio::test]
async fn email_change_revokes_account_tokens_while_preserving_integration_credentials() {
    let app = App::new().await;
    app.set_password().await;
    app.request().await;
    let login = app.key.generate_token();
    let login_hash = app.key.hash_token(TokenDomain::Login, &login);
    sqlx::query(
        "INSERT INTO login_tokens(user_id,token_hash,expires_at,created_at) VALUES (?,?,?,?)",
    )
    .bind(app.user_id)
    .bind(login_hash.as_bytes().as_slice())
    .bind(NOW + 1000)
    .bind(NOW)
    .execute(&app.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO mcp_grant(id,user_id,oauth_client_id,created_at) VALUES ('email-grant',?,'client',?)").bind(app.user_id).bind(NOW).execute(&app.pool).await.unwrap();
    let caldav = commoncal_backend::caldav::auth::CaldavAccountService::new_at(
        app.pool.clone(),
        app.key.clone(),
        ORIGIN.parse().unwrap(),
        NOW,
    );
    let credential = caldav
        .issue_credential(app.user_id, "Retained email client".into())
        .await
        .unwrap();
    app.change_email("new@example.test").await.unwrap();
    app.service()
        .confirm_email_change(app.email_token())
        .await
        .unwrap();
    for table in ["login_tokens", "password_reset_tokens"] {
        let query = format!(
            "SELECT COUNT(*) FROM {table} WHERE user_id = ? AND revoked_at IS NULL AND consumed_at IS NULL"
        );
        let count: i64 = sqlx::query_scalar(&query)
            .bind(app.user_id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM mcp_grant WHERE user_id = ? AND revoked_at IS NULL",
    )
    .bind(app.user_id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    use base64::{Engine, engine::general_purpose::STANDARD};
    let authorization = axum::http::HeaderValue::from_str(&format!(
        "Basic {}",
        STANDARD.encode(format!("new@example.test:{}", credential.password.expose()))
    ))
    .unwrap();
    assert!(caldav.authenticate(&authorization).await.is_ok());
}

#[tokio::test]
async fn email_confirmation_origin_limit_and_sensitive_headers_are_enforced() {
    let app = App::new().await;
    let router = app.router();
    let path = "/api/v1/auth/email-changes/consume";
    assert_eq!(
        post(
            router.clone(),
            path,
            r#"{"token":"invalid"}"#.into(),
            "https://evil.test",
            "one"
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    for attempt in 0..31 {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("origin", ORIGIN)
                    .header("content-type", "application/json")
                    .extension(ConnectInfo("127.0.0.1:2000".parse::<SocketAddr>().unwrap()))
                    .body(Body::from(r#"{"token":"invalid"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if attempt < 30 {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::TOO_MANY_REQUESTS
            }
        );
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.headers()["referrer-policy"], "no-referrer");
        assert!(!response.headers().contains_key("set-cookie"));
    }
    let html = commoncal_backend::http::secure_responses(
        Router::new().route(
            "/email/confirm",
            axum::routing::get(|| async { "confirmation" }),
        ),
        commoncal_backend::http::ResponseSecurityConfig::local_http(),
    );
    let response = html
        .oneshot(
            Request::builder()
                .uri("/email/confirm?token=secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
}

#[tokio::test]
async fn admin_email_change_requires_authentication() {
    use commoncal_backend::sessions::{SessionManager, SessionSecurityConfig};
    let app = App::new().await;
    let manager = SessionManager::new_at(
        app.pool.clone(),
        app.key.clone(),
        SessionSecurityConfig::new(3600, 300, ORIGIN).unwrap(),
        NOW,
    );
    let router = commoncal_backend::http::build_account_admin_router(
        app.service(),
        commoncal_backend::admin::AdminService::new_at(
            app.pool.clone(),
            app.key.clone(),
            86400,
            NOW,
        ),
        manager,
        AccountRateLimiter::new_at(NOW),
    );
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/admin/users/{}/email-changes", app.user_id))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"email":"new@example.test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

impl App {
    async fn admin_id(&self) -> i64 {
        sqlx::query_scalar("SELECT id FROM users WHERE normalized_email = 'admin@localhost'")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
    fn admin_service(&self) -> commoncal_backend::admin::AdminService {
        commoncal_backend::admin::AdminService::with_email_sender(
            self.pool.clone(),
            self.key.clone(),
            86400,
            format!("{ORIGIN}/invitations/accept"),
            self.sender.clone(),
        )
    }
    async fn invitee(&self, email: &str) -> (i64, String) {
        let invitation = self
            .admin_service()
            .invite(
                self.admin_id().await,
                commoncal_backend::admin::InviteUser {
                    email: email.into(),
                    display_name: Some("Retained display name".into()),
                },
            )
            .await
            .unwrap();
        let user: i64 = sqlx::query_scalar("SELECT id FROM users WHERE normalized_email = ?")
            .bind(email)
            .fetch_one(&self.pool)
            .await
            .unwrap();
        (user, invitation.token.expose().into())
    }
    fn invitation_token(&self) -> String {
        let messages = self.sender.invitations.lock().unwrap();
        url::Url::parse(messages.last().unwrap().authentication_link().expose())
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "token")
            .unwrap()
            .1
            .into_owned()
    }
}
#[tokio::test]
async fn admin_registered_email_uses_confirmation_and_attributes_the_request() {
    let app = App::new().await;
    let actor = app.admin_id().await;
    // Admin can initiate verification for a passwordless registered account.
    app.service()
        .request_admin_email_change(actor, app.user_id, "new@example.test".into())
        .await
        .unwrap();
    assert_eq!(
        app.service().summary(app.user_id).await.unwrap().email,
        "member@example.test"
    );
    let stored_actor: i64 = sqlx::query_scalar(
        "SELECT actor_user_id FROM email_change_requests WHERE user_id = ? AND revoked_at IS NULL",
    )
    .bind(app.user_id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(stored_actor, actor);
    let audited_actor: i64 = sqlx::query_scalar(
        "SELECT actor_user_id FROM audit_log WHERE action = 'account.email_change.request'",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(audited_actor, actor);
    let listing = commoncal_backend::admin::AdminService::new_at(
        app.pool.clone(),
        app.key.clone(),
        86400,
        NOW,
    )
    .list_users(Some("registered"), 1, 20)
    .await
    .unwrap();
    let pending = &listing
        .users
        .iter()
        .find(|u| u.id == app.user_id)
        .unwrap()
        .pending_email_change;
    assert_eq!(pending.as_ref().unwrap().email, "new@example.test");
    app.service()
        .confirm_email_change(app.email_token())
        .await
        .unwrap();
    assert_eq!(
        app.service().summary(app.user_id).await.unwrap().email,
        "new@example.test"
    );
    assert_eq!(
        app.sender.notices.lock().unwrap()[0].recipient(),
        "member@example.test"
    );
}
#[tokio::test]
async fn admin_registered_email_denies_stale_actors_and_ineligible_targets_and_recovers_delivery() {
    use commoncal_backend::account::AccountError;
    let app = App::new().await;
    let actor = app.admin_id().await;
    assert!(matches!(
        app.service()
            .request_admin_email_change(app.user_id, app.user_id, "new@example.test".into())
            .await,
        Err(AccountError::Forbidden)
    ));
    for (status, admin) in [("inactive", 1), ("registered", 0)] {
        sqlx::query("UPDATE users SET status = ?, is_superadmin = ? WHERE id = ?")
            .bind(status)
            .bind(admin)
            .bind(actor)
            .execute(&app.pool)
            .await
            .unwrap();
        assert!(matches!(
            app.service()
                .request_admin_email_change(actor, app.user_id, "new@example.test".into())
                .await,
            Err(AccountError::Forbidden)
        ));
    }
    sqlx::query("UPDATE users SET status = 'registered',is_superadmin = 1 WHERE id = ?")
        .bind(actor)
        .execute(&app.pool)
        .await
        .unwrap();
    for status in ["invited", "pending", "inactive", "deleted"] {
        sqlx::query("UPDATE users SET status = ? WHERE id = ?")
            .bind(status)
            .bind(app.user_id)
            .execute(&app.pool)
            .await
            .unwrap();
        assert!(matches!(
            app.service()
                .request_admin_email_change(actor, app.user_id, "new@example.test".into())
                .await,
            Err(AccountError::Ineligible)
        ));
    }
    sqlx::query("UPDATE users SET status = 'registered' WHERE id = ?")
        .bind(app.user_id)
        .execute(&app.pool)
        .await
        .unwrap();
    app.sender.fail.store(true, Ordering::SeqCst);
    assert!(matches!(
        app.service()
            .request_admin_email_change(actor, app.user_id, "new@example.test".into())
            .await,
        Err(AccountError::DeliveryFailed)
    ));
    assert!(
        app.service()
            .summary(app.user_id)
            .await
            .unwrap()
            .pending_email_change
            .is_none()
    );
    app.sender.fail.store(false, Ordering::SeqCst);
    app.service()
        .request_admin_email_change(actor, app.user_id, "new@example.test".into())
        .await
        .unwrap();
}
#[tokio::test]
async fn admin_invited_email_replaces_link_and_preserves_id_display_name_and_bootstrap_role() {
    let app = App::new().await;
    let actor = app.admin_id().await;
    let (id, old) = app.invitee("invited@example.test").await;
    sqlx::query("UPDATE invitations SET platform_role = 'superadmin' WHERE normalized_email = 'invited@example.test'").execute(&app.pool).await.unwrap();
    app.admin_service()
        .change_invited_email(actor, id, " NEW@example.test ".into())
        .await
        .unwrap();
    let consumer = commoncal_backend::invitations::InvitationConsumer::new_at(
        app.pool.clone(),
        app.key.clone(),
        NOW,
    );
    assert!(consumer.preview(old).await.is_err());
    let token = app.invitation_token();
    assert_eq!(
        consumer.preview(token.clone()).await.unwrap().email,
        "new@example.test"
    );
    let accepted = consumer
        .consume(commoncal_backend::invitations::ConsumeInvitation {
            token,
            password: PASSWORD.into(),
            password_confirmation: PASSWORD.into(),
        })
        .await
        .unwrap();
    assert_eq!(accepted.user.id, id);
    assert!(accepted.user.is_superadmin);
    let display: Option<String> = sqlx::query_scalar("SELECT display_name FROM users WHERE id = ?")
        .bind(id)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(display.as_deref(), Some("Retained display name"));
    assert_eq!(
        app.sender
            .invitations
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .recipient(),
        "new@example.test"
    );
}
#[tokio::test]
async fn invited_email_delivery_failure_retains_updated_account_for_resend() {
    use commoncal_backend::admin::AdminError;
    let app = App::new().await;
    let actor = app.admin_id().await;
    let (id, old) = app.invitee("invited@example.test").await;
    app.sender.fail.store(true, Ordering::SeqCst);
    assert!(matches!(
        app.admin_service()
            .change_invited_email(actor, id, "new@example.test".into())
            .await,
        Err(AdminError::DeliveryFailed)
    ));
    let users = app
        .admin_service()
        .list_users(Some("invited"), 1, 20)
        .await
        .unwrap();
    let user = users.users.iter().find(|u| u.id == id).unwrap();
    assert_eq!(user.email, "new@example.test");
    let revoked: Option<i64> =
        sqlx::query_scalar("SELECT revoked_at FROM invitations WHERE id = ?")
            .bind(user.invitation_id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert!(revoked.is_some());
    let consumer = commoncal_backend::invitations::InvitationConsumer::new_at(
        app.pool.clone(),
        app.key.clone(),
        NOW,
    );
    assert!(consumer.preview(old).await.is_err());
    app.sender.fail.store(false, Ordering::SeqCst);
    app.admin_service()
        .resend_invitation(actor, user.invitation_id.unwrap())
        .await
        .unwrap();
    assert_eq!(
        consumer
            .preview(app.invitation_token())
            .await
            .unwrap()
            .email,
        "new@example.test"
    );
}
#[tokio::test]
async fn invited_email_rejects_nonadmin_status_and_conflicts_and_audit_failure_rolls_back() {
    use commoncal_backend::admin::AdminError;
    let app = App::new().await;
    let actor = app.admin_id().await;
    let (id, old) = app.invitee("invited@example.test").await;
    assert!(matches!(
        app.admin_service()
            .change_invited_email(app.user_id, id, "new@example.test".into())
            .await,
        Err(AdminError::Forbidden)
    ));
    assert!(matches!(
        app.admin_service()
            .change_invited_email(actor, id, "member@example.test".into())
            .await,
        Err(AdminError::Conflict)
    ));
    assert!(matches!(
        app.admin_service()
            .change_invited_email(actor, id, "invited@example.test".into())
            .await,
        Err(AdminError::InvalidInput)
    ));
    sqlx::query("CREATE TRIGGER fail_invited_email_audit BEFORE INSERT ON audit_log WHEN NEW.action = 'admin.user.email_change' BEGIN SELECT RAISE(ABORT,'test'); END").execute(&app.pool).await.unwrap();
    assert!(
        app.admin_service()
            .change_invited_email(actor, id, "new@example.test".into())
            .await
            .is_err()
    );
    let consumer = commoncal_backend::invitations::InvitationConsumer::new_at(
        app.pool.clone(),
        app.key.clone(),
        NOW,
    );
    assert!(consumer.preview(old).await.is_ok());
    sqlx::query("DROP TRIGGER fail_invited_email_audit")
        .execute(&app.pool)
        .await
        .unwrap();
    for status in ["registered", "pending", "inactive", "deleted"] {
        sqlx::query("UPDATE users SET status = ? WHERE id = ?")
            .bind(status)
            .bind(id)
            .execute(&app.pool)
            .await
            .unwrap();
        assert!(matches!(
            app.admin_service()
                .change_invited_email(actor, id, "new@example.test".into())
                .await,
            Err(AdminError::Ineligible)
        ));
    }
}

impl App {
    async fn session_cookie(&self, user_id: i64) -> (String, String) {
        let token = self.key.generate_token();
        let hash = self.key.hash_token(TokenDomain::Session, &token);
        sqlx::query("INSERT INTO sessions(user_id,session_hash,expires_at,created_at,last_seen_at) VALUES (?,?,?,?,?)").bind(user_id).bind(hash.as_bytes().as_slice()).bind(NOW+3600).bind(NOW).bind(NOW).execute(&self.pool).await.unwrap();
        (
            format!("__Host-commoncal_session={}", token.expose()),
            self.key.generate_csrf_token(&token).expose().into(),
        )
    }
    fn admin_router(&self) -> Router {
        let manager = commoncal_backend::sessions::SessionManager::new_at(
            self.pool.clone(),
            self.key.clone(),
            commoncal_backend::sessions::SessionSecurityConfig::new(3600, 300, ORIGIN).unwrap(),
            NOW,
        );
        commoncal_backend::http::build_account_admin_router(
            self.service(),
            self.admin_service(),
            manager,
            AccountRateLimiter::new_at(NOW),
        )
    }
}
async fn admin_email_post(
    router: Router,
    user_id: i64,
    email: &str,
    cookie: &str,
    csrf: Option<&str>,
) -> axum::response::Response {
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/admin/users/{user_id}/email-changes"))
        .header("content-type", "application/json")
        .header("cookie", cookie)
        .header("origin", ORIGIN)
        .header("sec-fetch-site", "same-origin");
    if let Some(csrf) = csrf {
        builder = builder.header("x-csrf-token", csrf);
    }
    router
        .oneshot(
            builder
                .body(Body::from(serde_json::json!({"email":email}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}
#[tokio::test]
async fn admin_email_http_enforces_admin_csrf_status_and_both_success_paths() {
    let app = App::new().await;
    let actor = app.admin_id().await;
    let (admin_cookie, admin_csrf) = app.session_cookie(actor).await;
    let (member_cookie, member_csrf) = app.session_cookie(app.user_id).await;
    let router = app.admin_router();
    assert_eq!(
        admin_email_post(
            router.clone(),
            app.user_id,
            "new@example.test",
            &member_cookie,
            Some(&member_csrf)
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        admin_email_post(
            router.clone(),
            app.user_id,
            "new@example.test",
            &admin_cookie,
            None
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    for status in ["pending", "inactive"] {
        sqlx::query("UPDATE users SET status = ? WHERE id = ?")
            .bind(status)
            .bind(app.user_id)
            .execute(&app.pool)
            .await
            .unwrap();
        assert_eq!(
            admin_email_post(
                router.clone(),
                app.user_id,
                "new@example.test",
                &admin_cookie,
                Some(&admin_csrf)
            )
            .await
            .status(),
            StatusCode::CONFLICT
        );
    }
    sqlx::query("UPDATE users SET status = 'registered' WHERE id = ?")
        .bind(app.user_id)
        .execute(&app.pool)
        .await
        .unwrap();
    let registered = admin_email_post(
        router.clone(),
        app.user_id,
        "new@example.test",
        &admin_cookie,
        Some(&admin_csrf),
    )
    .await;
    assert_eq!(registered.status(), StatusCode::NO_CONTENT);
    assert!(!registered.headers().contains_key("set-cookie"));
    let (invited, _) = app.invitee("invited@example.test").await;
    assert_eq!(
        admin_email_post(
            router.clone(),
            invited,
            "replacement@example.test",
            &admin_cookie,
            Some(&admin_csrf)
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.admin_service()
            .list_users(Some("invited"), 1, 20)
            .await
            .unwrap()
            .users[0]
            .id,
        invited
    );
}
#[tokio::test]
async fn invited_email_respects_reservations_and_concurrent_claims() {
    use commoncal_backend::admin::AdminError;
    let app = App::new().await;
    let actor = app.admin_id().await;
    let (first, _) = app.invitee("first@example.test").await;
    let (second, _) = app.invitee("second@example.test").await;
    // Use the same real clock as invitation mutations so the reservation is live.
    let now = chrono::Utc::now().timestamp();
    sqlx::query("INSERT INTO email_change_requests(user_id, normalized_new_email, token_hash, expires_at, actor_user_id, created_at) VALUES (?, 'reserved@example.test', X'1234', ?, ?, ?)").bind(app.user_id).bind(now+86400).bind(app.user_id).bind(now).execute(&app.pool).await.unwrap();
    assert!(matches!(
        app.admin_service()
            .change_invited_email(actor, first, "reserved@example.test".into())
            .await,
        Err(AdminError::Conflict)
    ));
    let a = app.admin_service();
    let b = app.admin_service();
    let (one, two) = tokio::join!(
        a.change_invited_email(actor, first, "claimed@example.test".into()),
        b.change_invited_email(actor, second, "claimed@example.test".into())
    );
    assert_eq!(usize::from(one.is_ok()) + usize::from(two.is_ok()), 1);
    let loser = if one.is_ok() { second } else { first };
    sqlx::query("UPDATE email_change_requests SET expires_at = ? WHERE normalized_new_email = 'reserved@example.test'").bind(now-1).execute(&app.pool).await.unwrap();
    app.admin_service()
        .change_invited_email(actor, loser, "reserved@example.test".into())
        .await
        .unwrap();
    sqlx::query("UPDATE users SET is_superadmin = 0 WHERE id = ?")
        .bind(actor)
        .execute(&app.pool)
        .await
        .unwrap();
    assert!(matches!(
        app.admin_service()
            .change_invited_email(actor, loser, "another@example.test".into())
            .await,
        Err(AdminError::Forbidden)
    ));
}
