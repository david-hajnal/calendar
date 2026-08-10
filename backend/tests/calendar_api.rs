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

const NOW: i64 = 50_000;
const ORIGIN: &str = "https://commoncal.test";

struct TestApplication {
    _temp_dir: TempDir,
    pool: SqlitePool,
    key: SecretKey,
    owner: i64,
    manager: i64,
    editor: i64,
    viewer: i64,
    free_busy: i64,
    unrelated: i64,
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
        let owner = insert_user(&pool, "owner@example.com").await;
        let manager = insert_user(&pool, "manager@example.com").await;
        let editor = insert_user(&pool, "editor@example.com").await;
        let viewer = insert_user(&pool, "viewer@example.com").await;
        let free_busy = insert_user(&pool, "freebusy@example.com").await;
        let unrelated = insert_user(&pool, "unrelated@example.com").await;
        let repository = CalendarRepository::new(pool.clone());
        let calendar = repository
            .create_calendar(
                owner,
                NewCalendar {
                    name: "Private team".to_owned(),
                    description: Some("Secret planning".to_owned()),
                    color: "#3367d6".to_owned(),
                    default_timezone: "Europe/Budapest".to_owned(),
                    default_event_visibility: "private".to_owned(),
                    default_notification_rules_json: Some(r#"{"minutes":15}"#.to_owned()),
                    created_at: NOW - 100,
                },
            )
            .await
            .unwrap();
        for (user_id, role) in [
            (manager, CalendarRole::Manager),
            (editor, CalendarRole::Editor),
            (viewer, CalendarRole::Viewer),
            (free_busy, CalendarRole::FreeBusyViewer),
        ] {
            repository
                .add_acl(calendar.id, user_id, role, NOW - 90)
                .await
                .unwrap();
        }
        Self {
            _temp_dir: temp_dir,
            pool,
            key: SecretKey::new([91; 32]),
            owner,
            manager,
            editor,
            viewer,
            free_busy,
            unrelated,
            calendar_id: calendar.id,
        }
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
        self.router()
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
}

async fn insert_user(pool: &SqlitePool, email: &str) -> i64 {
    sqlx::query(
        "INSERT INTO users (
            normalized_email, display_name, status, is_superadmin, created_at
         ) VALUES (?, NULL, 'active', 0, ?)",
    )
    .bind(email)
    .bind(NOW - 1_000)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid()
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
async fn list_and_read_apply_role_projection_and_hide_unrelated_calendars() {
    let app = TestApplication::new().await;

    for user_id in [app.owner, app.manager, app.editor, app.viewer] {
        let list = app
            .request(Method::GET, "/api/v1/calendars", user_id, "")
            .await;
        assert_eq!(list.status(), StatusCode::OK);
        let text = body(list).await;
        assert!(text.contains(r#""name":"Private team""#));
        assert!(text.contains(r#""description":"Secret planning""#));

        let read = app
            .request(
                Method::GET,
                &format!("/api/v1/calendars/{}", app.calendar_id),
                user_id,
                "",
            )
            .await;
        assert_eq!(read.status(), StatusCode::OK);
        assert!(body(read).await.contains(r#""access":"details""#));
    }

    let free_busy = app
        .request(Method::GET, "/api/v1/calendars", app.free_busy, "")
        .await;
    assert_eq!(free_busy.status(), StatusCode::OK);
    let text = body(free_busy).await;
    assert!(text.contains(r#""access":"free_busy""#));
    assert!(!text.contains("Private team"));
    assert!(!text.contains("Secret planning"));
    assert!(!text.contains("default_timezone"));
    let free_busy_read = app
        .request(
            Method::GET,
            &format!("/api/v1/calendars/{}", app.calendar_id),
            app.free_busy,
            "",
        )
        .await;
    assert_eq!(free_busy_read.status(), StatusCode::OK);
    assert!(
        body(free_busy_read)
            .await
            .contains(r#""access":"free_busy""#)
    );

    let unrelated = app
        .request(Method::GET, "/api/v1/calendars", app.unrelated, "")
        .await;
    assert_eq!(unrelated.status(), StatusCode::OK);
    assert_eq!(body(unrelated).await, "[]");
}

#[tokio::test]
async fn id_substitution_and_deleted_resources_use_the_same_non_leaking_response() {
    let app = TestApplication::new().await;
    let inaccessible = app
        .request(
            Method::GET,
            &format!("/api/v1/calendars/{}", app.calendar_id),
            app.unrelated,
            "",
        )
        .await;
    let missing = app
        .request(Method::GET, "/api/v1/calendars/999999", app.unrelated, "")
        .await;

    assert_eq!(inaccessible.status(), StatusCode::NOT_FOUND);
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(body(inaccessible).await, body(missing).await);

    let deleted = app
        .request(
            Method::DELETE,
            &format!("/api/v1/calendars/{}", app.calendar_id),
            app.owner,
            "",
        )
        .await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    let read_deleted = app
        .request(
            Method::GET,
            &format!("/api/v1/calendars/{}", app.calendar_id),
            app.owner,
            "",
        )
        .await;
    assert_eq!(read_deleted.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn settings_are_manager_only_or_better_and_require_current_version() {
    let app = TestApplication::new().await;
    let update = r##"{
        "name":"Updated",
        "description":null,
        "color":"#123456",
        "default_timezone":"UTC",
        "default_event_visibility":"default",
        "default_notification_rules_json":null,
        "version":1
    }"##;

    let editor = app
        .request(
            Method::PATCH,
            &format!("/api/v1/calendars/{}", app.calendar_id),
            app.editor,
            update,
        )
        .await;
    assert_eq!(editor.status(), StatusCode::NOT_FOUND);

    let manager = app
        .request(
            Method::PATCH,
            &format!("/api/v1/calendars/{}", app.calendar_id),
            app.manager,
            update,
        )
        .await;
    assert_eq!(manager.status(), StatusCode::OK);
    assert!(body(manager).await.contains(r#""version":2"#));

    let stale = app
        .request(
            Method::PATCH,
            &format!("/api/v1/calendars/{}", app.calendar_id),
            app.owner,
            update,
        )
        .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert!(body(stale).await.contains(r#""current_version":2"#));
}

#[tokio::test]
async fn archive_restore_and_delete_follow_permissions_and_are_audited() {
    let app = TestApplication::new().await;
    let manager_delete = app
        .request(
            Method::DELETE,
            &format!("/api/v1/calendars/{}", app.calendar_id),
            app.manager,
            "",
        )
        .await;
    assert_eq!(manager_delete.status(), StatusCode::NOT_FOUND);

    for action in ["archive", "restore"] {
        let response = app
            .request(
                Method::POST,
                &format!("/api/v1/calendars/{}/{action}", app.calendar_id),
                app.manager,
                "",
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
    }
    let deleted = app
        .request(
            Method::DELETE,
            &format!("/api/v1/calendars/{}", app.calendar_id),
            app.owner,
            "",
        )
        .await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);

    let actions: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_log
         WHERE target_type = 'calendar' AND target_id = ? ORDER BY id",
    )
    .bind(app.calendar_id.to_string())
    .fetch_all(&app.pool)
    .await
    .unwrap();
    assert_eq!(
        actions,
        ["calendar.archive", "calendar.restore", "calendar.delete"]
    );
}

#[tokio::test]
async fn authenticated_user_can_create_a_calendar_with_an_owner_acl_and_audit() {
    let app = TestApplication::new().await;
    let response = app
        .request(
            Method::POST,
            "/api/v1/calendars",
            app.unrelated,
            r##"{
                "name":"Personal",
                "description":null,
                "color":"#abcdef",
                "default_timezone":"UTC",
                "default_event_visibility":"private",
                "default_notification_rules_json":null
            }"##,
        )
        .await;

    assert_eq!(response.status(), StatusCode::CREATED);
    let created = body(response).await;
    assert!(created.contains(r#""role":"owner""#));
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_log
         WHERE actor_user_id = ? AND action = 'calendar.create'",
    )
    .bind(app.unrelated)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(audit_count, 1);
}
