use axum::{
    Extension, Json, Router,
    extract::{Path, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{any, delete, get, post},
};
use serde::{Deserialize, Serialize};

use crate::{
    caldav::{
        MAX_LABEL_LENGTH,
        auth::CaldavAccountService,
        types::{CaldavAuthError, CredentialMetadata, DavSession, PrincipalInfo},
    },
    http::{ApiError, authenticated_session},
    sessions::{AuthenticatedSession, SessionManager},
};

const DAV_REALM: &str = "commoncal-dav";
const PROPFIND: &str = "PROPFIND";
const OPTIONS: &str = "OPTIONS";
const DAV: header::HeaderName = header::HeaderName::from_static("dav");
const DAV_CAPABILITIES: &str = "1, 2, access-control, calendar-access";
const DAV_ALLOW: &str = "PROPFIND, OPTIONS";

pub fn build_caldav_router(accounts: CaldavAccountService) -> Router {
    Router::new()
        .route("/.well-known/caldav", get(well_known_caldav))
        .route("/dav/", any(dav_root))
        .route("/dav/principals/:principal_id/", any(dav_principal))
        .route("/dav/calendars/:principal_id/", any(dav_calendar_home))
        .with_state(accounts)
}

pub fn build_connection_management_router(
    accounts: CaldavAccountService,
    session_manager: SessionManager,
) -> Router {
    Router::new()
        .route(
            "/api/v1/calendar-connections/apple",
            get(get_apple_connection).delete(disconnect_apple),
        )
        .route(
            "/api/v1/calendar-connections/apple/passwords",
            post(create_apple_password),
        )
        .route(
            "/api/v1/calendar-connections/apple/passwords/:id",
            delete(revoke_apple_password),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            session_manager,
            authenticated_session,
        ))
        .with_state(accounts)
}

async fn well_known_caldav(State(accounts): State<CaldavAccountService>) -> Response {
    (
        StatusCode::MOVED_PERMANENTLY,
        [(header::LOCATION, accounts.dav_root_url())],
    )
        .into_response()
}

async fn dav_root(State(accounts): State<CaldavAccountService>, request: Request) -> Response {
    match request.method().as_str() {
        OPTIONS => dav_capabilities(),
        PROPFIND => {
            let Some(authorization) = request.headers().get(header::AUTHORIZATION) else {
                return dav_unauthorized();
            };
            match accounts.authenticate(authorization).await {
                Ok(session) => render_propfind(&accounts, &session),
                Err(_) => dav_unauthorized(),
            }
        }
        _ => method_not_allowed(),
    }
}

async fn dav_principal(
    State(accounts): State<CaldavAccountService>,
    Path(principal_id): Path<String>,
    request: Request,
) -> Response {
    match request.method().as_str() {
        OPTIONS => dav_capabilities(),
        PROPFIND => {
            let Some(authorization) = request.headers().get(header::AUTHORIZATION) else {
                return dav_unauthorized();
            };
            let Ok(session) = accounts.authenticate(authorization).await else {
                return dav_unauthorized();
            };
            if depth_is_infinite(request.headers()) {
                return (StatusCode::BAD_REQUEST, "Depth: infinity is not supported")
                    .into_response();
            }
            let principal = match accounts.resolve_principal(&principal_id).await {
                Ok(Some(principal)) => principal,
                Ok(None) => return dav_not_found(),
                Err(_) => return dav_server_error(),
            };
            if principal.user_id != session.user_id {
                return dav_not_found();
            }
            render_principal(&accounts, &principal_id, &principal)
        }
        _ => method_not_allowed(),
    }
}

async fn dav_calendar_home(request: Request) -> Response {
    match request.method().as_str() {
        OPTIONS => dav_capabilities(),
        _ => method_not_allowed(),
    }
}

fn dav_capabilities() -> Response {
    (
        StatusCode::OK,
        [(DAV, DAV_CAPABILITIES), (header::ALLOW, DAV_ALLOW)],
    )
        .into_response()
}

fn method_not_allowed() -> Response {
    (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, DAV_ALLOW)]).into_response()
}

fn dav_not_found() -> Response {
    (StatusCode::NOT_FOUND, "principal not found").into_response()
}

fn dav_server_error() -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
}

fn depth_is_infinite(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get("depth")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("infinity"))
}

fn dav_unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(
            header::WWW_AUTHENTICATE,
            format!("Basic realm=\"{DAV_REALM}\""),
        )],
    )
        .into_response()
}

