use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::DateTime;
use commoncal_backend::{
    caldav::{auth::CaldavAccountService, http::build_caldav_router, repository::CaldavRepository},
    calendar::{CalendarRepository, CalendarService, NewCalendar},
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    event::{EventMutation, EventService, EventStatus, EventTiming},
    http::{Readiness, build_router_with_calendars_and_events},
    security::{SecretKey, TokenDomain},
    sessions::{SessionManager, SessionSecurityConfig},
};
use http_body_util::BodyExt;
use sqlx::SqlitePool;
use tempfile::TempDir;
use tower::ServiceExt;

fn instant(s: &str) -> i64 {
    DateTime::parse_from_rfc3339(s).unwrap().timestamp()
}
const BEFORE: &str = "2026-07-15T14:59:59Z";
const MASKED: &str = "2026-07-15T15:00:00Z";
const QUERY: &str = r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><D:prop><D:getetag/><C:calendar-data/></D:prop><C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT"/></C:comp-filter></C:filter></C:calendar-query>"#;
fn sync(token: &str) -> String {
    format!(
        r#"<D:sync-collection xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><D:sync-token>{token}</D:sync-token><D:sync-level>1</D:sync-level><D:prop><D:getetag/><C:calendar-data/></D:prop></D:sync-collection>"#
    )
}
fn token(body: &str) -> String {
    body.split("<D:sync-token>")
        .nth(1)
        .unwrap()
        .split("</D:sync-token>")
        .next()
        .unwrap()
        .to_owned()
}

