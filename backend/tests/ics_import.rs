use std::sync::{Arc, Mutex};

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
    event::{
        EventChange, EventCreateBatch, EventCreateException, EventCreateItem, EventMutation,
        EventRecurrenceKey, EventService, EventServiceError, EventStatus, EventTiming,
    },
    http::{Readiness, build_router_with_auth_flows_sessions_admin_and_calendars},
    ics_import::{IcsImportError, IcsImportService, MAX_ICS_IMPORT_EVENTS},
    invitations::InvitationConsumer,
    login::{AllowAllLoginRateLimiter, LoginService},
    security::{SecretKey, TokenDomain},
    sessions::{SessionManager, SessionSecurityConfig},
};
use http_body_util::BodyExt;
use tempfile::TempDir;
use tower::ServiceExt;

const NOW: i64 = 1_750_000_000;
const ORIGIN: &str = "https://commoncal.test";

#[tokio::test]
async fn ics_import_batch_persists_mixed_events_exceptions_and_side_effects() {
    let (_dir, pool, owner, calendar_id) = setup().await;
    let replanned = Arc::new(Mutex::new(Vec::new()));
    let captured = replanned.clone();
    let service = EventService::new_at_with_notification_replanner(
        pool.clone(),
        NOW,
        Arc::new(move |event_id| captured.lock().unwrap().push(event_id)),
    );

    let result = service
        .create_batch(owner, false, calendar_id, mixed_batch())
        .await
        .unwrap();

    assert_eq!(result.event_ids.len(), 2);
    assert_eq!(result.exception_count, 3);
    let event_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    let exception_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM event_recurrence_exceptions")
            .fetch_one(&pool)
            .await
            .unwrap();
    let audits: Vec<String> =
        sqlx::query_scalar("SELECT action FROM audit_log WHERE target_type = 'event' ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    let external_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM external_event_mapping")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(event_count, 2);
    assert_eq!(exception_count, 3);
    assert_eq!(audits, vec!["event.series.create", "event.series.create"]);
    assert_eq!(external_count, 0);

    let exceptions: Vec<(Option<i64>, Option<String>, bool, Option<String>)> = sqlx::query_as(
        "SELECT recurrence_id, recurrence_date, is_deleted, title
         FROM event_recurrence_exceptions ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(exceptions[0], (Some(NOW + 86_400), None, true, None));
    assert_eq!(exceptions[1].0, Some(NOW + 172_800));
    assert_eq!(exceptions[1].3.as_deref(), Some("Moved meeting"));
    assert_eq!(exceptions[2], (None, Some("2025-06-17".into()), true, None));

    for _ in 0..20 {
        if replanned.lock().unwrap().len() == 2 {
            break;
        }
        tokio::task::yield_now().await;
    }
    let mut notified = replanned.lock().unwrap().clone();
    notified.sort_unstable();
    assert_eq!(notified, result.event_ids);

    let updated = service
        .update(
            owner,
            false,
            calendar_id,
            result.event_ids[0],
            EventChange {
                expected_version: 1,
                target_calendar_id: calendar_id,
                event: timed("Editable import", NOW + 100, NOW + 700),
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.title.as_deref(), Some("Editable import"));
}

#[tokio::test]
async fn ics_import_batch_rejects_invalid_input_before_writes_or_notifications() {
    let (_dir, pool, owner, calendar_id) = setup().await;
    let replanned = Arc::new(Mutex::new(Vec::new()));
    let captured = replanned.clone();
    let service = EventService::new_at_with_notification_replanner(
        pool.clone(),
        NOW,
        Arc::new(move |event_id| captured.lock().unwrap().push(event_id)),
    );
    let batch = EventCreateBatch {
        events: vec![EventCreateItem {
            event: timed("Invalid", NOW + 100, NOW + 700),
            recurrence_rule: None,
            exceptions: vec![EventCreateException {
                recurrence: EventRecurrenceKey::Timed(NOW + 100),
                replacement: None,
            }],
        }],
    };

    assert!(matches!(
        service.create_batch(owner, false, calendar_id, batch).await,
        Err(EventServiceError::InvalidInput)
    ));
    assert_table_counts(&pool, 0, 0, 0).await;
    assert!(replanned.lock().unwrap().is_empty());
}

#[tokio::test]
async fn ics_import_batch_rolls_back_everything_on_database_failure() {
    let (_dir, pool, owner, calendar_id) = setup().await;
    sqlx::query(
        "CREATE TRIGGER fail_second_event_audit BEFORE INSERT ON audit_log
         WHEN NEW.target_type = 'event' AND (SELECT COUNT(*) FROM events) = 2
         BEGIN SELECT RAISE(ABORT, 'forced batch failure'); END",
    )
    .execute(&pool)
    .await
    .unwrap();
    let replanned = Arc::new(Mutex::new(Vec::new()));
    let captured = replanned.clone();
    let service = EventService::new_at_with_notification_replanner(
        pool.clone(),
        NOW,
        Arc::new(move |event_id| captured.lock().unwrap().push(event_id)),
    );

    assert!(matches!(
        service
            .create_batch(owner, false, calendar_id, mixed_batch())
            .await,
        Err(EventServiceError::Database(_))
    ));
    assert_table_counts(&pool, 0, 0, 0).await;
    assert!(replanned.lock().unwrap().is_empty());
}

#[tokio::test]
async fn ics_import_batch_authorizes_with_create_event_permission() {
    let (_dir, pool, _owner, calendar_id) = setup().await;
    let editor = insert_user(&pool, "editor@example.test").await;
    let viewer = insert_user(&pool, "viewer@example.test").await;
    let calendars = CalendarRepository::new(pool.clone());
    calendars
        .add_acl(calendar_id, editor, CalendarRole::Editor, NOW)
        .await
        .unwrap();
    calendars
        .add_acl(calendar_id, viewer, CalendarRole::Viewer, NOW)
        .await
        .unwrap();
    let service = EventService::new_at(pool.clone(), NOW);
    let single = || EventCreateBatch {
        events: vec![EventCreateItem {
            event: timed("Allowed", NOW + 100, NOW + 700),
            recurrence_rule: None,
            exceptions: vec![],
        }],
    };

    service
        .create_batch(editor, false, calendar_id, single())
        .await
        .unwrap();
    assert!(matches!(
        service
            .create_batch(viewer, false, calendar_id, single())
            .await,
        Err(EventServiceError::NotFound)
    ));
    assert_table_counts(&pool, 1, 0, 1).await;
}

#[tokio::test]
async fn ics_import_service_converts_standalone_recurrence_and_reimports() {
    let (_dir, pool, owner, calendar_id) = setup().await;
    let service = IcsImportService::new(EventService::new_at(pool.clone(), NOW));
    let input = b"BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:timed\r\nDTSTART;TZID=Europe/Budapest:20250616T100000\r\nDTEND;TZID=Europe/Budapest:20250616T110000\r\nSUMMARY:Timed\r\nDESCRIPTION:Description\r\nLOCATION:Room\r\nSTATUS:TENTATIVE\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:day\r\nDTSTART;VALUE=DATE:20250617\r\nDTEND;VALUE=DATE:20250618\r\nSUMMARY:All day\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:series\r\nDTSTART:20250619T090000Z\r\nDTEND:20250619T100000Z\r\nSUMMARY:Series\r\nRRULE:FREQ=DAILY;COUNT=3\r\nEXDATE:20250620T090000Z\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:series\r\nRECURRENCE-ID:20250621T090000Z\r\nDTSTART:20250621T120000Z\r\nDTEND:20250621T130000Z\r\nSUMMARY:Moved\r\nSTATUS:CANCELLED\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    let summary = service
        .import(owner, false, calendar_id, input)
        .await
        .unwrap();
    assert_eq!(summary.imported_events, 3);
    assert_eq!(summary.imported_exceptions, 2);
    let timed: (String, Option<String>, String, String, i64, i64) = sqlx::query_as(
        "SELECT title, description, status, event_timezone, timed_start_utc, timed_end_utc
         FROM events WHERE title = 'Timed'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(timed.0, "Timed");
    assert_eq!(timed.1.as_deref(), Some("Description"));
    assert_eq!(timed.2, "tentative");
    assert_eq!(timed.3, "Europe/Budapest");
    assert!(timed.4 < timed.5);
    let all_day: (String, String) = sqlx::query_as(
        "SELECT all_day_start_date, all_day_end_date FROM events WHERE title = 'All day'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(all_day, ("2025-06-17".into(), "2025-06-18".into()));
    let exceptions: Vec<(Option<i64>, bool, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT recurrence_id, is_deleted, title, status
         FROM event_recurrence_exceptions ORDER BY recurrence_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(exceptions.len(), 2);
    assert_eq!(exceptions[0].1, true);
    assert_eq!(exceptions[1].1, false);
    assert_eq!(exceptions[1].2.as_deref(), Some("Moved"));
    assert_eq!(exceptions[1].3.as_deref(), Some("cancelled"));

    let again = service
        .import(owner, false, calendar_id, input)
        .await
        .unwrap();
    assert_eq!(again.imported_events, 3);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM events")
            .fetch_one(&pool)
            .await
            .unwrap(),
        6
    );
}

#[tokio::test]
async fn ics_import_service_rejects_relationship_and_parser_failures_before_writes() {
    let (_dir, pool, owner, calendar_id) = setup().await;
    let service = IcsImportService::new(EventService::new_at(pool.clone(), NOW));
    let orphan = b"BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:missing\nRECURRENCE-ID:20250616T090000Z\nDTSTART:20250616T100000Z\nDTEND:20250616T110000Z\nSUMMARY:Orphan\nEND:VEVENT\nEND:VCALENDAR\n";
    assert!(matches!(
        service.import(owner, false, calendar_id, orphan).await,
        Err(IcsImportError::InvalidCalendar)
    ));
    assert_table_counts(&pool, 0, 0, 0).await;

    let duplicate_exception = b"BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:series\nDTSTART:20250616T090000Z\nDTEND:20250616T100000Z\nSUMMARY:Series\nRRULE:FREQ=DAILY;COUNT=2\nEXDATE:20250617T090000Z\nEND:VEVENT\nBEGIN:VEVENT\nUID:series\nRECURRENCE-ID:20250617T090000Z\nDTSTART:20250617T110000Z\nDTEND:20250617T120000Z\nSUMMARY:Duplicate\nEND:VEVENT\nEND:VCALENDAR\n";
    assert!(matches!(
        service
            .import(owner, false, calendar_id, duplicate_exception)
            .await,
        Err(IcsImportError::InvalidCalendar)
    ));
    assert_table_counts(&pool, 0, 0, 0).await;

    assert!(matches!(
        service
            .import(owner, false, calendar_id, b"not a calendar")
            .await,
        Err(IcsImportError::InvalidCalendar)
    ));
    assert!(matches!(
        service.import(owner, false, calendar_id, &[0xff]).await,
        Err(IcsImportError::InvalidCalendar)
    ));
    assert_table_counts(&pool, 0, 0, 0).await;
}

#[tokio::test]
async fn ics_import_service_maps_event_limits_without_writes() {
    let (_dir, pool, owner, calendar_id) = setup().await;
    let service = IcsImportService::new(EventService::new_at(pool.clone(), NOW));
    let mut input = String::from("BEGIN:VCALENDAR\n");
    for number in 0..=MAX_ICS_IMPORT_EVENTS {
        input.push_str(&format!("BEGIN:VEVENT\nUID:{number}\nDTSTART:20250616T090000Z\nDTEND:20250616T100000Z\nSUMMARY:E\nEND:VEVENT\n"));
    }
    input.push_str("END:VCALENDAR\n");
    assert!(matches!(
        service
            .import(owner, false, calendar_id, input.as_bytes())
            .await,
        Err(IcsImportError::LimitExceeded)
    ));
    assert_table_counts(&pool, 0, 0, 0).await;
}

#[tokio::test]
async fn ics_import_http_allows_writers_and_hides_denials() {
    let app = HttpTestApplication::new().await;
    let path = format!("/api/v1/calendars/{}/import-ics", app.calendar_id);
    let input = b"BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:http-import\r\nDTSTART:20250616T090000Z\r\nDTEND:20250616T100000Z\r\nSUMMARY:Imported\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    for user_id in [app.owner, app.manager, app.editor] {
        let response = app
            .request(
                user_id,
                &path,
                "text/calendar; charset=utf-8",
                input.to_vec(),
            )
            .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({"imported_events": 1, "imported_exceptions": 0})
        );
    }

    for user_id in [app.viewer, app.free_busy, app.unrelated] {
        let response = app
            .request(user_id, &path, "text/calendar", input.to_vec())
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["error"]["code"],
            "not_found"
        );
    }
}

#[tokio::test]
async fn ics_import_http_requires_calendar_content_and_bounds_the_body() {
    let app = HttpTestApplication::new().await;
    let path = format!("/api/v1/calendars/{}/import-ics", app.calendar_id);

    for (content_type, body) in [
        ("application/json", b"{}".to_vec()),
        ("text/calendar", Vec::new()),
    ] {
        let response = app.request(app.owner, &path, content_type, body).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    let malformed = app
        .request(
            app.owner,
            &path,
            "text/calendar",
            b"not a calendar".to_vec(),
        )
        .await;
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
    let malformed_body = malformed.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&malformed_body).unwrap()["error"]["code"],
        "invalid_calendar_file"
    );

    let oversized = app
        .request(
            app.owner,
            &path,
            "text/calendar",
            vec![b'x'; 1024 * 1024 + 1],
        )
        .await;
    assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

fn mixed_batch() -> EventCreateBatch {
    EventCreateBatch {
        events: vec![
            EventCreateItem {
                event: timed("Daily meeting", NOW + 100, NOW + 700),
                recurrence_rule: Some("FREQ=DAILY;COUNT=3".into()),
                exceptions: vec![
                    EventCreateException {
                        recurrence: EventRecurrenceKey::Timed(NOW + 86_400),
                        replacement: None,
                    },
                    EventCreateException {
                        recurrence: EventRecurrenceKey::Timed(NOW + 172_800),
                        replacement: Some(timed("Moved meeting", NOW + 173_000, NOW + 173_600)),
                    },
                ],
            },
            EventCreateItem {
                event: all_day("Conference", "2025-06-16", "2025-06-17"),
                recurrence_rule: Some("FREQ=DAILY;COUNT=2".into()),
                exceptions: vec![EventCreateException {
                    recurrence: EventRecurrenceKey::AllDay("2025-06-17".into()),
                    replacement: None,
                }],
            },
        ],
    }
}

fn timed(title: &str, start_utc: i64, end_utc: i64) -> EventMutation {
    EventMutation {
        title: title.into(),
        description: Some("Description".into()),
        location: Some("Room".into()),
        status: EventStatus::Confirmed,
        timing: EventTiming::Timed {
            start_utc,
            end_utc,
            timezone: "UTC".into(),
        },
    }
}

fn all_day(title: &str, start_date: &str, end_date: &str) -> EventMutation {
    EventMutation {
        title: title.into(),
        description: None,
        location: None,
        status: EventStatus::Tentative,
        timing: EventTiming::AllDay {
            start_date: start_date.into(),
            end_date: end_date.into(),
        },
    }
}

async fn setup() -> (TempDir, sqlx::SqlitePool, i64, i64) {
    let dir = TempDir::new().unwrap();
    let config = AppConfig::with_database_path(
        Environment::Development,
        "127.0.0.1:0",
        None,
        dir.path().join("test.sqlite"),
    )
    .unwrap();
    let pool = connect_and_migrate(&config, Readiness::new())
        .await
        .unwrap();
    let owner = insert_user(&pool, "owner@example.test").await;
    let calendar_id = CalendarRepository::new(pool.clone())
        .create_calendar(
            owner,
            NewCalendar {
                name: "Calendar".into(),
                description: None,
                color: "#123456".into(),
                default_timezone: "UTC".into(),
                default_event_visibility: "private".into(),
                default_notification_rules_json: None,
                created_at: NOW,
            },
        )
        .await
        .unwrap()
        .id;
    (dir, pool, owner, calendar_id)
}

async fn insert_user(pool: &sqlx::SqlitePool, email: &str) -> i64 {
    sqlx::query(
        "INSERT INTO users (normalized_email, display_name, status, created_at)
         VALUES (?, ?, 'active', ?)",
    )
    .bind(email)
    .bind(email)
    .bind(NOW)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid()
}

async fn assert_table_counts(pool: &sqlx::SqlitePool, events: i64, exceptions: i64, audits: i64) {
    let actual_events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(pool)
        .await
        .unwrap();
    let actual_exceptions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM event_recurrence_exceptions")
            .fetch_one(pool)
            .await
            .unwrap();
    let actual_audits: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM audit_log WHERE target_type = 'event'")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        (actual_events, actual_exceptions, actual_audits),
        (events, exceptions, audits)
    );
}

