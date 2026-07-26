use axum::{
    Extension, Json, Router,
    body::Body,
    extract::{ConnectInfo, Path, Query, State},
    http::{
        HeaderMap, HeaderName, HeaderValue, Request, StatusCode,
        header::{COOKIE, SET_COOKIE},
    },
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};
use tracing::Level;

use crate::{
    admin::{AdminError, AdminService, InviteUser},
    authorization::{
        AuthorizationDecision, CalendarRole, PlatformAction, PlatformRole,
        authorize_platform_action,
    },
    calendar::{CalendarService, CalendarServiceError, CalendarUpdate, NewCalendar},
    event::{
        AllDayOccurrenceChange, EventChange, EventMutation, EventRange, EventService,
        EventServiceError, EventStatus, EventTiming, OccurrenceChange,
    },
    identity::UserStatus,
    invitations::{ActiveUser, ConsumeInvitation, ConsumeInvitationError, InvitationConsumer},
    login::{
        ConsumeLoginLink, ConsumeLoginLinkError, LoginFlow, RequestLoginLink, RequestLoginLinkError,
    },
    security::SessionCookieBuilder,
    sessions::{AuthenticatedSession, SessionError, SessionManager},
    shared_view::{
        PublicViewConfiguration, PublicViewProjection, SharedViewCalendarInput, SharedViewError,
        SharedViewService,
    },
};

static REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");
const SESSION_COOKIE_NAME: &str = "__Host-commoncal_session";

pub fn build_router() -> Router {
    let readiness = Readiness::new();
    readiness.mark_ready();
    build_router_with_readiness(readiness)
}

pub fn build_router_with_readiness(readiness: Readiness) -> Router {
    build_application_router(ApplicationState {
        readiness,
        invitation_consumer: None,
        login_flow: None,
        session_manager: None,
        admin_service: None,
        calendar_service: None,
        event_service: None,
        shared_view_service: None,
    })
}

pub fn build_router_with_invitation_consumer(
    readiness: Readiness,
    invitation_consumer: InvitationConsumer,
) -> Router {
    build_application_router(ApplicationState {
        readiness,
        invitation_consumer: Some(invitation_consumer),
        login_flow: None,
        session_manager: None,
        admin_service: None,
        calendar_service: None,
        event_service: None,
        shared_view_service: None,
    })
}

pub fn build_router_with_login_service<L>(readiness: Readiness, login_flow: L) -> Router
where
    L: LoginFlow + 'static,
{
    build_application_router(ApplicationState {
        readiness,
        invitation_consumer: None,
        login_flow: Some(Arc::new(login_flow)),
        session_manager: None,
        admin_service: None,
        calendar_service: None,
        event_service: None,
        shared_view_service: None,
    })
}

pub fn build_router_with_auth_flows<L>(
    readiness: Readiness,
    invitation_consumer: InvitationConsumer,
    login_flow: L,
) -> Router
where
    L: LoginFlow + 'static,
{
    build_application_router(ApplicationState {
        readiness,
        invitation_consumer: Some(invitation_consumer),
        login_flow: Some(Arc::new(login_flow)),
        session_manager: None,
        admin_service: None,
        calendar_service: None,
        event_service: None,
        shared_view_service: None,
    })
}

pub fn build_router_with_sessions(readiness: Readiness, session_manager: SessionManager) -> Router {
    build_application_router(ApplicationState {
        readiness,
        invitation_consumer: None,
        login_flow: None,
        session_manager: Some(session_manager),
        admin_service: None,
        calendar_service: None,
        event_service: None,
        shared_view_service: None,
    })
}

pub fn build_router_with_auth_flows_and_sessions<L>(
    readiness: Readiness,
    invitation_consumer: InvitationConsumer,
    login_flow: L,
    session_manager: SessionManager,
) -> Router
where
    L: LoginFlow + 'static,
{
    build_application_router(ApplicationState {
        readiness,
        invitation_consumer: Some(invitation_consumer),
        login_flow: Some(Arc::new(login_flow)),
        session_manager: Some(session_manager),
        admin_service: None,
        calendar_service: None,
        event_service: None,
        shared_view_service: None,
    })
}

pub fn build_router_with_admin(
    readiness: Readiness,
    session_manager: SessionManager,
    admin_service: AdminService,
) -> Router {
    build_application_router(ApplicationState {
        readiness,
        invitation_consumer: None,
        login_flow: None,
        session_manager: Some(session_manager),
        admin_service: Some(admin_service),
        calendar_service: None,
        event_service: None,
        shared_view_service: None,
    })
}

pub fn build_router_with_auth_flows_sessions_and_admin<L>(
    readiness: Readiness,
    invitation_consumer: InvitationConsumer,
    login_flow: L,
    session_manager: SessionManager,
    admin_service: AdminService,
) -> Router
where
    L: LoginFlow + 'static,
{
    build_application_router(ApplicationState {
        readiness,
        invitation_consumer: Some(invitation_consumer),
        login_flow: Some(Arc::new(login_flow)),
        session_manager: Some(session_manager),
        admin_service: Some(admin_service),
        calendar_service: None,
        event_service: None,
        shared_view_service: None,
    })
}

pub fn build_router_with_calendars(
    readiness: Readiness,
    session_manager: SessionManager,
    calendar_service: CalendarService,
) -> Router {
    build_application_router(ApplicationState {
        readiness,
        invitation_consumer: None,
        login_flow: None,
        session_manager: Some(session_manager),
        admin_service: None,
        calendar_service: Some(calendar_service),
        event_service: None,
        shared_view_service: None,
    })
}

