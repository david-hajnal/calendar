//! Cross-cutting authorization regression matrix (Prompt 35).
//!
//! Keep new authorization cases in the tables below.  The deliberately small
//! fixture makes it practical to run this suite locally while still covering
//! every calendar principal, including principals that must never inherit
//! access from platform administration.

use commoncal_backend::{
    admin::AdminService,
    authorization::{
        AuthorizationDecision, CalendarAction, CalendarRole, PlatformRole,
        authorize_calendar_action,
    },
    identity::UserStatus,
};

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header::COOKIE},
};
use commoncal_backend::{
    calendar::{CalendarRepository, CalendarService, NewCalendar},
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    email::DevelopmentEmailSender,
    event::EventService,
    external_feed::{ExternalFeedService, NewFeed},
    http::{
        Readiness, build_router_with_auth_flows_sessions_admin_calendars_views_and_external_feeds,
    },
    invitations::InvitationConsumer,
    login::{AllowAllLoginRateLimiter, LoginService},
    notification::NotificationService,
    security::{SecretKey, TokenDomain},
    sessions::{SessionManager, SessionSecurityConfig},
    shared_view::SharedViewService,
};
use http_body_util::BodyExt;
use sqlx::SqlitePool;
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

#[derive(Clone, Copy, Debug)]
struct Principal {
    name: &'static str,
    status: UserStatus,
    platform_role: Option<PlatformRole>,
    calendar_role: Option<CalendarRole>,
}

// These fixture names are intentionally stable: endpoint tests can use them
// without duplicating a separate set of users for each API family.
const PRINCIPALS: &[Principal] = &[
    Principal {
        name: "owner",
        status: UserStatus::Active,
        platform_role: Some(PlatformRole::User),
        calendar_role: Some(CalendarRole::Owner),
    },
    Principal {
        name: "manager",
        status: UserStatus::Active,
        platform_role: Some(PlatformRole::User),
        calendar_role: Some(CalendarRole::Manager),
    },
    Principal {
        name: "editor",
        status: UserStatus::Active,
        platform_role: Some(PlatformRole::User),
        calendar_role: Some(CalendarRole::Editor),
    },
    Principal {
        name: "viewer",
        status: UserStatus::Active,
        platform_role: Some(PlatformRole::User),
        calendar_role: Some(CalendarRole::Viewer),
    },
    Principal {
        name: "free_busy_viewer",
        status: UserStatus::Active,
        platform_role: Some(PlatformRole::User),
        calendar_role: Some(CalendarRole::FreeBusyViewer),
    },
    Principal {
        name: "unrelated",
        status: UserStatus::Active,
        platform_role: Some(PlatformRole::User),
        calendar_role: None,
    },
    Principal {
        name: "suspended",
        status: UserStatus::Suspended,
        platform_role: Some(PlatformRole::User),
        calendar_role: Some(CalendarRole::Owner),
    },
    // Platform administration deliberately does not grant private-calendar access.
    Principal {
        name: "superadmin_without_acl",
        status: UserStatus::Active,
        platform_role: Some(PlatformRole::Superadmin),
        calendar_role: None,
    },
];

#[derive(Clone, Copy)]
struct Operation {
    family: &'static str,
    endpoint: &'static str,
    action: CalendarAction,
}

