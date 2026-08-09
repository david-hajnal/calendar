use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header::COOKIE},
};
use commoncal_backend::{
    admin::{AdminError, AdminService, InviteUser},
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    http::{Readiness, build_router_with_admin},
    security::{SecretKey, TokenDomain},
    sessions::{SessionManager, SessionSecurityConfig},
};
use http_body_util::BodyExt;
use sqlx::SqlitePool;
use tempfile::TempDir;
use tower::ServiceExt;

const NOW: i64 = 20_000;
const ORIGIN: &str = "https://commoncal.test";

struct TestApplication {
    _temp_dir: TempDir,
    pool: SqlitePool,
    key: SecretKey,
    admin_id: i64,
    member_id: i64,
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
        // Migration 0019 seeds a default superadmin; remove it so this
        // test's admin is the sole superadmin as the test expects.
        sqlx::query("DELETE FROM users WHERE normalized_email = 'admin@localhost'")
            .execute(&pool)
            .await
            .unwrap();
        let admin_id = insert_user(&pool, "admin@example.com", true).await;
        let member_id = insert_user(&pool, "member@example.com", false).await;
        Self {
            _temp_dir: temp_dir,
            pool,
            key: SecretKey::new([77; 32]),
            admin_id,
            member_id,
        }
    }

    fn admin_service(&self) -> AdminService {
        AdminService::new_at(self.pool.clone(), self.key.clone(), 3_600, NOW)
    }

    fn router(&self) -> axum::Router {
        build_router_with_admin(
            Readiness::new(),
            SessionManager::new_at(
                self.pool.clone(),
                self.key.clone(),
                SessionSecurityConfig::new(300, 60, ORIGIN).unwrap(),
                NOW,
            ),
            self.admin_service(),
        )
    }

    async fn session_for(&self, user_id: i64) -> (String, String) {
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
        (token.expose().to_owned(), csrf.expose().to_owned())
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        user_id: i64,
        body: &str,
    ) -> axum::response::Response {
        let (token, csrf) = self.session_for(user_id).await;
        self.router()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(COOKIE, format!("__Host-commoncal_session={token}"))
                    .header("content-type", "application/json")
                    .header("origin", ORIGIN)
                    .header("sec-fetch-site", "same-origin")
                    .header("x-csrf-token", csrf)
                    .body(Body::from(body.to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }
}

async fn insert_user(pool: &SqlitePool, email: &str, is_superadmin: bool) -> i64 {
    sqlx::query(
        "INSERT INTO users (
            normalized_email, display_name, status, is_superadmin, created_at
         ) VALUES (?, NULL, 'active', ?, ?)",
    )
    .bind(email)
    .bind(is_superadmin)
    .bind(NOW - 1_000)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid()
}

async fn body_text(response: axum::response::Response) -> String {
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
async fn normal_users_receive_denial() {
    let app = TestApplication::new().await;

    let response = app
        .request(
            Method::GET,
            "/api/v1/admin/users?status=active&page=1&per_page=20",
            app.member_id,
            "",
        )
        .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn final_superadmin_cannot_be_demoted_or_suspended() {
    let app = TestApplication::new().await;

    for action in ["demote", "suspend"] {
        let response = app
            .request(
                Method::POST,
                &format!("/api/v1/admin/users/{}/{action}", app.admin_id),
                app.admin_id,
                "{}",
            )
            .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    let row: (String, bool) =
        sqlx::query_as("SELECT status, is_superadmin FROM users WHERE id = ?")
            .bind(app.admin_id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(row, ("active".to_owned(), true));
}

#[tokio::test]
async fn duplicate_pending_invitation_is_handled_deterministically() {
    let app = TestApplication::new().await;
    let command = InviteUser {
        email: " Invitee@Example.com ".to_owned(),
        display_name: Some("Invitee".to_owned()),
    };

    app.admin_service()
        .invite(app.admin_id, command.clone())
        .await
        .unwrap();
    let duplicate = app.admin_service().invite(app.admin_id, command).await;

    assert!(matches!(duplicate, Err(AdminError::Conflict)));
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM invitations
         WHERE normalized_email = 'invitee@example.com'
           AND revoked_at IS NULL AND consumed_at IS NULL",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn resend_invalidates_the_previous_token() {
    let app = TestApplication::new().await;
    let first = app
        .admin_service()
        .invite(
            app.admin_id,
            InviteUser {
                email: "invitee@example.com".to_owned(),
                display_name: None,
            },
        )
        .await
        .unwrap();

    let second = app
        .admin_service()
        .resend_invitation(app.admin_id, first.invitation_id)
        .await
        .unwrap();

    assert_ne!(first.token.expose(), second.token.expose());
    let old_revoked: Option<i64> =
        sqlx::query_scalar("SELECT revoked_at FROM invitations WHERE id = ?")
            .bind(first.invitation_id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(old_revoked, Some(NOW));
    let old_hash = app.key.hash_token(TokenDomain::Invitation, &first.token);
    let active_old: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM invitations
         WHERE token_hash = ? AND revoked_at IS NULL",
    )
    .bind(old_hash.as_bytes().as_slice())
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(active_old, 0);
}

#[tokio::test]
async fn suspending_a_user_revokes_sessions() {
    let app = TestApplication::new().await;
    app.session_for(app.member_id).await;

    let response = app
        .request(
            Method::POST,
            &format!("/api/v1/admin/users/{}/suspend", app.member_id),
            app.admin_id,
            "{}",
        )
        .await;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let active_sessions: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sessions WHERE user_id = ? AND revoked_at IS NULL",
    )
    .bind(app.member_id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(active_sessions, 0);
    let audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_log
         WHERE actor_user_id = ? AND action = 'admin.user.suspend' AND target_id = ?",
    )
    .bind(app.admin_id)
    .bind(app.member_id.to_string())
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(audits, 1);
}

#[tokio::test]
async fn user_listing_never_exposes_token_hashes() {
    let app = TestApplication::new().await;
    app.admin_service()
        .invite(
            app.admin_id,
            InviteUser {
                email: "invitee@example.com".to_owned(),
                display_name: None,
            },
        )
        .await
        .unwrap();

    let response = app
        .request(
            Method::GET,
            "/api/v1/admin/users?status=active&page=1&per_page=1",
            app.admin_id,
            "",
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_text(response).await;
    assert!(body.contains(r#""page":1"#));
    assert!(body.contains(r#""per_page":1"#));
    assert!(!body.contains("token_hash"));
    assert!(!body.contains("session_hash"));
}

#[tokio::test]
async fn object_identifier_substitution_does_not_bypass_authorization() {
    let app = TestApplication::new().await;

    let response = app
        .request(
            Method::POST,
            &format!("/api/v1/admin/users/{}/promote", app.member_id),
            app.member_id,
            "{}",
        )
        .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let promoted: bool = sqlx::query_scalar("SELECT is_superadmin FROM users WHERE id = ?")
        .bind(app.member_id)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert!(!promoted);
}