pub fn build_router_with_auth_flows_sessions_admin_and_calendars<L>(
    readiness: Readiness,
    invitation_consumer: InvitationConsumer,
    login_flow: L,
    session_manager: SessionManager,
    admin_service: AdminService,
    calendar_service: CalendarService,
    event_service: EventService,
) -> Router
where
    L: LoginFlow + 'static,
{
    build_application_router(ApplicationState {
        readiness,
        invitation_consumer: Some(invitation_consumer),
        login_flow: Some(Arc::new(login_flow)),
        session_manager: Some(session_manager),
        admin_service: Some(admin_service),
        calendar_service: Some(calendar_service),
        event_service: Some(event_service),
        shared_view_service: None,
    })
}

pub fn build_router_with_calendars_and_events(
    readiness: Readiness,
    session_manager: SessionManager,
    calendar_service: CalendarService,
    event_service: EventService,
) -> Router {
    build_application_router(ApplicationState {
        readiness,
        invitation_consumer: None,
        login_flow: None,
        session_manager: Some(session_manager),
        admin_service: None,
        calendar_service: Some(calendar_service),
        event_service: Some(event_service),
        shared_view_service: None,
    })
}

pub fn build_router_with_calendars_events_and_views(
    readiness: Readiness,
    session_manager: SessionManager,
    calendar_service: CalendarService,
    event_service: EventService,
    shared_view_service: SharedViewService,
) -> Router {
    build_application_router(ApplicationState {
        readiness,
        invitation_consumer: None,
        login_flow: None,
        session_manager: Some(session_manager),
        admin_service: None,
        calendar_service: Some(calendar_service),
        event_service: Some(event_service),
        shared_view_service: Some(shared_view_service),
    })
}

#[allow(clippy::too_many_arguments)]
pub fn build_router_with_auth_flows_sessions_admin_calendars_and_views<L>(
    readiness: Readiness,
    invitation_consumer: InvitationConsumer,
    login_flow: L,
    session_manager: SessionManager,
    admin_service: AdminService,
    calendar_service: CalendarService,
    event_service: EventService,
    shared_view_service: SharedViewService,
) -> Router
where
    L: LoginFlow + 'static,
{
    build_application_router(ApplicationState {
        readiness,
        invitation_consumer: Some(invitation_consumer),
        login_flow: Some(Arc::new(login_flow)),
        session_manager: Some(session_manager),
        admin_service: Some(admin_service),
        calendar_service: Some(calendar_service),
        event_service: Some(event_service),
        shared_view_service: Some(shared_view_service),
    })
}

fn build_application_router(state: ApplicationState) -> Router {
    let mut router = Router::new()
        .route("/health/live", get(health_live))
        .route("/health/ready", get(health_ready))
        .route("/api/v1/auth/invitations/consume", post(consume_invitation))
        .route("/api/v1/auth/login-links", post(request_login_link))
        .route("/api/v1/auth/login-links/consume", post(consume_login_link))
        .fallback(not_found);
    if state.shared_view_service.is_some() && state.event_service.is_some() {
        let public = Router::new()
            .route("/api/v1/public/views/:token", get(read_public_view))
            .route(
                "/api/v1/public/views/:token/events",
                get(list_public_view_events),
            )
            .layer(middleware::from_fn(public_response_headers));
        router = router.merge(public);
    }
    if let Some(manager) = state.session_manager.clone() {
        let mut protected = Router::new()
            .route(
                "/api/v1/auth/session",
                get(inspect_session).delete(logout_current),
            )
            .route("/api/v1/auth/sessions", axum::routing::delete(logout_all));
        if state.admin_service.is_some() {
            protected = protected
                .route("/api/v1/admin/users", get(list_users))
                .route("/api/v1/admin/invitations", post(invite_user))
                .route("/api/v1/admin/invitations/:id", delete(revoke_invitation))
                .route(
                    "/api/v1/admin/invitations/:id/resend",
                    post(resend_invitation),
                )
                .route("/api/v1/admin/users/:id/suspend", post(suspend_user))
                .route("/api/v1/admin/users/:id/reactivate", post(reactivate_user))
                .route("/api/v1/admin/users/:id/promote", post(promote_user))
                .route("/api/v1/admin/users/:id/demote", post(demote_user))
                .route(
                    "/api/v1/admin/users/:id/revoke-sessions",
                    post(revoke_user_sessions),
                );
        }
        if state.calendar_service.is_some() {
            protected = protected
                .route(
                    "/api/v1/calendars",
                    get(list_calendars).post(create_calendar),
                )
                .route(
                    "/api/v1/calendars/:id",
                    get(read_calendar)
                        .patch(update_calendar)
                        .delete(delete_calendar),
                )
                .route("/api/v1/calendars/:id/archive", post(archive_calendar))
                .route("/api/v1/calendars/:id/restore", post(restore_calendar))
                .route("/api/v1/calendars/:id/acl", get(list_calendar_acl))
                .route(
                    "/api/v1/calendars/:id/acl/:user_id",
                    axum::routing::put(set_calendar_acl).delete(revoke_calendar_acl),
                )
                .route(
                    "/api/v1/calendars/:id/transfer",
                    post(transfer_calendar_ownership),
                );
        }
        if state.event_service.is_some() {
            protected = protected
                .route(
                    "/api/v1/calendars/:calendar_id/events",
                    get(list_events).post(create_event),
                )
                .route(
                    "/api/v1/calendars/:calendar_id/events/:event_id",
                    get(read_event).patch(update_event).delete(delete_event),
                )
                .route(
                    "/api/v1/calendars/:calendar_id/events/:event_id/occurrences/:recurrence_id",
                    axum::routing::patch(update_event_occurrence)
                        .delete(delete_event_occurrence),
                )
                .route(
                    "/api/v1/calendars/:calendar_id/events/:event_id/occurrences/:recurrence_id/following",
                    axum::routing::patch(update_this_and_following),
                );
        }
        if state.shared_view_service.is_some() {
            protected = protected
                .route(
                    "/api/v1/views",
                    get(list_shared_views).post(create_shared_view),
                )
                .route(
                    "/api/v1/views/:id",
                    get(read_shared_view)
                        .patch(update_shared_view)
                        .delete(delete_shared_view),
                )
                .route(
                    "/api/v1/views/:id/calendars",
                    axum::routing::put(replace_shared_view_calendars),
                )
                .route("/api/v1/views/:id/events", get(list_shared_view_events))
                .route(
                    "/api/v1/views/:id/publication",
                    post(create_publication).patch(configure_publication),
                )
                .route(
                    "/api/v1/views/:id/publication/rotate",
                    post(rotate_publication),
                )
                .route(
                    "/api/v1/views/:id/publication/revoke",
                    post(revoke_publication),
                );
        }
        let protected = protected.route_layer(middleware::from_fn_with_state(
            manager,
            authenticated_session,
        ));
        router = router.merge(protected);
    }
    router
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &Request<_>| {
                tracing::span!(
                    Level::INFO,
                    "http_request",
                    method = %request.method(),
                    path = safe_request_path(request.uri().path()),
                    request_id = request
                        .headers()
                        .get(&REQUEST_ID_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("")
                )
            }),
        )
        .layer(PropagateRequestIdLayer::new(REQUEST_ID_HEADER.clone()))
        .layer(SetRequestIdLayer::new(
            REQUEST_ID_HEADER.clone(),
            MakeRequestUuid,
        ))
        .with_state(state)
}

