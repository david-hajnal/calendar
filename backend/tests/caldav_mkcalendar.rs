use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use commoncal_backend::{
    caldav::{auth::CaldavAccountService, http::build_caldav_router},
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    http::Readiness,
    security::SecretKey,
};
use http_body_util::BodyExt;
use tower::ServiceExt;

#[tokio::test]
async fn mkcalendar_rejections_are_not_cacheable() {
    let (_dir, app, principal, auth, _, _) = setup().await;
    for (key, body, status) in [
        ("malformed", "<broken>", StatusCode::BAD_REQUEST),
        ("999", "", StatusCode::CONFLICT),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("MKCALENDAR")
                    .uri(format!("/dav/calendars/{principal}/{key}/"))
                    .header("Authorization", &auth)
                    .header("Content-Type", "application/xml")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(
            response
                .headers()
                .get("Cache-Control")
                .and_then(|v| v.to_str().ok()),
            Some("no-cache"),
            "MKCALENDAR rejection for {key}"
        );
    }
}

async fn setup() -> (
    tempfile::TempDir,
    Router,
    String,
    String,
    sqlx::SqlitePool,
    CaldavAccountService,
) {
    let dir = tempfile::TempDir::new().unwrap();
    let config = AppConfig::with_database_path(
        Environment::Development,
        "127.0.0.1:3000",
        None,
        dir.path().join("db.sqlite"),
    )
    .unwrap();
    let pool = connect_and_migrate(&config, Readiness::new())
        .await
        .unwrap();
    let user: i64 = sqlx::query_scalar("INSERT INTO users(normalized_email,status,created_at) VALUES ('mk@example.test','registered',100) RETURNING id").fetch_one(&pool).await.unwrap();
    let accounts = CaldavAccountService::new_at(
        pool.clone(),
        SecretKey::generate(),
        url::Url::parse("http://localhost:3000").unwrap(),
        1000,
    );
    let credential = accounts
        .issue_credential(user, "Apple".into())
        .await
        .unwrap();
    let principal = accounts.status(user).await.unwrap().principal_id.unwrap();
    let auth = format!(
        "Basic {}",
        STANDARD.encode(format!(
            "{}:{}",
            credential.username,
            credential.password.expose()
        ))
    );
    (
        dir,
        build_caldav_router(accounts.clone()),
        principal,
        auth,
        pool,
        accounts,
    )
}
async fn send(
    app: &Router,
    method: &str,
    path: &str,
    auth: Option<&str>,
    body: &str,
) -> (StatusCode, String) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("Depth", "1")
        .header("Content-Type", "application/xml");
    if let Some(auth) = auth {
        req = req.header("Authorization", auth);
    }
    if method == "PUT" {
        req = req.header("If-None-Match", "*");
    }
    if method == "DELETE" {
        let mut get = Request::builder().method("GET").uri(path);
        if let Some(auth) = auth {
            get = get.header("Authorization", auth);
        }
        let response = app
            .clone()
            .oneshot(get.body(Body::empty()).unwrap())
            .await
            .unwrap();
        req = req.header("If-Match", response.headers().get("ETag").unwrap());
    }

    let response = app
        .clone()
        .oneshot(req.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}
