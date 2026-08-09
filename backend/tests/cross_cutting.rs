use axum::{
    body::Body,
    http::{
        Method, Request, StatusCode,
        header::{COOKIE, ORIGIN},
    },
};
use commoncal_backend::{
    calendar::{CalendarRepository, CalendarService, NewCalendar},
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    http::{Readiness, build_router_with_calendars},
    security::{SecretKey, TokenDomain},
    sessions::{SessionManager, SessionSecurityConfig},
};
use http_body_util::BodyExt;
use sqlx::SqlitePool;
use tempfile::TempDir;
use tower::ServiceExt;

const NOW: i64 = 1_750_000_000;
const TEST_ORIGIN: &str = "https://commoncal.test";

struct TestApplication {
    _temp_dir: TempDir,
    pool: SqlitePool,
    key: SecretKey,
    user_id: i64,
    calendar_id: i64,
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
        let user_id = insert_user(&pool, "user@example.com").await;
        let repository = CalendarRepository::new(pool.clone());
        let calendar_id = repository
            .create_calendar(
                user_id,
                NewCalendar {
                    name: "Test calendar".to_owned(),
                    description: None,
                    color: "#3367d6".to_owned(),
                    default_timezone: "UTC".to_owned(),
                    default_event_visibility: "private".to_owned(),
                    default_notification_rules_json: None,
                    created_at: NOW - 50,
                },
            )
            .await
            .unwrap()
            .id;
        Self {
            _temp_dir: temp_dir,
            pool,
            key: SecretKey::new([87; 32]),
            user_id,
            calendar_id,
        }
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        origin: &str,
        body: String,
    ) -> axum::response::Response {
        let token = self.key.generate_token();
        let hash = self.key.hash_token(TokenDomain::Session, &token);
        sqlx::query(
            "INSERT INTO sessions (
                user_id, session_hash, expires_at, revoked_at, created_at, last_seen_at
             ) VALUES (?, ?, ?, NULL, ?, ?)",
        )
        .bind(self.user_id)
        .bind(hash.as_bytes().as_slice())
        .bind(NOW + 1_000)
        .bind(NOW - 10)
        .bind(NOW - 10)
        .execute(&self.pool)
        .await
        .unwrap();
        let csrf = self.key.generate_csrf_token(&token);
        build_router_with_calendars(
            Readiness::new(),
            SessionManager::new_at(
                self.pool.clone(),
                self.key.clone(),
                SessionSecurityConfig::new(300, 60, TEST_ORIGIN).unwrap(),
                NOW,
            ),
            CalendarService::new_at(self.pool.clone(), NOW),
        )
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(
                    COOKIE,
                    format!("__Host-commoncal_session={}", token.expose()),
                )
                .header("content-type", "application/json")
                .header(ORIGIN, origin)
                .header("sec-fetch-site", "same-origin")
                .header("x-csrf-token", csrf.expose())
                .body(Body::from(body))
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

async fn insert_user(pool: &SqlitePool, email: &str) -> i64 {
    sqlx::query(
        "INSERT INTO users (
            normalized_email, display_name, status, is_superadmin, created_at
         ) VALUES (?, NULL, 'active', 0, ?)",
    )
    .bind(email)
    .bind(NOW - 100)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid()
}

// --- CSRF ---

