use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use commoncal_backend::{
    caldav::{
        auth::CaldavAccountService,
        http::{build_caldav_router, build_connection_management_router},
    },
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    http::{
        AccessLogConfig, Readiness, ResponseSecurityConfig, apply_shared_middleware,
        build_router_with_sessions,
    },
    security::{SecretKey, TokenDomain},
    sessions::{SessionManager, SessionSecurityConfig},
};
use sqlx::SqlitePool;
use tempfile::TempDir;
use tower::ServiceExt;

const NOW: i64 = 1_750_000_000;
const ORIGIN: &str = "https://commoncal.test";

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
         VALUES (?, ?, 'active', ?) RETURNING id",
    )
    .bind(email)
    .bind(email)
    .bind(NOW)
    .fetch_one(pool)
    .await
    .unwrap()
}

fn basic_header(username: &str, password: &str) -> axum::http::HeaderValue {
    let encoded = B64.encode(format!("{username}:{password}"));
    axum::http::HeaderValue::from_str(&format!("Basic {encoded}")).unwrap()
}

fn make_session_manager(pool: &SqlitePool, key: &SecretKey) -> SessionManager {
    SessionManager::new_at(
        pool.clone(),
        key.clone(),
        SessionSecurityConfig::new(300, 60, ORIGIN).unwrap(),
        NOW,
    )
}

/// Build the fully-assembled router the way `main.rs` does: application
/// router + CalDAV DAV router + connection-management router, then apply
/// shared middleware on top.
fn assembled_router(pool: &SqlitePool, key: &SecretKey) -> Router {
    let accounts = CaldavAccountService::new_at(
        pool.clone(),
        key.clone(),
        url::Url::parse("http://127.0.0.1:3000").unwrap(),
        NOW,
    );
    let session_manager = make_session_manager(pool, key);
    let app_router =
        build_router_with_sessions(Readiness::new(), session_manager.clone(), None, None, None);
    let router = app_router
        .merge(build_caldav_router(accounts.clone()))
        .merge(build_connection_management_router(
            accounts,
            session_manager,
        ));
    apply_shared_middleware(
        router,
        AccessLogConfig::new(tracing::level_filters::LevelFilter::DEBUG),
        ResponseSecurityConfig::local_http(),
    )
}

fn extract_request_id(response: &axum::response::Response) -> String {
    response
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .expect("x-request-id header must be present")
        .to_owned()
}

// --- CalDAV PROPFIND /dav/ ---

