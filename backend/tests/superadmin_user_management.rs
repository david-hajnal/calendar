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
            None,
            None,
            None,
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
         ) VALUES (?, NULL, 'registered', ?, ?)",
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
            "/api/v1/admin/users?status=registered&page=1&per_page=20",
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
    assert_eq!(row, ("registered".to_owned(), true));
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

    assert!(matches!(duplicate, Err(AdminError::AccountExists(status)) if status == "invited"));
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
            "/api/v1/admin/users?status=registered&page=1&per_page=1",
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

#[tokio::test]
async fn revoked_delivery_can_be_retried_but_stale_resends_and_ineligible_users_cannot() {
    let app = TestApplication::new().await;
    let first = app
        .admin_service()
        .invite(
            app.admin_id,
            InviteUser {
                email: "retry@example.com".into(),
                display_name: None,
            },
        )
        .await
        .unwrap();
    sqlx::query("UPDATE invitations SET revoked_at = ?, expires_at = ? WHERE id = ?")
        .bind(NOW - 10)
        .bind(NOW - 1)
        .bind(first.invitation_id)
        .execute(&app.pool)
        .await
        .unwrap();
    let second = app
        .admin_service()
        .resend_invitation(app.admin_id, first.invitation_id)
        .await
        .unwrap();
    assert!(matches!(
        app.admin_service()
            .resend_invitation(app.admin_id, first.invitation_id)
            .await,
        Err(AdminError::NotFound)
    ));
    for status in ["pending", "inactive", "registered", "deleted"] {
        sqlx::query("UPDATE users SET status = ? WHERE normalized_email = 'retry@example.com'")
            .bind(status)
            .execute(&app.pool)
            .await
            .unwrap();
        assert!(matches!(
            app.admin_service()
                .resend_invitation(app.admin_id, second.invitation_id)
                .await,
            Err(AdminError::NotFound)
        ));
    }
}

#[tokio::test]
async fn invitation_respects_email_reservations_and_releases_expired_ones() {
    let app = TestApplication::new().await;
    sqlx::query("INSERT INTO email_change_requests(user_id, normalized_new_email, token_hash, expires_at, actor_user_id, created_at) VALUES (?, 'reserved@example.com', X'1234', ?, ?, ?)").bind(app.member_id).bind(NOW + 10).bind(app.member_id).bind(NOW).execute(&app.pool).await.unwrap();
    let command = InviteUser {
        email: " RESERVED@example.com ".into(),
        display_name: None,
    };
    assert!(matches!(
        app.admin_service()
            .invite(app.admin_id, command.clone())
            .await,
        Err(AdminError::Conflict)
    ));
    sqlx::query("UPDATE email_change_requests SET expires_at = ?")
        .bind(NOW)
        .execute(&app.pool)
        .await
        .unwrap();
    app.admin_service()
        .invite(app.admin_id, command)
        .await
        .unwrap();
    let revoked: Option<i64> = sqlx::query_scalar("SELECT revoked_at FROM email_change_requests")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(revoked, Some(NOW));
}

struct RetrySender(std::sync::atomic::AtomicBool);
impl commoncal_backend::email::EmailSender for RetrySender {
    async fn send_invitation(
        &self,
        _: commoncal_backend::email::InvitationEmail,
    ) -> Result<(), commoncal_backend::email::EmailError> {
        if self.0.load(std::sync::atomic::Ordering::SeqCst) {
            Err(commoncal_backend::email::EmailError::transient())
        } else {
            Ok(())
        }
    }
    async fn send_login_link(
        &self,
        _: commoncal_backend::email::LoginLinkEmail,
    ) -> Result<(), commoncal_backend::email::EmailError> {
        Ok(())
    }
}

