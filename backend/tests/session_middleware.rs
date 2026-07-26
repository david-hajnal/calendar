use axum::{
    body::Body,
    http::{
        Method, Request, StatusCode,
        header::{COOKIE, ORIGIN},
    },
};
use commoncal_backend::{
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    http::{Readiness, build_router_with_sessions},
    security::{SecretKey, TokenDomain},
    sessions::{SessionManager, SessionSecurityConfig},
};
use http_body_util::BodyExt;
use sqlx::SqlitePool;
use tempfile::TempDir;
use tower::ServiceExt;

const NOW: i64 = 10_000;
const IDLE_TIMEOUT: i64 = 300;
const WRITE_THROTTLE: i64 = 60;
const ORIGIN_URL: &str = "https://commoncal.test";
const COOKIE_NAME: &str = "__Host-commoncal_session";

struct TestApplication {
    _temp_dir: TempDir,
    pool: SqlitePool,
    key: SecretKey,
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
        let user_id = sqlx::query(
            "INSERT INTO users (
                normalized_email, display_name, status, created_at, is_superadmin
             ) VALUES (?, ?, 'active', ?, 0)",
        )
        .bind("member@example.com")
        .bind("Member")
        .bind(NOW - 1_000)
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
        assert!(user_id > 0);
        Self {
            _temp_dir: temp_dir,
            pool,
            key: SecretKey::new([42; 32]),
        }
    }

    fn router(&self) -> axum::Router {
        build_router_with_sessions(
            Readiness::new(),
            SessionManager::new_at(
                self.pool.clone(),
                self.key.clone(),
                SessionSecurityConfig::new(IDLE_TIMEOUT, WRITE_THROTTLE, ORIGIN_URL).unwrap(),
                NOW,
            ),
        )
    }

    async fn session(
        &self,
        created_at: i64,
        last_seen_at: i64,
        expires_at: i64,
        revoked_at: Option<i64>,
    ) -> String {
        let token = self.key.generate_token();
        let hash = self.key.hash_token(TokenDomain::Session, &token);
        sqlx::query(
            "INSERT INTO sessions (
                user_id, session_hash, expires_at, revoked_at, created_at, last_seen_at
             ) VALUES (1, ?, ?, ?, ?, ?)",
        )
        .bind(hash.as_bytes().as_slice())
        .bind(expires_at)
        .bind(revoked_at)
        .bind(created_at)
        .bind(last_seen_at)
        .execute(&self.pool)
        .await
        .unwrap();
        token.expose().to_owned()
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        csrf: Option<&str>,
        origin: Option<&str>,
        fetch_site: Option<&str>,
    ) -> axum::response::Response {
        let mut request = Request::builder().method(method).uri(path);
        if let Some(token) = token {
            request = request.header(COOKIE, format!("{COOKIE_NAME}={token}"));
        }
        if let Some(csrf) = csrf {
            request = request.header("x-csrf-token", csrf);
        }
        if let Some(origin) = origin {
            request = request.header(ORIGIN, origin);
        }
        if let Some(fetch_site) = fetch_site {
            request = request.header("sec-fetch-site", fetch_site);
        }
        self.router()
            .oneshot(request.body(Body::empty()).unwrap())
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
async fn valid_session_authenticates() {
    let application = TestApplication::new().await;
    let token = application
        .session(NOW - 1_000, NOW - WRITE_THROTTLE, NOW + 1_000, None)
        .await;

    let response = application
        .request(
            Method::GET,
            "/api/v1/auth/session",
            Some(&token),
            None,
            None,
            None,
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_body(response).await;
    assert!(body.contains(r#""email":"member@example.com""#));
    assert!(body.contains(r#""last_seen_at":10000"#));
    let stored: i64 = sqlx::query_scalar("SELECT last_seen_at FROM sessions")
        .fetch_one(&application.pool)
        .await
        .unwrap();
    assert_eq!(stored, NOW);
}

#[tokio::test]
async fn revoked_idle_expired_and_absolute_expired_sessions_fail() {
    let application = TestApplication::new().await;
    let cases = [
        (NOW - 10, NOW - 10, NOW + 100, Some(NOW - 1)),
        (NOW - 1_000, NOW - IDLE_TIMEOUT, NOW + 100, None),
        (NOW - 1_000, NOW - 10, NOW, None),
    ];

    for (created_at, last_seen_at, expires_at, revoked_at) in cases {
        let token = application
            .session(created_at, last_seen_at, expires_at, revoked_at)
            .await;
        let response = application
            .request(
                Method::GET,
                "/api/v1/auth/session",
                Some(&token),
                None,
                None,
                None,
            )
            .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn unsafe_request_without_csrf_fails() {
    let application = TestApplication::new().await;
    let token = application
        .session(NOW - 100, NOW - 10, NOW + 1_000, None)
        .await;

    let response = application
        .request(
            Method::DELETE,
            "/api/v1/auth/session",
            Some(&token),
            None,
            Some(ORIGIN_URL),
            Some("same-origin"),
        )
        .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn wrong_session_csrf_fails() {
    let application = TestApplication::new().await;
    let token = application
        .session(NOW - 100, NOW - 10, NOW + 1_000, None)
        .await;
    let other = application.key.generate_token();
    let wrong_csrf = application.key.generate_csrf_token(&other);

    let response = application
        .request(
            Method::DELETE,
            "/api/v1/auth/session",
            Some(&token),
            Some(wrong_csrf.expose()),
            Some(ORIGIN_URL),
            Some("same-origin"),
        )
        .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn cross_site_unsafe_request_fails() {
    let application = TestApplication::new().await;
    let token = application
        .session(NOW - 100, NOW - 10, NOW + 1_000, None)
        .await;
    let parsed = commoncal_backend::security::SecretToken::parse(token.clone()).unwrap();
    let csrf = application.key.generate_csrf_token(&parsed);

    let response = application
        .request(
            Method::DELETE,
            "/api/v1/auth/session",
            Some(&token),
            Some(csrf.expose()),
            Some("https://attacker.test"),
            Some("cross-site"),
        )
        .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn safe_get_does_not_require_csrf() {
    let application = TestApplication::new().await;
    let token = application
        .session(NOW - 100, NOW - 10, NOW + 1_000, None)
        .await;

    let response = application
        .request(
            Method::GET,
            "/api/v1/auth/session",
            Some(&token),
            None,
            None,
            Some("cross-site"),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn all_session_logout_revokes_every_session() {
    let application = TestApplication::new().await;
    let current = application
        .session(NOW - 100, NOW - 10, NOW + 1_000, None)
        .await;
    application
        .session(NOW - 200, NOW - 20, NOW + 1_000, None)
        .await;
    let parsed = commoncal_backend::security::SecretToken::parse(current.clone()).unwrap();
    let csrf = application.key.generate_csrf_token(&parsed);

    let response = application
        .request(
            Method::DELETE,
            "/api/v1/auth/sessions",
            Some(&current),
            Some(csrf.expose()),
            Some(ORIGIN_URL),
            Some("same-origin"),
        )
        .await;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        response.headers()["set-cookie"],
        format!("{COOKIE_NAME}=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Lax")
    );
    let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE revoked_at IS NULL")
        .fetch_one(&application.pool)
        .await
        .unwrap();
    assert_eq!(active, 0);
}