fn safe_request_path(path: &str) -> &str {
    if path.starts_with("/api/v1/public/views/") {
        "/api/v1/public/views/[REDACTED]"
    } else {
        path
    }
}

async fn list_calendars(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
) -> Result<impl IntoResponse, ApiError> {
    let calendars = state
        .calendar_service
        .ok_or_else(ApiError::service_unavailable)?
        .list(session.user.id, session.user.is_superadmin)
        .await
        .map_err(map_calendar_error)?;
    Ok(Json(calendars))
}

async fn create_calendar(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Json(request): Json<CalendarMutationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let calendar = state
        .calendar_service
        .ok_or_else(ApiError::service_unavailable)?
        .create(
            session.user.id,
            NewCalendar {
                name: request.name,
                description: request.description,
                color: request.color,
                default_timezone: request.default_timezone,
                default_event_visibility: request.default_event_visibility,
                default_notification_rules_json: request.default_notification_rules_json,
                created_at: 0,
            },
        )
        .await
        .map_err(map_calendar_error)?;
    Ok((StatusCode::CREATED, Json(calendar)))
}

async fn read_calendar(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(calendar_id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    let calendar = state
        .calendar_service
        .ok_or_else(ApiError::service_unavailable)?
        .get(session.user.id, session.user.is_superadmin, calendar_id)
        .await
        .map_err(map_calendar_error)?;
    Ok(Json(calendar))
}

async fn update_calendar(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(calendar_id): Path<i64>,
    Json(request): Json<CalendarUpdateRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let calendar = state
        .calendar_service
        .ok_or_else(ApiError::service_unavailable)?
        .update(
            session.user.id,
            session.user.is_superadmin,
            calendar_id,
            request.version,
            CalendarUpdate {
                name: request.settings.name,
                description: request.settings.description,
                color: request.settings.color,
                default_timezone: request.settings.default_timezone,
                default_event_visibility: request.settings.default_event_visibility,
                default_notification_rules_json: request.settings.default_notification_rules_json,
                archived: false,
                updated_at: 0,
            },
        )
        .await
        .map_err(map_calendar_error)?;
    Ok(Json(calendar))
}

async fn archive_calendar(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(calendar_id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    set_calendar_archived(state, session, calendar_id, true).await
}

async fn restore_calendar(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(calendar_id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    set_calendar_archived(state, session, calendar_id, false).await
}

async fn set_calendar_archived(
    state: ApplicationState,
    session: AuthenticatedSession,
    calendar_id: i64,
    archived: bool,
) -> Result<Json<crate::calendar::CalendarProjection>, ApiError> {
    let calendar = state
        .calendar_service
        .ok_or_else(ApiError::service_unavailable)?
        .set_archived(
            session.user.id,
            session.user.is_superadmin,
            calendar_id,
            archived,
        )
        .await
        .map_err(map_calendar_error)?;
    Ok(Json(calendar))
}

async fn delete_calendar(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(calendar_id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    state
        .calendar_service
        .ok_or_else(ApiError::service_unavailable)?
        .delete(session.user.id, session.user.is_superadmin, calendar_id)
        .await
        .map_err(map_calendar_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_calendar_acl(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(calendar_id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    let entries = state
        .calendar_service
        .ok_or_else(ApiError::service_unavailable)?
        .list_acl(session.user.id, session.user.is_superadmin, calendar_id)
        .await
        .map_err(map_calendar_error)?;
    Ok(Json(entries))
}

async fn set_calendar_acl(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path((calendar_id, user_id)): Path<(i64, i64)>,
    Json(request): Json<CalendarAclRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let role = request
        .role
        .parse::<CalendarRole>()
        .map_err(|_| ApiError::bad_request())?;
    let entry = state
        .calendar_service
        .ok_or_else(ApiError::service_unavailable)?
        .set_acl(
            session.user.id,
            session.user.is_superadmin,
            calendar_id,
            user_id,
            role,
        )
        .await
        .map_err(map_calendar_error)?;
    Ok(Json(entry))
}

async fn revoke_calendar_acl(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path((calendar_id, user_id)): Path<(i64, i64)>,
) -> Result<StatusCode, ApiError> {
    state
        .calendar_service
        .ok_or_else(ApiError::service_unavailable)?
        .revoke_acl(
            session.user.id,
            session.user.is_superadmin,
            calendar_id,
            user_id,
        )
        .await
        .map_err(map_calendar_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn transfer_calendar_ownership(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(calendar_id): Path<i64>,
    Json(request): Json<TransferOwnershipRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let calendar = state
        .calendar_service
        .ok_or_else(ApiError::service_unavailable)?
        .transfer_ownership(
            session.user.id,
            session.user.is_superadmin,
            calendar_id,
            request.new_owner_user_id,
            request.version,
        )
        .await
        .map_err(map_calendar_error)?;
    Ok(Json(calendar))
}

async fn list_events(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(calendar_id): Path<i64>,
    Query(query): Query<EventListQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let from = query.from.ok_or_else(ApiError::bad_request)?;
    let to = query.to.ok_or_else(ApiError::bad_request)?;
    let events = state
        .event_service
        .ok_or_else(ApiError::service_unavailable)?
        .list(
            session.user.id,
            session.user.is_superadmin,
            calendar_id,
            EventRange {
                start_utc: from,
                end_utc: to,
                start_date: unix_date(from),
                end_date: unix_exclusive_end_date(to),
            },
        )
        .await
        .map_err(map_event_error)?;
    Ok(Json(events))
}

async fn list_shared_views(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
) -> Result<impl IntoResponse, ApiError> {
    let views = state
        .shared_view_service
        .ok_or_else(ApiError::service_unavailable)?
        .list(session.user.id)
        .await
        .map_err(map_shared_view_error)?;
    Ok(Json(views))
}

async fn create_shared_view(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Json(request): Json<SharedViewMutationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let view = state
        .shared_view_service
        .ok_or_else(ApiError::service_unavailable)?
        .create(session.user.id, request.name)
        .await
        .map_err(map_shared_view_error)?;
    Ok((StatusCode::CREATED, Json(view)))
}

async fn read_shared_view(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(view_id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    let view = state
        .shared_view_service
        .ok_or_else(ApiError::service_unavailable)?
        .get(session.user.id, view_id)
        .await
        .map_err(map_shared_view_error)?;
    Ok(Json(view))
}

async fn update_shared_view(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(view_id): Path<i64>,
    Json(request): Json<SharedViewMutationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let view = state
        .shared_view_service
        .ok_or_else(ApiError::service_unavailable)?
        .update(session.user.id, view_id, request.name)
        .await
        .map_err(map_shared_view_error)?;
    Ok(Json(view))
}

async fn delete_shared_view(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(view_id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    state
        .shared_view_service
        .ok_or_else(ApiError::service_unavailable)?
        .delete(session.user.id, view_id)
        .await
        .map_err(map_shared_view_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn replace_shared_view_calendars(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(view_id): Path<i64>,
    Json(request): Json<SharedViewCalendarsRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let calendars = request
        .calendars
        .into_iter()
        .map(|calendar| SharedViewCalendarInput {
            calendar_id: calendar.calendar_id,
            position: calendar.position,
            color: calendar.color,
        })
        .collect();
    let view = state
        .shared_view_service
        .ok_or_else(ApiError::service_unavailable)?
        .replace_calendars(session.user.id, view_id, calendars)
        .await
        .map_err(map_shared_view_error)?;
    Ok(Json(view))
}

async fn list_shared_view_events(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(view_id): Path<i64>,
    Query(query): Query<EventListQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let from = query.from.ok_or_else(ApiError::bad_request)?;
    let to = query.to.ok_or_else(ApiError::bad_request)?;
    let event_service = state
        .event_service
        .as_ref()
        .ok_or_else(ApiError::service_unavailable)?;
    let events = state
        .shared_view_service
        .ok_or_else(ApiError::service_unavailable)?
        .events(
            event_service,
            session.user.id,
            session.user.is_superadmin,
            view_id,
            EventRange {
                start_utc: from,
                end_utc: to,
                start_date: unix_date(from),
                end_date: unix_exclusive_end_date(to),
            },
        )
        .await
        .map_err(map_shared_view_error)?;
    Ok(Json(events))
}

async fn create_publication(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(view_id): Path<i64>,
    Json(request): Json<PublicViewConfigurationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let publication = state
        .shared_view_service
        .ok_or_else(ApiError::service_unavailable)?
        .create_publication(session.user.id, view_id, request.try_into()?)
        .await
        .map_err(map_shared_view_error)?;
    Ok((StatusCode::CREATED, Json(publication)))
}

async fn configure_publication(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(view_id): Path<i64>,
    Json(request): Json<PublicViewConfigurationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let publication = state
        .shared_view_service
        .ok_or_else(ApiError::service_unavailable)?
        .configure_publication(session.user.id, view_id, request.try_into()?)
        .await
        .map_err(map_shared_view_error)?;
    Ok(Json(publication))
}

async fn rotate_publication(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(view_id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    let publication = state
        .shared_view_service
        .ok_or_else(ApiError::service_unavailable)?
        .rotate_publication(session.user.id, view_id)
        .await
        .map_err(map_shared_view_error)?;
    Ok(Json(publication))
}

async fn revoke_publication(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(view_id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    state
        .shared_view_service
        .ok_or_else(ApiError::service_unavailable)?
        .revoke_publication(session.user.id, view_id)
        .await
        .map_err(map_shared_view_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn read_public_view(
    State(state): State<ApplicationState>,
    Path(token): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let metadata = state
        .shared_view_service
        .ok_or_else(ApiError::service_unavailable)?
        .public_metadata(&token)
        .await
        .map_err(map_shared_view_error)?;
    Ok(Json(metadata))
}

async fn list_public_view_events(
    State(state): State<ApplicationState>,
    Path(token): Path<String>,
    Query(query): Query<EventListQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let from = query.from.ok_or_else(ApiError::bad_request)?;
    let to = query.to.ok_or_else(ApiError::bad_request)?;
    let event_service = state
        .event_service
        .as_ref()
        .ok_or_else(ApiError::service_unavailable)?;
    let events = state
        .shared_view_service
        .ok_or_else(ApiError::service_unavailable)?
        .public_events(
            event_service,
            &token,
            EventRange {
                start_utc: from,
                end_utc: to,
                start_date: unix_date(from),
                end_date: unix_exclusive_end_date(to),
            },
        )
        .await
        .map_err(map_shared_view_error)?;
    Ok(Json(events))
}

async fn public_response_headers(request: Request<Body>, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    headers.insert(
        axum::http::header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'"),
    );
    headers.insert(
        axum::http::header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        axum::http::header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        HeaderName::from_static("x-robots-tag"),
        HeaderValue::from_static("noindex, nofollow"),
    );
    headers.remove(SET_COOKIE);
    response
}

fn map_shared_view_error(error: SharedViewError) -> ApiError {
    match error {
        SharedViewError::Conflict => ApiError::conflict(),
        SharedViewError::InvalidInput => ApiError::bad_request(),
        SharedViewError::NotFound => ApiError::not_found(),
        SharedViewError::Event(error) => map_event_error(error),
        SharedViewError::Database(_) => {
            tracing::error!(error_code = "shared_view_operation_failed");
            ApiError::internal()
        }
    }
}

async fn create_event(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(calendar_id): Path<i64>,
    Json(request): Json<EventMutationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let recurrence_rule = request.recurrence_rule.clone();
    let service = state
        .event_service
        .ok_or_else(ApiError::service_unavailable)?;
    let mutation = request.into_mutation()?;
    let event = if let Some(recurrence_rule) = recurrence_rule {
        service
            .create_recurring(
                session.user.id,
                session.user.is_superadmin,
                calendar_id,
                mutation,
                recurrence_rule,
            )
            .await
    } else {
        service
            .create(
                session.user.id,
                session.user.is_superadmin,
                calendar_id,
                mutation,
            )
            .await
    }
    .map_err(map_event_error)?;
    Ok((StatusCode::CREATED, Json(event)))
}

async fn read_event(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path((calendar_id, event_id)): Path<(i64, i64)>,
) -> Result<impl IntoResponse, ApiError> {
    let event = state
        .event_service
        .ok_or_else(ApiError::service_unavailable)?
        .get(
            session.user.id,
            session.user.is_superadmin,
            calendar_id,
            event_id,
        )
        .await
        .map_err(map_event_error)?;
    Ok(Json(event))
}

async fn update_event(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path((calendar_id, event_id)): Path<(i64, i64)>,
    Json(request): Json<EventUpdateRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let event = state
        .event_service
        .ok_or_else(ApiError::service_unavailable)?
        .update(
            session.user.id,
            session.user.is_superadmin,
            calendar_id,
            event_id,
            EventChange {
                expected_version: request.version,
                target_calendar_id: request.calendar_id,
                event: request.event.into_mutation()?,
            },
        )
        .await
        .map_err(map_event_error)?;
    Ok(Json(event))
}

async fn delete_event(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path((calendar_id, event_id)): Path<(i64, i64)>,
) -> Result<StatusCode, ApiError> {
    state
        .event_service
        .ok_or_else(ApiError::service_unavailable)?
        .delete(
            session.user.id,
            session.user.is_superadmin,
            calendar_id,
            event_id,
        )
        .await
        .map_err(map_event_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn update_event_occurrence(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path((calendar_id, event_id, recurrence_id)): Path<(i64, i64, String)>,
    Json(request): Json<OccurrenceUpdateRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let service = state
        .event_service
        .ok_or_else(ApiError::service_unavailable)?;
    let event_mutation = request.event.into_mutation()?;
    let event = if let Ok(recurrence_id) = recurrence_id.parse::<i64>() {
        service
            .update_occurrence(
                session.user.id,
                session.user.is_superadmin,
                calendar_id,
                event_id,
                OccurrenceChange {
                    recurrence_id,
                    expected_version: request.version,
                    event: event_mutation,
                },
            )
            .await
    } else {
        service
            .update_all_day_occurrence(
                session.user.id,
                session.user.is_superadmin,
                calendar_id,
                event_id,
                AllDayOccurrenceChange {
                    recurrence_date: recurrence_id,
                    expected_version: request.version,
                    event: event_mutation,
                },
            )
            .await
    }
    .map_err(map_event_error)?;
    Ok(Json(event))
}

async fn delete_event_occurrence(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path((calendar_id, event_id, recurrence_id)): Path<(i64, i64, String)>,
    Json(request): Json<OccurrenceVersionRequest>,
) -> Result<StatusCode, ApiError> {
    let service = state
        .event_service
        .ok_or_else(ApiError::service_unavailable)?;
    if let Ok(recurrence_id) = recurrence_id.parse::<i64>() {
        service
            .delete_occurrence(
                session.user.id,
                session.user.is_superadmin,
                calendar_id,
                event_id,
                recurrence_id,
                request.version,
            )
            .await
    } else {
        service
            .delete_all_day_occurrence(
                session.user.id,
                session.user.is_superadmin,
                calendar_id,
                event_id,
                &recurrence_id,
                request.version,
            )
            .await
    }
    .map_err(map_event_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn update_this_and_following(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path((calendar_id, event_id, recurrence_id)): Path<(i64, i64, String)>,
) -> Result<StatusCode, ApiError> {
    let service = state
        .event_service
        .ok_or_else(ApiError::service_unavailable)?;
    if let Ok(recurrence_id) = recurrence_id.parse::<i64>() {
        service
            .update_this_and_following(
                session.user.id,
                session.user.is_superadmin,
                calendar_id,
                event_id,
                recurrence_id,
            )
            .await
    } else {
        service
            .update_all_day_this_and_following(
                session.user.id,
                session.user.is_superadmin,
                calendar_id,
                event_id,
                &recurrence_id,
            )
            .await
    }
    .map_err(map_event_error)?;
    Ok(StatusCode::NO_CONTENT)
}

fn map_event_error(error: EventServiceError) -> ApiError {
    match error {
        EventServiceError::InvalidInput => ApiError::bad_request(),
        EventServiceError::NotFound => ApiError::not_found(),
        EventServiceError::NotSupported => ApiError::not_implemented(),
        EventServiceError::Conflict { current_version } => {
            ApiError::version_conflict(current_version)
        }
        EventServiceError::Database(_) => {
            tracing::error!(error_code = "event_operation_failed");
            ApiError::internal()
        }
    }
}

fn unix_date(timestamp: i64) -> String {
    let days = timestamp.div_euclid(86_400);
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

fn unix_exclusive_end_date(timestamp: i64) -> String {
    let end_day = timestamp.saturating_sub(1).div_euclid(86_400) + 1;
    unix_date(end_day.saturating_mul(86_400))
}

fn map_calendar_error(error: CalendarServiceError) -> ApiError {
    match error {
        CalendarServiceError::InvalidInput => ApiError::bad_request(),
        CalendarServiceError::NotFound => ApiError::not_found(),
        CalendarServiceError::Conflict { current_version } => {
            ApiError::version_conflict(current_version)
        }
        CalendarServiceError::OperationConflict => ApiError::conflict(),
        CalendarServiceError::Database(_) => {
            tracing::error!(error_code = "calendar_operation_failed");
            ApiError::internal()
        }
    }
}

fn require_superadmin(session: &AuthenticatedSession) -> Result<(), ApiError> {
    let role = session
        .user
        .is_superadmin
        .then_some(PlatformRole::Superadmin)
        .or(Some(PlatformRole::User));
    match authorize_platform_action(UserStatus::Active, role, PlatformAction::ManageUsers) {
        AuthorizationDecision::Allow => Ok(()),
        AuthorizationDecision::Deny => Err(ApiError::forbidden()),
    }
}

async fn list_users(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Query(query): Query<UserListQuery>,
) -> Result<impl IntoResponse, ApiError> {
    require_superadmin(&session)?;
    let service = state
        .admin_service
        .ok_or_else(ApiError::service_unavailable)?;
    let page = query.page.unwrap_or(1);
    let per_page = query.per_page.unwrap_or(20);
    let users = service
        .list_users(query.status.as_deref(), page, per_page)
        .await
        .map_err(map_admin_error)?;
    Ok(Json(users))
}

async fn invite_user(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Json(request): Json<InviteUserRequest>,
) -> Result<impl IntoResponse, ApiError> {
    require_superadmin(&session)?;
    let service = state
        .admin_service
        .ok_or_else(ApiError::service_unavailable)?;
    let invitation = service
        .invite(
            session.user.id,
            InviteUser {
                email: request.email,
                display_name: request.display_name,
            },
        )
        .await
        .map_err(map_admin_error)?;
    Ok((
        StatusCode::CREATED,
        Json(InvitationResponse {
            id: invitation.invitation_id,
        }),
    ))
}

async fn revoke_invitation(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(invitation_id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    require_superadmin(&session)?;
    state
        .admin_service
        .ok_or_else(ApiError::service_unavailable)?
        .revoke_invitation(session.user.id, invitation_id)
        .await
        .map_err(map_admin_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn resend_invitation(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(invitation_id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    require_superadmin(&session)?;
    state
        .admin_service
        .ok_or_else(ApiError::service_unavailable)?
        .resend_invitation(session.user.id, invitation_id)
        .await
        .map_err(map_admin_error)?;
    Ok(StatusCode::NO_CONTENT)
}

macro_rules! user_mutation_handler {
    ($name:ident, $method:ident) => {
        async fn $name(
            State(state): State<ApplicationState>,
            Extension(session): Extension<AuthenticatedSession>,
            Path(user_id): Path<i64>,
        ) -> Result<StatusCode, ApiError> {
            require_superadmin(&session)?;
            state
                .admin_service
                .ok_or_else(ApiError::service_unavailable)?
                .$method(session.user.id, user_id)
                .await
                .map_err(map_admin_error)?;
            Ok(StatusCode::NO_CONTENT)
        }
    };
}

user_mutation_handler!(suspend_user, suspend_user);
user_mutation_handler!(reactivate_user, reactivate_user);
user_mutation_handler!(promote_user, promote_user);
user_mutation_handler!(demote_user, demote_user);
user_mutation_handler!(revoke_user_sessions, revoke_sessions);

fn map_admin_error(error: AdminError) -> ApiError {
    match error {
        AdminError::InvalidInput => ApiError::bad_request(),
        AdminError::NotFound => ApiError::not_found(),
        AdminError::Conflict | AdminError::FinalActiveSuperadmin => ApiError::conflict(),
        AdminError::Database(_) | AdminError::DeliveryFailed => {
            tracing::error!(error_code = "admin_operation_failed");
            ApiError::internal()
        }
    }
}

async fn authenticated_session(
    State(manager): State<SessionManager>,
    mut request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, ApiError> {
    let session = manager
        .authenticate(session_cookie(request.headers()))
        .await
        .map_err(map_session_error)?;
    manager
        .enforce_csrf(request.method(), request.headers(), &session)
        .map_err(map_session_error)?;
    request.extensions_mut().insert(session);
    Ok(next.run(request).await)
}

async fn inspect_session(
    Extension(session): Extension<AuthenticatedSession>,
) -> Json<AuthenticatedSession> {
    Json(session)
}

async fn logout_current(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
) -> Result<impl IntoResponse, ApiError> {
    let manager = state
        .session_manager
        .ok_or_else(ApiError::service_unavailable)?;
    manager
        .logout_current(&session)
        .await
        .map_err(map_session_error)?;
    Ok(logout_response())
}

async fn logout_all(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
) -> Result<impl IntoResponse, ApiError> {
    let manager = state
        .session_manager
        .ok_or_else(ApiError::service_unavailable)?;
    manager
        .logout_all(&session)
        .await
        .map_err(map_session_error)?;
    Ok(logout_response())
}

fn logout_response() -> (StatusCode, HeaderMap) {
    let mut headers = HeaderMap::new();
    headers.insert(
        SET_COOKIE,
        HeaderValue::from_static(
            "__Host-commoncal_session=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Lax",
        ),
    );
    (StatusCode::NO_CONTENT, headers)
}

fn map_session_error(error: SessionError) -> ApiError {
    match error {
        SessionError::Unauthorized => ApiError::unauthorized(),
        SessionError::Forbidden => ApiError::forbidden(),
        SessionError::Database(_) => {
            tracing::error!(error_code = "session_operation_failed");
            ApiError::internal()
        }
    }
}

async fn health_live() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn health_ready(State(state): State<ApplicationState>) -> impl IntoResponse {
    if state.readiness.is_ready() {
        (StatusCode::OK, Json(HealthResponse { status: "ok" }))
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(HealthResponse {
                status: "not_ready",
            }),
        )
    }
}

async fn request_login_link(
    State(state): State<ApplicationState>,
    connect_info: Option<ConnectInfo<SocketAddr>>,
    Json(request): Json<RequestLoginLinkRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let login_flow = state.login_flow.ok_or_else(ApiError::service_unavailable)?;
    let client_ip = connect_info
        .map(|ConnectInfo(address)| address.ip().to_string())
        .unwrap_or_else(|| "unknown".to_owned());
    match login_flow
        .request_link(RequestLoginLink {
            email: request.email,
            client_ip,
        })
        .await
    {
        Ok(()) => {}
        Err(RequestLoginLinkError::RateLimited) => return Err(ApiError::rate_limited()),
        Err(RequestLoginLinkError::Database(_)) => {
            tracing::error!(error_code = "login_link_request_failed");
        }
    }

    Ok((
        StatusCode::ACCEPTED,
        Json(RequestLoginLinkResponse {
            message: "If the account is eligible, a login link will be sent",
        }),
    ))
}

async fn consume_login_link(
    State(state): State<ApplicationState>,
    headers: HeaderMap,
    Json(request): Json<ConsumeLoginLinkRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let login_flow = state.login_flow.ok_or_else(ApiError::service_unavailable)?;
    let consumed = login_flow
        .consume_link(ConsumeLoginLink {
            token: request.token,
            prior_session_token: session_cookie(&headers).map(str::to_owned),
        })
        .await
        .map_err(|error| match error {
            ConsumeLoginLinkError::Invalid => ApiError::invalid_login_link(),
            ConsumeLoginLinkError::Database(_) => {
                tracing::error!(error_code = "login_link_consumption_failed");
                ApiError::internal()
            }
        })?;

    let cookie = SessionCookieBuilder::new(&consumed.session_token).build();
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        SET_COOKIE,
        HeaderValue::from_str(&cookie).map_err(|_| ApiError::internal())?,
    );
    Ok((
        response_headers,
        Json(ConsumeLoginLinkResponse {
            user: consumed.user,
            csrf_token: consumed.csrf_token.expose().to_owned(),
        }),
    ))
}

async fn consume_invitation(
    State(state): State<ApplicationState>,
    headers: HeaderMap,
    Json(request): Json<ConsumeInvitationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let consumer = state
        .invitation_consumer
        .ok_or_else(ApiError::service_unavailable)?;
    let prior_session_token = session_cookie(&headers).map(str::to_owned);
    let consumed = consumer
        .consume(ConsumeInvitation {
            token: request.token,
            prior_session_token,
        })
        .await
        .map_err(|error| match error {
            ConsumeInvitationError::Invalid => ApiError::invalid_invitation(),
            ConsumeInvitationError::Database(error) => {
                tracing::error!(error = %error, "invitation consumption failed");
                ApiError::internal()
            }
        })?;

    let cookie = SessionCookieBuilder::new(&consumed.session_token).build();
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        SET_COOKIE,
        HeaderValue::from_str(&cookie).map_err(|_| ApiError::internal())?,
    );
    Ok((
        response_headers,
        Json(ConsumeInvitationResponse {
            user: consumed.user,
            csrf_token: consumed.csrf_token.expose().to_owned(),
        }),
    ))
}

fn session_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|cookie| {
            let (name, value) = cookie.split_once('=')?;
            (name == SESSION_COOKIE_NAME).then_some(value)
        })
}

async fn not_found() -> ApiError {
    ApiError::not_found()
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConsumeInvitationRequest {
    token: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestLoginLinkRequest {
    email: String,
}

#[derive(Serialize)]
struct RequestLoginLinkResponse {
    message: &'static str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConsumeLoginLinkRequest {
    token: String,
}

#[derive(Serialize)]
struct ConsumeLoginLinkResponse {
    user: ActiveUser,
    csrf_token: String,
}

#[derive(Serialize)]
struct ConsumeInvitationResponse {
    user: ActiveUser,
    csrf_token: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InviteUserRequest {
    email: String,
    display_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CalendarMutationRequest {
    name: String,
    description: Option<String>,
    color: String,
    default_timezone: String,
    default_event_visibility: String,
    default_notification_rules_json: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CalendarUpdateRequest {
    #[serde(flatten)]
    settings: CalendarMutationRequest,
    version: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CalendarAclRequest {
    role: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransferOwnershipRequest {
    new_owner_user_id: i64,
    version: i64,
}

#[derive(Deserialize)]
struct EventListQuery {
    from: Option<i64>,
    to: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SharedViewMutationRequest {
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SharedViewCalendarsRequest {
    calendars: Vec<SharedViewCalendarRequest>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicViewConfigurationRequest {
    projection: String,
    display_timezone: String,
    expires_at: i64,
}

impl TryFrom<PublicViewConfigurationRequest> for PublicViewConfiguration {
    type Error = ApiError;

    fn try_from(request: PublicViewConfigurationRequest) -> Result<Self, Self::Error> {
        Ok(Self {
            projection: request
                .projection
                .parse::<PublicViewProjection>()
                .map_err(|_| ApiError::bad_request())?,
            display_timezone: request.display_timezone,
            expires_at: request.expires_at,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SharedViewCalendarRequest {
    calendar_id: i64,
    position: i64,
    color: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EventMutationRequest {
    title: String,
    description: Option<String>,
    location: Option<String>,
    status: String,
    start_utc: Option<i64>,
    end_utc: Option<i64>,
    timezone: Option<String>,
    start_date: Option<String>,
    end_date: Option<String>,
    recurrence_rule: Option<String>,
}

impl EventMutationRequest {
    fn into_mutation(self) -> Result<EventMutation, ApiError> {
        let status = match self.status.as_str() {
            "tentative" => EventStatus::Tentative,
            "confirmed" => EventStatus::Confirmed,
            "cancelled" => EventStatus::Cancelled,
            _ => return Err(ApiError::bad_request()),
        };
        let timing = match (
            self.start_utc,
            self.end_utc,
            self.timezone,
            self.start_date,
            self.end_date,
        ) {
            (Some(start_utc), Some(end_utc), Some(timezone), None, None) => EventTiming::Timed {
                start_utc,
                end_utc,
                timezone,
            },
            (None, None, None, Some(start_date), Some(end_date)) => EventTiming::AllDay {
                start_date,
                end_date,
            },
            _ => return Err(ApiError::bad_request()),
        };
        Ok(EventMutation {
            title: self.title,
            description: self.description,
            location: self.location,
            status,
            timing,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EventUpdateRequest {
    calendar_id: i64,
    version: i64,
    #[serde(flatten)]
    event: EventMutationRequest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OccurrenceUpdateRequest {
    version: i64,
    #[serde(flatten)]
    event: EventMutationRequest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OccurrenceVersionRequest {
    version: i64,
}

#[derive(Deserialize)]
struct UserListQuery {
    status: Option<String>,
    page: Option<u32>,
    per_page: Option<u32>,
}

#[derive(Serialize)]
struct InvitationResponse {
    id: i64,
}

#[derive(Clone)]
struct ApplicationState {
    readiness: Readiness,
    invitation_consumer: Option<InvitationConsumer>,
    login_flow: Option<Arc<dyn LoginFlow>>,
    session_manager: Option<SessionManager>,
    admin_service: Option<AdminService>,
    calendar_service: Option<CalendarService>,
    event_service: Option<EventService>,
    shared_view_service: Option<SharedViewService>,
}

#[derive(Clone, Debug)]
pub struct Readiness {
    ready: Arc<AtomicBool>,
}

impl Readiness {
    pub fn new() -> Self {
        Self {
            ready: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    pub(crate) fn mark_ready(&self) {
        self.ready.store(true, Ordering::Release);
    }

    pub(crate) fn mark_not_ready(&self) {
        self.ready.store(false, Ordering::Release);
    }
}

impl Default for Readiness {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
    current_version: Option<i64>,
}

impl ApiError {
    fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: "Authentication required",
            current_version: None,
        }
    }

    fn forbidden() -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "forbidden",
            message: "Request forbidden",
            current_version: None,
        }
    }

    fn not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message: "Resource not found",
            current_version: None,
        }
    }

    fn bad_request() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_request",
            message: "Invalid request",
            current_version: None,
        }
    }

    fn conflict() -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "conflict",
            message: "Request conflicts with current state",
            current_version: None,
        }
    }

    fn version_conflict(current_version: i64) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "conflict",
            message: "Request conflicts with current state",
            current_version: Some(current_version),
        }
    }

    fn invalid_invitation() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "invalid_invitation",
            message: "Invitation is invalid or expired",
            current_version: None,
        }
    }

    fn invalid_login_link() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "invalid_login_link",
            message: "Login link is invalid or expired",
            current_version: None,
        }
    }

    fn rate_limited() -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: "rate_limited",
            message: "Too many requests",
            current_version: None,
        }
    }

    fn internal() -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
            message: "An internal error occurred",
            current_version: None,
        }
    }

    fn service_unavailable() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "service_unavailable",
            message: "Service unavailable",
            current_version: None,
        }
    }

    fn not_implemented() -> Self {
        Self {
            status: StatusCode::NOT_IMPLEMENTED,
            code: "not_supported",
            message: "This and following recurring edits are not yet supported",
            current_version: None,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorEnvelope {
                error: ErrorBody {
                    code: self.code,
                    message: self.message,
                    current_version: self.current_version,
                },
            }),
        )
            .into_response()
    }
}

#[derive(Serialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_version: Option<i64>,
}