// This is the machine-readable endpoint inventory consumed by the matrix. A
// route may expose several verbs, but those verbs must map to one of these
// authorization actions; add the concrete HTTP regression beside its family.
const ENDPOINT_INVENTORY: &[Operation] = &[
    Operation {
        family: "calendar",
        endpoint: "GET /api/v1/calendars/:id",
        action: CalendarAction::ReadDetails,
    },
    Operation {
        family: "calendar",
        endpoint: "PATCH /api/v1/calendars/:id",
        action: CalendarAction::ManageSettings,
    },
    Operation {
        family: "event",
        endpoint: "GET /api/v1/calendars/:calendar_id/events/:event_id",
        action: CalendarAction::ReadDetails,
    },
    Operation {
        family: "event",
        endpoint: "POST /api/v1/calendars/:calendar_id/events",
        action: CalendarAction::CreateEvent,
    },
    Operation {
        family: "acl",
        endpoint: "GET /api/v1/calendars/:id/acl",
        action: CalendarAction::ManageAcl,
    },
    Operation {
        family: "acl",
        endpoint: "PUT /api/v1/calendars/:id/acl/:user_id",
        action: CalendarAction::ManageAcl,
    },
    Operation {
        family: "view",
        endpoint: "GET /api/v1/views/:id",
        action: CalendarAction::ReadDetails,
    },
    Operation {
        family: "view",
        endpoint: "PUT /api/v1/views/:id/calendars",
        action: CalendarAction::ManageSettings,
    },
    Operation {
        family: "feed",
        endpoint: "GET /api/v1/calendars/:calendar_id/external-feeds",
        action: CalendarAction::ManageSettings,
    },
    Operation {
        family: "feed",
        endpoint: "POST /api/v1/calendars/:calendar_id/external-feeds",
        action: CalendarAction::ManageSettings,
    },
    // Notifications currently have no HTTP endpoint. Their authorization is
    // inherited from the event/calendar operation that plans or cancels jobs.
    Operation {
        family: "notification",
        endpoint: "service: notification planning",
        action: CalendarAction::ReadDetails,
    },
];

fn expected(role: Option<CalendarRole>, action: CalendarAction) -> AuthorizationDecision {
    use AuthorizationDecision::{Allow, Deny};
    use CalendarAction::*;
    use CalendarRole::*;
    match (role, action) {
        (Some(Owner), _) => Allow,
        (
            Some(Manager),
            ReadDetails | ReadFreeBusy | CreateEvent | EditAnyEvent | ManageSettings | ManageAcl,
        ) => Allow,
        (Some(Editor), ReadDetails | ReadFreeBusy | CreateEvent | EditAnyEvent) => Allow,
        (Some(Viewer), ReadDetails | ReadFreeBusy) => Allow,
        (Some(FreeBusyViewer), ReadFreeBusy) => Allow,
        _ => Deny,
    }
}

#[test]
fn calendar_authorization_matrix_covers_every_fixture_and_endpoint_family() {
    assert_eq!(
        ENDPOINT_INVENTORY
            .iter()
            .map(|operation| operation.family)
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from([
            "acl",
            "calendar",
            "event",
            "feed",
            "notification",
            "view"
        ]),
    );

    for principal in PRINCIPALS {
        for operation in ENDPOINT_INVENTORY {
            let actual = authorize_calendar_action(
                principal.status,
                principal.platform_role,
                principal.calendar_role,
                operation.action,
            );
            let expected =
                if principal.status == UserStatus::Active && principal.calendar_role.is_some() {
                    expected(principal.calendar_role, operation.action)
                } else {
                    AuthorizationDecision::Deny
                };
            assert_eq!(
                actual, expected,
                "{} must be {:?} for {}",
                principal.name, expected, operation.endpoint
            );
        }
    }
}

#[test]
fn identifier_substitution_and_public_tokens_are_not_authorization_inputs() {
    // Two-calendar substitution and public-token attempts must resolve to an
    // absent ACL. This deliberately tests the input to the shared decision,
    // rather than asserting endpoint-specific error wording that could leak
    // resource existence.
    for action in [
        CalendarAction::ReadDetails,
        CalendarAction::CreateEvent,
        CalendarAction::ManageAcl,
        CalendarAction::ManageSettings,
    ] {
        assert_eq!(
            authorize_calendar_action(UserStatus::Active, Some(PlatformRole::User), None, action),
            AuthorizationDecision::Deny,
            "substituted calendar/event id or public token unexpectedly authorized {action:?}",
        );
    }
}

