use axum::{
    Router,
    body::Body,
    http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use commoncal_backend::{
    caldav::{auth::CaldavAccountService, http::build_caldav_router, repository::CaldavRepository},
    calendar::{CalendarRepository, NewCalendar},
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    event::{
        EventChange, EventMutation, EventRange, EventService, EventServiceError, EventStatus,
        EventTiming,
    },
    external_feed::{ExternalFeedService, FeedError, FeedFetcher, FetchResponse, NewFeed},
    http::Readiness,
    ics::{IcsParserLimits, parse_calendar},
    security::SecretKey,
};
use http_body_util::BodyExt;
use sqlx::SqlitePool;
use tempfile::TempDir;
use tower::ServiceExt;

async fn setup() -> (TempDir, SqlitePool) {
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
    (temp_dir, pool)
}

async fn create_user(pool: &SqlitePool, email: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (normalized_email, display_name, status, created_at)
         VALUES (?, ?, 'registered', ?) RETURNING id",
    )
    .bind(email)
    .bind(email)
    .bind(100_i64)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn create_calendar(pool: &SqlitePool, owner_user_id: i64) -> i64 {
    CalendarRepository::new(pool.clone())
        .create_calendar(
            owner_user_id,
            NewCalendar {
                name: "Work".to_owned(),
                description: None,
                color: "#3367d6".to_owned(),
                default_timezone: "UTC".to_owned(),
                default_event_visibility: "default".to_owned(),
                default_notification_rules_json: None,
                created_at: 200,
            },
        )
        .await
        .unwrap()
        .id
}

fn timed_mutation() -> EventMutation {
    EventMutation {
        title: "Planning".to_owned(),
        description: Some("Quarterly planning".to_owned()),
        location: Some("Room 1".to_owned()),
        status: EventStatus::Confirmed,
        timing: EventTiming::Timed {
            start_utc: 1_768_435_200,
            end_utc: 1_768_438_800,
            timezone: "UTC".to_owned(),
        },
    }
}

async fn create_event(pool: &SqlitePool, user_id: i64, calendar_id: i64) -> i64 {
    let service = EventService::new_at(pool.clone(), 300);
    service
        .create(user_id, false, calendar_id, timed_mutation())
        .await
        .unwrap()
        .id
}

fn basic_header(username: &str, password: &str) -> HeaderValue {
    let encoded = B64.encode(format!("{username}:{password}"));
    HeaderValue::from_str(&format!("Basic {encoded}")).unwrap()
}

async fn request(
    app: Router,
    method: Method,
    uri: &str,
    auth: Option<(&str, &str)>,
) -> (StatusCode, HeaderMap, String) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some((username, password)) = auth {
        builder = builder.header(header::AUTHORIZATION, basic_header(username, password));
    }
    let response = app
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, headers, String::from_utf8_lossy(&body).into_owned())
}

/// Owner with a credential, one calendar, one timed event, and a DAV mapping.
async fn scenario() -> (
    TempDir,
    SqlitePool,
    CaldavAccountService,
    i64,
    String,
    String,
    i64,
    i64,
    String,
) {
    let (temp_dir, pool) = setup().await;
    let key = SecretKey::generate();
    let accounts = CaldavAccountService::new_at(
        pool.clone(),
        key,
        url::Url::parse("http://127.0.0.1:3000").unwrap(),
        1000,
    );
    let user_id = create_user(&pool, "owner@example.test").await;
    let issued = accounts
        .issue_credential(user_id, "Phone".into())
        .await
        .unwrap();
    let principal_id = accounts
        .status(user_id)
        .await
        .unwrap()
        .principal_id
        .unwrap();
    let calendar_id = create_calendar(&pool, user_id).await;
    let event_id = create_event(&pool, user_id, calendar_id).await;
    let repository = CaldavRepository::new(pool.clone());
    let resource = repository
        .ensure_resource(calendar_id, event_id, 1000)
        .await
        .unwrap();
    (
        temp_dir,
        pool,
        accounts,
        user_id,
        issued.password.expose().to_owned(),
        principal_id,
        calendar_id,
        event_id,
        resource.resource_name,
    )
}

#[tokio::test]
async fn migration_enforces_unique_resource_identity() {
    let (_temp_dir, pool) = setup().await;
    let user_id = create_user(&pool, "owner@example.test").await;
    let calendar_id = create_calendar(&pool, user_id).await;
    let event_id = create_event(&pool, user_id, calendar_id).await;
    let other_event_id = create_event(&pool, user_id, calendar_id).await;
    let repository = CaldavRepository::new(pool.clone());
    let resource = repository
        .ensure_resource(calendar_id, event_id, 1000)
        .await
        .unwrap();

    // Duplicate (calendar_id, uid) must fail.
    assert!(
        insert_resource(
            &pool,
            other_event_id,
            calendar_id,
            &resource.uid,
            "other-name"
        )
        .await
        .is_err()
    );
    // Duplicate (calendar_id, resource_name) must fail.
    assert!(
        insert_resource(
            &pool,
            other_event_id,
            calendar_id,
            "other-uid",
            &resource.resource_name
        )
        .await
        .is_err()
    );
    // The same uid in a different calendar is allowed.
    let other_calendar_id = create_calendar(&pool, user_id).await;
    insert_resource(
        &pool,
        other_event_id,
        other_calendar_id,
        &resource.uid,
        "other-name",
    )
    .await
    .unwrap();
}

async fn insert_resource(
    pool: &SqlitePool,
    event_id: i64,
    calendar_id: i64,
    uid: &str,
    name: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO caldav_event_resources (
            event_id, calendar_id, uid, resource_name, created_at, updated_at
         ) VALUES (?, ?, ?, ?, 1, 1)",
    )
    .bind(event_id)
    .bind(calendar_id)
    .bind(uid)
    .bind(name)
    .execute(pool)
    .await
    .map(|_| ())
}

#[tokio::test]
async fn ensure_resource_is_stable_and_idempotent() {
    let (_temp_dir, pool) = setup().await;
    let user_id = create_user(&pool, "owner@example.test").await;
    let calendar_id = create_calendar(&pool, user_id).await;
    let event_id = create_event(&pool, user_id, calendar_id).await;
    let other_event_id = create_event(&pool, user_id, calendar_id).await;
    let repository = CaldavRepository::new(pool.clone());

    let first = repository
        .ensure_resource(calendar_id, event_id, 1000)
        .await
        .unwrap();
    let second = repository
        .ensure_resource(calendar_id, event_id, 2000)
        .await
        .unwrap();
    assert_eq!(first, second);

    let other = repository
        .ensure_resource(calendar_id, other_event_id, 1000)
        .await
        .unwrap();
    assert_ne!(first.uid, other.uid);
    assert_ne!(first.resource_name, other.resource_name);
}

#[tokio::test]
async fn get_returns_valid_vcalendar_with_stable_url_and_uid() {
    let (
        _temp_dir,
        pool,
        accounts,
        _user_id,
        password,
        principal_id,
        calendar_id,
        _event_id,
        resource_name,
    ) = scenario().await;

    let app = build_caldav_router(accounts);
    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/{resource_name}.ics");
    let (status, headers, body) = request(
        app,
        Method::GET,
        &uri,
        Some(("owner@example.test", &password)),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "text/calendar; charset=utf-8"
    );
    assert!(headers.get(header::ETAG).is_some());
    assert!(body.starts_with("BEGIN:VCALENDAR\r\n"));
    assert!(body.ends_with("END:VCALENDAR\r\n"));

    // The body must parse as a valid calendar with the mapped UID.
    let parsed = parse_calendar(&body, IcsParserLimits::default()).unwrap();
    assert_eq!(parsed.events.len(), 1);
    let event = &parsed.events[0];
    let repository = CaldavRepository::new(pool.clone());
    let resource = repository
        .resolve_by_name(calendar_id, &resource_name)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event.uid, resource.uid);
    assert_eq!(event.summary, "Planning");
    assert_eq!(event.location.as_deref(), Some("Room 1"));
    match &event.timing {
        commoncal_backend::ics::NormalizedTiming::Timed {
            starts_at, ends_at, ..
        } => {
            assert_eq!(starts_at.timestamp(), 1_768_435_200);
            assert_eq!(ends_at.timestamp(), 1_768_438_800);
        }
        other => panic!("expected timed event, got {other:?}"),
    }
}

#[tokio::test]
async fn head_matches_get_headers_without_body() {
    let (
        _temp_dir,
        _pool,
        accounts,
        _user_id,
        password,
        principal_id,
        calendar_id,
        _event_id,
        resource_name,
    ) = scenario().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/{resource_name}.ics");
    let (get_status, get_headers, get_body) = request(
        build_caldav_router(accounts.clone()),
        Method::GET,
        &uri,
        Some(("owner@example.test", &password)),
    )
    .await;
    let (head_status, head_headers, head_body) = request(
        build_caldav_router(accounts),
        Method::HEAD,
        &uri,
        Some(("owner@example.test", &password)),
    )
    .await;

    assert_eq!(head_status, get_status);
    assert_eq!(
        head_headers.get(header::ETAG),
        get_headers.get(header::ETAG)
    );
    assert_eq!(
        head_headers.get(header::CONTENT_TYPE),
        get_headers.get(header::CONTENT_TYPE)
    );
    assert_eq!(
        head_headers.get(header::CONTENT_LENGTH),
        get_headers.get(header::CONTENT_LENGTH)
    );
    assert!(head_body.is_empty(), "HEAD must not return a body");
    assert!(!get_body.is_empty());
}

#[tokio::test]
async fn etag_changes_only_with_canonical_content() {
    let (
        _temp_dir,
        pool,
        accounts,
        user_id,
        password,
        principal_id,
        calendar_id,
        event_id,
        resource_name,
    ) = scenario().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/{resource_name}.ics");
    let auth = ("owner@example.test", password.as_str());

    let (_, headers_before, _) = request(
        build_caldav_router(accounts.clone()),
        Method::GET,
        &uri,
        Some(auth),
    )
    .await;
    let etag_before = headers_before.get(header::ETAG).unwrap().to_str().unwrap();

    // Unchanged event: same ETag.
    let (_, headers_same, _) = request(
        build_caldav_router(accounts.clone()),
        Method::GET,
        &uri,
        Some(auth),
    )
    .await;
    assert_eq!(
        headers_same.get(header::ETAG).unwrap(),
        etag_before,
        "ETag must be stable while content is unchanged"
    );

    // Edit the event through the domain service: ETag must rotate.
    let service = EventService::new_at(pool.clone(), 400);
    let mut change = timed_mutation();
    change.title = "Renamed".to_owned();
    service
        .update(
            user_id,
            false,
            calendar_id,
            event_id,
            EventChange {
                expected_version: 1,
                target_calendar_id: calendar_id,
                event: change,
            },
        )
        .await
        .unwrap();

    let (_, headers_after, body_after) =
        request(build_caldav_router(accounts), Method::GET, &uri, Some(auth)).await;
    let etag_after = headers_after.get(header::ETAG).unwrap().to_str().unwrap();
    assert_ne!(etag_before, etag_after, "ETag must change with content");
    assert!(body_after.contains("SUMMARY:Renamed"));
}

#[tokio::test]
async fn propfind_depth_one_lists_event_resource_with_matching_etag() {
    let (
        _temp_dir,
        _pool,
        accounts,
        _user_id,
        password,
        principal_id,
        calendar_id,
        _event_id,
        resource_name,
    ) = scenario().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/");
    let builder = Request::builder()
        .method(Method::from_bytes(b"PROPFIND").unwrap())
        .uri(&uri)
        .header("depth", "1")
        .header(
            header::AUTHORIZATION,
            basic_header("owner@example.test", &password),
        );
    let response = build_caldav_router(accounts.clone())
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&body);
    let href = format!("/dav/calendars/{principal_id}/{calendar_id}/{resource_name}.ics");
    assert!(text.contains(&href));
    assert!(text.contains("<D:resourcetype/>"));
    assert!(text.contains("<D:getetag>"));

    // The listed ETag must match the resource GET ETag.
    let (status, headers, _) = request(
        build_caldav_router(accounts),
        Method::GET,
        &href,
        Some(("owner@example.test", &password)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let etag = headers.get(header::ETAG).unwrap().to_str().unwrap();
    assert!(
        xml_tree(&text)
            .descendants("DAV:", "getetag")
            .iter()
            .any(|node| node.text == etag)
    );
}

#[tokio::test]
async fn propfind_lists_event_without_preexisting_mapping_and_url_is_stable() {
    let (temp_dir, pool) = setup().await;
    let key = SecretKey::generate();
    let accounts = CaldavAccountService::new_at(
        pool.clone(),
        key,
        url::Url::parse("http://127.0.0.1:3000").unwrap(),
        1000,
    );
    let user_id = create_user(&pool, "owner@example.test").await;
    let password = accounts
        .issue_credential(user_id, "Phone".into())
        .await
        .unwrap()
        .password
        .expose()
        .to_owned();
    let principal_id = accounts
        .status(user_id)
        .await
        .unwrap()
        .principal_id
        .unwrap();
    let calendar_id = create_calendar(&pool, user_id).await;
    // Create the event but do NOT create a DAV mapping up front.
    create_event(&pool, user_id, calendar_id).await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/");
    let auth = ("owner@example.test", password.as_str());

    let first = propfind_calendar(&accounts, &uri, auth).await;
    let first_name =
        extract_resource_name(&first).expect("first listing must contain an event resource");

    // A second listing must expose the same stable resource name.
    let second = propfind_calendar(&accounts, &uri, auth).await;
    let second_name =
        extract_resource_name(&second).expect("second listing must contain an event resource");
    assert_eq!(
        first_name, second_name,
        "resource URL must be stable across listings"
    );

    // The listed resource must be GET-able and parse as a valid VCALENDAR.
    let href = format!("/dav/calendars/{principal_id}/{calendar_id}/{first_name}.ics");
    let (status, _headers, body) = request(
        build_caldav_router(accounts.clone()),
        Method::GET,
        &href,
        Some(auth),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let parsed = parse_calendar(&body, IcsParserLimits::default()).unwrap();
    assert_eq!(parsed.events.len(), 1);
    let repository = CaldavRepository::new(pool.clone());
    let resource = repository
        .resolve_by_name(calendar_id, &first_name)
        .await
        .unwrap()
        .expect("mapping must be persisted on first exposure");
    assert_eq!(parsed.events[0].uid, resource.uid);
    let _ = temp_dir;
}

async fn propfind_calendar(
    accounts: &CaldavAccountService,
    uri: &str,
    auth: (&str, &str),
) -> String {
    let builder = Request::builder()
        .method(Method::from_bytes(b"PROPFIND").unwrap())
        .uri(uri)
        .header("depth", "1")
        .header(header::AUTHORIZATION, basic_header(auth.0, auth.1));
    let response = build_caldav_router(accounts.clone())
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&body).into_owned()
}

/// Return the `.ics` resource name listed in a calendar PROPFIND response.
fn extract_resource_name(body: &str) -> Option<String> {
    for line in body.lines() {
        if line.contains("<D:href>") && line.contains(".ics") {
            let start = line.find("<D:href>")? + "<D:href>".len();
            let href = line[start..].trim_end().strip_suffix("</D:href>")?;
            let name = href.rsplit('/').next()?.strip_suffix(".ics")?;
            return Some(name.to_owned());
        }
    }
    None
}

#[tokio::test]
async fn unknown_resource_and_cross_user_are_404() {
    let (
        _temp_dir,
        pool,
        accounts,
        _user_id,
        password,
        principal_id,
        calendar_id,
        _event_id,
        resource_name,
    ) = scenario().await;

    // Unknown resource name for the owner's calendar.
    let (status, _, _) = request(
        build_caldav_router(accounts.clone()),
        Method::GET,
        &format!("/dav/calendars/{principal_id}/{calendar_id}/does-not-exist.ics"),
        Some(("owner@example.test", &password)),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A different authenticated user cannot read the resource with its real
    // name; existence must not leak.
    let intruder_id = create_user(&pool, "intruder@example.test").await;
    let intruder_password = accounts
        .issue_credential(intruder_id, "Phone".into())
        .await
        .unwrap()
        .password
        .expose()
        .to_owned();
    let (status, _, body) = request(
        build_caldav_router(accounts),
        Method::GET,
        &format!("/dav/calendars/{principal_id}/{calendar_id}/{resource_name}.ics"),
        Some(("intruder@example.test", &intruder_password)),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.is_empty());
}

#[tokio::test]
async fn unauthenticated_get_is_401() {
    let (
        _temp_dir,
        _pool,
        accounts,
        _user_id,
        _password,
        principal_id,
        calendar_id,
        _event_id,
        resource_name,
    ) = scenario().await;

    let (status, headers, _) = request(
        build_caldav_router(accounts),
        Method::GET,
        &format!("/dav/calendars/{principal_id}/{calendar_id}/{resource_name}.ics"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(headers.get(header::WWW_AUTHENTICATE).is_some());
}

// --- T07: Create from Apple ---

/// Setup for create tests: owner + credential + calendar, no pre-existing event.
async fn setup_create() -> (
    TempDir,
    SqlitePool,
    CaldavAccountService,
    i64,
    String,
    String,
    i64,
) {
    let (temp_dir, pool) = setup().await;
    let key = SecretKey::generate();
    let accounts = CaldavAccountService::new_at(
        pool.clone(),
        key,
        url::Url::parse("http://127.0.0.1:3000").unwrap(),
        1000,
    );
    let user_id = create_user(&pool, "owner@example.test").await;
    let password = accounts
        .issue_credential(user_id, "Phone".into())
        .await
        .unwrap()
        .password
        .expose()
        .to_owned();
    let principal_id = accounts
        .status(user_id)
        .await
        .unwrap()
        .principal_id
        .unwrap();
    let calendar_id = create_calendar(&pool, user_id).await;
    (
        temp_dir,
        pool,
        accounts,
        user_id,
        password,
        principal_id,
        calendar_id,
    )
}

fn vcalendar_body(uid: &str, summary: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         PRODID:-//Test//Test 1.0//EN\r\n\
         BEGIN:VEVENT\r\n\
         UID:{uid}\r\n\
         DTSTAMP:20260101T000000Z\r\n\
         DTSTART:20260115T000000Z\r\n\
         DTEND:20260115T010000Z\r\n\
         SUMMARY:{summary}\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    )
}

async fn put_request(
    app: Router,
    uri: &str,
    auth: Option<(&str, &str)>,
    body: &str,
    if_none_match: Option<&str>,
) -> (StatusCode, HeaderMap, String) {
    let mut builder = Request::builder()
        .method(Method::PUT)
        .uri(uri)
        .header(header::CONTENT_TYPE, "text/calendar; charset=utf-8");
    if let Some((username, password)) = auth {
        builder = builder.header(header::AUTHORIZATION, basic_header(username, password));
    }
    if let Some(value) = if_none_match {
        builder = builder.header(header::IF_NONE_MATCH, value);
    }
    let response = app
        .oneshot(builder.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, headers, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test]
async fn put_creates_event_visible_in_both_systems() {
    let (temp_dir, pool, accounts, user_id, password, principal_id, calendar_id) =
        setup_create().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/new-event.ics");
    let body = vcalendar_body("test-uid-1", "New Event");
    let (status, headers, response_body) = put_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(("owner@example.test", &password)),
        &body,
        Some("*"),
    )
    .await;

    assert_eq!(status, StatusCode::CREATED);
    assert!(headers.get(header::LOCATION).is_some());
    assert!(headers.get(header::ETAG).is_none());
    assert!(response_body.contains("BEGIN:VCALENDAR"));
    assert!(response_body.contains("SUMMARY:New Event"));

    // Verify the event is visible in Happening (domain store).
    let service = EventService::new_at(pool.clone(), 1000);
    let events = service
        .list(
            user_id,
            false,
            calendar_id,
            EventRange {
                start_utc: 1_767_225_600,
                end_utc: 1_770_000_000,
                start_date: "2026-01-01".to_owned(),
                end_date: "2026-02-01".to_owned(),
            },
        )
        .await
        .unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.title.as_deref() == Some("New Event")),
        "created event must be visible in Happening"
    );

    // Verify the event is readable via DAV GET.
    let (get_status, _, get_body) = request(
        build_caldav_router(accounts),
        Method::GET,
        &uri,
        Some(("owner@example.test", &password)),
    )
    .await;
    assert_eq!(get_status, StatusCode::OK);
    assert!(get_body.contains("SUMMARY:New Event"));
    let _ = temp_dir;
}

#[tokio::test]
async fn put_duplicate_create_fails_412() {
    let (temp_dir, _pool, accounts, _user_id, password, principal_id, calendar_id) =
        setup_create().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/dup-event.ics");
    let body = vcalendar_body("dup-uid-1", "Dup Event");

    // First create succeeds.
    let (first_status, _, _) = put_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(("owner@example.test", &password)),
        &body,
        Some("*"),
    )
    .await;
    assert_eq!(first_status, StatusCode::CREATED);

    // Second create at the same URL fails with 412.
    let (second_status, _, _) = put_request(
        build_caldav_router(accounts),
        &uri,
        Some(("owner@example.test", &password)),
        &body,
        Some("*"),
    )
    .await;
    assert_eq!(second_status, StatusCode::PRECONDITION_FAILED);
    let _ = temp_dir;
}

#[tokio::test]
async fn put_missing_if_none_match_fails_428() {
    let (temp_dir, _pool, accounts, _user_id, password, principal_id, calendar_id) =
        setup_create().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/no-precondition.ics");
    let body = vcalendar_body("no-precondition-uid", "No Precondition");

    let (status, _, _) = put_request(
        build_caldav_router(accounts),
        &uri,
        Some(("owner@example.test", &password)),
        &body,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::PRECONDITION_REQUIRED);
    let _ = temp_dir;
}

#[tokio::test]
async fn put_uid_conflict_fails_403() {
    let (temp_dir, _pool, accounts, _user_id, password, principal_id, calendar_id) =
        setup_create().await;

    let body = vcalendar_body("shared-uid", "Shared UID Event");

    // First create with UID "shared-uid" at URL "first.ics" succeeds.
    let first_uri = format!("/dav/calendars/{principal_id}/{calendar_id}/first.ics");
    let (first_status, _, _) = put_request(
        build_caldav_router(accounts.clone()),
        &first_uri,
        Some(("owner@example.test", &password)),
        &body,
        Some("*"),
    )
    .await;
    assert_eq!(first_status, StatusCode::CREATED);

    // Second create with the same UID at a different URL fails with the no-uid-conflict precondition.
    let second_uri = format!("/dav/calendars/{principal_id}/{calendar_id}/second.ics");
    let (second_status, _, error_body) = put_request(
        build_caldav_router(accounts),
        &second_uri,
        Some(("owner@example.test", &password)),
        &body,
        Some("*"),
    )
    .await;
    assert_eq!(second_status, StatusCode::FORBIDDEN);
    assert_eq!(
        xml_tree(&error_body)
            .descendants("urn:ietf:params:xml:ns:caldav", "no-uid-conflict")
            .len(),
        1
    );
    let _ = temp_dir;
}