fn creation(props: &str) -> String {
    format!(
        r#"<C:mkcalendar xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:D="DAV:" xmlns:A="http://apple.com/ns/ical/"><D:set><D:prop>{props}</D:prop></D:set></C:mkcalendar>"#
    )
}
#[tokio::test]
async fn creates_and_discovers_at_requested_url() {
    let (_dir, app, principal, auth, _pool, _) = setup().await;
    let path = format!("/dav/calendars/{principal}/AABB-1234/");
    let body = creation(
        "<D:displayname>Apple &amp; Work</D:displayname><A:calendar-color>#123456FF</A:calendar-color>",
    );
    assert_eq!(
        send(&app, "MKCALENDAR", &path, Some(&auth), &body).await.0,
        StatusCode::CREATED
    );
    let (status, discovery) = send(
        &app,
        "PROPFIND",
        &format!("/dav/calendars/{principal}/"),
        Some(&auth),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert!(discovery.contains(&path), "{discovery}");
    assert!(discovery.contains("Apple &amp; Work"));
    assert!(discovery.contains("#123456FF"));
    assert_eq!(
        send(&app, "MKCALENDAR", &path, Some(&auth), "").await.0,
        StatusCode::METHOD_NOT_ALLOWED
    );
}
#[tokio::test]
async fn rejects_invalid_creation_without_leaving_collection() {
    let (_dir, app, principal, auth, _pool, _) = setup().await;
    let path = format!("/dav/calendars/{principal}/invalid/");
    assert_eq!(
        send(&app, "MKCALENDAR", &path, None, "").await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &app,
            "MKCALENDAR",
            "/dav/calendars/other/invalid/",
            Some(&auth),
            ""
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&app, "MKCALENDAR", &path, Some(&auth), "<broken>")
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let (status, body) = send(
        &app,
        "MKCALENDAR",
        &path,
        Some(&auth),
        &creation("<D:unknown/>"),
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert!(body.contains("mkcalendar-response"));
    assert_eq!(
        send(&app, "PROPFIND", &path, Some(&auth), "").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&app, "MKCALENDAR", &path, Some(&auth), "").await.0,
        StatusCode::CREATED
    );
}
const TZ: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\nBEGIN:VTIMEZONE\r\nTZID:Europe/Budapest\r\nBEGIN:STANDARD\r\nDTSTART:19701025T030000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nEND:VCALENDAR\r\n";
const ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\nBEGIN:VEVENT\r\nUID:mk-event\r\nDTSTAMP:20261014T100000Z\r\nDTSTART:20261015T100000Z\r\nDTEND:20261015T110000Z\r\nSUMMARY:Apple event\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
#[tokio::test]
async fn timezone_event_crud_reports_sync_and_numeric_compatibility() {
    let (_dir, app, p, auth, _pool, _) = setup().await;
    let path = format!("/dav/calendars/{p}/client-uuid/");
    let props = format!(
        "<D:displayname>Timezone</D:displayname><C:calendar-timezone><![CDATA[{TZ}]]></C:calendar-timezone><C:supported-calendar-component-set><C:comp name=\"VEVENT\"/></C:supported-calendar-component-set>"
    );
    assert_eq!(
        send(&app, "MKCALENDAR", &path, Some(&auth), &creation(&props))
            .await
            .0,
        StatusCode::CREATED
    );
    let (_, body) = send(&app, "PROPFIND", &path, Some(&auth), "").await;
    assert!(body.contains("TZID:Europe/Budapest"), "{body}");
    let event = format!("{path}event.ics");
    assert_eq!(
        send(&app, "PUT", &event, Some(&auth), ICS).await.0,
        StatusCode::CREATED
    );
    assert!(
        send(&app, "GET", &event, Some(&auth), "")
            .await
            .1
            .contains("SUMMARY:Apple event")
    );
    let query = r#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:caldav"><C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT"/></C:comp-filter></C:filter></C:calendar-query>"#;
    let (status, body) = send(&app, "REPORT", &path, Some(&auth), query).await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert!(body.contains(&event), "{body}");
    let multiget = format!(
        r#"<C:calendar-multiget xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:D="DAV:"><D:href>{event}</D:href></C:calendar-multiget>"#
    );
    let (status, body) = send(&app, "REPORT", &path, Some(&auth), &multiget).await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert!(body.contains("200 OK"), "{body}");
    let sync = r#"<D:sync-collection xmlns:D="DAV:"><D:sync-token/><D:sync-level>1</D:sync-level><D:prop><D:getetag/></D:prop></D:sync-collection>"#;
    let (status, body) = send(&app, "REPORT", &path, Some(&auth), sync).await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert!(body.contains(&event));
    let token = body
        .split("<D:sync-token>")
        .nth(1)
        .unwrap()
        .split("</D:sync-token>")
        .next()
        .unwrap();
    assert_eq!(
        send(&app, "DELETE", &event, Some(&auth), "").await.0,
        StatusCode::NO_CONTENT
    );
    let incremental = sync.replace(
        "<D:sync-token/>",
        &format!("<D:sync-token>{token}</D:sync-token>"),
    );
    let (status, body) = send(&app, "REPORT", &path, Some(&auth), &incremental).await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert!(body.contains(&event));
    assert!(body.contains("404 Not Found"));
    assert_eq!(
        send(
            &app,
            "PROPFIND",
            &format!("/dav/calendars/{p}/1/"),
            Some(&auth),
            ""
        )
        .await
        .0,
        StatusCode::MULTI_STATUS
    );
    assert_eq!(
        send(
            &app,
            "MKCALENDAR",
            &format!("/dav/calendars/{p}/1/"),
            Some(&auth),
            ""
        )
        .await
        .0,
        StatusCode::METHOD_NOT_ALLOWED
    );
}
#[tokio::test]
async fn rejects_bad_paths_and_required_properties_atomically() {
    let (_dir, app, p, auth, _pool, _) = setup().await;
    for key in [
        "999",
        "-123",
        "%41lias",
        "bad%2Fkey",
        "bad%5Ckey",
        "bad%20key",
    ] {
        let path = format!("/dav/calendars/{p}/{key}/");
        assert_eq!(
            send(&app, "MKCALENDAR", &path, Some(&auth), "").await.0,
            StatusCode::CONFLICT,
            "{key}"
        );
    }
    for props in [
        "<D:displayname/>",
        "<C:calendar-timezone>UTC</C:calendar-timezone>",
        "<C:supported-calendar-component-set><C:comp name=\"VTODO\"/></C:supported-calendar-component-set>",
        "<D:resourcetype/>",
    ] {
        let path = format!("/dav/calendars/{p}/retry/");
        assert_eq!(
            send(&app, "MKCALENDAR", &path, Some(&auth), &creation(props))
                .await
                .0,
            StatusCode::MULTI_STATUS,
            "{props}"
        );
        assert_eq!(
            send(&app, "PROPFIND", &path, Some(&auth), "").await.0,
            StatusCode::NOT_FOUND
        );
    }
    let (_, body) = send(
        &app,
        "PROPFIND",
        &format!("/dav/calendars/{p}/"),
        Some(&auth),
        "",
    )
    .await;
    assert!(!body.contains("<C:calendar/>"), "{body}");
    assert_eq!(
        send(
            &app,
            "MKCALENDAR",
            &format!("/dav/calendars/{p}/retry/"),
            Some(&auth),
            ""
        )
        .await
        .0,
        StatusCode::CREATED
    );
}
#[tokio::test]
async fn database_failure_rolls_back_calendar_acl_and_identity() {
    let (_dir, app, p, auth, pool, _accounts) = setup().await;
    // Deliberately fail after calendar and owner ACL insertion to exercise rollback.
    sqlx::query("CREATE TRIGGER fail_dav_metadata BEFORE UPDATE OF dav_collection_key ON calendars BEGIN SELECT RAISE(ABORT, 'injected failure'); END").execute(&pool).await.unwrap();
    let path = format!("/dav/calendars/{p}/rollback/");
    assert_eq!(
        send(&app, "MKCALENDAR", &path, Some(&auth), "").await.0,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    let (_, body) = send(
        &app,
        "PROPFIND",
        &format!("/dav/calendars/{p}/"),
        Some(&auth),
        "",
    )
    .await;
    assert!(!body.contains("<C:calendar/>"));
    sqlx::query("DROP TRIGGER fail_dav_metadata")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        send(&app, "MKCALENDAR", &path, Some(&auth), "").await.0,
        StatusCode::CREATED
    );
}
#[tokio::test]
async fn principal_scoped_keys_and_existing_numeric_calendar_stay_isolated() {
    let (_dir, app, p, auth, pool, accounts) = setup().await;
    let user: i64 = sqlx::query_scalar("INSERT INTO users(normalized_email,status,created_at) VALUES ('other@example.test','registered',100) RETURNING id").fetch_one(&pool).await.unwrap();
    let c = accounts
        .issue_credential(user, "Other".into())
        .await
        .unwrap();
    let other = accounts.status(user).await.unwrap().principal_id.unwrap();
    let other_auth = format!(
        "Basic {}",
        STANDARD.encode(format!("{}:{}", c.username, c.password.expose()))
    );
    let path = format!("/dav/calendars/{p}/same-key/");
    let other_path = format!("/dav/calendars/{other}/same-key/");
    assert_eq!(
        send(
            &app,
            "MKCALENDAR",
            &path,
            Some(&auth),
            &creation("<D:displayname>First</D:displayname>")
        )
        .await
        .0,
        StatusCode::CREATED
    );
    assert_eq!(
        send(&app, "MKCALENDAR", &path, Some(&other_auth), "")
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &app,
            "MKCALENDAR",
            &other_path,
            Some(&other_auth),
            &creation("<D:displayname>Second</D:displayname>")
        )
        .await
        .0,
        StatusCode::CREATED
    );
    assert_eq!(
        send(&app, "PROPFIND", &other_path, Some(&auth), "").await.0,
        StatusCode::NOT_FOUND
    );
    let event = format!("{path}event.ics");
    assert_eq!(
        send(&app, "PUT", &event, Some(&auth), ICS).await.0,
        StatusCode::CREATED
    );
    assert_eq!(
        send(
            &app,
            "GET",
            &format!("{other_path}event.ics"),
            Some(&other_auth),
            ""
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let numeric = commoncal_backend::calendar::CalendarService::new_at(pool.clone(), 1000)
        .create(
            user,
            commoncal_backend::calendar::NewCalendar {
                name: "Legacy".into(),
                description: None,
                color: "#123456".into(),
                default_timezone: "UTC".into(),
                default_event_visibility: "default".into(),
                default_notification_rules_json: None,
                created_at: 1000,
            },
        )
        .await
        .unwrap()
        .id;
    let numeric_path = format!("/dav/calendars/{other}/{numeric}/");
    assert_eq!(
        send(
            &app,
            "PUT",
            &format!("{numeric_path}old.ics"),
            Some(&other_auth),
            ICS
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let (_, body) = send(
        &app,
        "PROPFIND",
        &format!("/dav/calendars/{other}/"),
        Some(&other_auth),
        "",
    )
    .await;
    assert!(body.contains(&numeric_path));
    assert!(body.contains(&other_path));
    assert!(!body.contains("First"));
    // Ownership changes must not move collection identities into another
    // principal's namespace or collide with that principal's existing key.
    let creator = accounts
        .resolve_principal(&p)
        .await
        .unwrap()
        .unwrap()
        .user_id;
    commoncal_backend::calendar::CalendarService::new_at(pool.clone(), 1000)
        .transfer_ownership(creator, false, 1, user, 1)
        .await
        .unwrap();
    assert!(
        send(&app, "PROPFIND", &path, Some(&auth), "")
            .await
            .1
            .contains("First")
    );
    assert!(
        send(&app, "GET", &event, Some(&auth), "")
            .await
            .1
            .contains("Apple event")
    );
    let (_, body) = send(
        &app,
        "PROPFIND",
        &format!("/dav/calendars/{other}/"),
        Some(&other_auth),
        "",
    )
    .await;
    assert!(body.contains(&other_path));
    assert!(body.contains(&format!("/dav/calendars/{other}/1/")));
}
#[tokio::test]
async fn concurrent_creation_has_one_winner_and_no_extra_calendar() {
    let (_dir, app, p, auth, _pool, _) = setup().await;
    let path = format!("/dav/calendars/{p}/race/");
    let (a, b) = tokio::join!(
        send(&app, "MKCALENDAR", &path, Some(&auth), ""),
        send(&app, "MKCALENDAR", &path, Some(&auth), "")
    );
    let mut statuses = [a.0.as_u16(), b.0.as_u16()];
    statuses.sort();
    assert_eq!(statuses, [201, 405]);
    let (_, body) = send(
        &app,
        "PROPFIND",
        &format!("/dav/calendars/{p}/"),
        Some(&auth),
        "",
    )
    .await;
    assert_eq!(body.matches("<C:calendar/>").count(), 1);
}
#[tokio::test]
async fn options_and_allow_match_collection_creation_permissions() {
    let (_dir, app, p, auth, _, _) = setup().await;
    for (key, expected) in [
        ("new-collection", "MKCALENDAR, OPTIONS"),
        ("bad%20key", "OPTIONS"),
    ] {
        let path = format!("/dav/calendars/{p}/{key}/");
        for method in ["OPTIONS", "POST"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(&path)
                        .header("Authorization", &auth)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.headers().get("Allow").unwrap(),
                expected,
                "{method} {key}"
            );
        }
    }
    let path = format!("/dav/calendars/{p}/new-collection/");
    assert_eq!(
        send(&app, "MKCALENDAR", &path, Some(&auth), "").await.0,
        StatusCode::CREATED
    );
    let (status, body) = send(&app, "MKCALENDAR", &path, Some(&auth), "").await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert!(body.contains("resource-must-be-null"));
    let (_, body) = send(
        &app,
        "PROPFIND",
        &format!("/dav/calendars/{p}/"),
        Some(&auth),
        "",
    )
    .await;
    assert!(body.contains("<D:bind/>"));
}
#[tokio::test]
async fn invalid_timezone_and_unknown_properties_report_atomic_failures() {
    let (_dir, app, p, auth, _, _) = setup().await;
    let path = format!("/dav/calendars/{p}/properties/");
    for timezone in [
        format!("{TZ}BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n"),
        TZ.replace(
            "TZID:Europe/Budapest",
            "TZID:Europe/Budapest\r\n???bad:property",
        ),
        TZ.replace("TZOFFSETTO:+0100", "TZOFFSETTO:+9999"),
    ] {
        let body = creation(&format!(
            "<D:displayname>Rollback</D:displayname><C:calendar-timezone><![CDATA[{timezone}]]></C:calendar-timezone>"
        ));
        let (status, body) = send(&app, "MKCALENDAR", &path, Some(&auth), &body).await;
        assert_eq!(status, StatusCode::MULTI_STATUS, "{body}");
        assert!(body.contains("409 Conflict"), "{body}");
        assert!(body.contains("valid-calendar-data"), "{body}");
        assert!(body.contains("424 Failed Dependency"), "{body}");
        assert_eq!(
            send(&app, "PROPFIND", &path, Some(&auth), "").await.0,
            StatusCode::NOT_FOUND
        );
    }
    let (status, body) = send(
        &app,
        "MKCALENDAR",
        &path,
        Some(&auth),
        &creation("<D:displayname>Rollback</D:displayname><D:unsupported/>"),
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert!(body.contains("403 Forbidden"));
    assert!(body.contains("424 Failed Dependency"));
}
#[tokio::test]
async fn unknown_property_without_namespace_has_well_formed_error_response() {
    let (_dir, app, p, auth, _, _) = setup().await;
    let (status, body) = send(
        &app,
        "MKCALENDAR",
        &format!("/dav/calendars/{p}/unknown/"),
        Some(&auth),
        &creation("<unknown/>"),
    )
    .await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    let mut reader = quick_xml::NsReader::from_str(&body);
    loop {
        let (namespace, event) = reader.read_resolved_event().unwrap();
        assert!(
            !matches!(namespace, quick_xml::name::ResolveResult::Unknown(_)),
            "{body}"
        );
        if matches!(event, quick_xml::events::Event::Eof) {
            break;
        }
    }
}