struct Fixture {
    _temp: TempDir,
    pool: SqlitePool,
    key: SecretKey,
    user: i64,
    calendar: i64,
    other: i64,
    event: i64,
    href: String,
    other_href: String,
    principal: String,
    username: String,
    password: String,
}
impl Fixture {
    async fn new(enabled: bool, timezone: &str) -> Self {
        let temp = TempDir::new().unwrap();
        let cfg = AppConfig::with_database_path(
            Environment::Development,
            "127.0.0.1:3000",
            None,
            temp.path().join("calendar.sqlite"),
        )
        .unwrap();
        let pool = connect_and_migrate(&cfg, Readiness::new()).await.unwrap();
        let user: i64 = sqlx::query_scalar("INSERT INTO users (normalized_email, display_name, status, created_at) VALUES ('worklife@example.test', 'Owner', 'registered', 100) RETURNING id").fetch_one(&pool).await.unwrap();
        let key = SecretKey::generate();
        let accounts = CaldavAccountService::new_at(
            pool.clone(),
            key.clone(),
            url::Url::parse("http://127.0.0.1:3000").unwrap(),
            instant(BEFORE),
        );
        let issued = accounts
            .issue_credential(user, "Test device".into())
            .await
            .unwrap();
        let principal = accounts.status(user).await.unwrap().principal_id.unwrap();
        let mut entries = Vec::new();
        for name in ["Protected work", "Unprotected personal"] {
            let calendar = CalendarRepository::new(pool.clone())
                .create_calendar(
                    user,
                    NewCalendar {
                        name: name.into(),
                        description: None,
                        color: "#3367d6".into(),
                        default_timezone: timezone.into(),
                        default_event_visibility: "default".into(),
                        default_notification_rules_json: None,
                        created_at: instant(BEFORE),
                    },
                )
                .await
                .unwrap()
                .id;
            let event = EventService::new_at(pool.clone(), instant(BEFORE))
                .create(
                    user,
                    false,
                    calendar,
                    EventMutation {
                        title: "Private work details".into(),
                        description: Some("Never leak this description".into()),
                        location: None,
                        status: EventStatus::Confirmed,
                        // The event is on a weekend; visibility must depend on request time instead.
                        timing: EventTiming::Timed {
                            start_utc: instant("2026-07-18T12:00:00Z"),
                            end_utc: instant("2026-07-18T13:00:00Z"),
                            timezone: "UTC".into(),
                        },
                    },
                )
                .await
                .unwrap()
                .id;
            let resource = CaldavRepository::new(pool.clone())
                .ensure_resource(calendar, event, instant(BEFORE))
                .await
                .unwrap();
            entries.push((
                calendar,
                event,
                format!(
                    "/dav/calendars/{principal}/{calendar}/{}.ics",
                    resource.resource_name
                ),
            ));
        }
        sqlx::query("UPDATE calendars SET work_life_balance_enabled = ? WHERE id = ?")
            .bind(enabled)
            .bind(entries[0].0)
            .execute(&pool)
            .await
            .unwrap();
        Self {
            _temp: temp,
            pool,
            key,
            user,
            calendar: entries[0].0,
            event: entries[0].1,
            href: entries[0].2.clone(),
            other: entries[1].0,
            other_href: entries[1].2.clone(),
            principal,
            username: issued.username,
            password: issued.password.expose().to_owned(),
        }
    }
    fn collection(&self, id: i64) -> String {
        format!("/dav/calendars/{}/{id}/", self.principal)
    }
    async fn send(
        &self,
        at: &str,
        method: &str,
        uri: &str,
        body: &str,
        depth: &str,
    ) -> (StatusCode, String) {
        let accounts = CaldavAccountService::new_at(
            self.pool.clone(),
            self.key.clone(),
            url::Url::parse("http://127.0.0.1:3000").unwrap(),
            instant(at),
        );
        let auth = STANDARD.encode(format!("{}:{}", self.username, self.password));
        let response = build_caldav_router(accounts)
            .oneshot(
                Request::builder()
                    .method(Method::from_bytes(method.as_bytes()).unwrap())
                    .uri(uri)
                    .header(header::AUTHORIZATION, format!("Basic {auth}"))
                    .header("depth", depth)
                    .header(header::CONTENT_TYPE, "application/xml")
                    .body(Body::from(body.to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }
}

#[tokio::test]
async fn enabled_calendar_is_hidden_at_exact_local_cutoff() {
    let f = Fixture::new(true, "Europe/Budapest").await;
    assert_eq!(
        f.send(BEFORE, "GET", &f.href, "", "0").await.0,
        StatusCode::OK
    );
    assert_eq!(
        f.send(MASKED, "GET", &f.href, "", "0").await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn masked_reads_hide_payloads_and_listings_without_changing_events_or_rest() {
    let f = Fixture::new(true, "Europe/Budapest").await;
    let before: (String, i64) = sqlx::query_as("SELECT title, version FROM events WHERE id = ?")
        .bind(f.event)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    for at in [MASKED, "2026-07-17T22:00:00Z", "2026-07-19T12:00:00Z"] {
        for method in ["GET", "HEAD", "PROPFIND"] {
            let (status, body) = f.send(at, method, &f.href, "", "0").await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{at} {method}: {body}");
            assert!(!body.contains("Private work details"));
        }
        for (method, body) in [
            ("PROPFIND", String::new()),
            ("REPORT", QUERY.into()),
            ("REPORT", sync("")),
        ] {
            let (status, body) = f
                .send(at, method, &f.collection(f.calendar), &body, "1")
                .await;
            assert_eq!(status, StatusCode::MULTI_STATUS, "{method}: {body}");
            assert!(!body.contains("BEGIN:VEVENT"));
            assert!(!body.contains("Private work details"));
            assert!(!body.contains(&f.href));
            assert_eq!(
                body.matches("<D:response>").count(),
                if method == "PROPFIND" { 1 } else { 0 }
            );
        }
        let multiget = format!(
            r#"<C:calendar-multiget xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><D:prop><C:calendar-data/></D:prop><D:href>{}</D:href></C:calendar-multiget>"#,
            f.href
        );
        let (status, body) = f
            .send(at, "REPORT", &f.collection(f.calendar), &multiget, "1")
            .await;
        assert_eq!(status, StatusCode::MULTI_STATUS);
        assert!(body.contains("404 Not Found"));
        assert!(!body.contains("BEGIN:VEVENT"));
        // Other calendars owned by the same user remain independent.
        assert_eq!(
            f.send(at, "GET", &f.other_href, "", "0").await.0,
            StatusCode::OK
        );
        let (status, body) = f
            .send(at, "PROPFIND", &f.collection(f.other), "", "1")
            .await;
        assert_eq!(status, StatusCode::MULTI_STATUS);
        assert!(body.contains(&f.other_href));
    }
    let after: (String, i64) = sqlx::query_as("SELECT title, version FROM events WHERE id = ?")
        .bind(f.event)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(before, after);
    // The browser REST route remains visible at the same masked request instant.
    let now = instant(MASKED);
    let session = f.key.generate_token();
    let hash = f.key.hash_token(TokenDomain::Session, &session);
    sqlx::query("INSERT INTO sessions (user_id, session_hash, expires_at, revoked_at, created_at, last_seen_at) VALUES (?, ?, ?, NULL, ?, ?)")
        .bind(f.user).bind(hash.as_bytes().as_slice()).bind(now + 1000).bind(now - 10).bind(now - 10).execute(&f.pool).await.unwrap();
    let app = build_router_with_calendars_and_events(
        Readiness::new(),
        SessionManager::new_at(
            f.pool.clone(),
            f.key.clone(),
            SessionSecurityConfig::new(300, 60, "https://commoncal.test").unwrap(),
            now,
        ),
        CalendarService::new_at(f.pool.clone(), now),
        EventService::new_at(f.pool.clone(), now),
        None,
        None,
        None,
    );
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/calendars/{}/events/{}",
                    f.calendar, f.event
                ))
                .header(
                    header::COOKIE,
                    format!("__Host-commoncal_session={}", session.expose()),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("Private work details"));
}

#[tokio::test]
async fn visibility_depends_on_request_time_and_returns_during_allowed_hours() {
    let f = Fixture::new(true, "Europe/Budapest").await;
    for at in [BEFORE, "2026-07-19T22:00:00Z", "2026-01-14T15:59:59Z"] {
        let (status, body) = f.send(at, "GET", &f.href, "", "0").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Private work details"));
        let (status, body) = f
            .send(at, "REPORT", &f.collection(f.calendar), QUERY, "1")
            .await;
        assert_eq!(status, StatusCode::MULTI_STATUS);
        assert!(body.contains("BEGIN:VEVENT"));
    }
    assert_eq!(
        f.send("2026-01-14T16:00:00Z", "GET", &f.href, "", "0")
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let disabled = Fixture::new(false, "Europe/Budapest").await;
    assert_eq!(
        disabled
            .send(MASKED, "GET", &disabled.href, "", "0")
            .await
            .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn sync_tokens_change_with_visibility_to_restore_events_after_masking() {
    let f = Fixture::new(true, "Europe/Budapest").await;
    let (status, body) = f
        .send(BEFORE, "REPORT", &f.collection(f.calendar), &sync(""), "1")
        .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert!(body.contains("BEGIN:VEVENT"));
    let visible_token = token(&body);
    let (status, body) = f
        .send(
            MASKED,
            "REPORT",
            &f.collection(f.calendar),
            &sync(&visible_token),
            "1",
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body.contains("valid-sync-token"));
    let (status, body) = f
        .send(MASKED, "REPORT", &f.collection(f.calendar), &sync(""), "1")
        .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert!(!body.contains(&f.href));
    let hidden_token = token(&body);
    let (status, body) = f
        .send(
            MASKED,
            "REPORT",
            &f.collection(f.calendar),
            &sync(&hidden_token),
            "1",
        )
        .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert!(!body.contains(&f.href));
    let (status, body) = f
        .send(
            "2026-07-16T07:00:00Z",
            "REPORT",
            &f.collection(f.calendar),
            &sync(&hidden_token),
            "1",
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body.contains("valid-sync-token"));
    let (status, body) = f
        .send(
            "2026-07-16T07:00:00Z",
            "REPORT",
            &f.collection(f.calendar),
            &sync(""),
            "1",
        )
        .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert!(body.contains("BEGIN:VEVENT"));
}

#[tokio::test]
async fn invalid_legacy_zone_fails_closed_only_for_enabled_protection() {
    let f = Fixture::new(true, "Mars/Olympus").await;
    assert_eq!(
        f.send(BEFORE, "GET", &f.href, "", "0").await.0,
        StatusCode::NOT_FOUND
    );
    sqlx::query("UPDATE calendars SET work_life_balance_enabled = 0 WHERE id = ?")
        .bind(f.calendar)
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        f.send(BEFORE, "GET", &f.href, "", "0").await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn masked_calendar_remains_discoverable_without_event_listings() {
    let f = Fixture::new(true, "Europe/Budapest").await;
    let home = format!("/dav/calendars/{}/", f.principal);
    let (status, body) = f.send(MASKED, "PROPFIND", &home, "", "1").await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert!(body.contains(&f.collection(f.calendar)));
    assert!(body.contains(&f.collection(f.other)));
    assert!(!body.contains(&f.href));
    assert!(!body.contains("BEGIN:VEVENT"));
}