// --- T13: All-day semantics ---

/// Build an all-day VCALENDAR body with DATE DTSTART/DTEND (exclusive end).
fn all_day_body(uid: &str, summary: &str, start: &str, end: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         BEGIN:VEVENT\r\n\
         UID:{uid}\r\n\
         DTSTAMP:20260101T000000Z\r\n\
         DTSTART;VALUE=DATE:{start}\r\n\
         DTEND;VALUE=DATE:{end}\r\n\
         SUMMARY:{summary}\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    )
}

/// T13: a single-day all-day event created from Apple round-trips as an
/// all-day event (DATE values, exclusive end) without becoming a timed event.
#[tokio::test]
async fn put_all_day_single_day_round_trips() {
    let (temp_dir, pool, accounts, user_id, password, principal_id, calendar_id) =
        setup_create().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/allday-single.ics");
    let body = all_day_body("allday-single-uid", "All Day", "20260115", "20260116");
    let (status, _, response_body) = put_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(("owner@example.test", &password)),
        &body,
        Some("*"),
    )
    .await;

    assert_eq!(status, StatusCode::CREATED);
    // The response must be an all-day event (DATE values, no DATE-TIME).
    assert!(response_body.contains("DTSTART;VALUE=DATE:20260115"));
    assert!(response_body.contains("DTEND;VALUE=DATE:20260116"));
    assert!(!response_body.contains("TZID="));

    // The domain store must hold an all-day event with the exclusive end date.
    let kind: String = sqlx::query_scalar("SELECT event_kind FROM events WHERE calendar_id = ?")
        .bind(calendar_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(kind, "all_day");
    let (start_date, end_date): (String, String) = sqlx::query_as(
        "SELECT all_day_start_date, all_day_end_date FROM events WHERE calendar_id = ?",
    )
    .bind(calendar_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(start_date, "2026-01-15");
    assert_eq!(end_date, "2026-01-16");

    // A subsequent GET must parse back as an all-day event with the same dates.
    let (get_status, _, get_body) = request(
        build_caldav_router(accounts),
        Method::GET,
        &uri,
        Some(("owner@example.test", &password)),
    )
    .await;
    assert_eq!(get_status, StatusCode::OK);
    let parsed = parse_calendar(&get_body, IcsParserLimits::default()).unwrap();
    assert_eq!(parsed.events.len(), 1);
    assert_eq!(
        parsed.events[0].timing,
        commoncal_backend::ics::NormalizedTiming::AllDay {
            start_date: chrono::NaiveDate::from_ymd_opt(2026, 1, 15).unwrap(),
            end_date: chrono::NaiveDate::from_ymd_opt(2026, 1, 16).unwrap(),
        }
    );
    let _ = (temp_dir, user_id);
}

/// T13: a multi-day all-day event round-trips with its exclusive end preserved.
#[tokio::test]
async fn put_all_day_multi_day_round_trips() {
    let (temp_dir, pool, accounts, _user_id, password, principal_id, calendar_id) =
        setup_create().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/allday-multi.ics");
    // 2026-01-15 through 2026-01-17 (exclusive) = a 3-day event.
    let body = all_day_body("allday-multi-uid", "Conference", "20260115", "20260118");
    let (status, _, response_body) = put_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(("owner@example.test", &password)),
        &body,
        Some("*"),
    )
    .await;

    assert_eq!(status, StatusCode::CREATED);
    assert!(response_body.contains("DTSTART;VALUE=DATE:20260115"));
    assert!(response_body.contains("DTEND;VALUE=DATE:20260118"));

    let (start_date, end_date): (String, String) = sqlx::query_as(
        "SELECT all_day_start_date, all_day_end_date FROM events WHERE calendar_id = ?",
    )
    .bind(calendar_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(start_date, "2026-01-15");
    assert_eq!(end_date, "2026-01-18");
    let _ = temp_dir;
}

/// T13: all-day events are timezone-independent — a date that falls on a DST
/// boundary must round-trip unchanged (no wall-time conversion, no date shift).
#[tokio::test]
async fn put_all_day_event_survives_dst_boundary() {
    let (temp_dir, pool, accounts, _user_id, password, principal_id, calendar_id) =
        setup_create().await;

    // 2026-11-01 is the US DST fallback day. An all-day event on that date must
    // round-trip as the same calendar date, unaffected by the timezone shift.
    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/allday-dst.ics");
    let body = all_day_body("allday-dst-uid", "DST Day", "20261101", "20261102");
    let (status, _, response_body) = put_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(("owner@example.test", &password)),
        &body,
        Some("*"),
    )
    .await;

    assert_eq!(status, StatusCode::CREATED);
    assert!(response_body.contains("DTSTART;VALUE=DATE:20261101"));
    assert!(response_body.contains("DTEND;VALUE=DATE:20261102"));
    // No timezone must be attached to an all-day event.
    assert!(!response_body.contains("TZID="));

    let (start_date, end_date): (String, String) = sqlx::query_as(
        "SELECT all_day_start_date, all_day_end_date FROM events WHERE calendar_id = ?",
    )
    .bind(calendar_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(start_date, "2026-11-01");
    assert_eq!(end_date, "2026-11-02");
    let _ = temp_dir;
}

/// T13: editing an all-day event preserves its all-day representation (it must
/// not be converted to a timed event).
#[tokio::test]
async fn put_update_all_day_preserves_all_day_representation() {
    let (temp_dir, pool, accounts, _user_id, password, principal_id, calendar_id) =
        setup_create().await;

    // Create an all-day event.
    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/allday-edit.ics");
    let create_body = all_day_body("allday-edit-uid", "Original", "20260115", "20260116");
    let (create_status, _, _) = put_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(("owner@example.test", &password)),
        &create_body,
        Some("*"),
    )
    .await;
    assert_eq!(create_status, StatusCode::CREATED);

    // Read the current ETag.
    let auth = ("owner@example.test", password.as_str());
    let etag = get_etag(&accounts, &principal_id, calendar_id, "allday-edit", auth).await;

    // Update the event (new title, new dates) — it must stay all-day.
    let update_body = all_day_body("allday-edit-uid", "Edited", "20260220", "20260222");
    let (status, _, response_body) = put_update_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(auth),
        &update_body,
        Some(etag.as_str()),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(response_body.contains("SUMMARY:Edited"));
    assert!(response_body.contains("DTSTART;VALUE=DATE:20260220"));
    assert!(response_body.contains("DTEND;VALUE=DATE:20260222"));
    assert!(!response_body.contains("TZID="));

    // The domain store must still hold an all-day event with the new dates.
    let kind: String = sqlx::query_scalar("SELECT event_kind FROM events WHERE calendar_id = ?")
        .bind(calendar_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(kind, "all_day");
    let (start_date, end_date): (String, String) = sqlx::query_as(
        "SELECT all_day_start_date, all_day_end_date FROM events WHERE calendar_id = ?",
    )
    .bind(calendar_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(start_date, "2026-02-20");
    assert_eq!(end_date, "2026-02-22");
    let _ = temp_dir;
}

/// T13: a mixed DATE DTSTART and DATE-TIME DTEND is rejected.
#[tokio::test]
async fn put_mixed_date_and_datetime_is_rejected() {
    let (temp_dir, _pool, accounts, _user_id, password, principal_id, calendar_id) =
        setup_create().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/mixed.ics");
    let body = "BEGIN:VCALENDAR\r\n\
                VERSION:2.0\r\n\
                BEGIN:VEVENT\r\n\
                UID:mixed-uid\r\n\
                DTSTAMP:20260101T000000Z\r\n\
                DTSTART;VALUE=DATE:20260115\r\n\
                DTEND:20260115T100000Z\r\n\
                SUMMARY:Mixed\r\n\
                END:VEVENT\r\n\
                END:VCALENDAR\r\n";

    let (status, _, _) = put_request(
        build_caldav_router(accounts),
        &uri,
        Some(("owner@example.test", &password)),
        body,
        Some("*"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let _ = temp_dir;
}

/// T13: a DATE-TIME DTSTART and DATE DTEND is also rejected.
#[tokio::test]
async fn put_mixed_datetime_and_date_is_rejected() {
    let (temp_dir, _pool, accounts, _user_id, password, principal_id, calendar_id) =
        setup_create().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/mixed2.ics");
    let body = "BEGIN:VCALENDAR\r\n\
                VERSION:2.0\r\n\
                BEGIN:VEVENT\r\n\
                UID:mixed2-uid\r\n\
                DTSTAMP:20260101T000000Z\r\n\
                DTSTART:20260115T090000Z\r\n\
                DTEND;VALUE=DATE:20260116\r\n\
                SUMMARY:Mixed2\r\n\
                END:VEVENT\r\n\
                END:VCALENDAR\r\n";

    let (status, _, _) = put_request(
        build_caldav_router(accounts),
        &uri,
        Some(("owner@example.test", &password)),
        body,
        Some("*"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let _ = temp_dir;
}

/// T13: an all-day event whose end is not after its start is rejected.
#[tokio::test]
async fn put_all_day_end_not_after_start_is_rejected() {
    let (temp_dir, _pool, accounts, _user_id, password, principal_id, calendar_id) =
        setup_create().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/allday-bad.ics");
    // Equal start and end.
    let body = all_day_body("allday-bad-uid", "Bad", "20260115", "20260115");
    let (status, _, _) = put_request(
        build_caldav_router(accounts),
        &uri,
        Some(("owner@example.test", &password)),
        &body,
        Some("*"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let _ = temp_dir;
}

// --- T14: Recurrence semantics ---

/// T14: a supported recurring series created from Apple round-trips as one
/// UID and one DAV resource with the RRULE preserved.
#[tokio::test]
async fn put_recurring_series_round_trips_as_one_uid_and_resource() {
    let (temp_dir, pool, accounts, _user_id, password, principal_id, calendar_id) =
        setup_create().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/recurring.ics");
    let body = "BEGIN:VCALENDAR\r\n\
                VERSION:2.0\r\n\
                BEGIN:VEVENT\r\n\
                UID:recurring-uid\r\n\
                DTSTAMP:20260101T000000Z\r\n\
                DTSTART:20260115T000000Z\r\n\
                DTEND:20260115T010000Z\r\n\
                RRULE:FREQ=DAILY;COUNT=5\r\n\
                SUMMARY:Recurring\r\n\
                END:VEVENT\r\n\
                END:VCALENDAR\r\n";

    let (status, _, response_body) = put_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(("owner@example.test", &password)),
        body,
        Some("*"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    // The response must contain the RRULE and the client-supplied UID.
    assert!(response_body.contains("RRULE:FREQ=DAILY;COUNT=5"));
    assert!(response_body.contains("UID:recurring-uid"));

    // A subsequent GET must return the same single resource with the same UID.
    let (get_status, _, get_body) = request(
        build_caldav_router(accounts.clone()),
        Method::GET,
        &uri,
        Some(("owner@example.test", &password)),
    )
    .await;
    assert_eq!(get_status, StatusCode::OK);
    let parsed = parse_calendar(&get_body, IcsParserLimits::default()).unwrap();
    // One VEVENT (the master), one UID.
    assert_eq!(parsed.events.len(), 1);
    assert_eq!(parsed.events[0].uid, "recurring-uid");
    assert_eq!(
        parsed.events[0].rrule.as_deref(),
        Some("FREQ=DAILY;COUNT=5")
    );

    // The domain store must hold exactly one event with the recurrence rule.
    let (count, rule): (i64, Option<String>) =
        sqlx::query_as("SELECT COUNT(*), recurrence_rule FROM events WHERE calendar_id = ?")
            .bind(calendar_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1, "series must be one event row");
    assert_eq!(rule.as_deref(), Some("FREQ=DAILY;COUNT=5"));

    // Exactly one DAV resource mapping exists.
    let resource_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM caldav_event_resources WHERE calendar_id = ? AND deleted_at IS NULL",
    )
    .bind(calendar_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(resource_count, 1, "series must be one DAV resource");
    let _ = temp_dir;
}

/// T14: a modified occurrence persists and is reflected in the serialized
/// output as a VEVENT with RECURRENCE-ID sharing the series UID.
#[tokio::test]
async fn put_recurring_modified_occurrence_persists() {
    let (temp_dir, pool, accounts, user_id, password, principal_id, calendar_id) =
        setup_create().await;

    // Create a daily recurring series.
    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/series.ics");
    let create_body = "BEGIN:VCALENDAR\r\n\
                       VERSION:2.0\r\n\
                       BEGIN:VEVENT\r\n\
                       UID:series-uid\r\n\
                       DTSTAMP:20260101T000000Z\r\n\
                       DTSTART:20260115T000000Z\r\n\
                       DTEND:20260115T010000Z\r\n\
                       RRULE:FREQ=DAILY;COUNT=5\r\n\
                       SUMMARY:Original\r\n\
                       END:VEVENT\r\n\
                       END:VCALENDAR\r\n";
    let (create_status, _, _) = put_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(("owner@example.test", &password)),
        create_body,
        Some("*"),
    )
    .await;
    assert_eq!(create_status, StatusCode::CREATED);

    // Get the current ETag.
    let auth = ("owner@example.test", password.as_str());
    let etag = get_etag(&accounts, &principal_id, calendar_id, "series", auth).await;

    // Update the second occurrence (2026-01-16) with a new title and time.
    let update_body = "BEGIN:VCALENDAR\r\n\
                      VERSION:2.0\r\n\
                      BEGIN:VEVENT\r\n\
                      UID:series-uid\r\n\
                      DTSTAMP:20260101T000000Z\r\n\
                      DTSTART:20260116T020000Z\r\n\
                      DTEND:20260116T030000Z\r\n\
                      RECURRENCE-ID:20260116T000000Z\r\n\
                      SUMMARY:Modified Occurrence\r\n\
                      END:VEVENT\r\n\
                      END:VCALENDAR\r\n";
    let (update_status, _, update_response) = put_update_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(auth),
        update_body,
        Some(etag.as_str()),
    )
    .await;
    assert_eq!(update_status, StatusCode::OK);
    // The response must contain both the master and the modified occurrence.
    assert!(update_response.contains("RRULE:FREQ=DAILY;COUNT=5"));
    assert!(update_response.contains("RECURRENCE-ID:"));
    assert!(update_response.contains("SUMMARY:Modified Occurrence"));

    // The domain store must have the exception recorded.
    let exception_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM event_recurrence_exceptions WHERE series_id = (SELECT id FROM events WHERE calendar_id = ?)",
    )
    .bind(calendar_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        exception_count, 1,
        "one modified occurrence must be persisted"
    );

    // A subsequent GET must show both the master and the modified occurrence
    // sharing the same UID.
    let (get_status, _, get_body) =
        request(build_caldav_router(accounts), Method::GET, &uri, Some(auth)).await;
    assert_eq!(get_status, StatusCode::OK);
    let parsed = parse_calendar(&get_body, IcsParserLimits::default()).unwrap();
    assert_eq!(parsed.events.len(), 2, "master + one modified occurrence");
    // All VEVENTs share the same UID.
    assert!(parsed.events.iter().all(|e| e.uid == "series-uid"));
    let _ = (temp_dir, user_id);
}

/// T14: a deleted occurrence persists as an EXDATE on the series master.
#[tokio::test]
async fn put_recurring_deleted_occurrence_persists_as_exdate() {
    let (temp_dir, pool, accounts, _user_id, password, principal_id, calendar_id) =
        setup_create().await;

    // Create a daily recurring series with 5 occurrences.
    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/exdate-series.ics");
    let create_body = "BEGIN:VCALENDAR\r\n\
                       VERSION:2.0\r\n\
                       BEGIN:VEVENT\r\n\
                       UID:exdate-uid\r\n\
                       DTSTAMP:20260101T000000Z\r\n\
                       DTSTART:20260115T000000Z\r\n\
                       DTEND:20260115T010000Z\r\n\
                       RRULE:FREQ=DAILY;COUNT=5\r\n\
                       SUMMARY:Exdate Series\r\n\
                       END:VEVENT\r\n\
                       END:VCALENDAR\r\n";
    let (create_status, _, _) = put_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(("owner@example.test", &password)),
        create_body,
        Some("*"),
    )
    .await;
    assert_eq!(create_status, StatusCode::CREATED);

    // Get the current ETag.
    let auth = ("owner@example.test", password.as_str());
    let etag = get_etag(&accounts, &principal_id, calendar_id, "exdate-series", auth).await;

    // Delete the third occurrence (2026-01-17) by sending a VEVENT with
    // RECURRENCE-ID and STATUS:CANCELLED — or more precisely, the DAV protocol
    // uses EXDATE on the master. We simulate this by updating the series with
    // an EXDATE.
    // Actually, the standard way to delete an occurrence in CalDAV is to PUT
    // the master with an EXDATE. Let's do that.
    let delete_body = "BEGIN:VCALENDAR\r\n\
                      VERSION:2.0\r\n\
                      BEGIN:VEVENT\r\n\
                      UID:exdate-uid\r\n\
                      DTSTAMP:20260101T000000Z\r\n\
                      DTSTART:20260115T000000Z\r\n\
                      DTEND:20260115T010000Z\r\n\
                      RRULE:FREQ=DAILY;COUNT=5\r\n\
                      EXDATE:20260117T000000Z\r\n\
                      SUMMARY:Exdate Series\r\n\
                      END:VEVENT\r\n\
                      END:VCALENDAR\r\n";
    let (delete_status, _, delete_response) = put_update_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(auth),
        delete_body,
        Some(etag.as_str()),
    )
    .await;
    assert_eq!(delete_status, StatusCode::OK);
    // The response must contain the EXDATE.
    assert!(delete_response.contains("EXDATE:20260117T000000Z"));

    // The domain store must have the deletion recorded.
    let deleted_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM event_recurrence_exceptions
         WHERE series_id = (SELECT id FROM events WHERE calendar_id = ?) AND is_deleted = 1",
    )
    .bind(calendar_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(deleted_count, 1, "one deleted occurrence must be persisted");

    // A subsequent GET must show the EXDATE on the master.
    let (get_status, _, get_body) =
        request(build_caldav_router(accounts), Method::GET, &uri, Some(auth)).await;
    assert_eq!(get_status, StatusCode::OK);
    let parsed = parse_calendar(&get_body, IcsParserLimits::default()).unwrap();
    assert_eq!(parsed.events.len(), 1, "only the master VEVENT");
    assert_eq!(parsed.events[0].exdates.len(), 1, "one EXDATE");
    let _ = temp_dir;
}

/// T14: multiple EXDATEs in a single PUT all persist without conflict.
#[tokio::test]
async fn put_recurring_multiple_exdates_in_one_put() {
    let (temp_dir, pool, accounts, _user_id, password, principal_id, calendar_id) =
        setup_create().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/multi-exdate.ics");
    let create_body = "BEGIN:VCALENDAR\r\n\
                       VERSION:2.0\r\n\
                       BEGIN:VEVENT\r\n\
                       UID:multi-exdate-uid\r\n\
                       DTSTAMP:20260101T000000Z\r\n\
                       DTSTART:20260115T000000Z\r\n\
                       DTEND:20260115T010000Z\r\n\
                       RRULE:FREQ=DAILY;COUNT=5\r\n\
                       SUMMARY:Multi Exdate Series\r\n\
                       END:VEVENT\r\n\
                       END:VCALENDAR\r\n";
    let (create_status, _, _) = put_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(("owner@example.test", &password)),
        create_body,
        Some("*"),
    )
    .await;
    assert_eq!(create_status, StatusCode::CREATED);

    let auth = ("owner@example.test", password.as_str());
    let etag = get_etag(&accounts, &principal_id, calendar_id, "multi-exdate", auth).await;

    // Delete two occurrences (2026-01-16 and 2026-01-18) in a single PUT.
    let delete_body = "BEGIN:VCALENDAR\r\n\
                      VERSION:2.0\r\n\
                      BEGIN:VEVENT\r\n\
                      UID:multi-exdate-uid\r\n\
                      DTSTAMP:20260101T000000Z\r\n\
                      DTSTART:20260115T000000Z\r\n\
                      DTEND:20260115T010000Z\r\n\
                      RRULE:FREQ=DAILY;COUNT=5\r\n\
                      EXDATE:20260116T000000Z\r\n\
                      EXDATE:20260118T000000Z\r\n\
                      SUMMARY:Multi Exdate Series\r\n\
                      END:VEVENT\r\n\
                      END:VCALENDAR\r\n";
    let (delete_status, _, delete_response) = put_update_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(auth),
        delete_body,
        Some(etag.as_str()),
    )
    .await;
    assert_eq!(
        delete_status,
        StatusCode::OK,
        "multiple EXDATEs must not conflict"
    );
    assert!(
        delete_response.contains("20260116T000000Z"),
        "first EXDATE present"
    );
    assert!(
        delete_response.contains("20260118T000000Z"),
        "second EXDATE present"
    );

    // Both deletions must be persisted.
    let deleted_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM event_recurrence_exceptions
         WHERE series_id = (SELECT id FROM events WHERE calendar_id = ?) AND is_deleted = 1",
    )
    .bind(calendar_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        deleted_count, 2,
        "two deleted occurrences must be persisted"
    );

    let _ = temp_dir;
}

/// T14: creating a series applies every supplied deletion and modification in
/// order, advancing the optimistic-lock version after each operation.
#[tokio::test]
async fn create_recurring_series_with_multiple_exdates_and_exceptions() {
    let (temp_dir, pool, accounts, _user_id, password, principal_id, calendar_id) =
        setup_create().await;
    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/create-many.ics");
    let body = "BEGIN:VCALENDAR\r\n\
                VERSION:2.0\r\n\
                BEGIN:VEVENT\r\n\
                UID:create-many-uid\r\n\
                DTSTAMP:20260101T000000Z\r\n\
                DTSTART:20260115T000000Z\r\n\
                DTEND:20260115T010000Z\r\n\
                RRULE:FREQ=DAILY;COUNT=8\r\n\
                EXDATE:20260116T000000Z\r\n\
                EXDATE:20260118T000000Z\r\n\
                SUMMARY:Master\r\n\
                END:VEVENT\r\n\
                BEGIN:VEVENT\r\n\
                UID:create-many-uid\r\n\
                DTSTAMP:20260101T000000Z\r\n\
                DTSTART:20260117T020000Z\r\n\
                DTEND:20260117T030000Z\r\n\
                RECURRENCE-ID:20260117T000000Z\r\n\
                SUMMARY:First exception\r\n\
                END:VEVENT\r\n\
                BEGIN:VEVENT\r\n\
                UID:create-many-uid\r\n\
                DTSTAMP:20260101T000000Z\r\n\
                DTSTART:20260119T020000Z\r\n\
                DTEND:20260119T030000Z\r\n\
                RECURRENCE-ID:20260119T000000Z\r\n\
                SUMMARY:Second exception\r\n\
                END:VEVENT\r\n\
                END:VCALENDAR\r\n";
    let (status, _, response) = put_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(("owner@example.test", &password)),
        body,
        Some("*"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(response.contains("First exception"));
    assert!(response.contains("Second exception"));

    let (deleted, modified): (i64, i64) = sqlx::query_as(
        "SELECT SUM(is_deleted), SUM(NOT is_deleted) FROM event_recurrence_exceptions WHERE series_id = (SELECT id FROM events WHERE calendar_id = ?)",
    )
    .bind(calendar_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((deleted, modified), (2, 2));
    let _ = temp_dir;
}

/// T14: every exception supplied when creating a series must use the master's
/// UID, and invalid input must be rejected before the series is created.
#[tokio::test]
async fn create_recurring_rejects_exception_with_different_uid() {
    let (temp_dir, pool, accounts, _user_id, password, principal_id, calendar_id) =
        setup_create().await;
    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/bad-uid.ics");
    let body = "BEGIN:VCALENDAR\r\n\
                VERSION:2.0\r\n\
                BEGIN:VEVENT\r\n\
                UID:master-uid\r\n\
                DTSTAMP:20260101T000000Z\r\n\
                DTSTART:20260115T000000Z\r\n\
                DTEND:20260115T010000Z\r\n\
                RRULE:FREQ=DAILY;COUNT=3\r\n\
                SUMMARY:Master\r\n\
                END:VEVENT\r\n\
                BEGIN:VEVENT\r\n\
                UID:other-uid\r\n\
                DTSTAMP:20260101T000000Z\r\n\
                DTSTART:20260116T020000Z\r\n\
                DTEND:20260116T030000Z\r\n\
                RECURRENCE-ID:20260116T000000Z\r\n\
                SUMMARY:Invalid exception\r\n\
                END:VEVENT\r\n\
                END:VCALENDAR\r\n";
    assert_eq!(
        put_request(
            build_caldav_router(accounts),
            &uri,
            Some(("owner@example.test", &password)),
            body,
            Some("*")
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE calendar_id = ?")
        .bind(calendar_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let _ = temp_dir;
}

/// T14: a calendar object cannot contain two master VEVENTs during CREATE.
#[tokio::test]
async fn create_recurring_rejects_second_master() {
    let (temp_dir, pool, accounts, _user_id, password, principal_id, calendar_id) =
        setup_create().await;
    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/two-masters.ics");
    let body = "BEGIN:VCALENDAR\r\n\
                VERSION:2.0\r\n\
                BEGIN:VEVENT\r\n\
                UID:one-uid\r\n\
                DTSTAMP:20260101T000000Z\r\n\
                DTSTART:20260115T000000Z\r\n\
                DTEND:20260115T010000Z\r\n\
                RRULE:FREQ=DAILY;COUNT=3\r\n\
                SUMMARY:First master\r\n\
                END:VEVENT\r\n\
                BEGIN:VEVENT\r\n\
                UID:two-uid\r\n\
                DTSTAMP:20260101T000000Z\r\n\
                DTSTART:20260116T000000Z\r\n\
                DTEND:20260116T010000Z\r\n\
                RRULE:FREQ=DAILY;COUNT=3\r\n\
                SUMMARY:Second master\r\n\
                END:VEVENT\r\n\
                END:VCALENDAR\r\n";
    assert_eq!(
        put_request(
            build_caldav_router(accounts),
            &uri,
            Some(("owner@example.test", &password)),
            body,
            Some("*")
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE calendar_id = ?")
        .bind(calendar_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let _ = temp_dir;
}

/// T14: the canonical master-plus-exception representation returned by GET is
/// accepted unchanged by a conditional PUT.
#[tokio::test]
async fn put_recurring_canonical_multi_vevent_round_trip() {
    let (temp_dir, _pool, accounts, _user_id, password, principal_id, calendar_id) =
        setup_create().await;
    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/round-trip.ics");
    let create = "BEGIN:VCALENDAR\r\n\
                  VERSION:2.0\r\n\
                  BEGIN:VEVENT\r\n\
                  UID:round-trip-uid\r\n\
                  DTSTAMP:20260101T000000Z\r\n\
                  DTSTART:20260115T000000Z\r\n\
                  DTEND:20260115T010000Z\r\n\
                  RRULE:FREQ=DAILY;COUNT=5\r\n\
                  SUMMARY:Master\r\n\
                  END:VEVENT\r\n\
                  END:VCALENDAR\r\n";
    assert_eq!(
        put_request(
            build_caldav_router(accounts.clone()),
            &uri,
            Some(("owner@example.test", &password)),
            create,
            Some("*")
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let auth = ("owner@example.test", password.as_str());
    let etag = get_etag(&accounts, &principal_id, calendar_id, "round-trip", auth).await;
    let occurrence = "BEGIN:VCALENDAR\r\n\
                      VERSION:2.0\r\n\
                      BEGIN:VEVENT\r\n\
                      UID:round-trip-uid\r\n\
                      DTSTAMP:20260101T000000Z\r\n\
                      DTSTART:20260116T020000Z\r\n\
                      DTEND:20260116T030000Z\r\n\
                      RECURRENCE-ID:20260116T000000Z\r\n\
                      SUMMARY:Exception\r\n\
                      END:VEVENT\r\n\
                      END:VCALENDAR\r\n";
    assert_eq!(
        put_update_request(
            build_caldav_router(accounts.clone()),
            &uri,
            Some(auth),
            occurrence,
            Some(&etag)
        )
        .await
        .0,
        StatusCode::OK
    );
    let (get_status, headers, canonical) = request(
        build_caldav_router(accounts.clone()),
        Method::GET,
        &uri,
        Some(auth),
    )
    .await;
    assert_eq!(get_status, StatusCode::OK);
    let etag = headers.get(header::ETAG).unwrap().to_str().unwrap();
    let (status, _, _) = put_update_request(
        build_caldav_router(accounts),
        &uri,
        Some(auth),
        &canonical,
        Some(etag),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let _ = temp_dir;
}

#[tokio::test]
async fn recurring_multi_vevent_failure_leaves_master_etag_and_exceptions_unchanged() {
    let (temp_dir, _pool, accounts, _user_id, password, principal_id, calendar_id) =
        setup_create().await;
    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/atomic.ics");
    let create = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:atomic-uid\r\nDTSTART:20260115T000000Z\r\nDTEND:20260115T010000Z\r\nRRULE:FREQ=DAILY;COUNT=3\r\nSUMMARY:Original\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let auth = ("owner@example.test", password.as_str());
    assert_eq!(
        put_request(
            build_caldav_router(accounts.clone()),
            &uri,
            Some(auth),
            create,
            Some("*")
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let etag = get_etag(&accounts, &principal_id, calendar_id, "atomic", auth).await;
    let exception = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:atomic-uid\r\nDTSTART:20260116T020000Z\r\nDTEND:20260116T030000Z\r\nRECURRENCE-ID:20260116T000000Z\r\nSUMMARY:Saved exception\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    assert_eq!(
        put_update_request(
            build_caldav_router(accounts.clone()),
            &uri,
            Some(auth),
            exception,
            Some(&etag)
        )
        .await
        .0,
        StatusCode::OK
    );
    let (get_status, headers, before) = request(
        build_caldav_router(accounts.clone()),
        Method::GET,
        &uri,
        Some(auth),
    )
    .await;
    assert_eq!(get_status, StatusCode::OK);
    let etag = headers
        .get(header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();

    let invalid = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:atomic-uid\r\nDTSTART:20260115T000000Z\r\nDTEND:20260115T010000Z\r\nRRULE:FREQ=DAILY;COUNT=3\r\nSUMMARY:Changed master\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:atomic-uid\r\nDTSTART:20260201T020000Z\r\nDTEND:20260201T030000Z\r\nRECURRENCE-ID:20260201T000000Z\r\nSUMMARY:Out of range\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    assert_eq!(
        put_update_request(
            build_caldav_router(accounts.clone()),
            &uri,
            Some(auth),
            invalid,
            Some(&etag)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let (get_status, headers, after) =
        request(build_caldav_router(accounts), Method::GET, &uri, Some(auth)).await;
    assert_eq!(get_status, StatusCode::OK);
    assert_eq!(headers.get(header::ETAG).unwrap().to_str().unwrap(), etag);
    assert_eq!(after, before);
    let _ = temp_dir;
}

#[tokio::test]
async fn recurring_rrule_change_is_rejected_without_mutating_resource() {
    let (temp_dir, _pool, accounts, _user_id, password, principal_id, calendar_id) =
        setup_create().await;
    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/rule.ics");
    let original = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:rule-uid\r\nDTSTART:20260115T000000Z\r\nDTEND:20260115T010000Z\r\nRRULE:FREQ=DAILY;COUNT=3\r\nSUMMARY:Original\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let auth = ("owner@example.test", password.as_str());
    assert_eq!(
        put_request(
            build_caldav_router(accounts.clone()),
            &uri,
            Some(auth),
            original,
            Some("*")
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let (get_status, headers, before) = request(
        build_caldav_router(accounts.clone()),
        Method::GET,
        &uri,
        Some(auth),
    )
    .await;
    assert_eq!(get_status, StatusCode::OK);
    let etag = headers
        .get(header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let changed = original.replace("FREQ=DAILY;COUNT=3", "FREQ=WEEKLY;COUNT=3");
    assert_eq!(
        put_update_request(
            build_caldav_router(accounts.clone()),
            &uri,
            Some(auth),
            &changed,
            Some(&etag)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (get_status, headers, after) =
        request(build_caldav_router(accounts), Method::GET, &uri, Some(auth)).await;
    assert_eq!(get_status, StatusCode::OK);
    assert_eq!(headers.get(header::ETAG).unwrap().to_str().unwrap(), etag);
    assert_eq!(after, before);
    let _ = temp_dir;
}

/// T14: an unsupported RRULE (e.g. BYDAY) is rejected explicitly with 400.
#[tokio::test]
async fn put_unsupported_rrule_fails_400() {
    let (temp_dir, _pool, accounts, _user_id, password, principal_id, calendar_id) =
        setup_create().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/unsupported.ics");
    let body = "BEGIN:VCALENDAR\r\n\
                VERSION:2.0\r\n\
                BEGIN:VEVENT\r\n\
                UID:unsupported-uid\r\n\
                DTSTAMP:20260101T000000Z\r\n\
                DTSTART:20260115T000000Z\r\n\
                DTEND:20260115T010000Z\r\n\
                RRULE:FREQ=WEEKLY;BYDAY=MO,WE,FR\r\n\
                SUMMARY:Unsupported\r\n\
                END:VEVENT\r\n\
                END:VCALENDAR\r\n";

    let (status, _, _) = put_request(
        build_caldav_router(accounts),
        &uri,
        Some(("owner@example.test", &password)),
        body,
        Some("*"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let _ = temp_dir;
}

/// T14: ETags for a recurring series are deterministic — the same content
/// produces the same ETag across requests.
#[tokio::test]
async fn put_recurring_etag_is_deterministic() {
    let (temp_dir, _pool, accounts, _user_id, password, principal_id, calendar_id) =
        setup_create().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/det-etag.ics");
    let body = "BEGIN:VCALENDAR\r\n\
                VERSION:2.0\r\n\
                BEGIN:VEVENT\r\n\
                UID:det-etag-uid\r\n\
                DTSTAMP:20260101T000000Z\r\n\
                DTSTART:20260115T000000Z\r\n\
                DTEND:20260115T010000Z\r\n\
                RRULE:FREQ=WEEKLY;COUNT=3\r\n\
                SUMMARY:Det ETag\r\n\
                END:VEVENT\r\n\
                END:VCALENDAR\r\n";

    let (status, _, _) = put_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(("owner@example.test", &password)),
        body,
        Some("*"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let auth = ("owner@example.test", password.as_str());
    let etag1 = get_etag(&accounts, &principal_id, calendar_id, "det-etag", auth).await;
    let etag2 = get_etag(&accounts, &principal_id, calendar_id, "det-etag", auth).await;
    assert_eq!(
        etag1, etag2,
        "ETag must be deterministic for unchanged content"
    );
    let _ = temp_dir;
}

#[tokio::test]
async fn put_unauthenticated_is_401() {
    let (temp_dir, _pool, accounts, _user_id, _password, principal_id, calendar_id) =
        setup_create().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/unauth.ics");
    let body = vcalendar_body("unauth-uid", "Unauth");

    let (status, headers, _) =
        put_request(build_caldav_router(accounts), &uri, None, &body, Some("*")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(headers.get(header::WWW_AUTHENTICATE).is_some());
    let _ = temp_dir;
}

#[tokio::test]
async fn put_cross_user_is_404() {
    let (temp_dir, pool, accounts, _owner_id, _owner_password, principal_id, calendar_id) =
        setup_create().await;

    let intruder_id = create_user(&pool, "intruder@example.test").await;
    let intruder_password = accounts
        .issue_credential(intruder_id, "Phone".into())
        .await
        .unwrap()
        .password
        .expose()
        .to_owned();

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/cross.ics");
    let body = vcalendar_body("cross-uid", "Cross User");

    let (status, _, body_text) = put_request(
        build_caldav_router(accounts),
        &uri,
        Some(("intruder@example.test", &intruder_password)),
        &body,
        Some("*"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body_text.is_empty());
    let _ = temp_dir;
}

#[tokio::test]
async fn put_viewer_role_is_404() {
    let (temp_dir, pool, accounts, _owner_id, _owner_password, principal_id, calendar_id) =
        setup_create().await;

    let viewer_id = create_user(&pool, "viewer@example.test").await;
    sqlx::query(
        "INSERT INTO calendar_acl (calendar_id, user_id, role, created_at, updated_at)
         VALUES (?, ?, 'viewer', 100, 100)",
    )
    .bind(calendar_id)
    .bind(viewer_id)
    .execute(&pool)
    .await
    .unwrap();
    let viewer_password = accounts
        .issue_credential(viewer_id, "Phone".into())
        .await
        .unwrap()
        .password
        .expose()
        .to_owned();

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/viewer.ics");
    let body = vcalendar_body("viewer-uid", "Viewer Event");

    let (status, _, _) = put_request(
        build_caldav_router(accounts),
        &uri,
        Some(("viewer@example.test", &viewer_password)),
        &body,
        Some("*"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let _ = temp_dir;
}

/// T07: a failed create (UID conflict) must roll back the event it created,
/// leaving no orphaned partial write in the domain store.
#[tokio::test]
async fn put_uid_conflict_rolls_back_partial_event_write() {
    let (temp_dir, pool, accounts, _user_id, password, principal_id, calendar_id) =
        setup_create().await;

    let body = vcalendar_body("shared-uid", "Shared UID Event");

    // First create with UID "shared-uid" at URL "first.ics" succeeds.
    let first_uri = format!("/dav/calendars/{principal_id}/{calendar_id}/first.ics");
    let (first_status, _, _) = put_request(
        build_caldav_router(accounts.clone()),
        &first_uri,
        Some(("owner@example.test", &password)),
        &body,
        Some("*"),
    )
    .await;
    assert_eq!(first_status, StatusCode::CREATED);

    // Exactly one event exists in the domain store.
    let count_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE calendar_id = ?")
        .bind(calendar_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count_before, 1, "first create must leave exactly one event");

    // Second create with the same UID at a different URL fails with the no-uid-conflict precondition.
    let second_uri = format!("/dav/calendars/{principal_id}/{calendar_id}/second.ics");
    let (second_status, _, error_body) = put_request(
        build_caldav_router(accounts),
        &second_uri,
        Some(("owner@example.test", &password)),
        &body,
        Some("*"),
    )
    .await;
    assert_eq!(second_status, StatusCode::FORBIDDEN);
    assert_eq!(
        xml_tree(&error_body)
            .descendants("urn:ietf:params:xml:ns:caldav", "no-uid-conflict")
            .len(),
        1
    );

    // The partial event from the failed create must be rolled back: the
    // domain store must still contain exactly one event.
    let count_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE calendar_id = ?")
        .bind(calendar_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count_after, 1,
        "failed create must roll back its partial event write"
    );

    // No DAV mapping may exist for the failed resource name.
    let mapping: Option<i64> = sqlx::query_scalar(
        "SELECT event_id FROM caldav_event_resources
         WHERE calendar_id = ? AND resource_name = ?",
    )
    .bind(calendar_id)
    .bind("second.ics")
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert!(
        mapping.is_none(),
        "failed create must not leave a DAV resource mapping"
    );
    let _ = temp_dir;
}

/// T07: a successful create must preserve audit behavior — an `event.create`
/// audit entry is recorded for the new event.
#[tokio::test]
async fn put_create_records_audit_entry() {
    let (temp_dir, pool, accounts, user_id, password, principal_id, calendar_id) =
        setup_create().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/audited.ics");
    let body = vcalendar_body("audit-uid", "Audited Event");
    let (status, _, _) = put_request(
        build_caldav_router(accounts),
        &uri,
        Some(("owner@example.test", &password)),
        &body,
        Some("*"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // An `event.create` audit entry must exist for the actor.
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_log
         WHERE action = 'event.create' AND actor_user_id = ? AND target_type = 'event'",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        audit_count, 1,
        "successful create must record an event.create audit entry"
    );
    let _ = temp_dir;
}

// --- T08: Conflict-safe update ---

/// Issue a `PUT` to an existing resource with an `If-Match` precondition.
async fn put_update_request(
    app: Router,
    uri: &str,
    auth: Option<(&str, &str)>,
    body: &str,
    if_match: Option<&str>,
) -> (StatusCode, HeaderMap, String) {
    let mut builder = Request::builder()
        .method(Method::PUT)
        .uri(uri)
        .header(header::CONTENT_TYPE, "text/calendar; charset=utf-8");
    if let Some((username, password)) = auth {
        builder = builder.header(header::AUTHORIZATION, basic_header(username, password));
    }
    if let Some(value) = if_match {
        builder = builder.header(header::IF_MATCH, value);
    }
    let response = app
        .oneshot(builder.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, headers, String::from_utf8_lossy(&body).into_owned())
}

/// Read the current ETag of a resource via `GET`.
async fn get_etag(
    accounts: &CaldavAccountService,
    principal_id: &str,
    calendar_id: i64,
    resource_name: &str,
    auth: (&str, &str),
) -> String {
    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/{resource_name}.ics");
    let (_, headers, _) = request(
        build_caldav_router(accounts.clone()),
        Method::GET,
        &uri,
        Some(auth),
    )
    .await;
    headers
        .get(header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned()
}

/// Read the current event title straight from the domain store.
async fn stored_title(pool: &SqlitePool, calendar_id: i64, event_id: i64) -> String {
    sqlx::query_scalar("SELECT title FROM events WHERE calendar_id = ? AND id = ?")
        .bind(calendar_id)
        .bind(event_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// T08: a matching `If-Match` updates both views and rotates the ETag.
#[tokio::test]
async fn put_update_matching_etag_succeeds_and_rotates_etag() {
    let (
        temp_dir,
        pool,
        accounts,
        user_id,
        password,
        principal_id,
        calendar_id,
        event_id,
        resource_name,
    ) = scenario().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/{resource_name}.ics");
    let auth = ("owner@example.test", password.as_str());
    let etag_before = get_etag(&accounts, &principal_id, calendar_id, &resource_name, auth).await;

    let body = vcalendar_body("update-uid", "Updated Event");
    let (status, headers, response_body) = put_update_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(auth),
        &body,
        Some(etag_before.as_str()),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(headers.get(header::ETAG).is_none());
    let etag_after = get_etag(&accounts, &principal_id, calendar_id, &resource_name, auth).await;
    assert_ne!(
        etag_before, etag_after,
        "ETag must rotate on a successful update"
    );
    assert!(response_body.contains("SUMMARY:Updated Event"));

    // The update must be visible in the Happening domain store.
    assert_eq!(
        stored_title(&pool, calendar_id, event_id).await,
        "Updated Event"
    );

    // A subsequent GET reflects the new content and the rotated ETag.
    let (get_status, get_headers, get_body) =
        request(build_caldav_router(accounts), Method::GET, &uri, Some(auth)).await;
    assert_eq!(get_status, StatusCode::OK);
    assert!(get_body.contains("SUMMARY:Updated Event"));
    assert_eq!(get_headers.get(header::ETAG).unwrap(), etag_after.as_str());
    let _ = (temp_dir, user_id);
}

/// T08: a stale `If-Match` yields 412 and leaves the event unmutated.
#[tokio::test]
async fn put_update_stale_etag_fails_412_without_mutation() {
    let (
        temp_dir,
        pool,
        accounts,
        user_id,
        password,
        principal_id,
        calendar_id,
        event_id,
        resource_name,
    ) = scenario().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/{resource_name}.ics");
    let auth = ("owner@example.test", password.as_str());
    let stale_etag = get_etag(&accounts, &principal_id, calendar_id, &resource_name, auth).await;

    // A newer Happening change rotates the ETag, making the client's tag stale.
    let service = EventService::new_at(pool.clone(), 400);
    let mut change = timed_mutation();
    change.title = "Newer Change".to_owned();
    service
        .update(
            user_id,
            false,
            calendar_id,
            event_id,
            EventChange {
                expected_version: 1,
                target_calendar_id: calendar_id,
                event: change,
            },
        )
        .await
        .unwrap();

    let body = vcalendar_body("stale-uid", "Stale Writer");
    let (status, _, _) = put_update_request(
        build_caldav_router(accounts),
        &uri,
        Some(auth),
        &body,
        Some(stale_etag.as_str()),
    )
    .await;

    assert_eq!(status, StatusCode::PRECONDITION_FAILED);
    assert_eq!(
        stored_title(&pool, calendar_id, event_id).await,
        "Newer Change",
        "a stale update must not mutate the event"
    );
    let _ = temp_dir;
}

/// T08: a missing `If-Match` precondition yields 428 and leaves the event
/// unmutated.
#[tokio::test]
async fn put_update_missing_if_match_fails_428_without_mutation() {
    let (
        temp_dir,
        pool,
        accounts,
        _user_id,
        password,
        principal_id,
        calendar_id,
        event_id,
        resource_name,
    ) = scenario().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/{resource_name}.ics");
    let body = vcalendar_body("no-precondition-uid", "No Precondition");
    let (status, _, _) = put_update_request(
        build_caldav_router(accounts),
        &uri,
        Some(("owner@example.test", &password)),
        &body,
        None,
    )
    .await;

    assert_eq!(status, StatusCode::PRECONDITION_REQUIRED);
    assert_eq!(
        stored_title(&pool, calendar_id, event_id).await,
        "Planning",
        "a missing precondition must not mutate the event"
    );
    let _ = temp_dir;
}

/// T08: an unauthorized update fails with 404 (no disclosure) and leaves the
/// event unmutated.
#[tokio::test]
async fn put_update_unauthorized_fails_404_without_mutation() {
    let (
        temp_dir,
        pool,
        accounts,
        _owner_id,
        _owner_password,
        principal_id,
        calendar_id,
        event_id,
        resource_name,
    ) = scenario().await;

    // A viewer may read but not edit.
    let viewer_id = create_user(&pool, "viewer@example.test").await;
    sqlx::query(
        "INSERT INTO calendar_acl (calendar_id, user_id, role, created_at, updated_at)
         VALUES (?, ?, 'viewer', 100, 100)",
    )
    .bind(calendar_id)
    .bind(viewer_id)
    .execute(&pool)
    .await
    .unwrap();
    let viewer_password = accounts
        .issue_credential(viewer_id, "Phone".into())
        .await
        .unwrap()
        .password
        .expose()
        .to_owned();

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/{resource_name}.ics");
    let body = vcalendar_body("viewer-uid", "Viewer Edit");
    let (status, _, response_body) = put_update_request(
        build_caldav_router(accounts),
        &uri,
        Some(("viewer@example.test", &viewer_password)),
        &body,
        Some("\"anything\""),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(response_body.is_empty(), "denial must not leak a body");
    assert_eq!(
        stored_title(&pool, calendar_id, event_id).await,
        "Planning",
        "an unauthorized update must not mutate the event"
    );
    let _ = temp_dir;
}

/// T08: concurrent writers — exactly one succeeds, the other gets 412, and the
/// event ends in a consistent state.
#[tokio::test]
async fn put_update_concurrent_writers_one_wins() {
    let (
        temp_dir,
        pool,
        accounts,
        _user_id,
        password,
        principal_id,
        calendar_id,
        event_id,
        resource_name,
    ) = scenario().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/{resource_name}.ics");
    let auth = ("owner@example.test", password.as_str());
    let shared_etag = get_etag(&accounts, &principal_id, calendar_id, &resource_name, auth).await;

    let body_a = vcalendar_body("concurrent-a", "Writer A");
    let body_b = vcalendar_body("concurrent-b", "Writer B");

    let (result_a, result_b) = tokio::join!(
        put_update_request(
            build_caldav_router(accounts.clone()),
            &uri,
            Some(auth),
            &body_a,
            Some(shared_etag.as_str())
        ),
        put_update_request(
            build_caldav_router(accounts),
            &uri,
            Some(auth),
            &body_b,
            Some(shared_etag.as_str())
        )
    );
    let (status_a, _, _) = result_a;
    let (status_b, _, _) = result_b;

    let outcomes = [status_a, status_b];
    let successes = outcomes
        .iter()
        .filter(|status| **status == StatusCode::OK)
        .count();
    let preconditions_failed = outcomes
        .iter()
        .filter(|status| **status == StatusCode::PRECONDITION_FAILED)
        .count();
    assert_eq!(successes, 1, "exactly one concurrent writer must succeed");
    assert_eq!(preconditions_failed, 1, "the losing writer must get 412");

    // The event must reflect exactly one of the two writes.
    let title = stored_title(&pool, calendar_id, event_id).await;
    assert!(
        title == "Writer A" || title == "Writer B",
        "event must reflect a single consistent write, got {title}"
    );
    let _ = temp_dir;
}

// --- T09: Delete and tombstone ---

/// Issue a conditional `DELETE` with an `If-Match` precondition.
async fn delete_request(
    app: Router,
    uri: &str,
    auth: Option<(&str, &str)>,
    if_match: Option<&str>,
) -> (StatusCode, HeaderMap, String) {
    let mut builder = Request::builder().method(Method::DELETE).uri(uri);
    if let Some((username, password)) = auth {
        builder = builder.header(header::AUTHORIZATION, basic_header(username, password));
    }
    if let Some(value) = if_match {
        builder = builder.header(header::IF_MATCH, value);
    }
    let response = app
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, headers, String::from_utf8_lossy(&body).into_owned())
}

/// T09: a matching `If-Match` deletes the event through the domain service and
/// leaves a durable tombstone that survives the event deletion.
#[tokio::test]
async fn delete_matching_etag_succeeds_and_tombstones() {
    let (
        temp_dir,
        pool,
        accounts,
        _user_id,
        password,
        principal_id,
        calendar_id,
        event_id,
        resource_name,
    ) = scenario().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/{resource_name}.ics");
    let auth = ("owner@example.test", password.as_str());
    let etag = get_etag(&accounts, &principal_id, calendar_id, &resource_name, auth).await;

    let (status, _, body) = delete_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(auth),
        Some(etag.as_str()),
    )
    .await;

    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(body.is_empty(), "DELETE must not return a body");

    // The event must be removed from the domain store.
    let event_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE calendar_id = ? AND id = ?")
            .bind(calendar_id)
            .bind(event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(event_count, 0, "the event must be deleted");

    // The DAV mapping must survive as a durable tombstone.
    let repository = CaldavRepository::new(pool.clone());
    assert!(
        repository
            .tombstone_exists(calendar_id, &resource_name)
            .await
            .unwrap(),
        "a durable tombstone must survive event deletion"
    );
    // The live mapping must be cleared.
    assert!(
        repository
            .resolve_by_name(calendar_id, &resource_name)
            .await
            .unwrap()
            .is_none(),
        "the live mapping must be cleared after deletion"
    );

    // A subsequent GET must return 404.
    let (get_status, _, _) =
        request(build_caldav_router(accounts), Method::GET, &uri, Some(auth)).await;
    assert_eq!(get_status, StatusCode::NOT_FOUND);
    let _ = temp_dir;
}

/// T09: a stale `If-Match` yields 412 and leaves the event unmutated with no
/// tombstone created.
#[tokio::test]
async fn delete_stale_etag_fails_412_without_mutation() {
    let (
        temp_dir,
        pool,
        accounts,
        user_id,
        password,
        principal_id,
        calendar_id,
        event_id,
        resource_name,
    ) = scenario().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/{resource_name}.ics");
    let auth = ("owner@example.test", password.as_str());
    let stale_etag = get_etag(&accounts, &principal_id, calendar_id, &resource_name, auth).await;

    // A newer Happening change rotates the ETag, making the client's tag stale.
    let service = EventService::new_at(pool.clone(), 400);
    let mut change = timed_mutation();
    change.title = "Newer Change".to_owned();
    service
        .update(
            user_id,
            false,
            calendar_id,
            event_id,
            EventChange {
                expected_version: 1,
                target_calendar_id: calendar_id,
                event: change,
            },
        )
        .await
        .unwrap();

    let (status, _, _) = delete_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(auth),
        Some(stale_etag.as_str()),
    )
    .await;

    assert_eq!(status, StatusCode::PRECONDITION_FAILED);
    assert_eq!(
        stored_title(&pool, calendar_id, event_id).await,
        "Newer Change",
        "a stale delete must not remove the event"
    );
    let repository = CaldavRepository::new(pool.clone());
    assert!(
        !repository
            .tombstone_exists(calendar_id, &resource_name)
            .await
            .unwrap(),
        "a stale delete must not create a tombstone"
    );
    let _ = temp_dir;
}

/// T09: a missing `If-Match` precondition yields 428 and leaves the event
/// unmutated.
#[tokio::test]
async fn delete_missing_if_match_fails_428() {
    let (
        temp_dir,
        pool,
        accounts,
        _user_id,
        password,
        principal_id,
        calendar_id,
        event_id,
        resource_name,
    ) = scenario().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/{resource_name}.ics");
    let (status, _, _) = delete_request(
        build_caldav_router(accounts),
        &uri,
        Some(("owner@example.test", &password)),
        None,
    )
    .await;

    assert_eq!(status, StatusCode::PRECONDITION_REQUIRED);
    assert_eq!(
        stored_title(&pool, calendar_id, event_id).await,
        "Planning",
        "a missing precondition must not remove the event"
    );
    let _ = temp_dir;
}

/// T09: a repeated delete is deterministic — the first delete succeeds and the
/// second is a 404 because the resource is already gone.
#[tokio::test]
async fn delete_repeated_is_deterministic() {
    let (
        temp_dir,
        _pool,
        accounts,
        _user_id,
        password,
        principal_id,
        calendar_id,
        _event_id,
        resource_name,
    ) = scenario().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/{resource_name}.ics");
    let auth = ("owner@example.test", password.as_str());
    let etag = get_etag(&accounts, &principal_id, calendar_id, &resource_name, auth).await;

    // First delete succeeds.
    let (first_status, _, _) = delete_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(auth),
        Some(etag.as_str()),
    )
    .await;
    assert_eq!(first_status, StatusCode::NO_CONTENT);

    // A repeated delete is deterministic: the resource is gone, so it is 404.
    let (second_status, _, second_body) = delete_request(
        build_caldav_router(accounts),
        &uri,
        Some(auth),
        Some(etag.as_str()),
    )
    .await;
    assert_eq!(second_status, StatusCode::NOT_FOUND);
    assert!(second_body.is_empty(), "denial must not leak a body");
    let _ = temp_dir;
}

/// T09: an unauthorized delete fails with 404 (no disclosure) and leaves the
/// event unmutated.
#[tokio::test]
async fn delete_unauthorized_fails_404_without_mutation() {
    let (
        temp_dir,
        pool,
        accounts,
        _owner_id,
        _owner_password,
        principal_id,
        calendar_id,
        event_id,
        resource_name,
    ) = scenario().await;

    // A viewer may read but not delete.
    let viewer_id = create_user(&pool, "viewer@example.test").await;
    sqlx::query(
        "INSERT INTO calendar_acl (calendar_id, user_id, role, created_at, updated_at)
         VALUES (?, ?, 'viewer', 100, 100)",
    )
    .bind(calendar_id)
    .bind(viewer_id)
    .execute(&pool)
    .await
    .unwrap();
    let viewer_password = accounts
        .issue_credential(viewer_id, "Phone".into())
        .await
        .unwrap()
        .password
        .expose()
        .to_owned();

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/{resource_name}.ics");
    let (status, _, response_body) = delete_request(
        build_caldav_router(accounts),
        &uri,
        Some(("viewer@example.test", &viewer_password)),
        Some("\"anything\""),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(response_body.is_empty(), "denial must not leak a body");
    assert_eq!(
        stored_title(&pool, calendar_id, event_id).await,
        "Planning",
        "an unauthorized delete must not remove the event"
    );
    let _ = temp_dir;
}

/// T09: an unauthenticated delete is 401.
#[tokio::test]
async fn delete_unauthenticated_is_401() {
    let (
        temp_dir,
        _pool,
        accounts,
        _user_id,
        _password,
        principal_id,
        calendar_id,
        _event_id,
        resource_name,
    ) = scenario().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/{resource_name}.ics");
    let (status, headers, _) =
        delete_request(build_caldav_router(accounts), &uri, None, Some("*")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(headers.get(header::WWW_AUTHENTICATE).is_some());
    let _ = temp_dir;
}

// --- T15: Permission and imported boundaries ---

/// Grant a calendar role to a new user and issue a DAV credential for them.
/// Returns the clear password and the user's own principal id (grantees address
/// shared calendars under their own principal).
async fn grant_and_issue(
    pool: &SqlitePool,
    accounts: &CaldavAccountService,
    email: &str,
    calendar_id: i64,
    role: &str,
) -> (String, String) {
    let user_id = create_user(pool, email).await;
    sqlx::query(
        "INSERT INTO calendar_acl (calendar_id, user_id, role, created_at, updated_at)
         VALUES (?, ?, ?, 100, 100)",
    )
    .bind(calendar_id)
    .bind(user_id)
    .bind(role)
    .execute(pool)
    .await
    .unwrap();
    let password = accounts
        .issue_credential(user_id, "Phone".into())
        .await
        .unwrap()
        .password
        .expose()
        .to_owned();
    let principal_id = accounts
        .status(user_id)
        .await
        .unwrap()
        .principal_id
        .unwrap();
    (password, principal_id)
}

/// A feed fetcher that always returns one fixed ICS body.
struct FixedFetcher {
    body: String,
}

impl FeedFetcher for FixedFetcher {
    fn fetch<'a>(
        &'a self,
        _url: &'a str,
        _etag: Option<&'a str>,
        _last_modified: Option<&'a str>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<FetchResponse, FeedError>> + Send + 'a>,
    > {
        let body = self.body.clone();
        Box::pin(async move {
            Ok(FetchResponse {
                status: 200,
                body: body.as_bytes().to_vec(),
                etag: Some("v1".into()),
                last_modified: None,
            })
        })
    }
}

/// Create an external feed on the calendar and import one timed event.
/// Returns the imported event's id.
async fn import_event(pool: &SqlitePool, user_id: i64, calendar_id: i64) -> i64 {
    let service = ExternalFeedService::new_at(pool.clone(), SecretKey::generate(), 1000);
    let feed_id = service
        .create(
            user_id,
            false,
            calendar_id,
            NewFeed {
                source_url: "https://feeds.example.test/imported.ics".into(),
                refresh_interval_seconds: Some(60),
            },
        )
        .await
        .unwrap()
        .id;
    let ics = "BEGIN:VCALENDAR\r\n\
               BEGIN:VEVENT\r\n\
               UID:imported-uid\r\n\
               DTSTART:20260115T000000Z\r\n\
               DTEND:20260115T010000Z\r\n\
               SUMMARY:Imported Event\r\n\
               END:VEVENT\r\n\
               END:VCALENDAR\r\n";
    service
        .refresh(
            user_id,
            false,
            feed_id,
            &FixedFetcher {
                body: ics.to_owned(),
            },
        )
        .await
        .unwrap();
    sqlx::query_scalar("SELECT event_id FROM external_event_mapping WHERE feed_id = ?")
        .bind(feed_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Issue a `REPORT` request and return the status and body text.
async fn report_request(
    app: Router,
    uri: &str,
    username: &str,
    password: &str,
    body: &str,
) -> (StatusCode, String) {
    let request = Request::builder()
        .method(Method::from_bytes(b"REPORT").unwrap())
        .uri(uri)
        .header(
            "Depth",
            if body.contains("calendar-query") {
                "1"
            } else {
                "0"
            },
        )
        .header(header::AUTHORIZATION, basic_header(username, password))
        .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
        .body(Body::from(body.to_owned()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

fn calendar_query_xml(start: &str, end: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <D:getetag/>
    <D:getcontenttype/>
    <C:calendar-data/>
  </D:prop>
  <C:filter>
    <C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT"><C:time-range start="{start}" end="{end}"/></C:comp-filter></C:comp-filter>
  </C:filter>
</C:calendar-query>"#
    )
}

fn multiget_xml(href: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-multiget xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop>
    <D:getetag/>
    <D:getcontenttype/>
    <C:calendar-data/>
  </D:prop>
  <D:href>{href}</D:href>
</C:calendar-multiget>"#
    )
}

/// T15: a free-busy viewer reads a busy placeholder with no private fields.
#[tokio::test]
async fn free_busy_viewer_get_receives_busy_placeholder_without_private_fields() {
    let (
        temp_dir,
        pool,
        accounts,
        _owner_id,
        _owner_password,
        _principal_id,
        calendar_id,
        _event_id,
        resource_name,
    ) = scenario().await;
    let (fb_password, fb_principal) = grant_and_issue(
        &pool,
        &accounts,
        "freebusy@example.test",
        calendar_id,
        "free_busy_viewer",
    )
    .await;

    let uri = format!("/dav/calendars/{fb_principal}/{calendar_id}/{resource_name}.ics");
    let (status, _headers, body) = request(
        build_caldav_router(accounts),
        Method::GET,
        &uri,
        Some(("freebusy@example.test", &fb_password)),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    // The busy time window is exposed.
    assert!(
        body.contains("DTSTART:20260115T000000Z"),
        "free-busy must expose DTSTART"
    );
    assert!(
        body.contains("DTEND:20260115T010000Z"),
        "free-busy must expose DTEND"
    );
    // No private fields leak.
    assert!(
        !body.contains("SUMMARY:"),
        "free-busy must not expose SUMMARY"
    );
    assert!(
        !body.contains("DESCRIPTION:"),
        "free-busy must not expose DESCRIPTION"
    );
    assert!(
        !body.contains("LOCATION:"),
        "free-busy must not expose LOCATION"
    );
    assert!(
        !body.contains("STATUS:"),
        "free-busy must not expose STATUS"
    );
    let _ = temp_dir;
}

/// T15: a viewer (full read) still receives the complete event, proving the
/// free-busy restriction is role-specific and not a global change.
#[tokio::test]
async fn viewer_get_receives_full_event_details() {
    let (
        temp_dir,
        pool,
        accounts,
        _owner_id,
        _owner_password,
        _principal_id,
        calendar_id,
        _event_id,
        resource_name,
    ) = scenario().await;
    let (viewer_password, viewer_principal) = grant_and_issue(
        &pool,
        &accounts,
        "viewer@example.test",
        calendar_id,
        "viewer",
    )
    .await;

    let uri = format!("/dav/calendars/{viewer_principal}/{calendar_id}/{resource_name}.ics");
    let (status, _headers, body) = request(
        build_caldav_router(accounts),
        Method::GET,
        &uri,
        Some(("viewer@example.test", &viewer_password)),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("SUMMARY:Planning"),
        "viewer must see the summary"
    );
    assert!(
        body.contains("LOCATION:Room 1"),
        "viewer must see the location"
    );
    let _ = temp_dir;
}

/// T15: an editor reads full details and can create an event (the writable
/// read-details class in the matrix).
#[tokio::test]
async fn editor_reads_details_and_can_create() {
    let (temp_dir, pool, accounts, _owner_id, _owner_password, _principal_id, calendar_id) =
        setup_create().await;
    let (editor_password, editor_principal) = grant_and_issue(
        &pool,
        &accounts,
        "editor@example.test",
        calendar_id,
        "editor",
    )
    .await;

    // Read an existing event's full details.
    let existing_id = create_event(&pool, _owner_id, calendar_id).await;
    let repository = CaldavRepository::new(pool.clone());
    let resource = repository
        .ensure_resource(calendar_id, existing_id, 1000)
        .await
        .unwrap();
    let read_uri = format!(
        "/dav/calendars/{editor_principal}/{calendar_id}/{}.ics",
        resource.resource_name
    );
    let (read_status, _headers, read_body) = request(
        build_caldav_router(accounts.clone()),
        Method::GET,
        &read_uri,
        Some(("editor@example.test", &editor_password)),
    )
    .await;
    assert_eq!(read_status, StatusCode::OK);
    assert!(
        read_body.contains("SUMMARY:Planning"),
        "editor must see the full summary"
    );

    // Create a new event.
    let create_uri = format!("/dav/calendars/{editor_principal}/{calendar_id}/editor-new.ics");
    let body = vcalendar_body("editor-uid", "Editor Event");
    let (create_status, _, _) = put_request(
        build_caldav_router(accounts),
        &create_uri,
        Some(("editor@example.test", &editor_password)),
        &body,
        Some("*"),
    )
    .await;
    assert_eq!(
        create_status,
        StatusCode::CREATED,
        "editor must be able to create"
    );
    let _ = temp_dir;
}

/// T15: a free-busy viewer's calendar-query returns busy placeholders.
#[tokio::test]
async fn free_busy_viewer_calendar_query_returns_busy_placeholders() {
    let (
        temp_dir,
        pool,
        accounts,
        _owner_id,
        _owner_password,
        _principal_id,
        calendar_id,
        _event_id,
        _resource_name,
    ) = scenario().await;
    let (fb_password, fb_principal) = grant_and_issue(
        &pool,
        &accounts,
        "freebusy@example.test",
        calendar_id,
        "free_busy_viewer",
    )
    .await;

    let body = calendar_query_xml("20260101T000000Z", "20260131T235959Z");
    let (status, resp) = report_request(
        build_caldav_router(accounts),
        &format!("/dav/calendars/{fb_principal}/{calendar_id}/"),
        "freebusy@example.test",
        &fb_password,
        &body,
    )
    .await;

    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert!(
        resp.contains("DTSTART:20260115T000000Z"),
        "free-busy query must expose the window"
    );
    assert!(
        !resp.contains("SUMMARY:Planning"),
        "free-busy query must not expose the summary"
    );
    assert!(
        !resp.contains("LOCATION:Room 1"),
        "free-busy query must not expose the location"
    );
    let _ = temp_dir;
}

/// T15: a free-busy viewer's multiget returns busy placeholders.
#[tokio::test]
async fn free_busy_viewer_multiget_returns_busy_placeholders() {
    let (
        temp_dir,
        pool,
        accounts,
        _owner_id,
        _owner_password,
        _principal_id,
        calendar_id,
        _event_id,
        resource_name,
    ) = scenario().await;
    let (fb_password, fb_principal) = grant_and_issue(
        &pool,
        &accounts,
        "freebusy@example.test",
        calendar_id,
        "free_busy_viewer",
    )
    .await;

    let href = format!("/dav/calendars/{fb_principal}/{calendar_id}/{resource_name}.ics");
    let (status, resp) = report_request(
        build_caldav_router(accounts),
        &format!("/dav/calendars/{fb_principal}/{calendar_id}/"),
        "freebusy@example.test",
        &fb_password,
        &multiget_xml(&href),
    )
    .await;

    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert!(
        resp.contains("HTTP/1.1 200 OK"),
        "free-busy multiget item must succeed"
    );
    assert!(
        !resp.contains("SUMMARY:Planning"),
        "free-busy multiget must not expose the summary"
    );
    assert!(
        !resp.contains("LOCATION:Room 1"),
        "free-busy multiget must not expose the location"
    );
    let _ = temp_dir;
}

/// T15: a free-busy viewer cannot create an event (404, no disclosure).
#[tokio::test]
async fn free_busy_viewer_cannot_put_create() {
    let (temp_dir, pool, accounts, _owner_id, _owner_password, _principal_id, calendar_id) =
        setup_create().await;
    let (fb_password, fb_principal) = grant_and_issue(
        &pool,
        &accounts,
        "freebusy@example.test",
        calendar_id,
        "free_busy_viewer",
    )
    .await;

    let uri = format!("/dav/calendars/{fb_principal}/{calendar_id}/fb-new.ics");
    let body = vcalendar_body("fb-uid", "FreeBusy Create");
    let (status, _, response_body) = put_request(
        build_caldav_router(accounts),
        &uri,
        Some(("freebusy@example.test", &fb_password)),
        &body,
        Some("*"),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(response_body.is_empty(), "denial must not leak a body");
    let _ = temp_dir;
}

/// T15: a free-busy viewer cannot delete an event (404, no mutation).
#[tokio::test]
async fn free_busy_viewer_cannot_delete() {
    let (
        temp_dir,
        pool,
        accounts,
        _owner_id,
        _owner_password,
        _principal_id,
        calendar_id,
        event_id,
        resource_name,
    ) = scenario().await;
    let (fb_password, fb_principal) = grant_and_issue(
        &pool,
        &accounts,
        "freebusy@example.test",
        calendar_id,
        "free_busy_viewer",
    )
    .await;

    let uri = format!("/dav/calendars/{fb_principal}/{calendar_id}/{resource_name}.ics");
    let (status, _, response_body) = delete_request(
        build_caldav_router(accounts),
        &uri,
        Some(("freebusy@example.test", &fb_password)),
        Some("\"anything\""),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(response_body.is_empty(), "denial must not leak a body");
    assert_eq!(
        stored_title(&pool, calendar_id, event_id).await,
        "Planning",
        "a free-busy viewer must not delete the event"
    );
    let _ = temp_dir;
}

/// T15: an imported external-feed event is readable through DAV.
#[tokio::test]
async fn imported_event_is_readable() {
    let (
        temp_dir,
        pool,
        accounts,
        user_id,
        password,
        principal_id,
        calendar_id,
        _event_id,
        _resource_name,
    ) = scenario().await;
    let imported_event_id = import_event(&pool, user_id, calendar_id).await;
    let repository = CaldavRepository::new(pool.clone());
    let resource = repository
        .ensure_resource(calendar_id, imported_event_id, 1000)
        .await
        .unwrap();

    let uri = format!(
        "/dav/calendars/{principal_id}/{calendar_id}/{}.ics",
        resource.resource_name
    );
    let (status, _headers, body) = request(
        build_caldav_router(accounts),
        Method::GET,
        &uri,
        Some(("owner@example.test", &password)),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("SUMMARY:Imported Event"),
        "an imported event must be readable"
    );
    let _ = temp_dir;
}

/// T15: a PUT update on an imported event fails with 404 and does not mutate.
#[tokio::test]
async fn imported_event_rejects_put_update() {
    let (
        temp_dir,
        pool,
        accounts,
        user_id,
        password,
        principal_id,
        calendar_id,
        _event_id,
        _resource_name,
    ) = scenario().await;
    let imported_event_id = import_event(&pool, user_id, calendar_id).await;
    let repository = CaldavRepository::new(pool.clone());
    let resource = repository
        .ensure_resource(calendar_id, imported_event_id, 1000)
        .await
        .unwrap();

    let uri = format!(
        "/dav/calendars/{principal_id}/{calendar_id}/{}.ics",
        resource.resource_name
    );
    let body = vcalendar_body("imported-update-uid", "Hacked");
    let (status, _, response_body) = put_update_request(
        build_caldav_router(accounts),
        &uri,
        Some(("owner@example.test", &password)),
        &body,
        Some("\"anything\""),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(response_body.is_empty(), "denial must not leak a body");
    assert_eq!(
        stored_title(&pool, calendar_id, imported_event_id).await,
        "Imported Event",
        "an imported event must not be mutated by a DAV update"
    );
    let _ = temp_dir;
}

/// T15: a DELETE on an imported event fails with 404, does not mutate, and
/// leaves no tombstone.
#[tokio::test]
async fn imported_event_rejects_delete() {
    let (
        temp_dir,
        pool,
        accounts,
        user_id,
        password,
        principal_id,
        calendar_id,
        _event_id,
        _resource_name,
    ) = scenario().await;
    let imported_event_id = import_event(&pool, user_id, calendar_id).await;
    let repository = CaldavRepository::new(pool.clone());
    let resource = repository
        .ensure_resource(calendar_id, imported_event_id, 1000)
        .await
        .unwrap();

    let uri = format!(
        "/dav/calendars/{principal_id}/{calendar_id}/{}.ics",
        resource.resource_name
    );
    let (status, _, response_body) = delete_request(
        build_caldav_router(accounts),
        &uri,
        Some(("owner@example.test", &password)),
        Some("\"anything\""),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(response_body.is_empty(), "denial must not leak a body");
    assert_eq!(
        stored_title(&pool, calendar_id, imported_event_id).await,
        "Imported Event",
        "an imported event must not be deleted by a DAV delete"
    );
    assert!(
        !repository
            .tombstone_exists(calendar_id, &resource.resource_name)
            .await
            .unwrap(),
        "a rejected delete of an imported event must not create a tombstone"
    );
    let _ = temp_dir;
}

// --- T10: Record native Happening mutations ---

/// Count the change-log rows for a calendar, optionally filtered by type.
async fn change_count(pool: &SqlitePool, calendar_id: i64, change_type: Option<&str>) -> i64 {
    match change_type {
        Some(change_type) => sqlx::query_scalar(
            "SELECT COUNT(*) FROM caldav_event_changes
              WHERE calendar_id = ? AND change_type = ?",
        )
        .bind(calendar_id)
        .bind(change_type)
        .fetch_one(pool)
        .await
        .unwrap(),
        None => {
            sqlx::query_scalar("SELECT COUNT(*) FROM caldav_event_changes WHERE calendar_id = ?")
                .bind(calendar_id)
                .fetch_one(pool)
                .await
                .unwrap()
        }
    }
}

/// T10: a native (browser/MCP-shaped) create through the domain service records
/// exactly one 'created' change.
#[tokio::test]
async fn native_create_records_exactly_one_change() {
    let (_temp_dir, pool) = setup().await;
    let user_id = create_user(&pool, "owner@example.test").await;
    let calendar_id = create_calendar(&pool, user_id).await;

    let service = EventService::new_at(pool.clone(), 300);
    service
        .create(user_id, false, calendar_id, timed_mutation())
        .await
        .unwrap();

    assert_eq!(
        change_count(&pool, calendar_id, Some("created")).await,
        1,
        "a committed native create must record exactly one 'created' change"
    );
    assert_eq!(
        change_count(&pool, calendar_id, None).await,
        1,
        "a committed native create must record exactly one change total"
    );
}

/// T10: a native update through the domain service records exactly one
/// 'updated' change, on top of the create's change.
#[tokio::test]
async fn native_update_records_exactly_one_change() {
    let (_temp_dir, pool) = setup().await;
    let user_id = create_user(&pool, "owner@example.test").await;
    let calendar_id = create_calendar(&pool, user_id).await;
    let event_id = create_event(&pool, user_id, calendar_id).await;

    let service = EventService::new_at(pool.clone(), 400);
    let mut change = timed_mutation();
    change.title = "Renamed".to_owned();
    service
        .update(
            user_id,
            false,
            calendar_id,
            event_id,
            EventChange {
                expected_version: 1,
                target_calendar_id: calendar_id,
                event: change,
            },
        )
        .await
        .unwrap();

    assert_eq!(
        change_count(&pool, calendar_id, Some("updated")).await,
        1,
        "a committed native update must record exactly one 'updated' change"
    );
    assert_eq!(
        change_count(&pool, calendar_id, None).await,
        2,
        "create + update must record exactly two changes total"
    );
}

/// T10: a native delete through the domain service records exactly one
/// 'deleted' change, tombstones the live DAV mapping, and removes the event —
/// without violating the mapping's foreign key.
#[tokio::test]
async fn native_delete_records_one_change_and_tombstones_mapping() {
    let (_temp_dir, pool) = setup().await;
    let user_id = create_user(&pool, "owner@example.test").await;
    let calendar_id = create_calendar(&pool, user_id).await;
    let event_id = create_event(&pool, user_id, calendar_id).await;
    // Establish a live DAV mapping so the delete must tombstone it.
    let repository = CaldavRepository::new(pool.clone());
    let resource = repository
        .ensure_resource(calendar_id, event_id, 1000)
        .await
        .unwrap();

    let service = EventService::new_at(pool.clone(), 500);
    service
        .delete(user_id, false, calendar_id, event_id)
        .await
        .unwrap();

    assert_eq!(
        change_count(&pool, calendar_id, Some("deleted")).await,
        1,
        "a committed native delete must record exactly one 'deleted' change"
    );
    // The event must be gone.
    let event_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE calendar_id = ? AND id = ?")
            .bind(calendar_id)
            .bind(event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(event_count, 0, "the event must be deleted");
    // The live mapping must be cleared and a durable tombstone must remain.
    assert!(
        repository
            .resolve_by_name(calendar_id, &resource.resource_name)
            .await
            .unwrap()
            .is_none(),
        "the live mapping must be cleared after a native delete"
    );
    assert!(
        repository
            .tombstone_exists(calendar_id, &resource.resource_name)
            .await
            .unwrap(),
        "a native delete must leave a durable tombstone"
    );
}

/// T10: a rolled-back mutation (stale-version update) records no change.
#[tokio::test]
async fn rolled_back_update_records_no_change() {
    let (_temp_dir, pool) = setup().await;
    let user_id = create_user(&pool, "owner@example.test").await;
    let calendar_id = create_calendar(&pool, user_id).await;
    let event_id = create_event(&pool, user_id, calendar_id).await;

    let baseline = change_count(&pool, calendar_id, None).await;

    // A stale expected_version makes the update roll back.
    let service = EventService::new_at(pool.clone(), 400);
    let mut change = timed_mutation();
    change.title = "Should Not Land".to_owned();
    let result = service
        .update(
            user_id,
            false,
            calendar_id,
            event_id,
            EventChange {
                expected_version: 999,
                target_calendar_id: calendar_id,
                event: change,
            },
        )
        .await;
    assert!(
        matches!(result, Err(EventServiceError::Conflict { .. })),
        "a stale-version update must fail with a conflict"
    );

    assert_eq!(
        change_count(&pool, calendar_id, None).await,
        baseline,
        "a rolled-back update must record no change"
    );
    assert_eq!(
        stored_title(&pool, calendar_id, event_id).await,
        "Planning",
        "a rolled-back update must not mutate the event"
    );
}

/// T10: a DAV-originated create (through the HTTP layer) records exactly one
/// change — the DAV layer must not double-count on top of the domain service.
#[tokio::test]
async fn dav_create_is_not_double_counted() {
    let (_temp_dir, pool, accounts, _user_id, password, principal_id, calendar_id) =
        setup_create().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/dc.ics");
    let body = vcalendar_body("dc-uid", "DAV Create");
    let (status, _, _) = put_request(
        build_caldav_router(accounts),
        &uri,
        Some(("owner@example.test", &password)),
        &body,
        Some("*"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    assert_eq!(
        change_count(&pool, calendar_id, Some("created")).await,
        1,
        "a DAV create must record exactly one 'created' change (no double-count)"
    );
    assert_eq!(
        change_count(&pool, calendar_id, None).await,
        1,
        "a DAV create must record exactly one change total (no double-count)"
    );
}

/// T10: a DAV-originated delete records exactly one 'deleted' change.
#[tokio::test]
async fn dav_delete_is_not_double_counted() {
    let (
        _temp_dir,
        pool,
        accounts,
        _user_id,
        password,
        principal_id,
        calendar_id,
        _event_id,
        resource_name,
    ) = scenario().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/{resource_name}.ics");
    let auth = ("owner@example.test", password.as_str());
    let etag = get_etag(&accounts, &principal_id, calendar_id, &resource_name, auth).await;

    let (status, _, _) = delete_request(
        build_caldav_router(accounts),
        &uri,
        Some(auth),
        Some(etag.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    assert_eq!(
        change_count(&pool, calendar_id, Some("deleted")).await,
        1,
        "a DAV delete must record exactly one 'deleted' change (no double-count)"
    );
}

/// T10: the ordered change-log API returns changes in ascending revision order
/// and pages after a given revision.
#[tokio::test]
async fn change_log_is_ordered_and_pageable() {
    let (_temp_dir, pool) = setup().await;
    let user_id = create_user(&pool, "owner@example.test").await;
    let calendar_id = create_calendar(&pool, user_id).await;

    let service = EventService::new_at(pool.clone(), 300);
    let first = service
        .create(user_id, false, calendar_id, timed_mutation())
        .await
        .unwrap();
    let mut change = timed_mutation();
    change.title = "Second".to_owned();
    let second = service
        .create(user_id, false, calendar_id, change)
        .await
        .unwrap();

    let repository = CaldavRepository::new(pool.clone());
    let all = repository
        .list_changes_since(calendar_id, 0, 100)
        .await
        .unwrap();
    assert_eq!(all.len(), 2, "two creates must yield two ordered changes");
    assert!(
        all[0].id < all[1].id,
        "changes must be returned in ascending revision order"
    );
    assert_eq!(all[0].event_id, Some(first.id));
    assert_eq!(all[1].event_id, Some(second.id));
    assert_eq!(all[0].change_type, "created");
    assert_eq!(all[1].change_type, "created");

    // Paging after the first revision returns only the second change.
    let page = repository
        .list_changes_since(calendar_id, all[0].id, 100)
        .await
        .unwrap();
    assert_eq!(
        page.len(),
        1,
        "paging after a revision must skip earlier changes"
    );
    assert_eq!(page[0].id, all[1].id);
    assert_eq!(page[0].event_id, Some(second.id));
}

// --- T11: Incremental sync collection ---

/// Build a `sync-collection` REPORT body. An empty token requests the initial
/// snapshot.
fn sync_collection_xml(sync_token: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8" ?>
<D:sync-collection xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:sync-token>{sync_token}</D:sync-token>
  <D:sync-level>1</D:sync-level>
  <D:prop>
    <D:getetag/>
    <D:getcontenttype/>
    <C:calendar-data/>
  </D:prop>
</D:sync-collection>"#
    )
}

/// Extract the `<D:sync-token>` value from a sync-collection response body.
fn extract_sync_token(body: &str) -> Option<String> {
    xml_tree(body)
        .children
        .iter()
        .find(|node| node.namespace == "DAV:" && node.name == "sync-token")
        .map(|node| node.text.clone())
}

/// Count the `<D:response>` elements in a multistatus body.
fn count_responses(body: &str) -> usize {
    body.matches("<D:response>").count()
}

/// T11: an initial snapshot (empty token) returns every exposed event and a
/// new opaque sync token.
#[tokio::test]
async fn sync_collection_initial_snapshot_returns_all_events_and_token() {
    let (
        _temp_dir,
        pool,
        accounts,
        user_id,
        password,
        principal_id,
        calendar_id,
        _event_id,
        _resource_name,
    ) = scenario().await;

    // Add a second event so the snapshot has more than one resource.
    create_event(&pool, user_id, calendar_id).await;

    let body = sync_collection_xml("");
    let (status, resp) = report_request(
        build_caldav_router(accounts),
        &format!("/dav/calendars/{principal_id}/{calendar_id}/"),
        "owner@example.test",
        &password,
        &body,
    )
    .await;

    assert_eq!(status, StatusCode::MULTI_STATUS);
    // One response per event; the token is a direct multistatus child.
    assert_eq!(
        count_responses(&resp),
        2,
        "initial snapshot must list both events"
    );
    let token = extract_sync_token(&resp).expect("initial snapshot must mint a sync token");
    assert!(!token.is_empty(), "the minted sync token must be non-empty");
    // Both events must be present with calendar data.
    assert!(
        resp.contains("SUMMARY:Planning"),
        "snapshot must include event data"
    );
    assert!(
        resp.contains("<C:calendar-data>"),
        "snapshot must carry calendar-data"
    );
}

/// T11: a later call with the minted token returns only changes made after the
/// snapshot, not the events already returned.
#[tokio::test]
async fn sync_collection_later_call_returns_only_newer_changes() {
    let (
        _temp_dir,
        pool,
        accounts,
        user_id,
        password,
        principal_id,
        calendar_id,
        _event_id,
        _resource_name,
    ) = scenario().await;

    // Initial snapshot.
    let body = sync_collection_xml("");
    let (status, resp) = report_request(
        build_caldav_router(accounts.clone()),
        &format!("/dav/calendars/{principal_id}/{calendar_id}/"),
        "owner@example.test",
        &password,
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    let token = extract_sync_token(&resp).expect("snapshot must mint a token");

    // A new event created after the snapshot.
    let service = EventService::new_at(pool.clone(), 500);
    let mut change = timed_mutation();
    change.title = "After Snapshot".to_owned();
    service
        .create(user_id, false, calendar_id, change)
        .await
        .unwrap();

    // Incremental sync with the minted token.
    let body = sync_collection_xml(&token);
    let (status, resp) = report_request(
        build_caldav_router(accounts),
        &format!("/dav/calendars/{principal_id}/{calendar_id}/"),
        "owner@example.test",
        &password,
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    // Exactly one response for the new event.
    assert_eq!(
        count_responses(&resp),
        1,
        "incremental sync must return only the new change"
    );
    assert!(
        resp.contains("SUMMARY:After Snapshot"),
        "incremental sync must include the new event"
    );
    assert!(
        !resp.contains("SUMMARY:Planning"),
        "incremental sync must not repeat the already-synced event"
    );
    // A fresh token must be minted for the next call.
    let next_token = extract_sync_token(&resp).expect("incremental sync must mint a token");
    assert!(!next_token.is_empty());
}

/// T11: a deletion is represented in the sync response with the RFC 6578
/// deleted-resource shape (a `D:status` of 207 as a direct child of the
/// response).
#[tokio::test]
async fn sync_collection_represents_deletions() {
    let (
        _temp_dir,
        pool,
        accounts,
        _user_id,
        password,
        principal_id,
        calendar_id,
        event_id,
        resource_name,
    ) = scenario().await;

    // Initial snapshot establishes the baseline token.
    let body = sync_collection_xml("");
    let (status, resp) = report_request(
        build_caldav_router(accounts.clone()),
        &format!("/dav/calendars/{principal_id}/{calendar_id}/"),
        "owner@example.test",
        &password,
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    let token = extract_sync_token(&resp).expect("snapshot must mint a token");

    // Delete the event through the DAV path (records a 'deleted' change).
    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/{resource_name}.ics");
    let auth = ("owner@example.test", password.as_str());
    let etag = get_etag(&accounts, &principal_id, calendar_id, &resource_name, auth).await;
    let (delete_status, _, _) = delete_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(auth),
        Some(etag.as_str()),
    )
    .await;
    assert_eq!(delete_status, StatusCode::NO_CONTENT);

    // Incremental sync must surface the deletion.
    let body = sync_collection_xml(&token);
    let (status, resp) = report_request(
        build_caldav_router(accounts),
        &format!("/dav/calendars/{principal_id}/{calendar_id}/"),
        "owner@example.test",
        &password,
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    // One response for the deleted resource.
    assert_eq!(
        count_responses(&resp),
        1,
        "incremental sync must include the deletion"
    );
    // The deleted resource is represented by a 207 status as a direct child.
    assert!(
        resp.contains(&format!(
            "/dav/calendars/{principal_id}/{calendar_id}/{resource_name}.ics"
        )),
        "deletion must reference the deleted resource href"
    );
    assert!(
        resp.contains("<D:status>HTTP/1.1 404 Not Found</D:status>"),
        "deletion must use the RFC 6578 deleted-resource status"
    );
    let _ = (pool, event_id);
}

/// T11: an invalid (malformed or tampered) sync token fails with the DAV precondition.
#[tokio::test]
async fn sync_collection_invalid_token_fails_safely() {
    let (
        _temp_dir,
        _pool,
        accounts,
        _user_id,
        password,
        principal_id,
        calendar_id,
        _event_id,
        _resource_name,
    ) = scenario().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/");

    // A structurally malformed token.
    let body = sync_collection_xml("not-a-valid-token");
    let (status, _) = report_request(
        build_caldav_router(accounts.clone()),
        &uri,
        "owner@example.test",
        &password,
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // A well-formed but tampered token (valid shape, wrong signature).
    let valid = accounts.encode_sync_token(0);
    let parts: Vec<&str> = valid.split('.').collect();
    assert_eq!(parts.len(), 2, "token must have two base64url parts");
    // Flip the revision bytes to break the signature.
    let mut bad_revision = parts[0].to_owned();
    let last = bad_revision.pop().unwrap();
    bad_revision.push(if last == 'A' { 'B' } else { 'A' });
    let tampered = format!("{bad_revision}.{}", parts[1]);
    let body = sync_collection_xml(&tampered);
    let (status, _) = report_request(
        build_caldav_router(accounts),
        &uri,
        "owner@example.test",
        &password,
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// T11: pagination cannot skip changes. Repeated incremental calls with the
/// minted token walk the change log in order, and every change is returned
/// exactly once across the pages.
#[tokio::test]
async fn sync_collection_pagination_does_not_skip_changes() {
    let (
        _temp_dir,
        pool,
        accounts,
        user_id,
        password,
        principal_id,
        calendar_id,
        _event_id,
        _resource_name,
    ) = scenario().await;

    // Baseline snapshot.
    let body = sync_collection_xml("");
    let (status, resp) = report_request(
        build_caldav_router(accounts.clone()),
        &format!("/dav/calendars/{principal_id}/{calendar_id}/"),
        "owner@example.test",
        &password,
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    let mut token = extract_sync_token(&resp).expect("snapshot must mint a token");

    // Create three events after the snapshot, syncing after each one. Every
    // page must contain exactly the change made since the previous token.
    let service = EventService::new_at(pool.clone(), 500);
    for (index, title) in ["Page One", "Page Two", "Page Three"]
        .into_iter()
        .enumerate()
    {
        let mut change = timed_mutation();
        change.title = title.to_owned();
        service
            .create(user_id, false, calendar_id, change)
            .await
            .unwrap();

        let body = sync_collection_xml(&token);
        let (status, resp) = report_request(
            build_caldav_router(accounts.clone()),
            &format!("/dav/calendars/{principal_id}/{calendar_id}/"),
            "owner@example.test",
            &password,
            &body,
        )
        .await;
        assert_eq!(status, StatusCode::MULTI_STATUS);
        assert_eq!(
            count_responses(&resp),
            1,
            "page {index} must contain exactly the one new change"
        );
        assert!(
            resp.contains(&format!("SUMMARY:{title}")),
            "page {index} must include its new event"
        );
        // The token must advance so the next page does not repeat this change.
        let next = extract_sync_token(&resp).expect("each page must mint a token");
        assert_ne!(
            next, token,
            "the token must advance past the returned change"
        );
        token = next;
    }

    // A final call with the latest token must be empty (no changes since).
    let body = sync_collection_xml(&token);
    let (status, resp) = report_request(
        build_caldav_router(accounts),
        &format!("/dav/calendars/{principal_id}/{calendar_id}/"),
        "owner@example.test",
        &password,
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert_eq!(
        count_responses(&resp),
        0,
        "a call with the latest token must return no resource responses"
    );
}

/// T11: a sync-collection with a bad sync-level is rejected.
#[tokio::test]
async fn sync_collection_rejects_bad_sync_level() {
    let (
        _temp_dir,
        _pool,
        accounts,
        _user_id,
        password,
        principal_id,
        calendar_id,
        _event_id,
        _resource_name,
    ) = scenario().await;

    let body = r#"<?xml version="1.0" encoding="utf-8" ?>
<D:sync-collection xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:sync-token></D:sync-token>
  <D:sync-level>2</D:sync-level>
  <D:prop><D:getetag/></D:prop>
</D:sync-collection>"#;
    let (status, _) = report_request(
        build_caldav_router(accounts),
        &format!("/dav/calendars/{principal_id}/{calendar_id}/"),
        "owner@example.test",
        &password,
        body,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// --- T12: Moves and complete mutation coverage ---

/// T12: a move (update with a different target calendar) atomically emits a
/// 'deleted' change on the source calendar and a 'created' change on the
/// target calendar, and updates the DAV resource mapping to the target.
#[tokio::test]
async fn move_emits_source_removal_and_target_addition() {
    let (_temp_dir, pool) = setup().await;
    let user_id = create_user(&pool, "owner@example.test").await;
    let source_calendar_id = create_calendar(&pool, user_id).await;
    let target_calendar_id = create_calendar(&pool, user_id).await;
    let event_id = create_event(&pool, user_id, source_calendar_id).await;

    // Establish a live DAV mapping on the source calendar.
    let repository = CaldavRepository::new(pool.clone());
    let resource = repository
        .ensure_resource(source_calendar_id, event_id, 1000)
        .await
        .unwrap();

    // Move the event to the target calendar.
    let service = EventService::new_at(pool.clone(), 400);
    let mut change = timed_mutation();
    change.title = "Moved Event".to_owned();
    service
        .update(
            user_id,
            false,
            source_calendar_id,
            event_id,
            EventChange {
                expected_version: 1,
                target_calendar_id,
                event: change,
            },
        )
        .await
        .unwrap();

    // The event must now live in the target calendar.
    let event_calendar: i64 = sqlx::query_scalar("SELECT calendar_id FROM events WHERE id = ?")
        .bind(event_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        event_calendar, target_calendar_id,
        "the event must be in the target calendar after a move"
    );

    // Source calendar must have exactly one 'deleted' change.
    assert_eq!(
        change_count(&pool, source_calendar_id, Some("deleted")).await,
        1,
        "a move must record exactly one 'deleted' change on the source calendar"
    );
    // Target calendar must have exactly one 'created' change.
    assert_eq!(
        change_count(&pool, target_calendar_id, Some("created")).await,
        1,
        "a move must record exactly one 'created' change on the target calendar"
    );
    // No 'updated' change should appear on either calendar for a move.
    assert_eq!(
        change_count(&pool, source_calendar_id, Some("updated")).await,
        0,
        "a move must not record an 'updated' change on the source"
    );
    assert_eq!(
        change_count(&pool, target_calendar_id, Some("updated")).await,
        0,
        "a move must not record an 'updated' change on the target"
    );

    // The DAV resource mapping must now point to the target calendar.
    let moved_resource = repository
        .resolve_by_name(target_calendar_id, &resource.resource_name)
        .await
        .unwrap()
        .expect("the DAV mapping must resolve in the target calendar after a move");
    assert_eq!(moved_resource.event_id, event_id);

    // The source calendar must no longer resolve the resource.
    assert!(
        repository
            .resolve_by_name(source_calendar_id, &resource.resource_name)
            .await
            .unwrap()
            .is_none(),
        "the DAV mapping must not resolve in the source calendar after a move"
    );
}

/// T12: a move records the resource name on the source 'deleted' change so
/// sync-collection can emit the correct href.
#[tokio::test]
async fn move_deleted_change_carries_resource_name() {
    let (_temp_dir, pool) = setup().await;
    let user_id = create_user(&pool, "owner@example.test").await;
    let source_calendar_id = create_calendar(&pool, user_id).await;
    let target_calendar_id = create_calendar(&pool, user_id).await;
    let event_id = create_event(&pool, user_id, source_calendar_id).await;

    let repository = CaldavRepository::new(pool.clone());
    let resource = repository
        .ensure_resource(source_calendar_id, event_id, 1000)
        .await
        .unwrap();

    let service = EventService::new_at(pool.clone(), 400);
    let change = timed_mutation();
    service
        .update(
            user_id,
            false,
            source_calendar_id,
            event_id,
            EventChange {
                expected_version: 1,
                target_calendar_id,
                event: change,
            },
        )
        .await
        .unwrap();

    // The 'deleted' change on the source must carry the resource name.
    let deleted_change: Option<(String,)> = sqlx::query_as(
        "SELECT resource_name FROM caldav_event_changes
          WHERE calendar_id = ? AND change_type = 'deleted' AND event_id = ?",
    )
    .bind(source_calendar_id)
    .bind(event_id)
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(
        deleted_change.map(|(name,)| name),
        Some(resource.resource_name.clone()),
        "the source 'deleted' change must carry the DAV resource name"
    );
}

/// T12: a non-move update (same calendar) still records exactly one 'updated'
/// change and does not emit spurious deleted/created pairs.
#[tokio::test]
async fn non_move_update_records_only_updated() {
    let (_temp_dir, pool) = setup().await;
    let user_id = create_user(&pool, "owner@example.test").await;
    let calendar_id = create_calendar(&pool, user_id).await;
    let event_id = create_event(&pool, user_id, calendar_id).await;

    let service = EventService::new_at(pool.clone(), 400);
    let mut change = timed_mutation();
    change.title = "In-Place Edit".to_owned();
    service
        .update(
            user_id,
            false,
            calendar_id,
            event_id,
            EventChange {
                expected_version: 1,
                target_calendar_id: calendar_id,
                event: change,
            },
        )
        .await
        .unwrap();

    assert_eq!(
        change_count(&pool, calendar_id, Some("updated")).await,
        1,
        "a non-move update must record exactly one 'updated' change"
    );
    assert_eq!(
        change_count(&pool, calendar_id, Some("deleted")).await,
        0,
        "a non-move update must not record a 'deleted' change"
    );
    assert_eq!(
        change_count(&pool, calendar_id, Some("created")).await,
        1,
        "only the original create should be the sole 'created' change"
    );
}

/// T12: a move that fails (stale version) records no changes on either
/// calendar and leaves the event in the source calendar.
#[tokio::test]
async fn failed_move_records_no_changes() {
    let (_temp_dir, pool) = setup().await;
    let user_id = create_user(&pool, "owner@example.test").await;
    let source_calendar_id = create_calendar(&pool, user_id).await;
    let target_calendar_id = create_calendar(&pool, user_id).await;
    let event_id = create_event(&pool, user_id, source_calendar_id).await;

    let repository = CaldavRepository::new(pool.clone());
    let resource = repository
        .ensure_resource(source_calendar_id, event_id, 1000)
        .await
        .unwrap();

    let baseline_source = change_count(&pool, source_calendar_id, None).await;
    let baseline_target = change_count(&pool, target_calendar_id, None).await;

    // A stale expected_version makes the move roll back.
    let service = EventService::new_at(pool.clone(), 400);
    let change = timed_mutation();
    let result = service
        .update(
            user_id,
            false,
            source_calendar_id,
            event_id,
            EventChange {
                expected_version: 999,
                target_calendar_id,
                event: change,
            },
        )
        .await;
    assert!(
        matches!(result, Err(EventServiceError::Conflict { .. })),
        "a stale-version move must fail with a conflict"
    );

    assert_eq!(
        change_count(&pool, source_calendar_id, None).await,
        baseline_source,
        "a failed move must record no changes on the source"
    );
    assert_eq!(
        change_count(&pool, target_calendar_id, None).await,
        baseline_target,
        "a failed move must record no changes on the target"
    );
    // The event must still be in the source calendar.
    let event_calendar: i64 = sqlx::query_scalar("SELECT calendar_id FROM events WHERE id = ?")
        .bind(event_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        event_calendar, source_calendar_id,
        "a failed move must leave the event in the source calendar"
    );
    // The DAV mapping must still resolve in the source.
    assert!(
        repository
            .resolve_by_name(source_calendar_id, &resource.resource_name)
            .await
            .unwrap()
            .is_some(),
        "a failed move must not move the DAV mapping"
    );
}

/// T12: an imported event cannot be moved (the domain service rejects it with
/// ReadOnly), and no changes are recorded on either calendar.
#[tokio::test]
async fn imported_event_rejects_move() {
    let (_temp_dir, pool) = setup().await;
    let user_id = create_user(&pool, "owner@example.test").await;
    let source_calendar_id = create_calendar(&pool, user_id).await;
    let target_calendar_id = create_calendar(&pool, user_id).await;
    let imported_event_id = import_event(&pool, user_id, source_calendar_id).await;

    let baseline_source = change_count(&pool, source_calendar_id, None).await;
    let baseline_target = change_count(&pool, target_calendar_id, None).await;

    let service = EventService::new_at(pool.clone(), 400);
    let change = timed_mutation();
    let result = service
        .update(
            user_id,
            false,
            source_calendar_id,
            imported_event_id,
            EventChange {
                expected_version: 1,
                target_calendar_id,
                event: change,
            },
        )
        .await;
    assert!(
        matches!(result, Err(EventServiceError::ReadOnly)),
        "an imported event must reject a move with ReadOnly"
    );

    assert_eq!(
        change_count(&pool, source_calendar_id, None).await,
        baseline_source,
        "a rejected move of an imported event must record no changes on the source"
    );
    assert_eq!(
        change_count(&pool, target_calendar_id, None).await,
        baseline_target,
        "a rejected move of an imported event must record no changes on the target"
    );
    // The event must still be in the source calendar.
    let event_calendar: i64 = sqlx::query_scalar("SELECT calendar_id FROM events WHERE id = ?")
        .bind(imported_event_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        event_calendar, source_calendar_id,
        "a rejected move must leave the imported event in the source calendar"
    );
}

/// T12: two-calendar sync-token test — after a move, the source calendar's
/// sync-collection surfaces the deletion and the target calendar's
/// sync-collection surfaces the addition.
#[tokio::test]
async fn two_calendar_sync_token_after_move() {
    let (
        _temp_dir,
        pool,
        accounts,
        user_id,
        password,
        principal_id,
        source_calendar_id,
        _event_id,
        _resource_name,
    ) = scenario().await;
    let target_calendar_id = create_calendar(&pool, user_id).await;
    let event_id = create_event(&pool, user_id, source_calendar_id).await;
    let repository = CaldavRepository::new(pool.clone());
    let resource = repository
        .ensure_resource(source_calendar_id, event_id, 1000)
        .await
        .unwrap();

    // Baseline snapshots on both calendars.
    let source_uri = format!("/dav/calendars/{principal_id}/{source_calendar_id}/");
    let target_uri = format!("/dav/calendars/{principal_id}/{target_calendar_id}/");
    let body = sync_collection_xml("");
    let (status, resp) = report_request(
        build_caldav_router(accounts.clone()),
        &source_uri,
        "owner@example.test",
        &password,
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    let source_token = extract_sync_token(&resp).expect("source snapshot must mint a token");

    let (status, resp) = report_request(
        build_caldav_router(accounts.clone()),
        &target_uri,
        "owner@example.test",
        &password,
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    let target_token = extract_sync_token(&resp).expect("target snapshot must mint a token");

    // Move the event from source to target.
    let service = EventService::new_at(pool.clone(), 500);
    let change = timed_mutation();
    service
        .update(
            user_id,
            false,
            source_calendar_id,
            event_id,
            EventChange {
                expected_version: 1,
                target_calendar_id,
                event: change,
            },
        )
        .await
        .unwrap();

    // Source calendar's incremental sync must surface the deletion.
    let body = sync_collection_xml(&source_token);
    let (status, resp) = report_request(
        build_caldav_router(accounts.clone()),
        &source_uri,
        "owner@example.test",
        &password,
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert!(
        resp.contains(&format!(
            "/dav/calendars/{principal_id}/{source_calendar_id}/{}.ics",
            resource.resource_name
        )),
        "source sync must reference the moved resource href"
    );
    assert!(
        resp.contains("<D:status>HTTP/1.1 404 Not Found</D:status>"),
        "source sync must represent the move as a deletion"
    );

    // Target calendar's incremental sync must surface the addition.
    let body = sync_collection_xml(&target_token);
    let (status, resp) = report_request(
        build_caldav_router(accounts),
        &target_uri,
        "owner@example.test",
        &password,
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert!(
        resp.contains(&format!(
            "/dav/calendars/{principal_id}/{target_calendar_id}/{}.ics",
            resource.resource_name
        )),
        "target sync must reference the moved resource href"
    );
    assert!(
        resp.contains("<C:calendar-data>"),
        "target sync must carry the event data"
    );
}

// --- T16: Safe client-property preservation ---

fn vcalendar_with_properties(uid: &str, summary: &str, categories: Option<&str>) -> String {
    let categories_line = categories
        .map(|c| format!("CATEGORIES:{c}\r\n"))
        .unwrap_or_default();
    format!(
        "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         PRODID:-//Test//Test 1.0//EN\r\n\
         BEGIN:VEVENT\r\n\
         UID:{uid}\r\n\
         DTSTAMP:20260101T000000Z\r\n\
         DTSTART:20260115T000000Z\r\n\
         DTEND:20260115T010000Z\r\n\
         SUMMARY:{summary}\r\n\
         {categories_line}\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
    )
}

/// T16: allowlisted metadata (CATEGORIES) survives an unrelated edit (title change).
#[tokio::test]
async fn t16_categories_survive_unrelated_edit() {
    let (temp_dir, pool, accounts, _user_id, password, principal_id, calendar_id) =
        setup_create().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/t16-survive.ics");
    let auth = ("owner@example.test", password.as_str());

    // Create with CATEGORIES.
    let body = vcalendar_with_properties("t16-uid-1", "Original", Some("Work,Personal"));
    let (status, _, response_body) = put_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(auth),
        &body,
        Some("*"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(response_body.contains("CATEGORIES:Work,Personal"));

    // Update the title (unrelated edit), keeping CATEGORIES in the body.
    let etag = get_etag(&accounts, &principal_id, calendar_id, "t16-survive", auth).await;
    let update_body = vcalendar_with_properties("t16-uid-1", "Renamed", Some("Work,Personal"));
    let (status, _, response_body) = put_update_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(auth),
        &update_body,
        Some(etag.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(response_body.contains("SUMMARY:Renamed"));
    assert!(
        response_body.contains("CATEGORIES:Work,Personal"),
        "CATEGORIES must survive an unrelated title edit"
    );

    // Verify via GET.
    let (_, _, get_body) =
        request(build_caldav_router(accounts), Method::GET, &uri, Some(auth)).await;
    assert!(get_body.contains("CATEGORIES:Work,Personal"));
    let _ = (temp_dir, pool);
}

/// T16: explicit removal of CATEGORIES stays removed after an update.
#[tokio::test]
async fn t16_explicit_removal_stays_removed() {
    let (temp_dir, pool, accounts, _user_id, password, principal_id, calendar_id) =
        setup_create().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/t16-remove.ics");
    let auth = ("owner@example.test", password.as_str());

    // Create with CATEGORIES.
    let body = vcalendar_with_properties("t16-uid-2", "Original", Some("Work,Personal"));
    let (status, _, _) = put_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(auth),
        &body,
        Some("*"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Update without CATEGORIES (explicit removal).
    let etag = get_etag(&accounts, &principal_id, calendar_id, "t16-remove", auth).await;
    let update_body = vcalendar_with_properties("t16-uid-2", "Renamed", None);
    let (status, _, response_body) = put_update_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(auth),
        &update_body,
        Some(etag.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !response_body.contains("CATEGORIES:"),
        "CATEGORIES must be removed when absent from the update body"
    );

    // Verify via GET that the removal persisted.
    let (_, _, get_body) =
        request(build_caldav_router(accounts), Method::GET, &uri, Some(auth)).await;
    assert!(
        !get_body.contains("CATEGORIES:"),
        "CATEGORIES removal must persist across reads"
    );
    let _ = (temp_dir, pool);
}

/// T16: scheduling properties (ATTENDEE, ORGANIZER) are rejected on create.
#[tokio::test]
async fn t16_scheduling_properties_rejected_on_create() {
    let (temp_dir, pool, accounts, _user_id, password, principal_id, calendar_id) =
        setup_create().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/t16-sched.ics");
    let auth = ("owner@example.test", password.as_str());

    // ATTENDEE must be rejected.
    let body = "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         PRODID:-//Test//Test 1.0//EN\r\n\
         BEGIN:VEVENT\r\n\
         UID:t16-sched-uid\r\n\
         DTSTAMP:20260101T000000Z\r\n\
         DTSTART:20260115T000000Z\r\n\
         DTEND:20260115T010000Z\r\n\
         SUMMARY:Test\r\n\
         ATTENDEE:mailto:user@example.com\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
        .to_string();
    let (status, _, _) = put_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(auth),
        &body,
        Some("*"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "ATTENDEE must be rejected");

    // ORGANIZER must be rejected.
    let body = "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         PRODID:-//Test//Test 1.0//EN\r\n\
         BEGIN:VEVENT\r\n\
         UID:t16-sched-uid\r\n\
         DTSTAMP:20260101T000000Z\r\n\
         DTSTART:20260115T000000Z\r\n\
         DTEND:20260115T010000Z\r\n\
         SUMMARY:Test\r\n\
         ORGANIZER:mailto:org@example.com\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
        .to_string();
    let (status, _, _) = put_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(auth),
        &body,
        Some("*"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "ORGANIZER must be rejected"
    );

    // METHOD must be rejected.
    let body = "BEGIN:VCALENDAR\r\n\
         VERSION:2.0\r\n\
         PRODID:-//Test//Test 1.0//EN\r\n\
         METHOD:REQUEST\r\n\
         BEGIN:VEVENT\r\n\
         UID:t16-sched-uid\r\n\
         DTSTAMP:20260101T000000Z\r\n\
         DTSTART:20260115T000000Z\r\n\
         DTEND:20260115T010000Z\r\n\
         SUMMARY:Test\r\n\
         END:VEVENT\r\n\
         END:VCALENDAR\r\n"
        .to_string();
    let (status, _, _) = put_request(
        build_caldav_router(accounts),
        &uri,
        Some(auth),
        &body,
        Some("*"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "METHOD must be rejected");
    let _ = (temp_dir, pool);
}

/// T16: ETags are deterministic — identical content yields identical ETags.
#[tokio::test]
async fn t16_etags_are_deterministic() {
    let (temp_dir, pool, accounts, _user_id, password, principal_id, calendar_id) =
        setup_create().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/t16-etag.ics");
    let auth = ("owner@example.test", password.as_str());

    let body = vcalendar_with_properties("t16-etag-uid", "Deterministic", Some("Work"));
    let (status, _, _) = put_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(auth),
        &body,
        Some("*"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Two consecutive GETs must return the same ETag.
    let etag1 = get_etag(&accounts, &principal_id, calendar_id, "t16-etag", auth).await;
    let etag2 = get_etag(&accounts, &principal_id, calendar_id, "t16-etag", auth).await;
    assert_eq!(
        etag1, etag2,
        "ETags must be deterministic for identical content"
    );
    let _ = (temp_dir, pool);
}

/// T16: the full allowlist (categories, URL, TRANSP, alarm, X-property)
/// survives an unrelated edit (title change).
#[tokio::test]
async fn t16_full_allowlist_survives_unrelated_edit() {
    let (temp_dir, pool, accounts, _user_id, password, principal_id, calendar_id) =
        setup_create().await;

    let uri = format!("/dav/calendars/{principal_id}/{calendar_id}/t16-full.ics");
    let auth = ("owner@example.test", password.as_str());

    let full_body = |summary: &str| {
        format!(
            "BEGIN:VCALENDAR\r\n\
             VERSION:2.0\r\n\
             PRODID:-//Test//Test 1.0//EN\r\n\
             BEGIN:VEVENT\r\n\
             UID:t16-full-uid\r\n\
             DTSTAMP:20260101T000000Z\r\n\
             DTSTART:20260115T000000Z\r\n\
             DTEND:20260115T010000Z\r\n\
             SUMMARY:{summary}\r\n\
             CATEGORIES:Work,Personal\r\n\
             URL:https://example.test/event\r\n\
             TRANSP:OPAQUE\r\n\
             X-APPLE-CEVENT-CATEGORY:TYPE:WORK\r\n\
             BEGIN:VALARM\r\n\
             ACTION:DISPLAY\r\n\
             DESCRIPTION:Reminder\r\n\
             TRIGGER:-PT10M\r\n\
             END:VALARM\r\n\
             END:VEVENT\r\n\
             END:VCALENDAR\r\n"
        )
    };

    // Create with the full allowlist.
    let (status, _, response_body) = put_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(auth),
        &full_body("Original"),
        Some("*"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(response_body.contains("CATEGORIES:Work,Personal"));
    assert!(response_body.contains("URL:https://example.test/event"));
    assert!(response_body.contains("TRANSP:OPAQUE"));
    assert!(response_body.contains("X-APPLE-CEVENT-CATEGORY:TYPE:WORK"));
    assert!(response_body.contains("BEGIN:VALARM"));
    assert!(response_body.contains("ACTION:DISPLAY"));

    // Unrelated edit: change only the title, resending the full allowlist.
    let etag = get_etag(&accounts, &principal_id, calendar_id, "t16-full", auth).await;
    let (status, _, response_body) = put_update_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(auth),
        &full_body("Renamed"),
        Some(etag.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(response_body.contains("SUMMARY:Renamed"));
    assert!(
        response_body.contains("CATEGORIES:Work,Personal"),
        "CATEGORIES must survive an unrelated title edit"
    );
    assert!(
        response_body.contains("URL:https://example.test/event"),
        "URL must survive an unrelated title edit"
    );
    assert!(
        response_body.contains("TRANSP:OPAQUE"),
        "TRANSP must survive an unrelated title edit"
    );
    assert!(
        response_body.contains("X-APPLE-CEVENT-CATEGORY:TYPE:WORK"),
        "X-property must survive an unrelated title edit"
    );
    assert!(
        response_body.contains("BEGIN:VALARM"),
        "VALARM must survive an unrelated title edit"
    );

    // Verify via GET that the full allowlist persisted.
    let (_, _, get_body) =
        request(build_caldav_router(accounts), Method::GET, &uri, Some(auth)).await;
    assert!(get_body.contains("CATEGORIES:Work,Personal"));
    assert!(get_body.contains("URL:https://example.test/event"));
    assert!(get_body.contains("TRANSP:OPAQUE"));
    assert!(get_body.contains("X-APPLE-CEVENT-CATEGORY:TYPE:WORK"));
    assert!(get_body.contains("BEGIN:VALARM"));
    let _ = (temp_dir, pool);
}

#[derive(Debug)]
struct XmlNode {
    namespace: String,
    name: String,
    text: String,
    children: Vec<XmlNode>,
}

impl XmlNode {
    fn descendants<'a>(&'a self, namespace: &str, name: &str) -> Vec<&'a Self> {
        let mut found = Vec::new();
        if self.namespace == namespace && self.name == name {
            found.push(self);
        }
        for child in &self.children {
            found.extend(child.descendants(namespace, name));
        }
        found
    }
}

fn xml_tree(xml: &str) -> XmlNode {
    use quick_xml::{NsReader, events::Event, name::ResolveResult};
    let mut reader = NsReader::from_str(xml);
    let mut stack: Vec<XmlNode> = Vec::new();
    let mut root = None;
    loop {
        let (namespace, event) = reader.read_resolved_event().expect("well-formed XML");
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let namespace = match namespace {
                    ResolveResult::Bound(ns) => {
                        quick_xml::escape::unescape(std::str::from_utf8(ns.as_ref()).unwrap())
                            .unwrap()
                            .into_owned()
                    }
                    ResolveResult::Unbound => String::new(),
                    ResolveResult::Unknown(prefix) => panic!("unbound XML prefix: {prefix:?}"),
                };
                let node = XmlNode {
                    namespace,
                    name: String::from_utf8(element.local_name().as_ref().to_vec()).unwrap(),
                    text: String::new(),
                    children: Vec::new(),
                };
                if matches!(event, Event::Start(_)) {
                    stack.push(node);
                } else if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                } else {
                    assert!(root.replace(node).is_none());
                }
            }
            Event::Text(text) => {
                if let Some(node) = stack.last_mut() {
                    node.text.push_str(&text.unescape().unwrap());
                }
            }
            Event::CData(text) => {
                if let Some(node) = stack.last_mut() {
                    node.text
                        .push_str(std::str::from_utf8(text.as_ref()).unwrap());
                }
            }
            Event::End(_) => {
                let node = stack.pop().expect("balanced XML");
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                } else {
                    assert!(root.replace(node).is_none(), "one root");
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    assert!(stack.is_empty());
    root.expect("XML document root")
}

async fn semantic_propfind(
    accounts: &CaldavAccountService,
    uri: &str,
    password: &str,
    depth: &str,
    body: &str,
) -> (StatusCode, XmlNode) {
    let response = build_caldav_router(accounts.clone())
        .oneshot(
            Request::builder()
                .method("PROPFIND")
                .uri(uri)
                .header("Depth", depth)
                .header("x-forwarded-for", "127.0.0.2")
                .header(
                    header::AUTHORIZATION,
                    basic_header("owner@example.test", password),
                )
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, xml_tree(std::str::from_utf8(&bytes).unwrap()))
}

#[tokio::test]
async fn xml_properties_preserve_namespaces_and_nonempty_status_groups() {
    let (_temp, _pool, accounts, _user, password, principal, calendar, _event, resource) =
        scenario().await;
    let uris = [
        "/dav/".to_owned(),
        format!("/dav/principals/{principal}/"),
        format!("/dav/calendars/{principal}/"),
        format!("/dav/calendars/{principal}/{calendar}/"),
        format!("/dav/calendars/{principal}/{calendar}/{resource}.ics"),
    ];
    for uri in uris {
        let body = r#"<D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:A="http://apple.com/ns/ical/" xmlns:X="urn:client:&amp;extension"><D:prop><D:current-user-principal></D:current-user-principal><D:unsupported/><C:unsupported/><A:unsupported/><X:unsupported/></D:prop></D:propfind>"#;
        let (status, root) = semantic_propfind(&accounts, &uri, &password, "0", body).await;
        assert_eq!(status, StatusCode::MULTI_STATUS, "{uri}");
        assert_eq!((&*root.namespace, &*root.name), ("DAV:", "multistatus"));
        let groups = root.descendants("DAV:", "propstat");
        assert_eq!(groups.len(), 2, "{uri}: {root:?}");
        for group in groups {
            let prop = group
                .children
                .iter()
                .find(|n| n.name == "prop" && n.namespace == "DAV:")
                .unwrap();
            assert!(!prop.children.is_empty());
            assert!(
                prop.text.trim().is_empty(),
                "property names cannot be bare text"
            );
            let status = &group
                .children
                .iter()
                .find(|n| n.name == "status")
                .unwrap()
                .text;
            if status.contains("200") {
                assert_eq!(prop.children.len(), 1);
                assert_eq!(prop.children[0].name, "current-user-principal");
                assert_eq!(
                    url::Url::parse("http://127.0.0.1:3000")
                        .unwrap()
                        .join(&prop.children[0].descendants("DAV:", "href")[0].text)
                        .unwrap()
                        .path(),
                    format!("/dav/principals/{principal}/")
                );
            } else {
                assert!(status.contains("404"));
                assert_eq!(prop.children.len(), 4);
                for ns in [
                    "DAV:",
                    "urn:ietf:params:xml:ns:caldav",
                    "http://apple.com/ns/ical/",
                    "urn:client:&extension",
                ] {
                    let property = prop
                        .children
                        .iter()
                        .find(|n| n.namespace == ns)
                        .unwrap_or_else(|| panic!("missing namespace {ns}: {root:?}"));
                    assert_eq!(property.name, "unsupported");
                    assert!(property.children.is_empty() && property.text.is_empty());
                }
            }
        }
    }
}

#[tokio::test]
async fn supported_and_empty_property_selection_have_no_404_group() {
    let (_temp, _pool, accounts, _user, password, principal, calendar, _event, resource) =
        scenario().await;
    let uri = format!("/dav/calendars/{principal}/{calendar}/{resource}.ics");
    for selection in ["<D:getetag/><D:resourcetype/>", ""] {
        let body =
            format!("<D:propfind xmlns:D=\"DAV:\"><D:prop>{selection}</D:prop></D:propfind>");
        let (status, root) = semantic_propfind(&accounts, &uri, &password, "0", &body).await;
        assert_eq!(status, StatusCode::MULTI_STATUS);
        let groups = root.descendants("DAV:", "propstat");
        assert_eq!(groups.len(), usize::from(!selection.is_empty()));
        if !selection.is_empty() {
            assert!(
                groups[0].descendants("DAV:", "status")[0]
                    .text
                    .contains("200")
            );
            assert_eq!(root.descendants("DAV:", "getetag").len(), 1);
            assert!(
                root.descendants("DAV:", "resourcetype")[0]
                    .children
                    .is_empty()
            );
        }
    }
}

#[tokio::test]
async fn semantic_discovery_crud_and_incremental_sync_sequence() {
    let (_temp, pool, accounts, user, password, principal, calendar, _event, _resource) =
        scenario().await;
    let root_props =
        r#"<D:propfind xmlns:D="DAV:"><D:prop><D:current-user-principal/></D:prop></D:propfind>"#;
    let (status, root) = semantic_propfind(&accounts, "/dav/", &password, "0", root_props).await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    let principal_uri = url::Url::parse("http://127.0.0.1:3000")
        .unwrap()
        .join(
            &root.descendants("DAV:", "current-user-principal")[0].descendants("DAV:", "href")[0]
                .text,
        )
        .unwrap();
    let (_, root) = semantic_propfind(&accounts, principal_uri.path(), &password, "0", r#"<D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><D:prop><C:calendar-home-set/></D:prop></D:propfind>"#).await;
    let home_uri = principal_uri
        .join(
            &root.descendants("urn:ietf:params:xml:ns:caldav", "calendar-home-set")[0]
                .descendants("DAV:", "href")[0]
                .text,
        )
        .unwrap();
    let (status, root) = semantic_propfind(&accounts, home_uri.path(), &password, "1", "").await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert_eq!(
        root.descendants("urn:ietf:params:xml:ns:caldav", "calendar")
            .len(),
        1
    );
    let collection = format!("/dav/calendars/{principal}/{calendar}/");
    let (status, initial) = report_request(
        build_caldav_router(accounts.clone()),
        &collection,
        "owner@example.test",
        &password,
        &sync_collection_xml(""),
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    let token = extract_sync_token(&initial).unwrap();
    assert!(
        url::Url::parse(&token).is_ok(),
        "opaque token is absolute URI"
    );
    let uri = format!("{collection}protocol-sequence.ics");
    let auth = ("owner@example.test", password.as_str());
    let (status, headers, _) = put_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(auth),
        &vcalendar_body("protocol-uid", "Created"),
        Some("*"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(headers.get(header::ETAG).is_none());
    let (status, headers, content) = request(
        build_caldav_router(accounts.clone()),
        Method::GET,
        &uri,
        Some(auth),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(content.contains("SUMMARY:Created"));
    let etag = headers.get(header::ETAG).unwrap().to_str().unwrap();
    let (_, root) = semantic_propfind(
        &accounts,
        &uri,
        &password,
        "0",
        r#"<D:propfind xmlns:D="DAV:"><D:prop><D:getetag/></D:prop></D:propfind>"#,
    )
    .await;
    assert_eq!(root.descendants("DAV:", "getetag")[0].text, etag);
    let (status, headers, _) = put_update_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(auth),
        &vcalendar_body("protocol-uid", "Updated"),
        Some(etag),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.get(header::ETAG).is_none());
    let (status, delta) = report_request(
        build_caldav_router(accounts.clone()),
        &collection,
        auth.0,
        auth.1,
        &sync_collection_xml(&token),
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    let root = xml_tree(&delta);
    assert_eq!(
        root.descendants("DAV:", "response").len(),
        1,
        "create/update coalesced"
    );
    assert!(
        root.descendants("urn:ietf:params:xml:ns:caldav", "calendar-data")[0]
            .text
            .contains("SUMMARY:Updated")
    );
    let token = extract_sync_token(&delta).unwrap();
    let etag = get_etag(&accounts, &principal, calendar, "protocol-sequence", auth).await;
    assert_eq!(
        delete_request(
            build_caldav_router(accounts.clone()),
            &uri,
            Some(auth),
            Some(&etag)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let (status, delta) = report_request(
        build_caldav_router(accounts.clone()),
        &collection,
        auth.0,
        auth.1,
        &sync_collection_xml(&token),
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    let root = xml_tree(&delta);
    let responses = root.descendants("DAV:", "response");
    assert_eq!(responses.len(), 1);
    assert!(
        responses[0]
            .children
            .iter()
            .any(|n| n.namespace == "DAV:" && n.name == "status" && n.text.contains("404"))
    );
    assert!(responses[0].descendants("DAV:", "propstat").is_empty());
    let other = create_calendar(&pool, user).await;
    let (status, error) = report_request(
        build_caldav_router(accounts),
        &format!("/dav/calendars/{principal}/{other}/"),
        auth.0,
        auth.1,
        &sync_collection_xml(&token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        xml_tree(&error)
            .descendants("DAV:", "valid-sync-token")
            .len(),
        1
    );
}

#[tokio::test]
async fn initial_sync_pages_preserve_concurrent_changes_after_snapshot() {
    let (_temp, pool, accounts, user, password, principal, calendar, _event, _resource) =
        scenario().await;
    create_event(&pool, user, calendar).await;
    create_event(&pool, user, calendar).await;
    let collection = format!("/dav/calendars/{principal}/{calendar}/");
    let limited = |token: &str| {
        sync_collection_xml(token).replace(
            "</D:sync-collection>",
            "<D:limit><D:nresults>1</D:nresults></D:limit></D:sync-collection>",
        )
    };
    let mut token = String::new();
    let mut seen = std::collections::HashSet::new();
    for page in 0..3 {
        let (status, body) = report_request(
            build_caldav_router(accounts.clone()),
            &collection,
            "owner@example.test",
            &password,
            &limited(&token),
        )
        .await;
        assert_eq!(status, StatusCode::MULTI_STATUS);
        let root = xml_tree(&body);
        let responses = root.descendants("DAV:", "response");
        let resources: Vec<_> = responses
            .iter()
            .filter(|node| !node.descendants("DAV:", "propstat").is_empty())
            .collect();
        assert_eq!(resources.len(), 1, "one resource per requested page");
        let href = &resources[0].descendants("DAV:", "href")[0].text;
        assert!(
            seen.insert(href.clone()),
            "snapshot pages cannot repeat href"
        );
        if page < 2 {
            assert_eq!(
                root.descendants("DAV:", "number-of-matches-within-limits")
                    .len(),
                1
            );
        }
        token = extract_sync_token(&body).unwrap();
        if page == 0 {
            let mut mutation = timed_mutation();
            mutation.title = "Concurrent".to_owned();
            EventService::new_at(pool.clone(), 600)
                .create(user, false, calendar, mutation)
                .await
                .unwrap();
        }
    }
    let (status, body) = report_request(
        build_caldav_router(accounts),
        &collection,
        "owner@example.test",
        &password,
        &sync_collection_xml(&token),
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    let root = xml_tree(&body);
    assert!(
        root.descendants("urn:ietf:params:xml:ns:caldav", "calendar-data")
            .iter()
            .any(|n| n.text.contains("SUMMARY:Concurrent")),
        "writes during paginated initial snapshot must reach incremental sync"
    );
}

#[tokio::test]
async fn multiget_absolute_href_respects_property_subset_and_namespace_status() {
    let (_temp, _pool, accounts, _user, password, principal, calendar, _event, resource) =
        scenario().await;
    let collection = format!("/dav/calendars/{principal}/{calendar}/");
    let href = format!("http://127.0.0.1:3000{collection}{resource}.ics");
    let body = format!(
        r#"<C:calendar-multiget xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:D="DAV:" xmlns:X="urn:client"><D:prop><D:getetag></D:getetag><X:unknown/></D:prop><D:href>{href}</D:href></C:calendar-multiget>"#
    );
    let (status, response) = report_request(
        build_caldav_router(accounts.clone()),
        &collection,
        "owner@example.test",
        &password,
        &body,
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    let root = xml_tree(&response);
    assert_eq!(root.descendants("DAV:", "response").len(), 1);
    assert_eq!(root.descendants("DAV:", "getetag").len(), 1);
    assert_eq!(root.descendants("urn:client", "unknown").len(), 1);
    assert!(
        root.descendants("urn:ietf:params:xml:ns:caldav", "calendar-data")
            .is_empty()
    );
    assert!(root.descendants("DAV:", "getcontenttype").is_empty());
    let (status, response) = report_request(
        build_caldav_router(accounts),
        &collection,
        "owner@example.test",
        &password,
        &body.replace("http://127.0.0.1:3000", "https://foreign.example"),
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    let root = xml_tree(&response);
    assert!(
        root.descendants("DAV:", "getetag").is_empty(),
        "foreign href cannot return authorized local content"
    );
    assert!(
        root.descendants("urn:ietf:params:xml:ns:caldav", "calendar-data")
            .is_empty()
    );
}

#[tokio::test]
async fn depth_and_permission_metadata_match_implemented_methods() {
    let (_temp, _pool, accounts, _user, password, principal, calendar, _event, _resource) =
        scenario().await;
    let collection = format!("/dav/calendars/{principal}/{calendar}/");
    for uri in ["/dav/".to_owned(), collection.clone()] {
        let response = build_caldav_router(accounts.clone())
            .oneshot(
                Request::builder()
                    .method("PROPFIND")
                    .uri(&uri)
                    .header("Depth", "infinity")
                    .header("x-forwarded-for", &uri)
                    .header(
                        header::AUTHORIZATION,
                        basic_header("owner@example.test", &password),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            xml_tree(std::str::from_utf8(&body).unwrap())
                .descendants("DAV:", "propfind-finite-depth")
                .len(),
            1
        );
    }
    let props = r#"<D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><D:prop><D:current-user-privilege-set/><C:supported-calendar-data/><C:supported-calendar-component-set/><D:sync-token/></D:prop></D:propfind>"#;
    let (status, root) = semantic_propfind(&accounts, &collection, &password, "0", props).await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert_eq!(root.descendants("DAV:", "propstat").len(), 1);
    let privileges = root.descendants("DAV:", "current-user-privilege-set")[0];
    for unimplemented in ["all", "write-acl", "write-properties"] {
        assert!(privileges.descendants("DAV:", unimplemented).is_empty());
    }
    for implemented in ["read", "write-content", "bind", "unbind"] {
        assert_eq!(privileges.descendants("DAV:", implemented).len(), 1);
    }
    assert_eq!(
        root.descendants("urn:ietf:params:xml:ns:caldav", "calendar-data")
            .len(),
        1
    );
    assert_eq!(
        root.descendants("urn:ietf:params:xml:ns:caldav", "comp")
            .len(),
        1
    );
    assert!(url::Url::parse(&root.descendants("DAV:", "sync-token")[0].text).is_ok());
}

#[tokio::test]
async fn visibility_changes_and_regrant_invalidate_previous_sync_tokens() {
    let (_temp, pool, accounts, _user, _password, _principal, calendar, _event, _resource) =
        scenario().await;
    let viewer = create_user(&pool, "viewer@example.test").await;
    sqlx::query("INSERT INTO calendar_acl (calendar_id,user_id,role,created_at,updated_at) VALUES (?,?,'viewer',100,100)").bind(calendar).bind(viewer).execute(&pool).await.unwrap();
    let password = accounts
        .issue_credential(viewer, "Viewer".to_owned())
        .await
        .unwrap()
        .password
        .expose()
        .to_owned();
    let principal = accounts.status(viewer).await.unwrap().principal_id.unwrap();
    let uri = format!("/dav/calendars/{principal}/{calendar}/");
    let (status, body) = report_request(
        build_caldav_router(accounts.clone()),
        &uri,
        "viewer@example.test",
        &password,
        &sync_collection_xml(""),
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert!(
        xml_tree(&body).descendants("urn:ietf:params:xml:ns:caldav", "calendar-data")[0]
            .text
            .contains("SUMMARY:Planning")
    );
    let token = extract_sync_token(&body).unwrap();
    sqlx::query(
        "UPDATE calendar_acl SET role='free_busy_viewer' WHERE calendar_id=? AND user_id=?",
    )
    .bind(calendar)
    .bind(viewer)
    .execute(&pool)
    .await
    .unwrap();
    let (status, body) = report_request(
        build_caldav_router(accounts.clone()),
        &uri,
        "viewer@example.test",
        &password,
        &sync_collection_xml(&token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        xml_tree(&body)
            .descendants("DAV:", "valid-sync-token")
            .len(),
        1
    );
    let (status, body) = report_request(
        build_caldav_router(accounts.clone()),
        &uri,
        "viewer@example.test",
        &password,
        &sync_collection_xml(""),
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert!(!body.contains("Planning"));
    let token = extract_sync_token(&body).unwrap();
    sqlx::query("DELETE FROM calendar_acl WHERE calendar_id=? AND user_id=?")
        .bind(calendar)
        .bind(viewer)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO calendar_acl (calendar_id,user_id,role,created_at,updated_at) VALUES (?,?,'free_busy_viewer',100,100)").bind(calendar).bind(viewer).execute(&pool).await.unwrap();
    let (status, body) = report_request(
        build_caldav_router(accounts),
        &uri,
        "viewer@example.test",
        &password,
        &sync_collection_xml(&token),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        xml_tree(&body)
            .descendants("DAV:", "valid-sync-token")
            .len(),
        1
    );
}

#[tokio::test]
async fn deletion_and_recreation_sync_emit_one_final_live_resource() {
    let (_temp, _pool, accounts, _user, password, principal, calendar, _event, _resource) =
        scenario().await;
    let collection = format!("/dav/calendars/{principal}/{calendar}/");
    let uri = format!("{collection}recreated.ics");
    let auth = ("owner@example.test", password.as_str());
    assert_eq!(
        put_request(
            build_caldav_router(accounts.clone()),
            &uri,
            Some(auth),
            &vcalendar_body("recreated-uid", "Before"),
            Some("*")
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let (_, body) = report_request(
        build_caldav_router(accounts.clone()),
        &collection,
        auth.0,
        auth.1,
        &sync_collection_xml(""),
    )
    .await;
    let token = extract_sync_token(&body).unwrap();
    let etag = get_etag(&accounts, &principal, calendar, "recreated", auth).await;
    assert_eq!(
        delete_request(
            build_caldav_router(accounts.clone()),
            &uri,
            Some(auth),
            Some(&etag)
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        put_request(
            build_caldav_router(accounts.clone()),
            &uri,
            Some(auth),
            &vcalendar_body("recreated-uid", "After"),
            Some("*")
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let (status, body) = report_request(
        build_caldav_router(accounts),
        &collection,
        auth.0,
        auth.1,
        &sync_collection_xml(&token),
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    let root = xml_tree(&body);
    let responses = root.descendants("DAV:", "response");
    assert_eq!(responses.len(), 1, "same href must coalesce to final state");
    assert_eq!(responses[0].descendants("DAV:", "href")[0].text, uri);
    assert!(
        responses[0].descendants("urn:ietf:params:xml:ns:caldav", "calendar-data")[0]
            .text
            .contains("SUMMARY:After")
    );
    assert!(
        responses[0]
            .descendants("DAV:", "status")
            .iter()
            .all(|n| n.text.contains("200"))
    );
}

#[tokio::test]
async fn entity_namespace_is_resolved_and_invalid_qnames_are_rejected() {
    let (_temp, _pool, accounts, _user, password, _principal, _calendar, _event, _resource) =
        scenario().await;
    let body = r#"<D:propfind xmlns:D="DAV&#58;"><D:prop><D:current-user-principal/></D:prop></D:propfind>"#;
    let (status, root) = semantic_propfind(&accounts, "/dav/", &password, "0", body).await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert_eq!(root.descendants("DAV:", "current-user-principal").len(), 1);
    for body in [
        r#"<D:propfind xmlns:D="DAV:"><D:prop><D:1etag/></D:prop></D:propfind>"#,
        r#"<D:propfind xmlns:D="DAV:"><D:prop><D:get:etag/></D:prop></D:propfind>"#,
    ] {
        let response = build_caldav_router(accounts.clone())
            .oneshot(
                Request::builder()
                    .method("PROPFIND")
                    .uri("/dav/")
                    .header("Depth", "0")
                    .header(
                        header::AUTHORIZATION,
                        basic_header("owner@example.test", &password),
                    )
                    .body(Body::from(body.to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn reports_accept_allprop_propname_and_exclude_calendar_data() {
    let (_temp, _pool, accounts, _user, password, principal, calendar, _event, resource) =
        scenario().await;
    let collection = format!("/dav/calendars/{principal}/{calendar}/");
    for selector in ["allprop", "propname"] {
        let body = format!(
            r#"<C:calendar-multiget xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:D="DAV:"><D:{selector}/><D:href>{collection}{resource}.ics</D:href></C:calendar-multiget>"#
        );
        let (status, body) = report_request(
            build_caldav_router(accounts.clone()),
            &collection,
            "owner@example.test",
            &password,
            &body,
        )
        .await;
        assert_eq!(status, StatusCode::MULTI_STATUS);
        let root = xml_tree(&body);
        assert_eq!(root.descendants("DAV:", "response").len(), 1);
        assert_eq!(root.descendants("DAV:", "getetag").len(), 1);
        assert!(
            root.descendants("urn:ietf:params:xml:ns:caldav", "calendar-data")
                .is_empty()
        );
        assert_eq!(root.descendants("DAV:", "propstat").len(), 1);
        if selector == "propname" {
            for property in &root.descendants("DAV:", "prop")[0].children {
                assert!(property.text.is_empty() && property.children.is_empty());
            }
        } else {
            assert!(!root.descendants("DAV:", "getetag")[0].text.is_empty());
        }
    }
}

#[tokio::test]
async fn calendar_query_depth_defaults_to_zero_and_one_searches_members() {
    let (_temp, _pool, accounts, _user, password, principal, calendar, _event, _resource) =
        scenario().await;
    let uri = format!("/dav/calendars/{principal}/{calendar}/");
    let body = calendar_query_xml("20260101T000000Z", "20260201T000000Z");
    for depth in [None, Some("0"), Some("1")] {
        let mut request = Request::builder().method("REPORT").uri(&uri).header(
            header::AUTHORIZATION,
            basic_header("owner@example.test", &password),
        );
        if let Some(depth) = depth {
            request = request.header("Depth", depth);
        }
        let response = build_caldav_router(accounts.clone())
            .oneshot(request.body(Body::from(body.clone())).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::MULTI_STATUS);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            xml_tree(std::str::from_utf8(&bytes).unwrap())
                .descendants("DAV:", "response")
                .len(),
            usize::from(depth == Some("1"))
        );
    }
}

#[tokio::test]
async fn future_and_tampered_collection_tokens_return_valid_sync_token_error() {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let (_temp, _pool, accounts, _user, password, principal, calendar, _event, _resource) =
        scenario().await;
    let uri = format!("/dav/calendars/{principal}/{calendar}/");
    let (_, body) = report_request(
        build_caldav_router(accounts.clone()),
        &uri,
        "owner@example.test",
        &password,
        &sync_collection_xml(""),
    )
    .await;
    let valid = extract_sync_token(&body).unwrap();
    let encoded = valid.strip_prefix("urn:happening:sync:v1:").unwrap();
    let (payload, tag) = encoded.split_once('.').unwrap();
    let payload = String::from_utf8(URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap();
    let context = payload.lines().next().unwrap();
    let future = accounts.collection_sync_token(context, i64::MAX);
    let mut bad_tag = tag.to_owned();
    let first = bad_tag.remove(0);
    bad_tag.insert(0, if first == 'A' { 'B' } else { 'A' });
    let tampered = format!(
        "urn:happening:sync:v1:{}.{}",
        URL_SAFE_NO_PAD.encode(payload),
        bad_tag
    );
    for token in [future, tampered] {
        let (status, error) = report_request(
            build_caldav_router(accounts.clone()),
            &uri,
            "owner@example.test",
            &password,
            &sync_collection_xml(&token),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(
            xml_tree(&error)
                .descendants("DAV:", "valid-sync-token")
                .len(),
            1
        );
    }
}

#[tokio::test]
async fn signed_collection_tokens_expire_at_boundary() {
    let (_temp, pool) = setup().await;
    let key = SecretKey::generate();
    let origin = url::Url::parse("http://127.0.0.1:3000").unwrap();
    let context = "calendar:visibility:epoch";
    let issued = CaldavAccountService::new_at(pool.clone(), key.clone(), origin.clone(), 1000)
        .collection_sync_token(context, 42);
    let before = CaldavAccountService::new_at(
        pool.clone(),
        key.clone(),
        origin.clone(),
        1000 + 30 * 86400 - 1,
    );
    assert_eq!(before.collection_sync_revision(&issued, context), Some(42));
    let expired = CaldavAccountService::new_at(pool, key, origin, 1000 + 30 * 86400);
    assert_eq!(expired.collection_sync_revision(&issued, context), None);
}

async fn query_resource_hrefs(
    accounts: &CaldavAccountService,
    uri: &str,
    password: &str,
    range: &str,
) -> Vec<String> {
    let body = format!(
        r#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:D="DAV:"><D:prop><D:getetag/></D:prop><C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT">{range}</C:comp-filter></C:comp-filter></C:filter></C:calendar-query>"#
    );
    let (status, response) = report_request(
        build_caldav_router(accounts.clone()),
        uri,
        "owner@example.test",
        password,
        &body,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::MULTI_STATUS,
        "query: {range}, response: {response}"
    );
    xml_tree(&response)
        .descendants("DAV:", "response")
        .iter()
        .map(|n| n.descendants("DAV:", "href")[0].text.clone())
        .collect()
}

#[tokio::test]
async fn recurrence_query_membership_accounts_for_exhaustion_deletions_and_moved_instances() {
    use commoncal_backend::event::OccurrenceChange;
    let (_temp, pool, accounts, user, password, principal, calendar) = setup_create().await;
    let service = EventService::new_at(pool.clone(), 2000);
    let exhausted = service
        .create_recurring(
            user,
            false,
            calendar,
            timed_mutation(),
            "FREQ=DAILY;COUNT=2".to_owned(),
        )
        .await
        .unwrap();
    let repository = CaldavRepository::new(pool.clone());
    let resource = repository
        .ensure_resource(calendar, exhausted.id, 2000)
        .await
        .unwrap();
    let uri = format!("/dav/calendars/{principal}/{calendar}/");
    let href = format!("{uri}{}.ics", resource.resource_name);
    assert!(
        query_resource_hrefs(
            &accounts,
            &uri,
            &password,
            r#"<C:time-range start="20260201T000000Z" end="20260202T000000Z"/>"#
        )
        .await
        .is_empty(),
        "exhausted series cannot match future ranges"
    );
    assert_eq!(
        query_resource_hrefs(
            &accounts,
            &uri,
            &password,
            r#"<C:time-range start="20260115T000000Z" end="20260115T010000Z"/>"#
        )
        .await,
        std::slice::from_ref(&href)
    );
    service
        .delete_occurrence(user, false, calendar, exhausted.id, 1_768_435_200, 1)
        .await
        .unwrap();
    assert!(
        query_resource_hrefs(
            &accounts,
            &uri,
            &password,
            r#"<C:time-range start="20260115T000000Z" end="20260115T010000Z"/>"#
        )
        .await
        .is_empty(),
        "deleted occurrence must stop matching"
    );
    let mut moved = timed_mutation();
    moved.timing = EventTiming::Timed {
        start_utc: 1_769_904_000,
        end_utc: 1_769_907_600,
        timezone: "UTC".to_owned(),
    };
    service
        .update_occurrence(
            user,
            false,
            calendar,
            exhausted.id,
            OccurrenceChange {
                recurrence_id: 1_768_521_600,
                expected_version: 2,
                event: moved,
            },
        )
        .await
        .unwrap();
    assert!(
        query_resource_hrefs(
            &accounts,
            &uri,
            &password,
            r#"<C:time-range start="20260116T000000Z" end="20260116T010000Z"/>"#
        )
        .await
        .is_empty(),
        "moved original occurrence must not match old range"
    );
    assert_eq!(
        query_resource_hrefs(
            &accounts,
            &uri,
            &password,
            r#"<C:time-range start="20260201T000000Z" end="20260201T010000Z"/>"#
        )
        .await,
        std::slice::from_ref(&href),
        "moved exception must match destination range outside original series"
    );
    assert_eq!(
        query_resource_hrefs(
            &accounts,
            &uri,
            &password,
            r#"<C:time-range start="20260201T000000Z"/>"#
        )
        .await,
        std::slice::from_ref(&href)
    );
    assert!(
        query_resource_hrefs(
            &accounts,
            &uri,
            &password,
            r#"<C:time-range end="20260116T000000Z"/>"#
        )
        .await
        .is_empty()
    );
    assert_eq!(
        query_resource_hrefs(&accounts, &uri, &password, "").await,
        [href]
    );
}

#[tokio::test]
async fn recurring_all_day_query_matches_subday_overlap_and_exclusive_end() {
    let (_temp, pool, accounts, user, password, principal, calendar) = setup_create().await;
    let mut mutation = timed_mutation();
    mutation.timing = EventTiming::AllDay {
        start_date: "2026-01-15".to_owned(),
        end_date: "2026-01-16".to_owned(),
    };
    EventService::new_at(pool, 2000)
        .create_recurring(
            user,
            false,
            calendar,
            mutation,
            "FREQ=DAILY;COUNT=2".to_owned(),
        )
        .await
        .unwrap();
    let uri = format!("/dav/calendars/{principal}/{calendar}/");
    assert_eq!(
        query_resource_hrefs(
            &accounts,
            &uri,
            &password,
            r#"<C:time-range start="20260116T120000Z" end="20260116T130000Z"/>"#
        )
        .await
        .len(),
        1
    );
    assert!(
        query_resource_hrefs(
            &accounts,
            &uri,
            &password,
            r#"<C:time-range start="20260117T000000Z" end="20260117T010000Z"/>"#
        )
        .await
        .is_empty()
    );
}

#[tokio::test]
async fn query_overflow_returns_limit_precondition_instead_of_partial_resources() {
    let (_temp, pool, accounts, user, password, principal, calendar) = setup_create().await;
    let event = create_event(&pool, user, calendar).await;
    let mut transaction = pool.begin().await.unwrap();
    for _ in 0..commoncal_backend::caldav::query::MAX_RESULTS {
        sqlx::query("INSERT INTO events (calendar_id,title,description,location,status,event_kind,timed_start_utc,timed_end_utc,event_timezone,created_by_user_id,last_edited_by_user_id,version,created_at,updated_at) SELECT calendar_id,title,description,location,status,event_kind,timed_start_utc,timed_end_utc,event_timezone,created_by_user_id,last_edited_by_user_id,version,created_at,updated_at FROM events WHERE id=?").bind(event).execute(&mut *transaction).await.unwrap();
    }
    transaction.commit().await.unwrap();
    let uri = format!("/dav/calendars/{principal}/{calendar}/");
    let (status, body) = report_request(
        build_caldav_router(accounts),
        &uri,
        "owner@example.test",
        &password,
        &calendar_query_xml("20260115T000000Z", "20260116T000000Z"),
    )
    .await;
    assert_eq!(status, StatusCode::INSUFFICIENT_STORAGE);
    let root = xml_tree(&body);
    assert_eq!(
        root.descendants("DAV:", "number-of-matches-within-limits")
            .len(),
        1
    );
    assert!(root.descendants("DAV:", "response").is_empty());
}

#[tokio::test]
async fn imported_feed_create_update_missing_item_and_feed_delete_produce_sync_changes() {
    let (_temp, pool, accounts, user, password, principal, calendar) = setup_create().await;
    let feed = ExternalFeedService::new_at(pool, SecretKey::generate(), 2000);
    let feed_id = feed
        .create(
            user,
            false,
            calendar,
            NewFeed {
                source_url: "https://feeds.example.test/sync.ics".to_owned(),
                refresh_interval_seconds: Some(60),
            },
        )
        .await
        .unwrap()
        .id;
    let uri = format!("/dav/calendars/{principal}/{calendar}/");
    let (_, initial) = report_request(
        build_caldav_router(accounts.clone()),
        &uri,
        "owner@example.test",
        &password,
        &sync_collection_xml(""),
    )
    .await;
    let mut token = extract_sync_token(&initial).unwrap();
    let mut imported_href = String::new();
    for summary in ["Imported first", "Imported updated"] {
        feed.refresh(
            user,
            false,
            feed_id,
            &FixedFetcher {
                body: vcalendar_body("feed-sync-uid", summary),
            },
        )
        .await
        .unwrap();
        let (status, body) = report_request(
            build_caldav_router(accounts.clone()),
            &uri,
            "owner@example.test",
            &password,
            &sync_collection_xml(&token),
        )
        .await;
        assert_eq!(status, StatusCode::MULTI_STATUS);
        let root = xml_tree(&body);
        let responses = root.descendants("DAV:", "response");
        assert_eq!(responses.len(), 1);
        let href = responses[0].descendants("DAV:", "href")[0].text.clone();
        if imported_href.is_empty() {
            imported_href = href;
        } else {
            assert_eq!(href, imported_href);
        }
        assert!(
            responses[0].descendants("urn:ietf:params:xml:ns:caldav", "calendar-data")[0]
                .text
                .contains(summary)
        );
        token = extract_sync_token(&body).unwrap();
    }
    // A valid replacement feed omits the original UID and introduces another.
    feed.refresh(
        user,
        false,
        feed_id,
        &FixedFetcher {
            body: vcalendar_body("feed-replacement-uid", "Replacement"),
        },
    )
    .await
    .unwrap();
    let (status, body) = report_request(
        build_caldav_router(accounts.clone()),
        &uri,
        "owner@example.test",
        &password,
        &sync_collection_xml(&token),
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    let root = xml_tree(&body);
    let responses = root.descendants("DAV:", "response");
    assert_eq!(
        responses.len(),
        2,
        "replacement emits original tombstone and new event"
    );
    let deleted = responses
        .iter()
        .find(|n| n.descendants("DAV:", "href")[0].text == imported_href)
        .unwrap();
    assert!(
        deleted
            .children
            .iter()
            .any(|n| n.namespace == "DAV:" && n.name == "status" && n.text.contains("404"))
    );
    assert!(deleted.descendants("DAV:", "propstat").is_empty());
    let replacement = responses
        .iter()
        .find(|n| {
            !n.descendants("urn:ietf:params:xml:ns:caldav", "calendar-data")
                .is_empty()
        })
        .unwrap();
    assert!(
        replacement.descendants("urn:ietf:params:xml:ns:caldav", "calendar-data")[0]
            .text
            .contains("Replacement")
    );
    imported_href = replacement.descendants("DAV:", "href")[0].text.clone();
    token = extract_sync_token(&body).unwrap();
    feed.delete(user, false, feed_id).await.unwrap();
    let (status, body) = report_request(
        build_caldav_router(accounts),
        &uri,
        "owner@example.test",
        &password,
        &sync_collection_xml(&token),
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    let root = xml_tree(&body);
    assert_eq!(root.descendants("DAV:", "response").len(), 1);
    assert_eq!(root.descendants("DAV:", "href")[0].text, imported_href);
    assert!(root.descendants("DAV:", "status")[0].text.contains("404"));
}

#[tokio::test]
async fn recurrence_query_preserves_local_time_across_dst_transition() {
    let (_temp, pool, accounts, user, password, principal, calendar) = setup_create().await;
    let start = chrono::DateTime::parse_from_rfc3339("2026-03-28T09:00:00Z")
        .unwrap()
        .timestamp();
    let mut mutation = timed_mutation();
    mutation.timing = EventTiming::Timed {
        start_utc: start,
        end_utc: start + 3600,
        timezone: "Europe/Budapest".to_owned(),
    };
    EventService::new_at(pool, 2000)
        .create_recurring(
            user,
            false,
            calendar,
            mutation,
            "FREQ=DAILY;COUNT=3".to_owned(),
        )
        .await
        .unwrap();
    let uri = format!("/dav/calendars/{principal}/{calendar}/");
    assert_eq!(
        query_resource_hrefs(
            &accounts,
            &uri,
            &password,
            r#"<C:time-range start="20260329T083000Z" end="20260329T084500Z"/>"#
        )
        .await
        .len(),
        1,
        "10:00 local becomes08:00UTC after DST"
    );
    assert!(
        query_resource_hrefs(
            &accounts,
            &uri,
            &password,
            r#"<C:time-range start="20260329T093000Z" end="20260329T094500Z"/>"#
        )
        .await
        .is_empty(),
        "series must not drift one hour in local time"
    );
}

#[tokio::test]
async fn reserved_resource_name_href_round_trips_through_propfind_get_and_multiget() {
    let (_temp, _pool, accounts, _user, password, principal, calendar) = setup_create().await;
    let collection = format!("/dav/calendars/{principal}/{calendar}/");
    let uri = format!("{collection}space%20name%26%3F%23%25.ics");
    let auth = ("owner@example.test", password.as_str());
    assert_eq!(
        put_request(
            build_caldav_router(accounts.clone()),
            &uri,
            Some(auth),
            &vcalendar_body("reserved-resource-uid", "Reserved resource"),
            Some("*")
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let (_, root) = semantic_propfind(
        &accounts,
        &collection,
        &password,
        "1",
        r#"<D:propfind xmlns:D="DAV:"><D:prop><D:getetag/></D:prop></D:propfind>"#,
    )
    .await;
    let responses = root.descendants("DAV:", "response");
    let object = responses
        .iter()
        .find(|n| n.descendants("DAV:", "href")[0].text.ends_with(".ics"))
        .unwrap();
    let href = &object.descendants("DAV:", "href")[0].text;
    let parsed = url::Url::parse("http://127.0.0.1:3000")
        .unwrap()
        .join(href)
        .unwrap();
    assert!(
        parsed.query().is_none() && parsed.fragment().is_none(),
        "resource name delimiters must be encoded in href: {href}"
    );
    let (status, _, content) = request(
        build_caldav_router(accounts.clone()),
        Method::GET,
        href,
        Some(auth),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "advertised href must round-trip to original resource"
    );
    assert!(content.contains("SUMMARY:Reserved resource"));
    let (status, body) = report_request(
        build_caldav_router(accounts),
        &collection,
        auth.0,
        auth.1,
        &multiget_xml(&quick_xml::escape::escape(href)),
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    let root = xml_tree(&body);
    assert_eq!(root.descendants("DAV:", "response").len(), 1);
    assert!(
        root.descendants("urn:ietf:params:xml:ns:caldav", "calendar-data")[0]
            .text
            .contains("SUMMARY:Reserved resource")
    );
}

#[tokio::test]
async fn categories_escaped_comma_and_uri_delimiters_survive_put_get() {
    let (_temp, _pool, accounts, _user, password, principal, calendar) = setup_create().await;
    let uri = format!("/dav/calendars/{principal}/{calendar}/metadata-delimiters.ics");
    let body = vcalendar_with_properties(
        "metadata-delimiter-uid",
        "Metadata",
        Some(r"one\,two,three"),
    )
    .replace("END:VEVENT", "URL:https://example.test/a,b;c\r\nEND:VEVENT");
    assert_eq!(
        put_request(
            build_caldav_router(accounts.clone()),
            &uri,
            Some(("owner@example.test", &password)),
            &body,
            Some("*")
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let (status, _, content) = request(
        build_caldav_router(accounts),
        Method::GET,
        &uri,
        Some(("owner@example.test", &password)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let parsed = parse_calendar(&content, IcsParserLimits::default()).unwrap();
    assert_eq!(parsed.events[0].categories, ["one,two", "three"]);
    assert!(
        content.contains("URL:https://example.test/a,b;c\r\n"),
        "URI-valued property delimiters must remain unescaped: {content}"
    );
}

#[tokio::test]
async fn quoted_parameter_uri_delimiters_do_not_corrupt_property_value() {
    let (_temp, _pool, accounts, _user, password, principal, calendar) = setup_create().await;
    let uri = format!("/dav/calendars/{principal}/{calendar}/quoted-parameter.ics");
    let body = vcalendar_body("quoted-parameter-uid", "Example").replace(
        "SUMMARY:Example",
        "SUMMARY;ALTREP=\"https://example.test/a;b\":Example",
    );
    let (status, _, response) = put_request(
        build_caldav_router(accounts.clone()),
        &uri,
        Some(("owner@example.test", &password)),
        &body,
        Some("*"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "valid quoted ALTREP parameter: {response}"
    );
    let (status, _, content) = request(
        build_caldav_router(accounts),
        Method::GET,
        &uri,
        Some(("owner@example.test", &password)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        parse_calendar(&content, IcsParserLimits::default())
            .unwrap()
            .events[0]
            .summary,
        "Example"
    );
}

#[tokio::test]
async fn imported_categories_and_uri_delimiters_survive_dav_serialization() {
    let (_temp, pool, accounts, user, password, principal, calendar) = setup_create().await;
    let feeds = ExternalFeedService::new_at(pool.clone(), SecretKey::generate(), 2000);
    let feed = feeds
        .create(
            user,
            false,
            calendar,
            NewFeed {
                source_url: "https://feeds.example.test/metadata.ics".to_owned(),
                refresh_interval_seconds: Some(60),
            },
        )
        .await
        .unwrap();
    let body = vcalendar_with_properties(
        "imported-metadata-uid",
        "Imported metadata",
        Some(r"one\,two,three"),
    )
    .replace("END:VEVENT", "URL:https://example.test/a,b;c\r\nEND:VEVENT");
    feeds
        .refresh(user, false, feed.id, &FixedFetcher { body })
        .await
        .unwrap();
    let uri = format!("/dav/calendars/{principal}/{calendar}/");
    let (status, body) = report_request(
        build_caldav_router(accounts.clone()),
        &uri,
        "owner@example.test",
        &password,
        &sync_collection_xml(""),
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    let root = xml_tree(&body);
    let content = &root.descendants("urn:ietf:params:xml:ns:caldav", "calendar-data")[0].text;
    let parsed = parse_calendar(content, IcsParserLimits::default()).unwrap();
    assert_eq!(parsed.events[0].categories, ["one,two", "three"]);
    assert!(content.contains("URL:https://example.test/a,b;c\r\n"));
    let token = extract_sync_token(&body).unwrap();
    sqlx::query("DELETE FROM caldav_event_properties WHERE event_id IN (SELECT event_id FROM external_event_mapping WHERE feed_id=?)").bind(feed.id).execute(&pool).await.unwrap();
    let unchanged = vcalendar_with_properties(
        "imported-metadata-uid",
        "Imported metadata",
        Some(r"one\,two,three"),
    )
    .replace("END:VEVENT", "URL:https://example.test/a,b;c\r\nEND:VEVENT");
    feeds
        .refresh(user, false, feed.id, &FixedFetcher { body: unchanged })
        .await
        .unwrap();
    let (status, delta) = report_request(
        build_caldav_router(accounts),
        &uri,
        "owner@example.test",
        &password,
        &sync_collection_xml(&token),
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    let root = xml_tree(&delta);
    assert_eq!(
        root.descendants("DAV:", "response").len(),
        1,
        "unchanged imported source must restore missing metadata and emit sync change"
    );
    let content = &root.descendants("urn:ietf:params:xml:ns:caldav", "calendar-data")[0].text;
    assert_eq!(
        parse_calendar(content, IcsParserLimits::default())
            .unwrap()
            .events[0]
            .categories,
        ["one,two", "three"]
    );
    assert!(content.contains("URL:https://example.test/a,b;c\r\n"));
}
