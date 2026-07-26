use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header::COOKIE},
};
use commoncal_backend::{
    authorization::CalendarRole,
    calendar::{CalendarRepository, CalendarService, NewCalendar},
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    event::EventService,
    http::{Readiness, build_router_with_calendars_events_and_views},
    security::{SecretKey, TokenDomain},
    sessions::{SessionManager, SessionSecurityConfig},
    shared_view::SharedViewService,
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
    other: i64,
    first_calendar: i64,
    second_calendar: i64,
    inaccessible_calendar: i64,
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
        let other = insert_user(&pool, "other@example.com").await;
        let repository = CalendarRepository::new(pool.clone());
        let first_calendar = create_calendar(&repository, owner, "Work").await;
        let second_calendar = create_calendar(&repository, other, "Family").await;
        let inaccessible_calendar = create_calendar(&repository, other, "Secret").await;
        repository
            .add_acl(second_calendar, owner, CalendarRole::Viewer, NOW - 10)
            .await
            .unwrap();
        Self {
            _temp_dir: temp_dir,
            pool,
            key: SecretKey::new([46; 32]),
            owner,
            other,
            first_calendar,
            second_calendar,
            inaccessible_calendar,
        }
    }

    fn router(&self) -> axum::Router {
        build_router_with_calendars_events_and_views(
            Readiness::new(),
            SessionManager::new_at(
                self.pool.clone(),
                self.key.clone(),
                SessionSecurityConfig::new(300, 60, ORIGIN).unwrap(),
                NOW,
            ),
            CalendarService::new_at(self.pool.clone(), NOW),
            EventService::new_at(self.pool.clone(), NOW),
            SharedViewService::new_at(self.pool.clone(), NOW),
        )
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        user_id: i64,
        request_body: &str,
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
                    .body(Body::from(request_body.to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn create_view(&self) -> i64 {
        let response = self
            .request(
                Method::POST,
                "/api/v1/views",
                self.owner,
                r##"{"name":"My view"}"##,
            )
            .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        sqlx::query_scalar("SELECT id FROM shared_views WHERE owner_user_id = ?")
            .bind(self.owner)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn set_sources(&self, view_id: i64, sources: &str) -> StatusCode {
        self.request(
            Method::PUT,
            &format!("/api/v1/views/{view_id}/calendars"),
            self.owner,
            sources,
        )
        .await
        .status()
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

async fn insert_event(pool: &SqlitePool, calendar_id: i64, user_id: i64, title: &str) {
    sqlx::query(
        "INSERT INTO events (
            calendar_id, title, description, location, status, event_kind,
            timed_start_utc, timed_end_utc, event_timezone,
            all_day_start_date, all_day_end_date, created_by_user_id,
            last_edited_by_user_id, version, created_at, updated_at
         ) VALUES (?, ?, 'private description', NULL, 'confirmed', 'timed',
                   ?, ?, 'UTC', NULL, NULL, ?, ?, 1, ?, ?)",
    )
    .bind(calendar_id)
    .bind(title)
    .bind(NOW + 100)
    .bind(NOW + 200)
    .bind(user_id)
    .bind(user_id)
    .bind(NOW)
    .bind(NOW)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn view_owner_can_create_read_update_delete_and_manage_ordered_colored_sources() {
    let app = TestApplication::new().await;
    let view_id = app.create_view().await;

    let list = response_body(
        app.request(Method::GET, "/api/v1/views", app.owner, "")
            .await,
    )
    .await;
    assert!(list.contains(r#""name":"My view""#));

    let update = app
        .request(
            Method::PATCH,
            &format!("/api/v1/views/{view_id}"),
            app.owner,
            r#"{"name":"Everything"}"#,
        )
        .await;
    assert_eq!(update.status(), StatusCode::OK);

    let sources = format!(
        r##"{{"calendars":[{{"calendar_id":{},"position":1,"color":"#222222"}},{{"calendar_id":{},"position":0,"color":"#111111"}}]}}"##,
        app.first_calendar, app.second_calendar
    );
    assert_eq!(app.set_sources(view_id, &sources).await, StatusCode::OK);
    let read = response_body(
        app.request(
            Method::GET,
            &format!("/api/v1/views/{view_id}"),
            app.owner,
            "",
        )
        .await,
    )
    .await;
    assert!(read.contains(r#""name":"Everything""#));
    assert!(read.find("#111111").unwrap() < read.find("#222222").unwrap());

    let recolor_and_remove = format!(
        r##"{{"calendars":[{{"calendar_id":{},"position":0,"color":"#abcdef"}}]}}"##,
        app.first_calendar
    );
    assert_eq!(
        app.set_sources(view_id, &recolor_and_remove).await,
        StatusCode::OK
    );
    let read = response_body(
        app.request(
            Method::GET,
            &format!("/api/v1/views/{view_id}"),
            app.owner,
            "",
        )
        .await,
    )
    .await;
    assert!(read.contains("#abcdef"));
    assert!(!read.contains("#111111"));

    let deleted = app
        .request(
            Method::DELETE,
            &format!("/api/v1/views/{view_id}"),
            app.owner,
            "",
        )
        .await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn another_user_cannot_edit_a_private_view() {
    let app = TestApplication::new().await;
    let view_id = app.create_view().await;
    for (method, suffix, body) in [
        (Method::PATCH, "", r#"{"name":"Stolen"}"#),
        (Method::PUT, "/calendars", r#"{"calendars":[]}"#),
        (Method::DELETE, "", ""),
    ] {
        let response = app
            .request(
                method,
                &format!("/api/v1/views/{view_id}{suffix}"),
                app.other,
                body,
            )
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
async fn revoked_calendar_access_removes_the_source_and_its_events_on_the_next_read() {
    let app = TestApplication::new().await;
    let view_id = app.create_view().await;
    insert_event(&app.pool, app.first_calendar, app.owner, "Visible event").await;
    insert_event(&app.pool, app.second_calendar, app.other, "Revoked event").await;
    let sources = format!(
        r##"{{"calendars":[{{"calendar_id":{},"position":0,"color":"#111111"}},{{"calendar_id":{},"position":1,"color":"#222222"}}]}}"##,
        app.first_calendar, app.second_calendar
    );
    assert_eq!(app.set_sources(view_id, &sources).await, StatusCode::OK);
    sqlx::query("DELETE FROM calendar_acl WHERE calendar_id = ? AND user_id = ?")
        .bind(app.second_calendar)
        .bind(app.owner)
        .execute(&app.pool)
        .await
        .unwrap();

    let view = response_body(
        app.request(
            Method::GET,
            &format!("/api/v1/views/{view_id}"),
            app.owner,
            "",
        )
        .await,
    )
    .await;
    assert!(!view.contains(&format!(r#""calendar_id":{}"#, app.second_calendar)));
    let events = response_body(
        app.request(
            Method::GET,
            &format!(
                "/api/v1/views/{view_id}/events?from={}&to={}",
                NOW,
                NOW + 300
            ),
            app.owner,
            "",
        )
        .await,
    )
    .await;
    assert!(events.contains("Visible event"));
    assert!(!events.contains("Revoked event"));
}

#[tokio::test]
async fn adding_an_inaccessible_calendar_fails_without_leaking_it() {
    let app = TestApplication::new().await;
    let view_id = app.create_view().await;
    let sources = format!(
        r##"{{"calendars":[{{"calendar_id":{},"position":0,"color":"#111111"}}]}}"##,
        app.inaccessible_calendar
    );
    assert_eq!(
        app.set_sources(view_id, &sources).await,
        StatusCode::NOT_FOUND
    );
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM shared_view_calendars WHERE view_id = ?")
            .bind(view_id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn adding_a_source_does_not_grant_or_upgrade_calendar_permission() {
    let app = TestApplication::new().await;
    let view_id = app.create_view().await;
    let sources = format!(
        r##"{{"calendars":[{{"calendar_id":{},"position":0,"color":"#111111"}}]}}"##,
        app.second_calendar
    );
    assert_eq!(app.set_sources(view_id, &sources).await, StatusCode::OK);
    let role: String =
        sqlx::query_scalar("SELECT role FROM calendar_acl WHERE calendar_id = ? AND user_id = ?")
            .bind(app.second_calendar)
            .bind(app.owner)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(role, "viewer");
}

#[tokio::test]
async fn combined_private_results_preserve_source_calendar_identity_for_the_authorized_user() {
    let app = TestApplication::new().await;
    let view_id = app.create_view().await;
    let oversized_empty_query = app
        .request(
            Method::GET,
            &format!(
                "/api/v1/views/{view_id}/events?from={}&to={}",
                NOW,
                NOW + 367 * 24 * 60 * 60
            ),
            app.owner,
            "",
        )
        .await;
    assert_eq!(oversized_empty_query.status(), StatusCode::BAD_REQUEST);

    insert_event(&app.pool, app.first_calendar, app.owner, "Work event").await;
    insert_event(&app.pool, app.second_calendar, app.other, "Family event").await;
    let sources = format!(
        r##"{{"calendars":[{{"calendar_id":{},"position":0,"color":"#111111"}},{{"calendar_id":{},"position":1,"color":"#222222"}}]}}"##,
        app.first_calendar, app.second_calendar
    );
    assert_eq!(app.set_sources(view_id, &sources).await, StatusCode::OK);

    let events = response_body(
        app.request(
            Method::GET,
            &format!(
                "/api/v1/views/{view_id}/events?from={}&to={}",
                NOW,
                NOW + 300
            ),
            app.owner,
            "",
        )
        .await,
    )
    .await;
    assert!(events.contains(&format!(r#""calendar_id":{}"#, app.first_calendar)));
    assert!(events.contains(&format!(r#""calendar_id":{}"#, app.second_calendar)));
    assert!(events.contains("private description"));
}