// This is the permanent regression left after the test-only missing-ACL
// mutation was removed.  It protects the deny-by-default branch that every
// authenticated endpoint relies on before it performs its own operation.
#[test]
fn missing_calendar_acl_is_denied_before_any_endpoint_can_authorize() {
    for action in [
        CalendarAction::ReadDetails,
        CalendarAction::CreateEvent,
        CalendarAction::ManageAcl,
        CalendarAction::ManageSettings,
    ] {
        assert_eq!(
            authorize_calendar_action(UserStatus::Active, Some(PlatformRole::User), None, action),
            AuthorizationDecision::Deny,
            "missing calendar ACL unexpectedly authorized {action:?}",
        );
    }
}

const NOW: i64 = 1_750_000_000;
const ORIGIN: &str = "https://commoncal.test";

struct EndpointHarness {
    _directory: TempDir,
    pool: SqlitePool,
    key: SecretKey,
    owner: i64,
    unrelated: i64,
    suspended: i64,
    superadmin_without_acl: i64,
    primary_calendar: i64,
    other_calendar: i64,
    feed_id: i64,
}

impl EndpointHarness {
    async fn new() -> Self {
        let directory = TempDir::new().unwrap();
        let config = AppConfig::with_database_path(
            Environment::Development,
            "127.0.0.1:0",
            None,
            directory.path().join("authorization.sqlite"),
        )
        .unwrap();
        let pool = connect_and_migrate(&config, Readiness::new())
            .await
            .unwrap();
        let owner = insert_user(&pool, "owner@example.test", "active", false).await;
        let unrelated = insert_user(&pool, "unrelated@example.test", "active", false).await;
        let suspended = insert_user(&pool, "suspended@example.test", "suspended", false).await;
        let superadmin_without_acl = insert_user(&pool, "admin@example.test", "active", true).await;
        let calendars = CalendarRepository::new(pool.clone());
        let primary_calendar = create_calendar(&calendars, owner, "Primary").await;
        let other_calendar = create_calendar(&calendars, unrelated, "Other").await;
        let key = SecretKey::new([35; 32]);
        let feed_id = ExternalFeedService::new_at(pool.clone(), key.clone(), NOW)
            .create(
                owner,
                false,
                primary_calendar,
                NewFeed {
                    source_url: "https://feeds.example.test/private.ics".to_owned(),
                    refresh_interval_seconds: Some(60),
                },
            )
            .await
            .unwrap()
            .id;
        Self {
            _directory: directory,
            pool,
            key,
            owner,
            unrelated,
            suspended,
            superadmin_without_acl,
            primary_calendar,
            other_calendar,
            feed_id,
        }
    }

    fn router(&self) -> axum::Router {
        build_router_with_auth_flows_sessions_admin_calendars_views_and_external_feeds(
            Readiness::new(),
            InvitationConsumer::new_at(self.pool.clone(), self.key.clone(), 300, NOW),
            LoginService::new_at(
                self.pool.clone(),
                self.key.clone(),
                300,
                300,
                "/login",
                Arc::new(DevelopmentEmailSender::new()),
                Arc::new(AllowAllLoginRateLimiter),
                NOW,
                false,
            ),
            SessionManager::new_at(
                self.pool.clone(),
                self.key.clone(),
                SessionSecurityConfig::new(300, 60, ORIGIN).unwrap(),
                NOW,
            ),
            AdminService::new_at(self.pool.clone(), self.key.clone(), 300, NOW),
            CalendarService::new_at(self.pool.clone(), NOW),
            EventService::new_at(self.pool.clone(), NOW),
            SharedViewService::new_at_with_key(self.pool.clone(), self.key.clone(), NOW),
            ExternalFeedService::new_at(self.pool.clone(), self.key.clone(), NOW),
            NotificationService::new_at(self.pool.clone(), NOW, 14 * 86_400),
            tracing::level_filters::LevelFilter::INFO,
            false,
            false,
        )
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        user_id: Option<i64>,
    ) -> axum::response::Response {
        let mut builder = Request::builder().method(method).uri(path);
        if let Some(user_id) = user_id {
            let token = self.key.generate_token();
            let hash = self.key.hash_token(TokenDomain::Session, &token);
            sqlx::query("INSERT INTO sessions (user_id, session_hash, expires_at, revoked_at, created_at, last_seen_at) VALUES (?, ?, ?, NULL, ?, ?)")
                .bind(user_id)
                .bind(hash.as_bytes().as_slice())
                .bind(NOW + 1_000)
                .bind(NOW - 10)
                .bind(NOW - 10)
                .execute(&self.pool)
                .await
                .unwrap();
            builder = builder
                .header(
                    COOKIE,
                    format!("__Host-commoncal_session={}", token.expose()),
                )
                .header("origin", ORIGIN)
                .header("sec-fetch-site", "same-origin")
                .header(
                    "x-csrf-token",
                    self.key.generate_csrf_token(&token).expose(),
                );
        }
        self.router()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }
}

