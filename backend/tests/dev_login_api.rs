use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header::LOCATION, header::SET_COOKIE},
};
use commoncal_backend::{
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    http::{Readiness, build_router_with_auth_flows_and_sessions},
    identity::{IdentityRepository, NewUser, UserStatus},
    login::{AllowAllLoginRateLimiter, LoginService},
    security::SecretKey,
    sessions::{SessionManager, SessionSecurityConfig},
};
use http_body_util::BodyExt;
use sqlx::{Row, SqlitePool};
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

const NOW: i64 = 1_750_000_000;
const ORIGIN: &str = "https://commoncal.test";

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
        Self {
            _temp_dir: temp_dir,
            pool,
            key: SecretKey::new([86; 32]),
        }
    }

    fn router(&self) -> axum::Router {
        unsafe {
            std::env::set_var("APP_ENV", "development");
        }
        build_router_with_auth_flows_and_sessions(
            Readiness::new(),
            commoncal_backend::invitations::InvitationConsumer::new_at(
                self.pool.clone(),
                self.key.clone(),
                300,
                NOW,
            ),
            LoginService::new_at(
                self.pool.clone(),
                self.key.clone(),
                300,
                300,
                "/login",
                Arc::new(commoncal_backend::email::DevelopmentEmailSender::new()),
                Arc::new(AllowAllLoginRateLimiter),
                NOW,
                false,
            ),
            SessionManager::new_at(
                self.pool.clone(),
                self.key.clone(),
                SessionSecurityConfig::new(300, 60, ORIGIN).unwrap(),
                NOW,
            ),
        )
    }

    async fn request(&self, path: &str) -> axum::response::Response {
        self.router()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
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
async fn dev_login_creates_user_and_redirects_when_user_does_not_exist() {
    let app = TestApplication::new().await;

    let response = app
        .request("/api/v1/dev/login?email=New.User%40Example.com&display_name=NewUser")
        .await;

    assert_eq!(response.status(), StatusCode::FOUND);
    let location = response.headers().get(LOCATION).unwrap().to_str().unwrap();
    assert!(location.contains("/dev-login"));
    assert!(location.contains("csrf_token="));
    let set_cookie = response
        .headers()
        .get(SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(set_cookie.contains("__Host-commoncal_session="));
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("SameSite=Lax"));
    let user = sqlx::query("SELECT normalized_email, display_name, status FROM users ORDER BY id DESC LIMIT 1")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(
        user.get::<String, _>("normalized_email"),
        "new.user@example.com"
    );
    assert_eq!(
        user.get::<Option<String>, _>("display_name"),
        Some("NewUser".to_owned())
    );
    assert_eq!(user.get::<String, _>("status"), "active");
}

#[tokio::test]
async fn dev_login_redirects_existing_user_without_recreating() {
    let app = TestApplication::new().await;
    IdentityRepository::new(app.pool.clone())
        .create_user(NewUser {
            normalized_email: "existing@example.com".to_owned(),
            display_name: Some("Existing".to_owned()),
            status: UserStatus::Active,
            created_at: NOW - 1_000,
        })
        .await
        .unwrap();

    let response = app
        .request("/api/v1/dev/login?email=existing%40example.com")
        .await;

    assert_eq!(response.status(), StatusCode::FOUND);
    let location = response.headers().get(LOCATION).unwrap().to_str().unwrap();
    assert!(location.contains("/dev-login"));
    assert!(location.contains("csrf_token="));
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE normalized_email = ?")
        .bind("existing@example.com")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn dev_login_requires_email_parameter() {
    let app = TestApplication::new().await;

    let response = app.request("/api/v1/dev/login").await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn dev_login_normalizes_email() {
    let app = TestApplication::new().await;

    app.request("/api/v1/dev/login?email=UPPER%40Example.COM")
        .await;

    let normalized: String = sqlx::query_scalar("SELECT normalized_email FROM users ORDER BY id DESC LIMIT 1")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(normalized, "upper@example.com");
}

#[tokio::test]
async fn dev_login_session_has_correct_expires_and_last_seen() {
    let app = TestApplication::new().await;

    app.request("/api/v1/dev/login?email=session%40test.com")
        .await;

    let session: (i64, i64) =
        sqlx::query_as("SELECT expires_at, last_seen_at FROM sessions ORDER BY id DESC LIMIT 1")
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert!(session.0 > NOW);
    assert_eq!(session.1, NOW);
}
