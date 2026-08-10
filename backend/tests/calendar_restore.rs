use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header::COOKIE},
};
use commoncal_backend::{
    authorization::CalendarRole,
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
const ORIGIN: &str = "https://commoncal.test";

struct TestApplication {
    _temp_dir: TempDir,
    pool: SqlitePool,
    key: SecretKey,
    owner: i64,
    manager: i64,
    editor: i64,
    unrelated: i64,
    calendar_id: i64,
    archived_calendar_id: i64,
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
        let owner = insert_user(&pool, "owner@example.com").await;
        let manager = insert_user(&pool, "manager@example.com").await;
        let editor = insert_user(&pool, "editor@example.com").await;
        let unrelated = insert_user(&pool, "unrelated@example.com").await;
        let repository = CalendarRepository::new(pool.clone());
        let calendar_id = create_calendar(&repository, owner, "Active calendar").await;
        let archived_calendar_id = create_archived_calendar(&pool, owner).await;
        for (user_id, role) in [
            (manager, CalendarRole::Manager),
            (editor, CalendarRole::Editor),
        ] {
            repository
                .add_acl(calendar_id, user_id, role, NOW - 10)
                .await
                .unwrap();
            repository
                .add_acl(archived_calendar_id, user_id, role, NOW - 10)
                .await
                .unwrap();
        }
        Self {
            _temp_dir: temp_dir,
            pool,
            key: SecretKey::new([85; 32]),
            owner,
            manager,
            editor,
            unrelated,
            calendar_id,
            archived_calendar_id,
        }
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        user_id: i64,
        body: &str,
    ) -> axum::response::Response {
        let token = self.key.generate_token();
        let hash = self.key.hash_token(TokenDomain::Session, &token);
        sqlx::query(
            "INSERT INTO sessions (
                user_id, session_hash, expires_at, revoked_at, created_at, last_seen_at
             ) VALUES (?, ?, ?, NULL, ?, ?)",
        )
        .bind(user_id)
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
                SessionSecurityConfig::new(300, 60, ORIGIN).unwrap(),
                NOW,
            ),
            CalendarService::new_at(self.pool.clone(), NOW),
            None,
            None,
            None,
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
                .header("origin", ORIGIN)
                .header("sec-fetch-site", "same-origin")
                .header("x-csrf-token", csrf.expose())
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap()
    }

    fn router(&self) -> axum::Router {
        build_router_with_calendars(
            Readiness::new(),
            SessionManager::new_at(
                self.pool.clone(),
                self.key.clone(),
                SessionSecurityConfig::new(300, 60, ORIGIN).unwrap(),
                NOW,
            ),
            CalendarService::new_at(self.pool.clone(), NOW),
            None,
            None,
            None,
        )
    }
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

async fn create_calendar(repository: &CalendarRepository, owner: i64, name: &str) -> i64 {
    repository
        .create_calendar(
            owner,
            NewCalendar {
                name: name.to_owned(),
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
        .id
}

async fn create_archived_calendar(pool: &SqlitePool, owner: i64) -> i64 {
    let repository = CalendarRepository::new(pool.clone());
    let id = repository
        .create_calendar(
            owner,
            NewCalendar {
                name: "Archived calendar".to_owned(),
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
    sqlx::query("UPDATE calendars SET archived = 1 WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    id
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
async fn restore_returns_200_and_sets_archived_to_false() {
    let app = TestApplication::new().await;

    let response = app
        .request(
            Method::POST,
            &format!("/api/v1/calendars/{}/restore", app.archived_calendar_id),
            app.owner,
            "",
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);
    let response_body = body(response).await;
    assert!(response_body.contains(r#""archived":false"#));
    let archived: i32 = sqlx::query_scalar("SELECT archived FROM calendars WHERE id = ?")
        .bind(app.archived_calendar_id)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(archived, 0);
}

#[tokio::test]
async fn restore_returns_404_for_non_archived_calendar() {
    let app = TestApplication::new().await;

    let response = app
        .request(
            Method::POST,
            &format!("/api/v1/calendars/{}/restore", app.calendar_id),
            app.owner,
            "",
        )
        .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response_body = body(response).await;
    assert!(response_body.contains(r#""code":"not_found""#));
}

#[tokio::test]
async fn restore_returns_404_for_non_existent_calendar() {
    let app = TestApplication::new().await;

    let response = app
        .request(
            Method::POST,
            "/api/v1/calendars/999999/restore",
            app.owner,
            "",
        )
        .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response_body = body(response).await;
    assert!(response_body.contains(r#""code":"not_found""#));
}

#[tokio::test]
async fn restore_requires_owner_or_manager_role() {
    let app = TestApplication::new().await;

    let editor_response = app
        .request(
            Method::POST,
            &format!("/api/v1/calendars/{}/restore", app.archived_calendar_id),
            app.editor,
            "",
        )
        .await;
    assert_eq!(editor_response.status(), StatusCode::NOT_FOUND);

    let unrelated_response = app
        .request(
            Method::POST,
            &format!("/api/v1/calendars/{}/restore", app.archived_calendar_id),
            app.unrelated,
            "",
        )
        .await;
    assert_eq!(unrelated_response.status(), StatusCode::NOT_FOUND);

    let manager_response = app
        .request(
            Method::POST,
            &format!("/api/v1/calendars/{}/restore", app.archived_calendar_id),
            app.manager,
            "",
        )
        .await;
    assert_eq!(manager_response.status(), StatusCode::OK);
    let response_body = body(manager_response).await;
    assert!(response_body.contains(r#""archived":false"#));
}

#[tokio::test]
async fn restore_requires_authentication() {
    let app = TestApplication::new().await;

    let response = app
        .router()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(&format!(
                    "/api/v1/calendars/{}/restore",
                    app.archived_calendar_id
                ))
                .header("content-type", "application/json")
                .header("origin", ORIGIN)
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(""))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn double_restore_returns_404_second_time() {
    let app = TestApplication::new().await;

    let first = app
        .request(
            Method::POST,
            &format!("/api/v1/calendars/{}/restore", app.archived_calendar_id),
            app.owner,
            "",
        )
        .await;
    assert_eq!(first.status(), StatusCode::OK);

    let second = app
        .request(
            Method::POST,
            &format!("/api/v1/calendars/{}/restore", app.archived_calendar_id),
            app.owner,
            "",
        )
        .await;
    assert_eq!(second.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn archived_calendar_is_excluded_from_list() {
    let app = TestApplication::new().await;

    let list_response = app
        .request(Method::GET, "/api/v1/calendars", app.owner, "")
        .await;
    assert_eq!(list_response.status(), StatusCode::OK);
    let list_body = body(list_response).await;
    assert!(!list_body.contains("Archived calendar"));
}

#[tokio::test]
async fn restored_calendar_appears_in_list() {
    let app = TestApplication::new().await;

    let list_before = app
        .request(Method::GET, "/api/v1/calendars", app.owner, "")
        .await;
    let list_before_body = body(list_before).await;
    assert!(!list_before_body.contains("Archived calendar"));

    app.request(
        Method::POST,
        &format!("/api/v1/calendars/{}/restore", app.archived_calendar_id),
        app.owner,
        "",
    )
    .await;

    let list_after = app
        .request(Method::GET, "/api/v1/calendars", app.owner, "")
        .await;
    let list_after_body = body(list_after).await;
    assert!(list_after_body.contains("Archived calendar"));
}
