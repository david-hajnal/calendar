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
        types::{CaldavAuthError, CredentialMetadata, DavSession},
    },
    http::{ApiError, authenticated_session},
    sessions::{AuthenticatedSession, SessionManager},
};

const DAV_REALM: &str = "commoncal-dav";
const PROPFIND: &str = "PROPFIND";

pub fn build_caldav_router(accounts: CaldavAccountService) -> Router {
    Router::new()
        .route("/dav/", any(dav_root))
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

async fn dav_root(State(accounts): State<CaldavAccountService>, request: Request) -> Response {
    if request.method().as_str() != PROPFIND {
        return (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, PROPFIND)]).into_response();
    }
    let Some(authorization) = request.headers().get(header::AUTHORIZATION) else {
        return dav_unauthorized();
    };
    match accounts.authenticate(authorization).await {
        Ok(session) => render_propfind(&accounts, &session),
        Err(_) => dav_unauthorized(),
    }
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
}