fn render_propfind(accounts: &CaldavAccountService, session: &DavSession) -> Response {
    let principal_url = accounts.principal_url(&session.principal_id);
    let body = format!(
        r#"<?xml version="1.0" encoding="utf-8" ?>
<D:multistatus xmlns:D="DAV:">
  <D:response>
    <D:href>/dav/</D:href>
    <D:propstat>
      <D:prop>
        <D:current-principal>
          <D:href>{principal_url}</D:href>
        </D:current-principal>
      </D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>
</D:multistatus>"#
    );
    (
        StatusCode::MULTI_STATUS,
        [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
        body,
    )
        .into_response()
}

fn render_principal(
    accounts: &CaldavAccountService,
    principal_id: &str,
    principal: &PrincipalInfo,
) -> Response {
    let principal_url = accounts.principal_url(principal_id);
    let calendar_home = accounts.calendar_home_url(principal_id);
    let display_name = xml_escape(&principal.display_name);
    let body = format!(
        r#"<?xml version="1.0" encoding="utf-8" ?>
<D:multistatus xmlns:D="DAV:">
  <D:response>
    <D:href>/dav/principals/{principal_id}/</D:href>
    <D:propstat>
      <D:prop>
        <D:principal-URL>
          <D:href>{principal_url}</D:href>
        </D:principal-URL>
        <D:calendar-home-set>
          <D:href>{calendar_home}</D:href>
        </D:calendar-home-set>
        <D:displayname>{display_name}</D:displayname>
        <D:supported-report-set>
          <D:report>
            <D:name>calendar-query</D:name>
          </D:report>
          <D:report>
            <D:name>calendar-multiget</D:name>
          </D:report>
          <D:report>
            <D:name>sync-collection</D:name>
          </D:report>
        </D:supported-report-set>
      </D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>
</D:multistatus>"#
    );
    (
        StatusCode::MULTI_STATUS,
        [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
        body,
    )
        .into_response()
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[derive(Deserialize)]
struct CreatePasswordRequest {
    label: String,
}

#[derive(Serialize)]
struct CreatePasswordResponse {
    server_url: String,
    username: String,
    clear_password: String,
    password: CredentialMetadata,
}

async fn get_apple_connection(
    State(accounts): State<CaldavAccountService>,
    Extension(session): Extension<AuthenticatedSession>,
) -> Result<impl IntoResponse, ApiError> {
    let status = accounts
        .status(session.user.id)
        .await
        .map_err(map_caldav_error)?;
    Ok(Json(status))
}

async fn create_apple_password(
    State(accounts): State<CaldavAccountService>,
    Extension(session): Extension<AuthenticatedSession>,
    Json(request): Json<CreatePasswordRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let label = request.label.trim().to_owned();
    if label.is_empty() || label.chars().count() > MAX_LABEL_LENGTH {
        return Err(ApiError::bad_request());
    }
    let issued = accounts
        .issue_credential(session.user.id, label)
        .await
        .map_err(map_caldav_error)?;
    let response = CreatePasswordResponse {
        server_url: issued.server_url,
        username: issued.username,
        clear_password: issued.password.expose().to_owned(),
        password: issued.metadata,
    };
    Ok((StatusCode::CREATED, Json(response)))
}

async fn revoke_apple_password(
    State(accounts): State<CaldavAccountService>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(credential_id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    accounts
        .revoke_credential(session.user.id, credential_id)
        .await
        .map_err(|error| match error {
            CaldavAuthError::InvalidCredentials | CaldavAuthError::Revoked => ApiError::not_found(),
            other => map_caldav_error(other),
        })?;
    Ok(StatusCode::NO_CONTENT)
}

async fn disconnect_apple(
    State(accounts): State<CaldavAccountService>,
    Extension(session): Extension<AuthenticatedSession>,
) -> Result<impl IntoResponse, ApiError> {
    accounts
        .revoke_all(session.user.id)
        .await
        .map_err(map_caldav_error)?;
    Ok(StatusCode::NO_CONTENT)
}

fn map_caldav_error(error: CaldavAuthError) -> ApiError {
    match error {
        CaldavAuthError::InvalidCredentials | CaldavAuthError::Revoked => ApiError::bad_request(),
        CaldavAuthError::RateLimited => ApiError::rate_limited(),
        CaldavAuthError::Persistence => ApiError::internal(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::SecretKey;
    use axum::body::Body;
    use axum::http::{HeaderValue, Method, Request};
    use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
    use http_body_util::BodyExt;
    use sqlx::SqlitePool;
    use tempfile::NamedTempFile;
    use tower::ServiceExt;

    struct TestDb {
        _file: NamedTempFile,
        pool: SqlitePool,
    }

    impl TestDb {
        async fn new() -> Self {
            let file = NamedTempFile::new().unwrap();
            let conn_str = format!("sqlite:{}", file.path().to_str().unwrap());
            let pool = SqlitePool::connect(&conn_str).await.unwrap();
            sqlx::query(
                "CREATE TABLE users (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    normalized_email TEXT NOT NULL UNIQUE COLLATE NOCASE,
                    display_name TEXT,
                    status TEXT NOT NULL CHECK (status IN ('invited', 'active', 'suspended', 'deleted')),
                    created_at INTEGER NOT NULL
                )",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "CREATE TABLE caldav_accounts (
                    user_id INTEGER PRIMARY KEY REFERENCES users(id),
                    principal_id TEXT NOT NULL UNIQUE CHECK (length(principal_id) > 0),
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                )",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "CREATE TABLE caldav_credentials (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    user_id INTEGER NOT NULL REFERENCES users(id),
                    label TEXT NOT NULL CHECK (length(trim(label)) > 0),
                    token_prefix TEXT NOT NULL CHECK (length(token_prefix) = 8),
                    token_hash BLOB NOT NULL CHECK (length(token_hash) = 32),
                    created_at INTEGER NOT NULL,
                    last_used_at INTEGER,
                    revoked_at INTEGER
                )",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "CREATE TABLE audit_log (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    actor_user_id INTEGER REFERENCES users(id),
                    action TEXT NOT NULL,
                    target_type TEXT NOT NULL,
                    target_id TEXT,
                    metadata_json TEXT,
                    created_at INTEGER NOT NULL
                )",
            )
            .execute(&pool)
            .await
            .unwrap();
            Self { _file: file, pool }
        }

        async fn insert_user(&self, email: &str) -> i64 {
            let now = 1000i64;
            sqlx::query_scalar(
                "INSERT INTO users (normalized_email, display_name, status, created_at)
                 VALUES (?, 'Test', 'active', ?) RETURNING id",
            )
            .bind(email)
            .bind(now)
            .fetch_one(&self.pool)
            .await
            .unwrap()
        }
    }

    fn basic_header(username: &str, password: &str) -> HeaderValue {
        let encoded = B64.encode(format!("{username}:{password}"));
        HeaderValue::from_str(&format!("Basic {encoded}")).unwrap()
    }

    #[tokio::test]
    async fn propfind_dav_root_returns_207_with_principal_link() {
        let db = TestDb::new().await;
        let key = SecretKey::generate();
        let accounts = CaldavAccountService::new_at(
            db.pool.clone(),
            key,
            url::Url::parse("http://127.0.0.1:3000").unwrap(),
            1000,
        );
        let user_id = db.insert_user("frank@example.test").await;
        let issued = accounts
            .issue_credential(user_id, "Phone".into())
            .await
            .unwrap();

        let app = build_caldav_router(accounts);
        let request = Request::builder()
            .method(Method::from_bytes(b"PROPFIND").unwrap())
            .uri("/dav/")
            .header(
                header::AUTHORIZATION,
                basic_header("frank@example.test", issued.password.expose()),
            )
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::MULTI_STATUS);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("<D:current-principal>"));
        assert!(text.contains("/dav/principals/"));
        assert!(text.contains("HTTP/1.1 200 OK"));
    }

    #[tokio::test]
    async fn propfind_dav_root_returns_401_without_auth() {
        let db = TestDb::new().await;
        let key = SecretKey::generate();
        let accounts = CaldavAccountService::new_at(
            db.pool.clone(),
            key,
            url::Url::parse("http://127.0.0.1:3000").unwrap(),
            1000,
        );
        db.insert_user("gina@example.test").await;

        let app = build_caldav_router(accounts);
        let request = Request::builder()
            .method(Method::from_bytes(b"PROPFIND").unwrap())
            .uri("/dav/")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(response.headers().get(header::WWW_AUTHENTICATE).is_some());
    }

    #[tokio::test]
    async fn get_dav_root_returns_405() {
        let db = TestDb::new().await;
        let key = SecretKey::generate();
        let accounts = CaldavAccountService::new_at(
            db.pool.clone(),
            key,
            url::Url::parse("http://127.0.0.1:3000").unwrap(),
            1000,
        );

        let app = build_caldav_router(accounts);
        let request = Request::builder()
            .method(Method::GET)
            .uri("/dav/")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn well_known_caldav_redirects_to_dav_root() {
        let db = TestDb::new().await;
        let key = SecretKey::generate();
        let accounts = CaldavAccountService::new_at(
            db.pool.clone(),
            key,
            url::Url::parse("http://127.0.0.1:3000").unwrap(),
            1000,
        );

        let app = build_caldav_router(accounts);
        let request = Request::builder()
            .method(Method::GET)
            .uri("/.well-known/caldav")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::MOVED_PERMANENTLY);
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap(),
            "http://127.0.0.1:3000/dav/"
        );
    }

    async fn assert_options_capabilities(app: Router, uri: &str) {
        let request = Request::builder()
            .method(Method::OPTIONS)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK, "OPTIONS {uri}");
        let dav = response.headers().get("dav").unwrap();
        assert!(
            dav.to_str().unwrap().contains("calendar-access"),
            "OPTIONS {uri} should advertise calendar-access"
        );
        assert_eq!(
            response.headers().get(header::ALLOW).unwrap(),
            "PROPFIND, OPTIONS",
            "OPTIONS {uri} should list only implemented methods"
        );
    }

    #[tokio::test]
    async fn options_advertises_only_implemented_dav_capabilities() {
        let db = TestDb::new().await;
        let key = SecretKey::generate();
        let accounts = CaldavAccountService::new_at(
            db.pool.clone(),
            key,
            url::Url::parse("http://127.0.0.1:3000").unwrap(),
            1000,
        );

        let app = build_caldav_router(accounts);
        assert_options_capabilities(app.clone(), "/dav/").await;
        assert_options_capabilities(app.clone(), "/dav/principals/some-principal/").await;
        assert_options_capabilities(app, "/dav/calendars/some-principal/").await;
    }

    #[tokio::test]
    async fn principal_discovery_returns_calendar_home_for_authenticated_user() {
        let db = TestDb::new().await;
        let key = SecretKey::generate();
        let accounts = CaldavAccountService::new_at(
            db.pool.clone(),
            key,
            url::Url::parse("http://127.0.0.1:3000").unwrap(),
            1000,
        );
        let user_id = db.insert_user("hank@example.test").await;
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

        let app = build_caldav_router(accounts);
        let request = Request::builder()
            .method(Method::from_bytes(b"PROPFIND").unwrap())
            .uri(format!("/dav/principals/{principal_id}/"))
            .header(
                header::AUTHORIZATION,
                basic_header("hank@example.test", issued.password.expose()),
            )
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::MULTI_STATUS);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("<D:principal-URL>"));
        assert!(text.contains("<D:calendar-home-set>"));
        assert!(text.contains("/dav/calendars/"));
        assert!(text.contains("<D:displayname>"));
        assert!(text.contains("calendar-query"));
        assert!(text.contains("calendar-multiget"));
        assert!(text.contains("sync-collection"));
    }

    #[tokio::test]
    async fn unknown_principal_and_calendar_do_not_leak_cross_user_existence() {
        let db = TestDb::new().await;
        let key = SecretKey::generate();
        let accounts = CaldavAccountService::new_at(
            db.pool.clone(),
            key,
            url::Url::parse("http://127.0.0.1:3000").unwrap(),
            1000,
        );
        let owner_id = db.insert_user("owner@example.test").await;
        let intruder_id = db.insert_user("intruder@example.test").await;
        let owner_issued = accounts
            .issue_credential(owner_id, "Phone".into())
            .await
            .unwrap();
        let intruder_issued = accounts
            .issue_credential(intruder_id, "Phone".into())
            .await
            .unwrap();
        let owner_principal = accounts
            .status(owner_id)
            .await
            .unwrap()
            .principal_id
            .unwrap();

        let app = build_caldav_router(accounts);
        let cross_user = Request::builder()
            .method(Method::from_bytes(b"PROPFIND").unwrap())
            .uri(format!("/dav/principals/{owner_principal}/"))
            .header(
                header::AUTHORIZATION,
                basic_header("intruder@example.test", intruder_issued.password.expose()),
            )
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(cross_user).await.unwrap().status(),
            StatusCode::NOT_FOUND
        );

        let unknown = Request::builder()
            .method(Method::from_bytes(b"PROPFIND").unwrap())
            .uri("/dav/principals/00000000-0000-0000-0000-000000000000/")
            .header(
                header::AUTHORIZATION,
                basic_header("owner@example.test", owner_issued.password.expose()),
            )
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.oneshot(unknown).await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn propfind_rejects_infinite_depth() {
        let db = TestDb::new().await;
        let key = SecretKey::generate();
        let accounts = CaldavAccountService::new_at(
            db.pool.clone(),
            key,
            url::Url::parse("http://127.0.0.1:3000").unwrap(),
            1000,
        );
        let user_id = db.insert_user("ivy@example.test").await;
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

        let app = build_caldav_router(accounts);
        let request = Request::builder()
            .method(Method::from_bytes(b"PROPFIND").unwrap())
            .uri(format!("/dav/principals/{principal_id}/"))
            .header(
                header::AUTHORIZATION,
                basic_header("ivy@example.test", issued.password.expose()),
            )
            .header("depth", "infinity")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