#[tokio::test]
async fn propfind_dav_root_with_valid_auth_produces_request_id_header() {
    let (_dir, pool) = setup().await;
    let key = SecretKey::generate();
    let user_id = create_user(&pool, "dav@example.test").await;
    let accounts = CaldavAccountService::new_at(
        pool.clone(),
        key.clone(),
        url::Url::parse("http://127.0.0.1:3000").unwrap(),
        NOW,
    );
    let issued = accounts
        .issue_credential(user_id, "Test Phone".into())
        .await
        .unwrap();

    let app = assembled_router(&pool, &key);

    let response = app
        .oneshot(
            Request::builder()
                .method("PROPFIND")
                .uri("/dav/")
                .header(
                    header::AUTHORIZATION,
                    basic_header("dav@example.test", issued.password.expose()),
                )
                .header("depth", "0")
                .body(Body::from(
                    r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:resourcetype/>
    <D:displayname/>
  </D:prop>
</D:propfind>"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    let request_id = extract_request_id(&response);
    assert!(!request_id.is_empty(), "request ID must not be empty");
}

#[tokio::test]
async fn propfind_dav_root_auth_failure_produces_request_id_header() {
    let (_dir, pool) = setup().await;
    let key = SecretKey::generate();
    let _user_id = create_user(&pool, "dav@example.test").await;

    let app = assembled_router(&pool, &key);

    let response = app
        .oneshot(
            Request::builder()
                .method("PROPFIND")
                .uri("/dav/")
                .header(
                    header::AUTHORIZATION,
                    basic_header("dav@example.test", "wrong-password"),
                )
                .header("depth", "0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let request_id = extract_request_id(&response);
    assert!(!request_id.is_empty(), "request ID must not be empty");
}

#[tokio::test]
async fn propfind_dav_root_no_auth_produces_request_id_header() {
    let (_dir, pool) = setup().await;
    let key = SecretKey::generate();
    let _user_id = create_user(&pool, "dav@example.test").await;

    let app = assembled_router(&pool, &key);

    let response = app
        .oneshot(
            Request::builder()
                .method("PROPFIND")
                .uri("/dav/")
                .header("depth", "0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let request_id = extract_request_id(&response);
    assert!(!request_id.is_empty(), "request ID must not be empty");
}

// --- Apple connection management ---

#[tokio::test]
async fn apple_connection_management_unauthenticated_produces_request_id_header() {
    let (_dir, pool) = setup().await;
    let key = SecretKey::generate();
    let _user_id = create_user(&pool, "apple@example.test").await;

    let app = assembled_router(&pool, &key);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/v1/calendar-connections/apple")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let request_id = extract_request_id(&response);
    assert!(!request_id.is_empty(), "request ID must not be empty");
}

#[tokio::test]
async fn apple_connection_management_with_session_produces_request_id_header() {
    let (_dir, pool) = setup().await;
    let key = SecretKey::generate();
    let user_id = create_user(&pool, "apple@example.test").await;

    let token = key.generate_token();
    let hash = key.hash_token(TokenDomain::Session, &token);
    sqlx::query(
        "INSERT INTO sessions (user_id, session_hash, expires_at, revoked_at, created_at, last_seen_at)
         VALUES (?, ?, ?, NULL, ?, ?)",
    )
    .bind(user_id)
    .bind(hash.as_bytes().as_slice())
    .bind(NOW + 1000)
    .bind(NOW)
    .bind(NOW)
    .execute(&pool)
    .await
    .unwrap();

    let app = assembled_router(&pool, &key);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/v1/calendar-connections/apple")
                .header(
                    header::COOKIE,
                    format!("__Host-commoncal_session={}", token.expose()),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let request_id = extract_request_id(&response);
    assert!(!request_id.is_empty(), "request ID must not be empty");
}

// --- No duplicate completion logs ---
//
// The key regression: before the fix, middleware was applied inside
// `build_application_router` and then `main.rs` merged additional routers
// on top. In axum, layers applied before a `merge()` do not cover the
// merged-in routes. After the fix, `apply_shared_middleware` is called
// once on the fully-assembled router, so every route gets exactly one
// pass through the TraceLayer (which emits "finished processing request").
//
// We verify this by checking that the response carries exactly one
// `x-request-id` header (set by `SetRequestIdLayer` + propagated by
// `PropagateRequestIdLayer`). If the middleware were applied twice, we
// would see duplicate or conflicting headers.

#[tokio::test]
async fn normal_api_request_has_single_request_id_header() {
    let (_dir, pool) = setup().await;
    let key = SecretKey::generate();
    let _user_id = create_user(&pool, "api@example.test").await;

    let app = assembled_router(&pool, &key);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let request_id_values: Vec<&str> = response
        .headers()
        .get_all("x-request-id")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    assert_eq!(
        request_id_values.len(),
        1,
        "expected exactly one x-request-id header, got {:?}",
        request_id_values
    );
    assert!(!request_id_values[0].is_empty());
}

#[tokio::test]
async fn caldav_request_has_single_request_id_header() {
    let (_dir, pool) = setup().await;
    let key = SecretKey::generate();
    let user_id = create_user(&pool, "dav@example.test").await;
    let accounts = CaldavAccountService::new_at(
        pool.clone(),
        key.clone(),
        url::Url::parse("http://127.0.0.1:3000").unwrap(),
        NOW,
    );
    let issued = accounts
        .issue_credential(user_id, "Test".into())
        .await
        .unwrap();

    let app = assembled_router(&pool, &key);

    let response = app
        .oneshot(
            Request::builder()
                .method("PROPFIND")
                .uri("/dav/")
                .header(
                    header::AUTHORIZATION,
                    basic_header("dav@example.test", issued.password.expose()),
                )
                .header("depth", "0")
                .body(Body::from(
                    r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:resourcetype/>
  </D:prop>
</D:propfind>"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    let request_id_values: Vec<&str> = response
        .headers()
        .get_all("x-request-id")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    assert_eq!(
        request_id_values.len(),
        1,
        "expected exactly one x-request-id header for CalDAV request, got {:?}",
        request_id_values
    );
    assert!(!request_id_values[0].is_empty());
}

// --- Security headers present on CalDAV and Apple routes ---

#[tokio::test]
async fn caldav_response_includes_security_headers() {
    let (_dir, pool) = setup().await;
    let key = SecretKey::generate();
    let _user_id = create_user(&pool, "dav@example.test").await;

    let app = assembled_router(&pool, &key);

    let response = app
        .oneshot(
            Request::builder()
                .method("PROPFIND")
                .uri("/dav/")
                .header("depth", "0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response
            .headers()
            .get("x-content-type-options")
            .and_then(|v| v.to_str().ok()),
        Some("nosniff")
    );
    assert_eq!(
        response
            .headers()
            .get("referrer-policy")
            .and_then(|v| v.to_str().ok()),
        Some("strict-origin-when-cross-origin")
    );
}

#[tokio::test]
async fn apple_connection_response_includes_security_headers() {
    let (_dir, pool) = setup().await;
    let key = SecretKey::generate();
    let _user_id = create_user(&pool, "apple@example.test").await;

    let app = assembled_router(&pool, &key);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/v1/calendar-connections/apple")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response
            .headers()
            .get("x-content-type-options")
            .and_then(|v| v.to_str().ok()),
        Some("nosniff")
    );
    assert_eq!(
        response
            .headers()
            .get("referrer-policy")
            .and_then(|v| v.to_str().ok()),
        Some("strict-origin-when-cross-origin")
    );
}

#[tokio::test]
async fn well_known_propfind_preserves_discovery_through_shared_middleware() {
    let (_dir, pool) = setup().await;
    let key = SecretKey::derive(b"discovery-test-secret");
    let user_id = create_user(&pool, "discovery@example.test").await;
    for name in ["First", "Second"] {
        let calendar_id: i64 = sqlx::query_scalar("INSERT INTO calendars (owner_user_id, name, color, default_timezone, default_event_visibility, archived, version, created_at, updated_at) VALUES (?, ?, '#112233', 'UTC', 'default', 0, 1, ?, ?) RETURNING id")
            .bind(user_id).bind(name).bind(NOW).bind(NOW).fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO calendar_acl (calendar_id, user_id, role, created_at, updated_at) VALUES (?, ?, 'owner', ?, ?)")
            .bind(calendar_id).bind(user_id).bind(NOW).bind(NOW).execute(&pool).await.unwrap();
    }
    let accounts = CaldavAccountService::new_at(
        pool.clone(),
        key.clone(),
        url::Url::parse("http://127.0.0.1:3000").unwrap(),
        NOW,
    );
    let issued = accounts
        .issue_credential(user_id, "Discovery".into())
        .await
        .unwrap();
    let auth = basic_header(&issued.username, issued.password.expose());
    let app = assembled_router(&pool, &key);
    let body = r#"<D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><D:prop><D:current-user-principal/><D:resourcetype/><C:calendar-home-set/></D:prop></D:propfind>"#;
    let discovery = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PROPFIND")
                .uri("/.well-known/caldav")
                .header("depth", "0")
                .header(header::AUTHORIZATION, auth.clone())
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(discovery.status(), StatusCode::TEMPORARY_REDIRECT);
    let target = url::Url::parse(discovery.headers()[header::LOCATION].to_str().unwrap()).unwrap();
    assert_eq!(
        target.origin(),
        url::Url::parse("http://127.0.0.1:3000").unwrap().origin()
    );
    assert_eq!(target.path(), "/dav/");
    assert_eq!(discovery.headers()[header::CACHE_CONTROL], "no-cache");
    let get = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/.well-known/caldav")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get.status(), StatusCode::TEMPORARY_REDIRECT);
    let unauthenticated = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PROPFIND")
                .uri(target.path())
                .header("depth", "0")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    let mut path = target.path().to_owned();
    for stage in 0..3 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PROPFIND")
                    .uri(&path)
                    .header("depth", if stage == 2 { "1" } else { "0" })
                    .header(header::AUTHORIZATION, auth.clone())
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::MULTI_STATUS);
        let bytes = http_body_util::BodyExt::collect(response.into_body())
            .await
            .unwrap()
            .to_bytes();
        let mut reader = quick_xml::NsReader::from_reader(bytes.as_ref());
        let mut property = String::new();
        let mut inside_href = false;
        let mut inside_status = false;
        let mut group_next = None;
        let mut group_succeeded = false;
        let mut next = None;
        let mut calendars = 0;
        let mut successful_groups = 0;
        let mut multistatus = false;
        loop {
            let (namespace, event) = reader.read_resolved_event().unwrap();
            match event {
                quick_xml::events::Event::Start(ref e) | quick_xml::events::Event::Empty(ref e) => {
                    let local = String::from_utf8(e.local_name().as_ref().to_vec()).unwrap();
                    if ["multistatus", "current-user-principal", "href", "status"]
                        .contains(&local.as_str())
                    {
                        assert_eq!(
                            namespace,
                            quick_xml::name::ResolveResult::Bound(quick_xml::name::Namespace(
                                b"DAV:"
                            ))
                        );
                    }
                    if local == "multistatus" {
                        multistatus = true;
                    }
                    if local == "calendar-home-set" {
                        assert_eq!(
                            namespace,
                            quick_xml::name::ResolveResult::Bound(quick_xml::name::Namespace(
                                b"urn:ietf:params:xml:ns:caldav"
                            ))
                        );
                    }
                    if local == "propstat" {
                        group_succeeded = false;
                        group_next = None;
                    }
                    if ["current-user-principal", "calendar-home-set"].contains(&local.as_str()) {
                        property = local.clone();
                    }
                    inside_href = local == "href";
                    inside_status = local == "status";
                    if local == "calendar" {
                        assert_eq!(
                            namespace,
                            quick_xml::name::ResolveResult::Bound(quick_xml::name::Namespace(
                                b"urn:ietf:params:xml:ns:caldav"
                            ))
                        );
                        calendars += 1;
                    }
                }
                quick_xml::events::Event::Text(e)
                    if inside_href
                        && ((stage == 0 && property == "current-user-principal")
                            || (stage == 1 && property == "calendar-home-set")) =>
                {
                    group_next = Some(e.unescape().unwrap().into_owned());
                }
                quick_xml::events::Event::Text(e) if inside_status => {
                    let status = e.unescape().unwrap();
                    assert!(matches!(
                        status.as_ref(),
                        "HTTP/1.1 200 OK" | "HTTP/1.1 404 Not Found"
                    ));
                    group_succeeded = status == "HTTP/1.1 200 OK";
                }
                quick_xml::events::Event::End(e) => {
                    if e.local_name().as_ref() == b"propstat" && group_succeeded {
                        successful_groups += 1;
                        if group_next.is_some() {
                            next = group_next.take();
                        }
                    }
                    inside_href = false;
                    inside_status = false;
                    if e.local_name().as_ref() == property.as_bytes() {
                        property.clear();
                    }
                }
                quick_xml::events::Event::Eof => break,
                _ => {}
            }
        }
        assert!(multistatus);
        assert!(successful_groups > 0);
        if stage < 2 {
            let next = url::Url::parse(&next.expect("required discovery property href")).unwrap();
            assert_eq!(next.origin(), target.origin());
            path = next.path().to_owned();
        } else {
            assert_eq!(calendars, 2);
        }
    }
}
