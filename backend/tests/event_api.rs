use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header::COOKIE},
};
use commoncal_backend::{
    admin::AdminService,
    authorization::CalendarRole,
    calendar::{CalendarRepository, CalendarService, NewCalendar},
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    email::DevelopmentEmailSender,
    event::EventService,
    http::{Readiness, build_router_with_auth_flows_sessions_admin_and_calendars},
    invitations::InvitationConsumer,
    login::{AllowAllLoginRateLimiter, LoginService},
    security::{SecretKey, TokenDomain},
    sessions::{SessionManager, SessionSecurityConfig},
};
use http_body_util::BodyExt;
use sqlx::SqlitePool;
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

const NOW: i64 = 1_750_000_000;
const ORIGIN: &str = "https://commoncal.test";
const EVENT: &str = r#"{
    "title":"Private planning",
    "description":"Acquisition target",
    "location":"Board room",
    "status":"confirmed",
    "start_utc":1750000100,
    "end_utc":1750003700,
    "timezone":"Europe/Budapest"
}"#;
const RECURRING_EVENT: &str = r#"{
    "title":"Daily standup",
    "description":"Recurring private details",
    "location":"Room 1",
    "status":"confirmed",
    "start_utc":1750000100,
    "end_utc":1750000700,
    "timezone":"UTC",
    "recurrence_rule":"FREQ=DAILY;COUNT=3"
}"#;
const ALL_DAY_RECURRING_EVENT: &str = r#"{
    "title":"Conference",
    "description":"All-day recurring details",
    "location":"Campus",
    "status":"confirmed",
    "start_date":"2025-06-15",
    "end_date":"2025-06-17",
    "recurrence_rule":"FREQ=DAILY;COUNT=3"
}"#;

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
    other_calendar_id: i64,
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
        let calendar_id = create_calendar(&repository, owner, "Team").await;
        let other_calendar_id = create_calendar(&repository, unrelated, "Other").await;
        for (user_id, role) in [
            (manager, CalendarRole::Manager),
            (editor, CalendarRole::Editor),
            (viewer, CalendarRole::Viewer),
            (free_busy, CalendarRole::FreeBusyViewer),
        ] {
            repository
                .add_acl(calendar_id, user_id, role, NOW - 10)
                .await
                .unwrap();
        }
        Self {
            _temp_dir: temp_dir,
            pool,
            key: SecretKey::new([73; 32]),
            owner,
            manager,
            editor,
            viewer,
            free_busy,
            unrelated,
            calendar_id,
            other_calendar_id,
        }
    }

    fn router(&self) -> axum::Router {
        build_router_with_auth_flows_sessions_admin_and_calendars(
            Readiness::new(),
            InvitationConsumer::new_at(self.pool.clone(), self.key.clone(), 300, NOW),
            LoginService::new_at(
                self.pool.clone(),
                self.key.clone(),
                300,
                300,
                "/login",
                Arc::new(DevelopmentEmailSender::new()),
                Arc::new(AllowAllLoginRateLimiter),
                NOW,
            ),
            SessionManager::new_at(
                self.pool.clone(),
                self.key.clone(),
                SessionSecurityConfig::new(300, 60, ORIGIN).unwrap(),
                NOW,
            ),
            AdminService::new_at(self.pool.clone(), self.key.clone(), 300, NOW),
            CalendarService::new_at(self.pool.clone(), NOW),
            EventService::new_at(self.pool.clone(), NOW),
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

    async fn create_event(&self, user_id: i64) -> (StatusCode, i64) {
        let response = self
            .request(
                Method::POST,
                &format!("/api/v1/calendars/{}/events", self.calendar_id),
                user_id,
                EVENT,
            )
            .await;
        let status = response.status();
        let id = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) FROM events")
            .fetch_one(&self.pool)
            .await
            .unwrap();
        (status, id)
    }
}

#[tokio::test]
async fn production_composition_keeps_auth_admin_calendar_and_event_routes_mounted() {
    let app = TestApplication::new().await;

    for (method, path) in [
        (Method::GET, "/api/v1/auth/session"),
        (Method::GET, "/api/v1/admin/users"),
        (Method::GET, "/api/v1/calendars"),
        (
            Method::GET,
            "/api/v1/calendars/1/events?from=1750000000&to=1750004000",
        ),
    ] {
        let response = app
            .router()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
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
                default_timezone: "Europe/Budapest".to_owned(),
                default_event_visibility: "private".to_owned(),
                default_notification_rules_json: None,
                created_at: NOW - 50,
            },
        )
        .await
        .unwrap()
        .id
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

fn update_body(calendar_id: i64, version: i64) -> String {
    format!(
        r#"{{
            "calendar_id":{calendar_id},
            "version":{version},
            "title":"Updated title",
            "description":"Updated private text",
            "location":"Room 2",
            "status":"tentative",
            "start_utc":1750000200,
            "end_utc":1750003800,
            "timezone":"UTC"
        }}"#
    )
}