#[tokio::test]
async fn failed_delivery_remains_visible_and_retries_register_the_same_user() {
    use commoncal_backend::invitations::{ConsumeInvitation, InvitationConsumer};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let app = TestApplication::new().await;
    let sender = Arc::new(RetrySender(AtomicBool::new(true)));
    let service = AdminService::with_email_sender(
        app.pool.clone(),
        app.key.clone(),
        3600,
        "https://commoncal.test/invitations/accept",
        sender.clone(),
    );
    assert!(matches!(
        service
            .invite(
                app.admin_id,
                InviteUser {
                    email: "failure@example.com".into(),
                    display_name: None
                }
            )
            .await,
        Err(AdminError::DeliveryFailed)
    ));
    let users = service.list_users(Some("invited"), 1, 20).await.unwrap();
    let user = &users.users[0];
    let failed_id = user.invitation_id.unwrap();
    assert!(matches!(
        service.resend_invitation(app.admin_id, failed_id).await,
        Err(AdminError::DeliveryFailed)
    ));
    let latest = service
        .list_users(Some("invited"), 1, 20)
        .await
        .unwrap()
        .users[0]
        .invitation_id
        .unwrap();
    sender.0.store(false, Ordering::SeqCst);
    let replacement = service
        .resend_invitation(app.admin_id, latest)
        .await
        .unwrap();
    let consumer = InvitationConsumer::new(app.pool.clone(), app.key.clone());
    let accepted = consumer
        .consume(ConsumeInvitation {
            token: replacement.token.expose().into(),
            password: "a-long-new-password".into(),
            password_confirmation: "a-long-new-password".into(),
        })
        .await
        .unwrap();
    assert_eq!(accepted.user.id, user.id);
}