#[tokio::test]
async fn missing_csrf_token_returns_403() {
    let app = TestApplication::new().await;
    let token = app.key.generate_token();
    let hash = app.key.hash_token(TokenDomain::Session, &token);
    sqlx::query(
        "INSERT INTO sessions (
            user_id, session_hash, expires_at, revoked_at, created_at, last_seen_at
         ) VALUES (?, ?, ?, NULL, ?, ?)",
    )
    .bind(app.user_id)
    .bind(hash.as_bytes().as_slice())
    .bind(NOW + 1_000)
    .bind(NOW - 10)
    .bind(NOW - 10)
    .execute(&app.pool)
    .await
    .unwrap();
    let router = build_router_with_calendars(
        Readiness::new(),
        SessionManager::new_at(
            app.pool.clone(),
            app.key.clone(),
            SessionSecurityConfig::new(300, 60, TEST_ORIGIN).unwrap(),
            NOW,
        ),
        CalendarService::new_at(app.pool.clone(), NOW),
    );
    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/calendars")
                .header(
                    COOKIE,
                    format!("__Host-commoncal_session={}", token.expose()),
                )
                .header("content-type", "application/json")
                .header(ORIGIN, TEST_ORIGIN)
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(""))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn wrong_csrf_token_returns_403() {
    let app = TestApplication::new().await;

    let response = app
        .request(
            Method::POST,
            "/api/v1/calendars",
            "x-wrong-token",
            String::new(),
        )
        .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn csrf_validation_ignores_origin_header() {
    let app = TestApplication::new().await;

    let response = app
        .request(
            Method::GET,
            "/api/v1/calendars",
            "https://evil.com",
            String::new(),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn get_requests_skip_csrf_check() {
    let app = TestApplication::new().await;

    let response = app
        .request(Method::GET, "/api/v1/calendars", "", String::new())
        .await;

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn post_requests_require_valid_csrf() {
    let app = TestApplication::new().await;

    let response = app
        .request(
            Method::POST,
            "/api/v1/calendars",
            TEST_ORIGIN,
            r#"{"name":"test","color":"000000","default_timezone":"UTC","default_event_visibility":"public"}"#
                .to_string(),
        )
        .await;

    assert_eq!(response.status(), StatusCode::CREATED);
}

// --- CORS ---

#[tokio::test]
async fn cors_headers_only_for_preflight() {
    let app = TestApplication::new().await;

    let response = app
        .request(
            Method::OPTIONS,
            "/api/v1/calendars",
            TEST_ORIGIN,
            String::new(),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers();
    assert_eq!(
        headers.get("access-control-allow-origin"),
        Some(&TEST_ORIGIN.parse().unwrap())
    );
}

#[tokio::test]
async fn regular_requests_do_not_include_cors_headers() {
    let app = TestApplication::new().await;

    let response = app
        .request(Method::GET, "/api/v1/calendars", TEST_ORIGIN, String::new())
        .await;

    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers();
    assert!(headers.get("access-control-allow-origin").is_none());
}

// --- Error envelope shape ---

#[tokio::test]
async fn unauthorized_response_has_correct_json_shape() {
    let app = TestApplication::new().await;
    let router = build_router_with_calendars(
        Readiness::new(),
        SessionManager::new_at(
            app.pool.clone(),
            app.key.clone(),
            SessionSecurityConfig::new(300, 60, TEST_ORIGIN).unwrap(),
            NOW,
        ),
        CalendarService::new_at(app.pool.clone(), NOW),
    );
    let response = router
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/v1/calendars")
                .header(ORIGIN, TEST_ORIGIN)
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(""))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response_body = body(response).await;
    assert!(response_body.contains(r#""error":{"code":"unauthorized""#));
    assert!(response_body.contains(r#""message":"Authentication required""#));
}

#[tokio::test]
async fn forbidden_response_has_correct_json_shape() {
    let app = TestApplication::new().await;
    let token = app.key.generate_token();
    let hash = app.key.hash_token(TokenDomain::Session, &token);
    sqlx::query(
        "INSERT INTO sessions (
            user_id, session_hash, expires_at, revoked_at, created_at, last_seen_at
         ) VALUES (?, ?, ?, NULL, ?, ?)",
    )
    .bind(app.user_id)
    .bind(hash.as_bytes().as_slice())
    .bind(NOW + 1_000)
    .bind(NOW - 10)
    .bind(NOW - 10)
    .execute(&app.pool)
    .await
    .unwrap();
    let router = build_router_with_calendars(
        Readiness::new(),
        SessionManager::new_at(
            app.pool.clone(),
            app.key.clone(),
            SessionSecurityConfig::new(300, 60, TEST_ORIGIN).unwrap(),
            NOW,
        ),
        CalendarService::new_at(app.pool.clone(), NOW),
    );
    let response = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/calendars")
                .header(
                    COOKIE,
                    format!("__Host-commoncal_session={}", token.expose()),
                )
                .header("content-type", "application/json")
                .header(ORIGIN, TEST_ORIGIN)
                .header("sec-fetch-site", "same-origin")
                .header("x-csrf-token", "wrong")
                .body(Body::from(""))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response_body = body(response).await;
    assert!(response_body.contains(r#""error":{"code":"forbidden""#));
    assert!(response_body.contains(r#""message":"Request forbidden""#));
}

#[tokio::test]
async fn not_found_response_has_correct_json_shape() {
    let app = TestApplication::new().await;

    let response = app
        .request(Method::GET, "/api/v1/nonexistent", "", String::new())
        .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response_body = body(response).await;
    assert!(response_body.contains(r#""error":{"code":"not_found""#));
    assert!(response_body.contains(r#""message":"Resource not found""#));
}

// --- Version conflict envelope ---

#[tokio::test]
async fn version_conflict_includes_current_version() {
    let app = TestApplication::new().await;
    let calendar: (i64, i64) = sqlx::query_as("SELECT id, version FROM calendars WHERE id = ?")
        .bind(app.calendar_id)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    let token = app.key.generate_token();
    let hash = app.key.hash_token(TokenDomain::Session, &token);
    sqlx::query(
        "INSERT INTO sessions (
            user_id, session_hash, expires_at, revoked_at, created_at, last_seen_at
         ) VALUES (?, ?, ?, NULL, ?, ?)",
    )
    .bind(app.user_id)
    .bind(hash.as_bytes().as_slice())
    .bind(NOW + 1_000)
    .bind(NOW - 10)
    .bind(NOW - 10)
    .execute(&app.pool)
    .await
    .unwrap();
    let router = build_router_with_calendars(
        Readiness::new(),
        SessionManager::new_at(
            app.pool.clone(),
            app.key.clone(),
            SessionSecurityConfig::new(300, 60, TEST_ORIGIN).unwrap(),
            NOW,
        ),
        CalendarService::new_at(app.pool.clone(), NOW),
    );
    let csrf = app.key.generate_csrf_token(&token);
    let response = router
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(&format!("/api/v1/calendars/{}", app.calendar_id))
                .header(
                    COOKIE,
                    format!("__Host-commoncal_session={}", token.expose()),
                )
                .header("content-type", "application/json")
                .header(ORIGIN, TEST_ORIGIN)
                .header("sec-fetch-site", "same-origin")
                .header("x-csrf-token", csrf.expose())
                .body(Body::from(format!(
                    r##"{{"name":"X","color":"#000000","default_timezone":"UTC","default_event_visibility":"private","version":0}}"##
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let response_body = body(response).await;
    assert!(response_body.contains(r#""code":"conflict""#));
    assert!(response_body.contains(r#""current_version":1"#));
}
