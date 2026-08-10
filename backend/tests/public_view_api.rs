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
    calendar_id: i64,
    view_id: i64,
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
        let source_owner = insert_user(&pool, "source@example.com").await;
        let repository = CalendarRepository::new(pool.clone());
        let calendar_id = repository
            .create_calendar(
                source_owner,
                NewCalendar {
                    name: "Source".to_owned(),
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
            .id;
        repository
            .add_acl(calendar_id, owner, CalendarRole::Viewer, NOW - 10)
            .await
            .unwrap();
        let view_id = sqlx::query(
            "INSERT INTO shared_views (owner_user_id, name, version, created_at, updated_at)
             VALUES (?, 'Published view', 1, ?, ?)",
        )
        .bind(owner)
        .bind(NOW)
        .bind(NOW)
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
        sqlx::query(
            "INSERT INTO shared_view_calendars (view_id, calendar_id, position, color)
             VALUES (?, ?, 0, '#3367d6')",
        )
        .bind(view_id)
        .bind(calendar_id)
        .execute(&pool)
        .await
        .unwrap();
        insert_event(&pool, calendar_id, source_owner).await;
        Self {
            _temp_dir: temp_dir,
            pool,
            key: SecretKey::new([83; 32]),
            owner,
            calendar_id,
            view_id,
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
            SharedViewService::new_at_with_key(self.pool.clone(), self.key.clone(), NOW),
            None,
            None,
            None,
        )
    }

    async fn authenticated(
        &self,
        method: Method,
        path: &str,
        body: &str,
    ) -> axum::response::Response {
        let token = self.key.generate_token();
        let hash = self.key.hash_token(TokenDomain::Session, &token);
        sqlx::query(
            "INSERT INTO sessions (
                user_id, session_hash, expires_at, revoked_at, created_at, last_seen_at
             ) VALUES (?, ?, ?, NULL, ?, ?)",
        )
        .bind(self.owner)
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

    async fn public(&self, method: Method, path: &str) -> axum::response::Response {
        self.router()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn publish(&self, projection: &str, expires_at: i64) -> String {
        let response = self
            .authenticated(
                Method::POST,
                &format!("/api/v1/views/{}/publication", self.view_id),
                &format!(
                    r#"{{"projection":"{projection}","display_timezone":"UTC","expires_at":{expires_at}}}"#
                ),
            )
            .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let token = json_string(&response_body(response).await, "token");
        assert_eq!(token.len(), 43);
        token
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

async fn insert_event(pool: &SqlitePool, calendar_id: i64, user_id: i64) {
    sqlx::query(
        "INSERT INTO events (
            calendar_id, title, description, location, status, event_kind,
            timed_start_utc, timed_end_utc, event_timezone,
            all_day_start_date, all_day_end_date, created_by_user_id,
            last_edited_by_user_id, version, created_at, updated_at
         ) VALUES (?, 'Planning', 'private description', 'Secret room',
                   'confirmed', 'timed', ?, ?, 'UTC', NULL, NULL,
                   ?, ?, 1, ?, ?)",
    )
    .bind(calendar_id)
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

fn json_string(body: &str, key: &str) -> String {
    let marker = format!(r#""{key}":""#);
    let start = body.find(&marker).unwrap() + marker.len();
    let end = body[start..].find('"').unwrap() + start;
    body[start..end].to_owned()
}

fn events_path(token: &str) -> String {
    format!(
        "/api/v1/public/views/{token}/events?from={}&to={}",
        NOW,
        NOW + 300
    )
}

#[tokio::test]
async fn projections_contain_only_their_permitted_fields_and_no_source_or_user_ids() {
    for (projection, permitted, forbidden) in [
        ("full_details", "private description", ""),
        ("title_and_time", "Planning", "private description"),
        ("free_busy", r#""busy":true"#, "Planning"),
    ] {
        let app = TestApplication::new().await;
        let token = app.publish(projection, NOW + 1_000).await;
        let body = response_body(app.public(Method::GET, &events_path(&token)).await).await;
        assert!(body.contains(permitted), "{body}");
        if !forbidden.is_empty() {
            assert!(!body.contains(forbidden), "{body}");
        }
        if projection != "full_details" {
            assert!(!body.contains("Secret room"), "{body}");
        }
        for private_field in [
            "calendar_id",
            "owner_user_id",
            "created_by_user_id",
            "last_edited_by_user_id",
        ] {
            assert!(!body.contains(private_field), "{body}");
        }
    }
}

#[tokio::test]
async fn raw_token_is_not_stored_and_rotation_invalidates_the_old_token() {
    let app = TestApplication::new().await;
    let old_token = app.publish("title_and_time", NOW + 1_000).await;
    let (prefix, hash): (String, Vec<u8>) =
        sqlx::query_as("SELECT token_prefix, token_hash FROM public_view_links")
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_ne!(prefix, old_token);
    assert!(!String::from_utf8_lossy(&hash).contains(&old_token));

    let rotated = app
        .authenticated(
            Method::POST,
            &format!("/api/v1/views/{}/publication/rotate", app.view_id),
            "",
        )
        .await;
    assert_eq!(rotated.status(), StatusCode::OK);
    let new_token = json_string(&response_body(rotated).await, "token");
    assert_ne!(old_token, new_token);
    assert_eq!(
        app.public(Method::GET, &format!("/api/v1/public/views/{old_token}"))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.public(Method::GET, &format!("/api/v1/public/views/{new_token}"))
            .await
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn expired_revoked_and_invalid_tokens_return_the_same_generic_not_found() {
    let expired_app = TestApplication::new().await;
    let expired = expired_app.publish("full_details", NOW).await;
    let expired_response = expired_app
        .public(Method::GET, &format!("/api/v1/public/views/{expired}"))
        .await;
    let expired_body = response_body(expired_response).await;

    let revoked_app = TestApplication::new().await;
    let revoked = revoked_app.publish("full_details", NOW + 1_000).await;
    let revoke = revoked_app
        .authenticated(
            Method::POST,
            &format!("/api/v1/views/{}/publication/revoke", revoked_app.view_id),
            "",
        )
        .await;
    assert_eq!(revoke.status(), StatusCode::NO_CONTENT);
    let revoked_response = revoked_app
        .public(Method::GET, &format!("/api/v1/public/views/{revoked}"))
        .await;
    let revoked_body = response_body(revoked_response).await;

    let invalid_response = revoked_app
        .public(
            Method::GET,
            "/api/v1/public/views/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        )
        .await;
    let invalid_body = response_body(invalid_response).await;
    assert_eq!(expired_body, revoked_body);
    assert_eq!(revoked_body, invalid_body);
    assert!(invalid_body.contains(r#""code":"not_found""#));
}

#[tokio::test]
async fn publication_can_be_reconfigured_and_public_reads_are_secure_and_read_only() {
    let app = TestApplication::new().await;
    let token = app.publish("full_details", NOW + 1_000).await;
    let configured = app
        .authenticated(
            Method::PATCH,
            &format!("/api/v1/views/{}/publication", app.view_id),
            &format!(
                r#"{{"projection":"free_busy","display_timezone":"Europe/Budapest","expires_at":{}}}"#,
                NOW + 500
            ),
        )
        .await;
    assert_eq!(configured.status(), StatusCode::OK);

    let metadata = app
        .public(Method::GET, &format!("/api/v1/public/views/{token}"))
        .await;
    assert_eq!(metadata.status(), StatusCode::OK);
    assert_eq!(
        metadata.headers().get("cache-control").unwrap(),
        "private, no-store"
    );
    assert_eq!(
        metadata.headers().get("x-robots-tag").unwrap(),
        "noindex, nofollow"
    );
    assert_eq!(
        metadata.headers().get("x-content-type-options").unwrap(),
        "nosniff"
    );
    assert!(metadata.headers().get("set-cookie").is_none());
    let metadata_body = response_body(metadata).await;
    assert!(metadata_body.contains(r#""projection":"free_busy""#));
    assert!(metadata_body.contains(r#""display_timezone":"Europe/Budapest""#));
    assert!(!metadata_body.contains("owner_user_id"));
    assert!(!metadata_body.contains("calendar_id"));

    for path in [format!("/api/v1/public/views/{token}"), events_path(&token)] {
        for method in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
            assert_eq!(
                app.public(method, &path).await.status(),
                StatusCode::METHOD_NOT_ALLOWED
            );
        }
    }
}

#[tokio::test]
async fn source_acl_changes_are_reflected_on_the_next_public_read() {
    let app = TestApplication::new().await;
    let token = app.publish("full_details", NOW + 1_000).await;
    let before = response_body(app.public(Method::GET, &events_path(&token)).await).await;
    assert!(before.contains("Planning"));

    sqlx::query("DELETE FROM calendar_acl WHERE calendar_id = ? AND user_id = ?")
        .bind(app.calendar_id)
        .bind(app.owner)
        .execute(&app.pool)
        .await
        .unwrap();

    let after = response_body(app.public(Method::GET, &events_path(&token)).await).await;
    assert!(!after.contains("Planning"), "{after}");
}