struct HttpTestApplication {
    _dir: TempDir,
    pool: sqlx::SqlitePool,
    key: SecretKey,
    owner: i64,
    manager: i64,
    editor: i64,
    viewer: i64,
    free_busy: i64,
    unrelated: i64,
    calendar_id: i64,
}

impl HttpTestApplication {
    async fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let config = AppConfig::with_database_path(
            Environment::Development,
            "127.0.0.1:0",
            None,
            dir.path().join("http-test.sqlite"),
        )
        .unwrap();
        let pool = connect_and_migrate(&config, Readiness::new())
            .await
            .unwrap();
        let owner = insert_user(&pool, "http-owner@example.test").await;
        let manager = insert_user(&pool, "http-manager@example.test").await;
        let editor = insert_user(&pool, "http-editor@example.test").await;
        let viewer = insert_user(&pool, "http-viewer@example.test").await;
        let free_busy = insert_user(&pool, "http-freebusy@example.test").await;
        let unrelated = insert_user(&pool, "http-unrelated@example.test").await;
        let calendars = CalendarRepository::new(pool.clone());
        let calendar_id = calendars
            .create_calendar(
                owner,
                NewCalendar {
                    name: "HTTP calendar".into(),
                    description: None,
                    color: "#123456".into(),
                    default_timezone: "UTC".into(),
                    default_event_visibility: "private".into(),
                    default_notification_rules_json: None,
                    created_at: NOW,
                },
            )
            .await
            .unwrap()
            .id;
        for (user_id, role) in [
            (manager, CalendarRole::Manager),
            (editor, CalendarRole::Editor),
            (viewer, CalendarRole::Viewer),
            (free_busy, CalendarRole::FreeBusyViewer),
        ] {
            calendars
                .add_acl(calendar_id, user_id, role, NOW)
                .await
                .unwrap();
        }
        Self {
            _dir: dir,
            pool,
            key: SecretKey::new([83; 32]),
            owner,
            manager,
            editor,
            viewer,
            free_busy,
            unrelated,
            calendar_id,
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
                false,
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
            None,
            None,
            None,
        )
    }

    async fn request(
        &self,
        user_id: i64,
        path: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> axum::response::Response {
        let token = self.key.generate_token();
        let hash = self.key.hash_token(TokenDomain::Session, &token);
        sqlx::query(
            "INSERT INTO sessions (user_id, session_hash, expires_at, revoked_at, created_at, last_seen_at)
             VALUES (?, ?, ?, NULL, ?, ?)",
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
                    .method(Method::POST)
                    .uri(path)
                    .header(
                        COOKIE,
                        format!("__Host-commoncal_session={}", token.expose()),
                    )
                    .header("content-type", content_type)
                    .header("origin", ORIGIN)
                    .header("sec-fetch-site", "same-origin")
                    .header("x-csrf-token", csrf.expose())
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap()
    }
}
