use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header::COOKIE},
};
use commoncal_backend::{
    authorization::CalendarRole,
    calendar::{CalendarRepository, CalendarService, NewCalendar, PendingNotificationCanceller},
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    http::{Readiness, build_router_with_calendars},
    security::{SecretKey, TokenDomain},
    sessions::{SessionManager, SessionSecurityConfig},
};
use http_body_util::BodyExt;
use sqlx::{SqliteConnection, SqlitePool};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};
use tempfile::TempDir;
use tower::ServiceExt;

const NOW: i64 = 60_000;
const ORIGIN: &str = "https://commoncal.test";

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
    suspended: i64,
    deleted: i64,
    target: i64,
    calendar_id: i64,
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
        let owner = insert_user(&pool, "owner@example.com", "active").await;
        let manager = insert_user(&pool, "manager@example.com", "active").await;
        let editor = insert_user(&pool, "editor@example.com", "active").await;
        let viewer = insert_user(&pool, "viewer@example.com", "active").await;
        let free_busy = insert_user(&pool, "freebusy@example.com", "active").await;
        let unrelated = insert_user(&pool, "unrelated@example.com", "active").await;
        let suspended = insert_user(&pool, "suspended@example.com", "suspended").await;
        let deleted = insert_user(&pool, "deleted@example.com", "deleted").await;
        let target = insert_user(&pool, "target@example.com", "active").await;
        let repository = CalendarRepository::new(pool.clone());
        let calendar = repository
            .create_calendar(
                owner,
                NewCalendar {
                    name: "Shared calendar".to_owned(),
                    description: None,
                    color: "#3367d6".to_owned(),
                    default_timezone: "UTC".to_owned(),
                    default_event_visibility: "private".to_owned(),
                    default_notification_rules_json: None,
                    created_at: NOW - 100,
                },
            )
            .await
            .unwrap();
        for (user_id, role) in [
            (manager, CalendarRole::Manager),
            (editor, CalendarRole::Editor),
            (viewer, CalendarRole::Viewer),
            (free_busy, CalendarRole::FreeBusyViewer),
        ] {
            repository
                .add_acl(calendar.id, user_id, role, NOW - 90)
                .await
                .unwrap();
        }
        Self {
            _temp_dir: temp_dir,
            pool,
            key: SecretKey::new([92; 32]),
            owner,
            manager,
            editor,
            viewer,
            free_busy,
            unrelated,
            suspended,
            deleted,
            target,
            calendar_id: calendar.id,
        }
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
        build_router_with_calendars(
            Readiness::new(),
            SessionManager::new_at(
                self.pool.clone(),
                self.key.clone(),
                SessionSecurityConfig::new(300, 60, ORIGIN).unwrap(),
                NOW,
            ),
            CalendarService::new_at(self.pool.clone(), NOW),
            None,
            None,
            None,
        )
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
}

async fn insert_user(pool: &SqlitePool, email: &str, status: &str) -> i64 {
    sqlx::query(
        "INSERT INTO users (
            normalized_email, display_name, status, is_superadmin, created_at
         ) VALUES (?, NULL, ?, 0, ?)",
    )
    .bind(email)
    .bind(status)
    .bind(NOW - 1_000)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid()
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