#[tokio::test]
async fn resend_requires_admin_and_csrf_and_duplicate_reports_status() {
    let app = TestApplication::new().await;
    let first = app
        .admin_service()
        .invite(
            app.admin_id,
            InviteUser {
                email: "duplicate@example.com".into(),
                display_name: None,
            },
        )
        .await
        .unwrap();
    let path = format!("/api/v1/admin/invitations/{}/resend", first.invitation_id);
    assert_eq!(
        app.request(Method::POST, &path, app.member_id, "")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let (token, _) = app.session_for(app.admin_id).await;
    let response = app
        .router()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(&path)
                .header(COOKIE, format!("__Host-commoncal_session={token}"))
                .header("origin", ORIGIN)
                .header("sec-fetch-site", "same-origin")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = app
        .request(
            Method::POST,
            "/api/v1/admin/invitations",
            app.admin_id,
            r#"{"email":"DUPLICATE@example.com"}"#,
        )
        .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(
        body_text(response)
            .await
            .contains("This account is invited. Use Resend invitation")
    );
}

#[tokio::test]
async fn expired_invitation_resend_has_one_winner_and_replacement_is_valid() {
    use commoncal_backend::invitations::InvitationConsumer;
    let app = TestApplication::new().await;
    let service = app.admin_service();
    let first = service
        .invite(
            app.admin_id,
            InviteUser {
                email: "expired@example.com".into(),
                display_name: None,
            },
        )
        .await
        .unwrap();
    sqlx::query("UPDATE invitations SET expires_at = ? WHERE id = ?")
        .bind(NOW)
        .bind(first.invitation_id)
        .execute(&app.pool)
        .await
        .unwrap();
    let consumer = InvitationConsumer::new_at(app.pool.clone(), app.key.clone(), NOW);
    assert!(consumer.preview(first.token.expose().into()).await.is_err());
    let (left, right) = tokio::join!(
        service.resend_invitation(app.admin_id, first.invitation_id),
        service.resend_invitation(app.admin_id, first.invitation_id)
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    let replacement = left.or(right).unwrap();
    assert_eq!(
        consumer
            .preview(replacement.token.expose().into())
            .await
            .unwrap()
            .email,
        "expired@example.com"
    );
    assert!(consumer.preview(first.token.expose().into()).await.is_err());
}

#[tokio::test]
async fn disable_invited_and_pending_users_revokes_their_account_tokens() {
    let app = TestApplication::new().await;
    for status in ["invited", "pending"] {
        let email = format!("{status}@example.com");
        let invitation = app
            .admin_service()
            .invite(
                app.admin_id,
                InviteUser {
                    email: email.clone(),
                    display_name: None,
                },
            )
            .await
            .unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE normalized_email = ?")
            .bind(&email)
            .fetch_one(&app.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE users SET status = ? WHERE id = ?")
            .bind(status)
            .bind(id)
            .execute(&app.pool)
            .await
            .unwrap();
        app.admin_service()
            .suspend_user(app.admin_id, id)
            .await
            .unwrap();
        let status: String = sqlx::query_scalar("SELECT status FROM users WHERE id = ?")
            .bind(id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
        assert_eq!(status, "inactive");
        let revoked: Option<i64> =
            sqlx::query_scalar("SELECT revoked_at FROM invitations WHERE id = ?")
                .bind(invitation.invitation_id)
                .fetch_one(&app.pool)
                .await
                .unwrap();
        assert_eq!(revoked, Some(NOW));
        let consumer = commoncal_backend::invitations::InvitationConsumer::new_at(
            app.pool.clone(),
            app.key.clone(),
            NOW,
        );
        assert!(
            consumer
                .preview(invitation.token.expose().into())
                .await
                .is_err()
        );
        assert!(
            app.admin_service()
                .resend_invitation(app.admin_id, invitation.invitation_id)
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn admins_cannot_disable_themselves_even_with_another_admin() {
    let app = TestApplication::new().await;
    insert_user(&app.pool, "second-admin@example.com", true).await;
    let response = app
        .request(
            Method::POST,
            &format!("/api/v1/admin/users/{}/suspend", app.admin_id),
            app.admin_id,
            "",
        )
        .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let status: String = sqlx::query_scalar("SELECT status FROM users WHERE id = ?")
        .bind(app.admin_id)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(status, "registered");
}

#[tokio::test]
async fn disable_revokes_browser_login_and_integration_credentials_permanently() {
    use axum::http::HeaderValue;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use commoncal_backend::caldav::auth::CaldavAccountService;
    let app = TestApplication::new().await;
    let (session, _) = app.session_for(app.member_id).await;
    let caldav = CaldavAccountService::new_at(
        app.pool.clone(),
        app.key.clone(),
        "https://commoncal.test".parse().unwrap(),
        NOW,
    );
    let credential = caldav
        .issue_credential(app.member_id, "Calendar client".into())
        .await
        .unwrap();
    let basic = HeaderValue::from_str(&format!(
        "Basic {}",
        STANDARD.encode(format!(
            "member@example.com:{}",
            credential.password.expose()
        ))
    ))
    .unwrap();
    assert!(caldav.authenticate(&basic).await.is_ok());
    let login_token = app.key.generate_token();
    let login_hash = app.key.hash_token(TokenDomain::Login, &login_token);
    for table in ["login_tokens", "password_reset_tokens"] {
        let statement = format!(
            "INSERT INTO {table}(user_id, token_hash, expires_at, created_at) VALUES (?, ?, ?, ?)"
        );
        sqlx::query(&statement)
            .bind(app.member_id)
            .bind(login_hash.as_bytes().as_slice())
            .bind(NOW + 100)
            .bind(NOW)
            .execute(&app.pool)
            .await
            .unwrap();
    }
    sqlx::query("INSERT INTO email_change_requests(user_id, normalized_new_email, token_hash, expires_at, actor_user_id, created_at) VALUES (?, 'new@example.com', X'1234', ?, ?, ?)").bind(app.member_id).bind(NOW + 100).bind(app.member_id).bind(NOW).execute(&app.pool).await.unwrap();
    sqlx::query("INSERT INTO mcp_grant(id, user_id, oauth_client_id, created_at) VALUES ('grant-member', ?, 'client', ?)").bind(app.member_id).bind(NOW).execute(&app.pool).await.unwrap();
    app.admin_service()
        .suspend_user(app.admin_id, app.member_id)
        .await
        .unwrap();
    let manager = SessionManager::new_at(
        app.pool.clone(),
        app.key.clone(),
        SessionSecurityConfig::new(300, 60, ORIGIN).unwrap(),
        NOW,
    );
    assert!(manager.authenticate(Some(&session)).await.is_err());
    assert!(caldav.authenticate(&basic).await.is_err());
    assert!(
        caldav
            .issue_credential(app.member_id, "After disable".into())
            .await
            .is_err()
    );
    // The grant must be revoked (revoked_at set) after suspension.
    let grant_revoked: Option<i64> =
        sqlx::query_scalar("SELECT revoked_at FROM mcp_grant WHERE id = 'grant-member'")
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(grant_revoked, Some(NOW));
    for table in [
        "sessions",
        "login_tokens",
        "password_reset_tokens",
        "email_change_requests",
        "mcp_grant",
        "caldav_credentials",
    ] {
        let statement =
            format!("SELECT COUNT(*) FROM {table} WHERE user_id = ? AND revoked_at IS NULL");
        let count: i64 = sqlx::query_scalar(&statement)
            .bind(app.member_id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
        assert_eq!(count, 0, "{table}");
    }
    use commoncal_backend::login::{
        ConsumeLoginLink, FixedWindowLoginRateLimiter, LoginFlow, LoginService,
    };
    let login = LoginService::new_at(
        app.pool.clone(),
        app.key.clone(),
        900,
        3600,
        "https://commoncal.test/login",
        std::sync::Arc::new(commoncal_backend::email::InMemoryEmailSender::new()),
        std::sync::Arc::new(FixedWindowLoginRateLimiter::new_at(10, 900, NOW)),
        NOW,
        true,
    );
    assert!(
        login
            .consume_link(ConsumeLoginLink {
                token: login_token.expose().into(),
                prior_session_token: None
            })
            .await
            .is_err()
    );
    app.admin_service()
        .reactivate_user(app.admin_id, app.member_id)
        .await
        .unwrap();
    assert!(manager.authenticate(Some(&session)).await.is_err());
    assert!(caldav.authenticate(&basic).await.is_err());
}

#[tokio::test]
async fn concurrent_admin_disables_leave_one_active_admin() {
    let app = TestApplication::new().await;
    let second = insert_user(&app.pool, "second-admin@example.com", true).await;
    let service = app.admin_service();
    let (left, right) = tokio::join!(
        service.suspend_user(app.admin_id, second),
        service.suspend_user(second, app.admin_id)
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM users WHERE is_superadmin = 1 AND status = 'registered'",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn disable_rolls_back_all_credentials_when_audit_fails() {
    let app = TestApplication::new().await;
    app.session_for(app.member_id).await;
    sqlx::query("INSERT INTO mcp_grant(id, user_id, oauth_client_id, created_at) VALUES ('rollback-grant', ?, 'client', ?)").bind(app.member_id).bind(NOW).execute(&app.pool).await.unwrap();
    sqlx::query("CREATE TRIGGER fail_disable_audit BEFORE INSERT ON audit_log WHEN NEW.action = 'admin.user.suspend' BEGIN SELECT RAISE(ABORT, 'test audit failure'); END").execute(&app.pool).await.unwrap();
    assert!(
        app.admin_service()
            .suspend_user(app.admin_id, app.member_id)
            .await
            .is_err()
    );
    let status: String = sqlx::query_scalar("SELECT status FROM users WHERE id = ?")
        .bind(app.member_id)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(status, "registered");
    for table in ["sessions", "mcp_grant"] {
        let query =
            format!("SELECT COUNT(*) FROM {table} WHERE user_id = ? AND revoked_at IS NULL");
        let count: i64 = sqlx::query_scalar(&query)
            .bind(app.member_id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }
}

#[tokio::test]
async fn stale_or_nonadmin_actor_cannot_disable_and_inactive_target_is_unchanged() {
    let app = TestApplication::new().await;
    assert!(
        app.admin_service()
            .suspend_user(app.member_id, app.admin_id)
            .await
            .is_err()
    );
    let second = insert_user(&app.pool, "second-admin@example.com", true).await;
    app.admin_service()
        .suspend_user(second, app.admin_id)
        .await
        .unwrap();
    assert!(
        app.admin_service()
            .suspend_user(app.admin_id, app.member_id)
            .await
            .is_err()
    );
    app.admin_service()
        .suspend_user(second, app.member_id)
        .await
        .unwrap();
    assert!(matches!(
        app.admin_service()
            .suspend_user(second, app.member_id)
            .await,
        Err(AdminError::NotFound)
    ));
}

#[tokio::test]
async fn concurrent_caldav_issuance_cannot_leave_a_live_credential_after_disable() {
    let app = TestApplication::new().await;
    let caldav = commoncal_backend::caldav::auth::CaldavAccountService::new_at(
        app.pool.clone(),
        app.key.clone(),
        "https://commoncal.test".parse().unwrap(),
        NOW,
    );
    let admin = app.admin_service();
    let (_, disabled) = tokio::join!(
        caldav.issue_credential(app.member_id, "Racing calendar client".into()),
        admin.suspend_user(app.admin_id, app.member_id)
    );
    disabled.unwrap();
    let live: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM caldav_credentials WHERE user_id = ? AND revoked_at IS NULL",
    )
    .bind(app.member_id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(live, 0);
}