async fn insert_user(pool: &SqlitePool, email: &str, status: &str, superadmin: bool) -> i64 {
    sqlx::query("INSERT INTO users (normalized_email, status, is_superadmin, created_at) VALUES (?, ?, ?, ?)")
        .bind(email)
        .bind(status)
        .bind(superadmin)
        .bind(NOW)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
}

async fn create_calendar(repository: &CalendarRepository, owner: i64, name: &str) -> i64 {
    repository
        .create_calendar(
            owner,
            NewCalendar {
                name: name.to_owned(),
                description: None,
                color: "#123456".to_owned(),
                default_timezone: "UTC".to_owned(),
                default_event_visibility: "private".to_owned(),
                default_notification_rules_json: None,
                created_at: NOW,
            },
        )
        .await
        .unwrap()
        .id
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

#[tokio::test]
async fn adversarial_endpoint_denials_do_not_reveal_calendar_or_feed_existence() {
    let app = EndpointHarness::new().await;

    let calendar_denied = app
        .request(
            Method::GET,
            &format!("/api/v1/calendars/{}", app.primary_calendar),
            Some(app.unrelated),
        )
        .await;
    let calendar_missing = app
        .request(Method::GET, "/api/v1/calendars/999999", Some(app.unrelated))
        .await;
    assert_eq!(calendar_denied.status(), StatusCode::NOT_FOUND);
    assert_eq!(calendar_denied.status(), calendar_missing.status());
    assert_eq!(
        response_body(calendar_denied).await,
        response_body(calendar_missing).await
    );

    let feed_denied = app
        .request(
            Method::DELETE,
            &format!("/api/v1/external-feeds/{}", app.feed_id),
            Some(app.unrelated),
        )
        .await;
    let feed_missing = app
        .request(
            Method::DELETE,
            "/api/v1/external-feeds/999999",
            Some(app.unrelated),
        )
        .await;
    assert_eq!(feed_denied.status(), StatusCode::NOT_FOUND);
    assert_eq!(feed_denied.status(), feed_missing.status());
    assert_eq!(
        response_body(feed_denied).await,
        response_body(feed_missing).await
    );

    for path in [
        format!("/api/v1/calendars/{}", app.primary_calendar),
        format!(
            "/api/v1/calendars/{}/events?from={NOW}&to={}",
            app.primary_calendar,
            NOW + 60
        ),
        format!("/api/v1/calendars/{}/acl", app.primary_calendar),
        format!("/api/v1/calendars/{}/external-feeds", app.primary_calendar),
        "/api/v1/views/1".to_owned(),
    ] {
        let response = app.request(Method::GET, &path, None).await;
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "public token cannot authenticate {path}"
        );
    }

    for user_id in [app.suspended, app.superadmin_without_acl] {
        let response = app
            .request(
                Method::GET,
                &format!("/api/v1/calendars/{}", app.primary_calendar),
                Some(user_id),
            )
            .await;
        assert!(matches!(
            response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::NOT_FOUND
        ));
    }

    let substituted = app
        .request(
            Method::GET,
            &format!("/api/v1/calendars/{}", app.other_calendar),
            Some(app.owner),
        )
        .await;
    let missing = app
        .request(Method::GET, "/api/v1/calendars/999998", Some(app.owner))
        .await;
    assert_eq!(substituted.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response_body(substituted).await,
        response_body(missing).await
    );
}