#[tokio::test]
async fn acl_role_action_matrix_is_enforced_for_every_calendar_role() {
    for (actor, can_manage) in [
        ("owner", true),
        ("manager", true),
        ("editor", false),
        ("viewer", false),
        ("free_busy", false),
        ("unrelated", false),
    ] {
        let app = TestApplication::new().await;
        let actor_id = match actor {
            "owner" => app.owner,
            "manager" => app.manager,
            "editor" => app.editor,
            "viewer" => app.viewer,
            "free_busy" => app.free_busy,
            _ => app.unrelated,
        };
        let acl_path = format!("/api/v1/calendars/{}/acl", app.calendar_id);
        let list = app.request(Method::GET, &acl_path, actor_id, "").await;
        assert_eq!(
            list.status(),
            if can_manage {
                StatusCode::OK
            } else {
                StatusCode::NOT_FOUND
            },
            "{actor} list ACL"
        );

        let entry_path = format!("{acl_path}/{}", app.target);
        let grant = app
            .request(Method::PUT, &entry_path, actor_id, r#"{"role":"viewer"}"#)
            .await;
        assert_eq!(
            grant.status(),
            if can_manage {
                StatusCode::OK
            } else {
                StatusCode::NOT_FOUND
            },
            "{actor} grant ACL"
        );

        let revoke = app.request(Method::DELETE, &entry_path, actor_id, "").await;
        assert_eq!(
            revoke.status(),
            if can_manage {
                StatusCode::NO_CONTENT
            } else {
                StatusCode::NOT_FOUND
            },
            "{actor} revoke ACL"
        );
    }
}

#[tokio::test]
async fn owner_and_manager_can_grant_and_update_every_non_owner_role() {
    for actor in ["owner", "manager"] {
        let app = TestApplication::new().await;
        let actor_id = if actor == "owner" {
            app.owner
        } else {
            app.manager
        };
        let path = format!("/api/v1/calendars/{}/acl/{}", app.calendar_id, app.target);
        for role in ["manager", "editor", "viewer", "free_busy_viewer"] {
            let response = app
                .request(
                    Method::PUT,
                    &path,
                    actor_id,
                    &format!(r#"{{"role":"{role}"}}"#),
                )
                .await;
            assert_eq!(response.status(), StatusCode::OK, "{actor} set {role}");
            assert!(
                body(response)
                    .await
                    .contains(&format!(r#""role":"{role}""#))
            );
        }
    }
}

#[tokio::test]
async fn manager_cannot_transfer_and_editor_cannot_manage_sharing() {
    let app = TestApplication::new().await;
    let transfer_path = format!("/api/v1/calendars/{}/transfer", app.calendar_id);
    let transfer = app
        .request(
            Method::POST,
            &transfer_path,
            app.manager,
            &format!(r#"{{"new_owner_user_id":{},"version":1}}"#, app.target),
        )
        .await;
    assert_eq!(transfer.status(), StatusCode::NOT_FOUND);

    let acl_path = format!("/api/v1/calendars/{}/acl", app.calendar_id);
    let editor_list = app.request(Method::GET, &acl_path, app.editor, "").await;
    assert_eq!(editor_list.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn owner_cannot_remove_or_demote_self() {
    let app = TestApplication::new().await;
    let path = format!("/api/v1/calendars/{}/acl/{}", app.calendar_id, app.owner);

    let revoke = app.request(Method::DELETE, &path, app.owner, "").await;
    assert_eq!(revoke.status(), StatusCode::CONFLICT);
    let demote = app
        .request(Method::PUT, &path, app.owner, r#"{"role":"manager"}"#)
        .await;
    assert_eq!(demote.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn ownership_transfer_is_atomic_audited_and_requires_an_active_target() {
    let app = TestApplication::new().await;
    let path = format!("/api/v1/calendars/{}/transfer", app.calendar_id);

    for inactive_target in [app.suspended, app.deleted] {
        let rejected = app
            .request(
                Method::POST,
                &path,
                app.owner,
                &format!(r#"{{"new_owner_user_id":{inactive_target},"version":1}}"#),
            )
            .await;
        assert_eq!(rejected.status(), StatusCode::CONFLICT);
        let owner: i64 = sqlx::query_scalar("SELECT owner_user_id FROM calendars WHERE id = ?")
            .bind(app.calendar_id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
        assert_eq!(owner, app.owner);
    }

    let transferred = app
        .request(
            Method::POST,
            &path,
            app.owner,
            &format!(r#"{{"new_owner_user_id":{},"version":1}}"#, app.target),
        )
        .await;
    assert_eq!(transferred.status(), StatusCode::OK);
    let entries: Vec<(i64, String)> = sqlx::query_as(
        "SELECT user_id, role FROM calendar_acl
         WHERE calendar_id = ? AND role IN ('owner', 'manager') ORDER BY user_id",
    )
    .bind(app.calendar_id)
    .fetch_all(&app.pool)
    .await
    .unwrap();
    assert!(entries.contains(&(app.owner, "manager".to_owned())));
    assert!(entries.contains(&(app.target, "owner".to_owned())));
    assert_eq!(
        entries.iter().filter(|(_, role)| role == "owner").count(),
        1
    );
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_log
         WHERE action = 'calendar.acl.transfer'
           AND target_type = 'calendar_acl' AND target_id = ?",
    )
    .bind(format!("{}:{}", app.calendar_id, app.target))
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(audit_count, 1);
}

#[tokio::test]
async fn revocation_removes_access_immediately_and_every_change_is_audited() {
    let app = TestApplication::new().await;
    let path = format!("/api/v1/calendars/{}/acl/{}", app.calendar_id, app.target);
    assert_eq!(
        app.request(Method::PUT, &path, app.manager, r#"{"role":"editor"}"#)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        app.request(Method::PUT, &path, app.owner, r#"{"role":"viewer"}"#)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        app.request(Method::DELETE, &path, app.manager, "")
            .await
            .status(),
        StatusCode::NO_CONTENT
    );

    let calendar = app
        .request(
            Method::GET,
            &format!("/api/v1/calendars/{}", app.calendar_id),
            app.target,
            "",
        )
        .await;
    assert_eq!(calendar.status(), StatusCode::NOT_FOUND);
    let actions: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_log
         WHERE target_type = 'calendar_acl' AND target_id = ? ORDER BY id",
    )
    .bind(format!("{}:{}", app.calendar_id, app.target))
    .fetch_all(&app.pool)
    .await
    .unwrap();
    assert_eq!(
        actions,
        [
            "calendar.acl.grant",
            "calendar.acl.update",
            "calendar.acl.revoke"
        ]
    );
}

#[derive(Default)]
struct RecordingNotificationCanceller {
    calls: Mutex<Vec<(i64, i64)>>,
}

impl PendingNotificationCanceller for RecordingNotificationCanceller {
    fn cancel_pending<'a>(
        &'a self,
        _connection: &'a mut SqliteConnection,
        calendar_id: i64,
        user_id: i64,
    ) -> Pin<Box<dyn Future<Output = Result<(), sqlx::Error>> + Send + 'a>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push((calendar_id, user_id));
            Ok(())
        })
    }
}

#[tokio::test]
async fn revocation_invokes_pending_notification_cancellation_contract() {
    let app = TestApplication::new().await;
    let canceller = Arc::new(RecordingNotificationCanceller::default());
    let service = CalendarService::new_at_with_notification_canceller(
        app.pool.clone(),
        NOW,
        canceller.clone(),
    );

    service
        .revoke_acl(app.owner, false, app.calendar_id, app.viewer)
        .await
        .unwrap();

    assert_eq!(
        *canceller.calls.lock().unwrap(),
        [(app.calendar_id, app.viewer)]
    );
}