#[tokio::test]
async fn every_calendar_role_gets_the_expected_event_permissions() {
    let app = TestApplication::new().await;
    for user_id in [app.owner, app.manager, app.editor] {
        assert_eq!(app.create_event(user_id).await.0, StatusCode::CREATED);
    }
    for user_id in [app.viewer, app.free_busy, app.unrelated] {
        assert_eq!(app.create_event(user_id).await.0, StatusCode::NOT_FOUND);
    }
    let descriptions: Vec<Option<String>> = sqlx::query_scalar(
        "SELECT metadata_json FROM audit_log WHERE action = 'event.create' ORDER BY id",
    )
    .fetch_all(&app.pool)
    .await
    .unwrap();
    assert_eq!(descriptions, vec![None, None, None]);
}

#[tokio::test]
async fn free_busy_projection_redacts_details_and_list_requires_a_bounded_range() {
    let app = TestApplication::new().await;
    let (_, event_id) = app.create_event(app.owner).await;
    let read = app
        .request(
            Method::GET,
            &format!("/api/v1/calendars/{}/events/{event_id}", app.calendar_id),
            app.free_busy,
            "",
        )
        .await;
    assert_eq!(read.status(), StatusCode::OK);
    let text = body(read).await;
    assert!(text.contains(r#""access":"free_busy""#));
    for secret in ["Private planning", "Acquisition target", "Board room"] {
        assert!(!text.contains(secret));
    }

    let missing_range = app
        .request(
            Method::GET,
            &format!("/api/v1/calendars/{}/events", app.calendar_id),
            app.owner,
            "",
        )
        .await;
    assert_eq!(missing_range.status(), StatusCode::BAD_REQUEST);
    let list = app
        .request(
            Method::GET,
            &format!(
                "/api/v1/calendars/{}/events?from=1750000000&to=1750004000",
                app.calendar_id
            ),
            app.free_busy,
            "",
        )
        .await;
    let list_text = body(list).await;
    assert!(list_text.contains(r#""access":"free_busy""#));
    assert!(!list_text.contains("Private planning"));
}

#[tokio::test]
async fn viewer_can_read_but_cannot_update_or_delete() {
    let app = TestApplication::new().await;
    let (_, event_id) = app.create_event(app.owner).await;
    let read = app
        .request(
            Method::GET,
            &format!("/api/v1/calendars/{}/events/{event_id}", app.calendar_id),
            app.viewer,
            "",
        )
        .await;
    assert_eq!(read.status(), StatusCode::OK);
    assert!(body(read).await.contains("Private planning"));
    let update = app
        .request(
            Method::PATCH,
            &format!("/api/v1/calendars/{}/events/{event_id}", app.calendar_id),
            app.viewer,
            &update_body(app.calendar_id, 1),
        )
        .await;
    let delete = app
        .request(
            Method::DELETE,
            &format!("/api/v1/calendars/{}/events/{event_id}", app.calendar_id),
            app.viewer,
            "",
        )
        .await;
    assert_eq!(update.status(), StatusCode::NOT_FOUND);
    assert_eq!(delete.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn event_id_substitution_cannot_bypass_calendar_authorization() {
    let app = TestApplication::new().await;
    let (_, event_id) = app.create_event(app.owner).await;
    let substituted = app
        .request(
            Method::GET,
            &format!(
                "/api/v1/calendars/{}/events/{event_id}",
                app.other_calendar_id
            ),
            app.unrelated,
            "",
        )
        .await;
    let missing = app
        .request(
            Method::GET,
            &format!("/api/v1/calendars/{}/events/999999", app.other_calendar_id),
            app.unrelated,
            "",
        )
        .await;
    assert_eq!(substituted.status(), StatusCode::NOT_FOUND);
    assert_eq!(body(substituted).await, body(missing).await);
}

#[tokio::test]
async fn move_to_unauthorized_calendar_fails_atomically() {
    let app = TestApplication::new().await;
    let (_, event_id) = app.create_event(app.editor).await;
    let response = app
        .request(
            Method::PATCH,
            &format!("/api/v1/calendars/{}/events/{event_id}", app.calendar_id),
            app.editor,
            &update_body(app.other_calendar_id, 1),
        )
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let stored: (i64, String, i64) =
        sqlx::query_as("SELECT calendar_id, title, version FROM events WHERE id = ?")
            .bind(event_id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(stored, (app.calendar_id, "Private planning".to_owned(), 1));
}

#[tokio::test]
async fn stale_update_returns_conflict_and_crud_mutations_are_audited_safely() {
    let app = TestApplication::new().await;
    let (_, event_id) = app.create_event(app.owner).await;
    let path = format!("/api/v1/calendars/{}/events/{event_id}", app.calendar_id);
    let updated = app
        .request(
            Method::PATCH,
            &path,
            app.owner,
            &update_body(app.calendar_id, 1),
        )
        .await;
    assert_eq!(updated.status(), StatusCode::OK);
    let stale = app
        .request(
            Method::PATCH,
            &path,
            app.owner,
            &update_body(app.calendar_id, 1),
        )
        .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert!(body(stale).await.contains(r#""current_version":2"#));
    let deleted = app.request(Method::DELETE, &path, app.owner, "").await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    let actions: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT action, metadata_json FROM audit_log
         WHERE target_type = 'event' ORDER BY id",
    )
    .fetch_all(&app.pool)
    .await
    .unwrap();
    assert_eq!(
        actions,
        vec![
            ("event.create".to_owned(), None),
            ("event.update".to_owned(), None),
            ("event.delete".to_owned(), None),
        ]
    );
}

#[tokio::test]
async fn recurring_routes_expand_and_mutate_occurrences_with_existing_session_and_csrf_controls() {
    let app = TestApplication::new().await;
    let created = app
        .request(
            Method::POST,
            &format!("/api/v1/calendars/{}/events", app.calendar_id),
            app.owner,
            RECURRING_EVENT,
        )
        .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let event_id: i64 = sqlx::query_scalar("SELECT MAX(id) FROM events")
        .fetch_one(&app.pool)
        .await
        .unwrap();

    let list = app
        .request(
            Method::GET,
            &format!(
                "/api/v1/calendars/{}/events?from=1750000000&to=1750259300",
                app.calendar_id
            ),
            app.owner,
            "",
        )
        .await;
    let list_body = body(list).await;
    assert_eq!(list_body.matches(r#""series_id":"#).count(), 3);

    let update = app
        .request(
            Method::PATCH,
            &format!(
                "/api/v1/calendars/{}/events/{event_id}/occurrences/1750086500",
                app.calendar_id
            ),
            app.owner,
            r#"{
                "version":1,
                "title":"Moved standup",
                "description":null,
                "location":null,
                "status":"confirmed",
                "start_utc":1750090100,
                "end_utc":1750090700,
                "timezone":"UTC"
            }"#,
        )
        .await;
    assert_eq!(update.status(), StatusCode::OK);

    let denied_delete = app
        .request(
            Method::DELETE,
            &format!(
                "/api/v1/calendars/{}/events/{event_id}/occurrences/1750172900",
                app.calendar_id
            ),
            app.viewer,
            r#"{"version":2}"#,
        )
        .await;
    assert_eq!(denied_delete.status(), StatusCode::NOT_FOUND);

    let deleted = app
        .request(
            Method::DELETE,
            &format!(
                "/api/v1/calendars/{}/events/{event_id}/occurrences/1750172900",
                app.calendar_id
            ),
            app.owner,
            r#"{"version":2}"#,
        )
        .await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);

    let following = app
        .request(
            Method::PATCH,
            &format!(
                "/api/v1/calendars/{}/events/{event_id}/occurrences/1750086500/following",
                app.calendar_id
            ),
            app.owner,
            "{}",
        )
        .await;
    assert_eq!(following.status(), StatusCode::NOT_IMPLEMENTED);
    assert!(body(following).await.contains(r#""code":"not_supported""#));
}

#[tokio::test]
async fn all_day_recurring_routes_use_iso_recurrence_dates_and_exclusive_end_dates() {
    let app = TestApplication::new().await;
    let created = app
        .request(
            Method::POST,
            &format!("/api/v1/calendars/{}/events", app.calendar_id),
            app.owner,
            ALL_DAY_RECURRING_EVENT,
        )
        .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let event_id: i64 = sqlx::query_scalar("SELECT MAX(id) FROM events")
        .fetch_one(&app.pool)
        .await
        .unwrap();

    let updated = app
        .request(
            Method::PATCH,
            &format!(
                "/api/v1/calendars/{}/events/{event_id}/occurrences/2025-06-16",
                app.calendar_id
            ),
            app.owner,
            r#"{
                "version":1,
                "title":"Moved conference",
                "description":null,
                "location":null,
                "status":"confirmed",
                "start_date":"2025-06-20",
                "end_date":"2025-06-22"
            }"#,
        )
        .await;
    assert_eq!(updated.status(), StatusCode::OK);

    let deleted = app
        .request(
            Method::DELETE,
            &format!(
                "/api/v1/calendars/{}/events/{event_id}/occurrences/2025-06-17",
                app.calendar_id
            ),
            app.owner,
            r#"{"version":2}"#,
        )
        .await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);

    let list = app
        .request(
            Method::GET,
            &format!(
                "/api/v1/calendars/{}/events?from=1750000000&to=1750604800",
                app.calendar_id
            ),
            app.owner,
            "",
        )
        .await;
    let list_body = body(list).await;
    assert!(list_body.contains(r#""recurrence_date":"2025-06-16""#));
    assert!(list_body.contains(r#""start_date":"2025-06-20""#));
    assert!(list_body.contains(r#""end_date":"2025-06-22""#));
    assert!(!list_body.contains(r#""recurrence_date":"2025-06-17""#));
}
