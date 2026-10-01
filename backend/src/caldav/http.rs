use std::str::FromStr;

use axum::{
    Extension, Json, Router,
    body::Body,
    extract::{Path, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{any, delete, get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::{
    authorization::{
        AuthorizationDecision, CalendarAction, CalendarRole, PlatformRole,
        authorize_calendar_action,
    },
    caldav::{
        MAX_LABEL_LENGTH,
        auth::CaldavAccountService,
        ical::{
            CaldavIcalDateValue, CaldavIcalEvent, CaldavIcalTiming, etag_for_ical,
            serialize_event_resource, serialize_recurring_series,
        },
        query::{self, CalendarMultiget, CalendarQuery, MAX_RESULTS},
        repository::CaldavRepository,
        types::{
            CaldavAuthError, CaldavCalendar, CaldavClientProperties, CaldavEventResource,
            CaldavMetrics, CredentialMetadata, DavSession, PrincipalInfo,
        },
    },
    event::{
        AllDayOccurrenceChange, Event, EventChange, EventMutation, EventRepository, EventService,
        EventServiceError, EventStatus, EventTiming, OccurrenceChange, RecurringExceptionChange,
        RecurringSeriesChange,
    },
    http::{ApiError, authenticated_session},
    ics::{IcsParserLimits, parse_calendar},
    identity::UserStatus,
    sessions::{AuthenticatedSession, SessionManager},
};

const DAV_REALM: &str = "commoncal-dav";
const PROPFIND: &str = "PROPFIND";
const OPTIONS: &str = "OPTIONS";
const REPORT: &str = "REPORT";
const DAV: header::HeaderName = header::HeaderName::from_static("dav");
const DAV_CAPABILITIES: &str = "1, 2, access-control, calendar-access";
const DAV_ALLOW: &str = "PROPFIND, OPTIONS, REPORT";

/// Effective event-read scope for a calendar role, derived from the single
/// authorization projection. `Details` exposes full event content; `FreeBusy`
/// exposes only the busy time window. `None` means the role may not read the
/// calendar at all and the resource must be hidden.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DavReadAccess {
    Details,
    FreeBusy,
}

fn read_access_for_role(role: &str) -> Option<DavReadAccess> {
    let Ok(role) = CalendarRole::from_str(role) else {
        return None;
    };
    let allows = |action: CalendarAction| {
        authorize_calendar_action(
            UserStatus::Active,
            Some(PlatformRole::User),
            Some(role),
            action,
        ) == AuthorizationDecision::Allow
    };
    if allows(CalendarAction::ReadDetails) {
        Some(DavReadAccess::Details)
    } else if allows(CalendarAction::ReadFreeBusy) {
        Some(DavReadAccess::FreeBusy)
    } else {
        None
    }
}

/// True when the event is an imported external-feed event and therefore
/// read-only. Imported resources must reject mutation.
async fn event_is_imported(pool: &SqlitePool, event_id: i64) -> bool {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM external_event_mapping WHERE event_id = ?)")
        .bind(event_id)
        .fetch_one(pool)
        .await
        .unwrap_or(false)
}

pub fn build_caldav_router(accounts: CaldavAccountService) -> Router {
    Router::new()
        .route("/.well-known/caldav", get(well_known_caldav))
        .route("/dav/", any(dav_root))
        .route("/dav/principals/:principal_id/", any(dav_principal))
        .route("/dav/calendars/:principal_id/", any(dav_calendar_home))
        .route(
            "/dav/calendars/:principal_id/:calendar_key/",
            any(dav_calendar),
        )
        .route(
            "/dav/calendars/:principal_id/:calendar_key/:resource_name",
            any(dav_event_resource),
        )
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
            let (session, _request) = match authenticate_dav_request(&accounts, request).await {
                Ok((session, request)) => (session, request),
                Err(response) => return response,
            };
            render_propfind(&accounts, &session)
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
            let (session, request) = match authenticate_dav_request(&accounts, request).await {
                Ok((session, request)) => (session, request),
                Err(response) => return response,
            };
            if depth_is_infinite(request.headers()) {
                return (StatusCode::BAD_REQUEST, "Depth: infinity is not supported")
                    .into_response();
            }
            let principal = match accounts.resolve_principal(&principal_id).await {
                Ok(Some(principal)) => principal,
                Ok(None) => return dav_not_found(accounts.metrics()),
                Err(_) => return dav_server_error(accounts.metrics()),
            };
            if principal.user_id != session.user_id {
                return dav_not_found(accounts.metrics());
            }
            render_principal(&accounts, &principal_id, &principal)
        }
        _ => method_not_allowed(),
    }
}

async fn dav_calendar_home(
    State(accounts): State<CaldavAccountService>,
    Path(principal_id): Path<String>,
    request: Request,
) -> Response {
    match request.method().as_str() {
        OPTIONS => dav_capabilities(),
        PROPFIND => {
            let (session, request) = match authenticate_dav_request(&accounts, request).await {
                Ok((session, request)) => (session, request),
                Err(response) => return response,
            };
            let Some(depth) = parse_depth(request.headers()) else {
                return dav_bad_depth(accounts.metrics());
            };
            let principal = match accounts.resolve_principal(&principal_id).await {
                Ok(Some(principal)) => principal,
                Ok(None) => return dav_not_found(accounts.metrics()),
                Err(_) => return dav_server_error(accounts.metrics()),
            };
            if principal.user_id != session.user_id {
                return dav_not_found(accounts.metrics());
            }
            let Ok(calendars) = accounts.list_calendars(session.user_id).await else {
                return dav_server_error(accounts.metrics());
            };
            render_calendar_home(&accounts, &principal_id, &principal, &calendars, depth)
        }
        _ => method_not_allowed(),
    }
}

async fn dav_calendar(
    State(accounts): State<CaldavAccountService>,
    Path((principal_id, calendar_key)): Path<(String, String)>,
    request: Request,
) -> Response {
    match request.method().as_str() {
        OPTIONS => dav_capabilities(),
        PROPFIND => {
            let (session, _request) = match authenticate_dav_request(&accounts, request).await {
                Ok((session, request)) => (session, request),
                Err(response) => return response,
            };
            let Some(depth) = parse_depth(_request.headers()) else {
                return dav_bad_depth(accounts.metrics());
            };
            let principal = match accounts.resolve_principal(&principal_id).await {
                Ok(Some(principal)) => principal,
                Ok(None) => return dav_not_found(accounts.metrics()),
                Err(_) => return dav_server_error(accounts.metrics()),
            };
            let Ok(calendar_id) = calendar_key.parse::<i64>() else {
                return dav_calendar_not_found(accounts.metrics());
            };
            let calendar = match accounts
                .resolve_calendar(session.user_id, calendar_id)
                .await
            {
                Ok(Some(calendar)) => calendar,
                Ok(None) => return dav_calendar_not_found(accounts.metrics()),
                Err(_) => return dav_server_error(accounts.metrics()),
            };
            let path_principal_ok =
                principal.user_id == calendar.owner_user_id || principal.user_id == session.user_id;
            if !path_principal_ok {
                return dav_calendar_not_found(accounts.metrics());
            }
            let Some(access) = read_access_for_role(&calendar.role) else {
                return dav_calendar_not_found(accounts.metrics());
            };
            let repository = CaldavRepository::new(accounts.pool().clone());
            let event_ids = match repository.list_exposed_event_ids(calendar_id).await {
                Ok(event_ids) => event_ids,
                Err(_) => return dav_server_error(accounts.metrics()),
            };
            let events = EventRepository::new(accounts.pool().clone());
            let now = accounts.now();
            let mut listed = Vec::new();
            for event_id in event_ids {
                let resource = match repository.ensure_resource(calendar_id, event_id, now).await {
                    Ok(resource) => resource,
                    Err(_) => return dav_server_error(accounts.metrics()),
                };
                match events.event(calendar_id, event_id).await {
                    Ok(Some(event)) => {
                        if let Some((_, etag)) =
                            ical_and_etag(accounts.pool(), calendar_id, &resource, &event, access)
                                .await
                        {
                            listed.push((resource, etag));
                        }
                    }
                    Ok(None) => {}
                    Err(_) => return dav_server_error(accounts.metrics()),
                }
            }
            render_calendar(&accounts, &principal_id, &calendar, depth, &listed)
        }
        REPORT => {
            let (session, request) = match authenticate_dav_request(&accounts, request).await {
                Ok((session, request)) => (session, request),
                Err(response) => return response,
            };
            let principal = match accounts.resolve_principal(&principal_id).await {
                Ok(Some(principal)) => principal,
                Ok(None) => return dav_not_found(accounts.metrics()),
                Err(_) => return dav_server_error(accounts.metrics()),
            };
            let Ok(calendar_id) = calendar_key.parse::<i64>() else {
                return dav_calendar_not_found(accounts.metrics());
            };
            let calendar = match accounts
                .resolve_calendar(session.user_id, calendar_id)
                .await
            {
                Ok(Some(calendar)) => calendar,
                Ok(None) => return dav_calendar_not_found(accounts.metrics()),
                Err(_) => return dav_server_error(accounts.metrics()),
            };
            let path_principal_ok =
                principal.user_id == calendar.owner_user_id || principal.user_id == session.user_id;
            if !path_principal_ok {
                return dav_calendar_not_found(accounts.metrics());
            }
            let body = match axum::body::to_bytes(request.into_body(), query::MAX_BODY_BYTES).await
            {
                Ok(body) => body,
                Err(_) => return dav_oversized_body(accounts.metrics()),
            };
            let report = match query::parse_calendar_report(&body) {
                Ok(report) => report,
                Err(e) => return dav_bad_request(accounts.metrics(), e.to_string()),
            };
            match report {
                query::CalendarReport::Query(cal_query) => {
                    handle_calendar_query(&accounts, &principal_id, &calendar, &cal_query).await
                }
                query::CalendarReport::Multiget(multiget) => {
                    handle_calendar_multiget(&accounts, &principal_id, &calendar, &multiget).await
                }
                query::CalendarReport::SyncCollection(sync) => {
                    handle_sync_collection(&accounts, &principal_id, &calendar, &sync).await
                }
            }
        }
        _ => method_not_allowed(),
    }
}

async fn dav_event_resource(
    State(accounts): State<CaldavAccountService>,
    Path((principal_id, calendar_key, resource_name)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    match request.method().as_str() {
        OPTIONS => dav_capabilities(),
        "GET" | "HEAD" => {
            let head = request.method().as_str() == "HEAD";
            let (session, _request) = match authenticate_dav_request(&accounts, request).await {
                Ok((session, request)) => (session, request),
                Err(response) => return response,
            };
            let principal = match accounts.resolve_principal(&principal_id).await {
                Ok(Some(principal)) => principal,
                Ok(None) => return dav_resource_not_found(accounts.metrics()),
                Err(_) => return dav_server_error(accounts.metrics()),
            };
            if principal.user_id != session.user_id {
                return dav_resource_not_found(accounts.metrics());
            }
            let Ok(calendar_id) = calendar_key.parse::<i64>() else {
                return dav_resource_not_found(accounts.metrics());
            };
            let calendar = match accounts
                .resolve_calendar(session.user_id, calendar_id)
                .await
            {
                Ok(Some(calendar)) => calendar,
                Ok(None) => return dav_resource_not_found(accounts.metrics()),
                Err(_) => return dav_server_error(accounts.metrics()),
            };
            let path_principal_ok =
                principal.user_id == calendar.owner_user_id || principal.user_id == session.user_id;
            if !path_principal_ok {
                return dav_resource_not_found(accounts.metrics());
            }
            let Some(access) = read_access_for_role(&calendar.role) else {
                return dav_resource_not_found(accounts.metrics());
            };
            let Some(resource_name) = resource_name.strip_suffix(".ics") else {
                return dav_resource_not_found(accounts.metrics());
            };
            let repository = CaldavRepository::new(accounts.pool().clone());
            let resource = match repository.resolve_by_name(calendar_id, resource_name).await {
                Ok(Some(resource)) => resource,
                Ok(None) => return dav_resource_not_found(accounts.metrics()),
                Err(_) => return dav_server_error(accounts.metrics()),
            };
            let event = match EventRepository::new(accounts.pool().clone())
                .event(calendar_id, resource.event_id)
                .await
            {
                Ok(Some(event)) => event,
                Ok(None) => return dav_resource_not_found(accounts.metrics()),
                Err(_) => return dav_server_error(accounts.metrics()),
            };
            let Some((ical, etag)) =
                ical_and_etag(accounts.pool(), calendar_id, &resource, &event, access).await
            else {
                return dav_resource_not_found(accounts.metrics());
            };
            let content_length = ical.len().to_string();
            let mut response = (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, "text/calendar; charset=utf-8"),
                    (header::ETAG, etag.as_str()),
                    (header::CONTENT_LENGTH, content_length.as_str()),
                ],
                ical,
            )
                .into_response();
            if head {
                *response.body_mut() = Body::empty();
            }
            response
        }
        "PUT" => {
            let (session, request) = match authenticate_dav_request(&accounts, request).await {
                Ok((session, request)) => (session, request),
                Err(response) => return response,
            };
            let principal = match accounts.resolve_principal(&principal_id).await {
                Ok(Some(principal)) => principal,
                Ok(None) => return dav_resource_not_found(accounts.metrics()),
                Err(_) => return dav_server_error(accounts.metrics()),
            };
            if principal.user_id != session.user_id {
                return dav_resource_not_found(accounts.metrics());
            }
            let Ok(calendar_id) = calendar_key.parse::<i64>() else {
                return dav_resource_not_found(accounts.metrics());
            };
            let calendar = match accounts
                .resolve_calendar(session.user_id, calendar_id)
                .await
            {
                Ok(Some(calendar)) => calendar,
                Ok(None) => return dav_resource_not_found(accounts.metrics()),
                Err(_) => return dav_server_error(accounts.metrics()),
            };
            let path_principal_ok =
                principal.user_id == calendar.owner_user_id || principal.user_id == session.user_id;
            if !path_principal_ok {
                return dav_resource_not_found(accounts.metrics());
            }
            let Some(resource_name) = resource_name.strip_suffix(".ics") else {
                return dav_resource_not_found(accounts.metrics());
            };
            handle_put_event_resource(
                &accounts,
                &session,
                &principal_id,
                &calendar,
                resource_name,
                request,
            )
            .await
        }
        "DELETE" => {
            let (session, request) = match authenticate_dav_request(&accounts, request).await {
                Ok((session, request)) => (session, request),
                Err(response) => return response,
            };
            let principal = match accounts.resolve_principal(&principal_id).await {
                Ok(Some(principal)) => principal,
                Ok(None) => return dav_resource_not_found(accounts.metrics()),
                Err(_) => return dav_server_error(accounts.metrics()),
            };
            if principal.user_id != session.user_id {
                return dav_resource_not_found(accounts.metrics());
            }
            let Ok(calendar_id) = calendar_key.parse::<i64>() else {
                return dav_resource_not_found(accounts.metrics());
            };
            let calendar = match accounts
                .resolve_calendar(session.user_id, calendar_id)
                .await
            {
                Ok(Some(calendar)) => calendar,
                Ok(None) => return dav_resource_not_found(accounts.metrics()),
                Err(_) => return dav_server_error(accounts.metrics()),
            };
            let path_principal_ok =
                principal.user_id == calendar.owner_user_id || principal.user_id == session.user_id;
            if !path_principal_ok {
                return dav_resource_not_found(accounts.metrics());
            }
            let Some(resource_name) = resource_name.strip_suffix(".ics") else {
                return dav_resource_not_found(accounts.metrics());
            };
            handle_delete_event_resource(&accounts, &session, &calendar, resource_name, request)
                .await
        }
        _ => method_not_allowed(),
    }
}

/// Route a conditional `PUT` to the create or update path based on whether
/// the resource already exists in the calendar.
async fn handle_put_event_resource(
    accounts: &CaldavAccountService,
    session: &DavSession,
    principal_id: &str,
    calendar: &CaldavCalendar,
    resource_name: &str,
    request: Request,
) -> Response {
    let repository = CaldavRepository::new(accounts.pool().clone());
    let existing = match repository
        .resolve_by_name(calendar.calendar_id, resource_name)
        .await
    {
        Ok(existing) => existing,
        Err(_) => return dav_server_error(accounts.metrics()),
    };
    match existing {
        Some(resource) => {
            handle_put_update_event_resource(
                accounts,
                session,
                principal_id,
                calendar,
                &resource,
                request,
            )
            .await
        }
        None => {
            handle_put_create_event_resource(
                accounts,
                session,
                principal_id,
                calendar,
                resource_name,
                request,
            )
            .await
        }
    }
}

/// Conditional `PUT` that creates one supported timed, non-recurring event
/// through the domain service and persists the DAV resource mapping.
///
/// Requires `If-None-Match: *`. Fails with 412 when the resource already
/// exists, 409 when the client-supplied UID is already mapped in the
/// calendar, and rolls back the event when the mapping cannot be written.
async fn handle_put_create_event_resource(
    accounts: &CaldavAccountService,
    session: &DavSession,
    principal_id: &str,
    calendar: &CaldavCalendar,
    resource_name: &str,
    request: Request,
) -> Response {
    let if_none_match = request
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok());
    if if_none_match != Some("*") {
        return (
            StatusCode::PRECONDITION_REQUIRED,
            "If-None-Match: * is required to create a resource",
        )
            .into_response();
    }

    let Ok(role) = CalendarRole::from_str(&calendar.role) else {
        return dav_server_error(accounts.metrics());
    };
    if authorize_calendar_action(
        UserStatus::Active,
        Some(PlatformRole::User),
        Some(role),
        CalendarAction::CreateEvent,
    ) != AuthorizationDecision::Allow
    {
        return dav_resource_not_found(accounts.metrics());
    }

    let body = match axum::body::to_bytes(request.into_body(), query::MAX_BODY_BYTES).await {
        Ok(body) => body,
        Err(_) => return dav_oversized_body(accounts.metrics()),
    };
    let body_str = match std::str::from_utf8(&body) {
        Ok(value) => value,
        Err(_) => return dav_bad_request(accounts.metrics(), "request body must be UTF-8"),
    };
    let parsed = match parse_calendar(body_str, IcsParserLimits::default()) {
        Ok(parsed) => parsed,
        Err(_) => return dav_bad_request(accounts.metrics(), "invalid calendar data"),
    };
    if parsed.events.is_empty() {
        return dav_bad_request(accounts.metrics(), "at least one VEVENT is required");
    }
    if let Err(message) = validate_dav_write(&parsed) {
        return dav_bad_request(accounts.metrics(), message);
    }
    let masters: Vec<_> = parsed
        .events
        .iter()
        .filter(|event| event.recurrence_id.is_none())
        .collect();
    if masters.len() != 1 {
        return dav_bad_request(
            accounts.metrics(),
            "a recurring resource requires exactly one master VEVENT",
        );
    }
    let master = masters[0];
    let exceptions: Vec<_> = parsed
        .events
        .iter()
        .filter(|e| e.recurrence_id.is_some())
        .collect();
    if exceptions.len() + 1 != parsed.events.len()
        || exceptions.iter().any(|event| event.uid != master.uid)
    {
        return dav_bad_request(accounts.metrics(), "recurring VEVENTs must share one UID");
    }
    let timing = match &master.timing {
        crate::ics::NormalizedTiming::Timed {
            starts_at,
            ends_at,
            timezone,
        } => EventTiming::Timed {
            start_utc: starts_at.timestamp(),
            end_utc: ends_at.timestamp(),
            timezone: timezone.clone().unwrap_or_else(|| "UTC".to_owned()),
        },
        crate::ics::NormalizedTiming::AllDay {
            start_date,
            end_date,
        } => EventTiming::AllDay {
            start_date: start_date.format("%Y-%m-%d").to_string(),
            end_date: end_date.format("%Y-%m-%d").to_string(),
        },
    };
    if timing_end_not_after_start(&timing) {
        return dav_bad_request(accounts.metrics(), "event end must be after start");
    }
    let status = match master.status.as_deref() {
        Some("TENTATIVE") => EventStatus::Tentative,
        Some("CANCELLED") => EventStatus::Cancelled,
        _ => EventStatus::Confirmed,
    };
    if master.summary.trim().is_empty() {
        return dav_bad_request(accounts.metrics(), "SUMMARY is required");
    }
    let client_properties = client_properties_from(master);

    let repository = CaldavRepository::new(accounts.pool().clone());
    if repository
        .resolve_by_name(calendar.calendar_id, resource_name)
        .await
        .ok()
        .flatten()
        .is_some()
    {
        accounts.metrics().record_precondition_failed();
        return (StatusCode::PRECONDITION_FAILED, "resource already exists").into_response();
    }

    let mutation = EventMutation {
        title: master.summary.clone(),
        description: master.description.clone(),
        location: master.location.clone(),
        status,
        timing,
    };
    let event_service = EventService::new_at(accounts.pool().clone(), accounts.now());
    let projection = if let Some(rrule) = &master.rrule {
        match event_service
            .create_recurring(
                session.user_id,
                false,
                calendar.calendar_id,
                mutation,
                rrule.clone(),
            )
            .await
        {
            Ok(projection) => projection,
            Err(_) => return dav_server_error(accounts.metrics()),
        }
    } else {
        if !exceptions.is_empty() || !master.exdates.is_empty() {
            return dav_bad_request(accounts.metrics(), "EXDATE and RECURRENCE-ID require RRULE");
        }
        match event_service
            .create(session.user_id, false, calendar.calendar_id, mutation)
            .await
        {
            Ok(projection) => projection,
            Err(_) => return dav_server_error(accounts.metrics()),
        }
    };
    let event_id = projection.id;

    if master.rrule.is_some() {
        let mut expected_version = projection.version.unwrap_or(1);
        for exdate in &master.exdates {
            let result = match exdate {
                crate::ics::NormalizedDateValue::Timed(dt) => {
                    event_service
                        .delete_occurrence(
                            session.user_id,
                            false,
                            calendar.calendar_id,
                            event_id,
                            dt.timestamp(),
                            expected_version,
                        )
                        .await
                }
                crate::ics::NormalizedDateValue::AllDay(date) => {
                    event_service
                        .delete_all_day_occurrence(
                            session.user_id,
                            false,
                            calendar.calendar_id,
                            event_id,
                            &date.format("%Y-%m-%d").to_string(),
                            expected_version,
                        )
                        .await
                }
            };
            match result {
                Ok(next_projection) => {
                    expected_version = next_projection.version.unwrap_or(expected_version);
                }
                Err(_) => {
                    let _ = event_service
                        .delete(session.user_id, false, calendar.calendar_id, event_id)
                        .await;
                    return dav_server_error(accounts.metrics());
                }
            }
        }
        for exception in &exceptions {
            let result = match exception.recurrence_id.as_ref().unwrap() {
                crate::ics::NormalizedDateValue::Timed(dt) => {
                    let exc_timing = match &exception.timing {
                        crate::ics::NormalizedTiming::Timed {
                            starts_at,
                            ends_at,
                            timezone,
                        } => EventTiming::Timed {
                            start_utc: starts_at.timestamp(),
                            end_utc: ends_at.timestamp(),
                            timezone: timezone.clone().unwrap_or_else(|| "UTC".to_owned()),
                        },
                        crate::ics::NormalizedTiming::AllDay {
                            start_date,
                            end_date,
                        } => EventTiming::AllDay {
                            start_date: start_date.format("%Y-%m-%d").to_string(),
                            end_date: end_date.format("%Y-%m-%d").to_string(),
                        },
                    };
                    let exc_status = match exception.status.as_deref() {
                        Some("TENTATIVE") => EventStatus::Tentative,
                        Some("CANCELLED") => EventStatus::Cancelled,
                        _ => EventStatus::Confirmed,
                    };
                    event_service
                        .update_occurrence(
                            session.user_id,
                            false,
                            calendar.calendar_id,
                            event_id,
                            crate::event::OccurrenceChange {
                                recurrence_id: dt.timestamp(),
                                expected_version,
                                event: EventMutation {
                                    title: exception.summary.clone(),
                                    description: exception.description.clone(),
                                    location: exception.location.clone(),
                                    status: exc_status,
                                    timing: exc_timing,
                                },
                            },
                        )
                        .await
                }
                crate::ics::NormalizedDateValue::AllDay(date) => {
                    let exc_timing = match &exception.timing {
                        crate::ics::NormalizedTiming::Timed {
                            starts_at,
                            ends_at,
                            timezone,
                        } => EventTiming::Timed {
                            start_utc: starts_at.timestamp(),
                            end_utc: ends_at.timestamp(),
                            timezone: timezone.clone().unwrap_or_else(|| "UTC".to_owned()),
                        },
                        crate::ics::NormalizedTiming::AllDay {
                            start_date,
                            end_date,
                        } => EventTiming::AllDay {
                            start_date: start_date.format("%Y-%m-%d").to_string(),
                            end_date: end_date.format("%Y-%m-%d").to_string(),
                        },
                    };
                    let exc_status = match exception.status.as_deref() {
                        Some("TENTATIVE") => EventStatus::Tentative,
                        Some("CANCELLED") => EventStatus::Cancelled,
                        _ => EventStatus::Confirmed,
                    };
                    event_service
                        .update_all_day_occurrence(
                            session.user_id,
                            false,
                            calendar.calendar_id,
                            event_id,
                            crate::event::AllDayOccurrenceChange {
                                recurrence_date: date.format("%Y-%m-%d").to_string(),
                                expected_version,
                                event: EventMutation {
                                    title: exception.summary.clone(),
                                    description: exception.description.clone(),
                                    location: exception.location.clone(),
                                    status: exc_status,
                                    timing: exc_timing,
                                },
                            },
                        )
                        .await
                }
            };
            match result {
                Ok(next_projection) => {
                    expected_version = next_projection.version.unwrap_or(expected_version);
                }
                Err(_) => {
                    let _ = event_service
                        .delete(session.user_id, false, calendar.calendar_id, event_id)
                        .await;
                    return dav_server_error(accounts.metrics());
                }
            }
        }
    }

    let now = accounts.now();
    let resource = match repository
        .create_resource(
            calendar.calendar_id,
            event_id,
            &master.uid,
            resource_name,
            now,
        )
        .await
    {
        Ok(resource) => resource,
        Err(error) => {
            let _ = event_service
                .delete(session.user_id, false, calendar.calendar_id, event_id)
                .await;
            return match error {
                CaldavAuthError::ResourceExists => {
                    accounts.metrics().record_precondition_failed();
                    (StatusCode::PRECONDITION_FAILED, "resource already exists").into_response()
                }
                CaldavAuthError::UidConflict => {
                    (StatusCode::CONFLICT, "uid already exists in calendar").into_response()
                }
                _ => dav_server_error(accounts.metrics()),
            };
        }
    };

    if repository
        .save_client_properties(event_id, &client_properties, now)
        .await
        .is_err()
    {
        let _ = event_service
            .delete(session.user_id, false, calendar.calendar_id, event_id)
            .await;
        return dav_server_error(accounts.metrics());
    }

    let event_record = match EventRepository::new(accounts.pool().clone())
        .event(calendar.calendar_id, event_id)
        .await
    {
        Ok(Some(event_record)) => event_record,
        _ => return dav_server_error(accounts.metrics()),
    };
    let Some((ical, etag)) = ical_and_etag(
        accounts.pool(),
        calendar.calendar_id,
        &resource,
        &event_record,
        DavReadAccess::Details,
    )
    .await
    else {
        return dav_server_error(accounts.metrics());
    };
    let location = format!(
        "/dav/calendars/{principal_id}/{}/{}.ics",
        calendar.calendar_id, resource.resource_name
    );
    (
        StatusCode::CREATED,
        [
            (header::LOCATION, location.as_str()),
            (header::ETAG, etag.as_str()),
            (header::CONTENT_TYPE, "text/calendar; charset=utf-8"),
        ],
        ical,
    )
        .into_response()
}

/// Conditional `PUT` that updates one existing supported timed, non-recurring
/// event through the domain service.
///
/// Requires `If-Match` carrying the current ETag. Fails with 412 when the
/// precondition is missing or stale, without mutating the event. On success
/// the update is applied atomically against the observed version and the
/// ETag rotates to reflect the new canonical content.
async fn handle_put_update_event_resource(
    accounts: &CaldavAccountService,
    session: &DavSession,
    _principal_id: &str,
    calendar: &CaldavCalendar,
    resource: &CaldavEventResource,
    request: Request,
) -> Response {
    let Ok(role) = CalendarRole::from_str(&calendar.role) else {
        return dav_server_error(accounts.metrics());
    };
    if authorize_calendar_action(
        UserStatus::Active,
        Some(PlatformRole::User),
        Some(role),
        CalendarAction::EditAnyEvent,
    ) != AuthorizationDecision::Allow
    {
        return dav_resource_not_found(accounts.metrics());
    }
    if event_is_imported(accounts.pool(), resource.event_id).await {
        return dav_resource_not_found(accounts.metrics());
    }

    let if_match = request
        .headers()
        .get(header::IF_MATCH)
        .and_then(|value| value.to_str().ok());
    let Some(if_match) = if_match else {
        accounts.metrics().record_precondition_failed();
        return (
            StatusCode::PRECONDITION_FAILED,
            "If-Match is required to update a resource",
        )
            .into_response();
    };

    let event = match EventRepository::new(accounts.pool().clone())
        .event(calendar.calendar_id, resource.event_id)
        .await
    {
        Ok(Some(event)) => event,
        Ok(None) => return dav_resource_not_found(accounts.metrics()),
        Err(_) => return dav_server_error(accounts.metrics()),
    };
    let Some((_, current_etag)) = ical_and_etag(
        accounts.pool(),
        calendar.calendar_id,
        resource,
        &event,
        DavReadAccess::Details,
    )
    .await
    else {
        return dav_resource_not_found(accounts.metrics());
    };
    if !etag_matches(if_match, &current_etag) {
        accounts.metrics().record_precondition_failed();
        return (StatusCode::PRECONDITION_FAILED, "precondition failed").into_response();
    }

    let body = match axum::body::to_bytes(request.into_body(), query::MAX_BODY_BYTES).await {
        Ok(body) => body,
        Err(_) => return dav_oversized_body(accounts.metrics()),
    };
    let body_str = match std::str::from_utf8(&body) {
        Ok(value) => value,
        Err(_) => return dav_bad_request(accounts.metrics(), "request body must be UTF-8"),
    };
    let parsed = match parse_calendar(body_str, IcsParserLimits::default()) {
        Ok(parsed) => parsed,
        Err(_) => return dav_bad_request(accounts.metrics(), "invalid calendar data"),
    };
    if parsed.events.is_empty() {
        return dav_bad_request(accounts.metrics(), "at least one VEVENT is required");
    }
    if let Err(message) = validate_dav_write(&parsed) {
        return dav_bad_request(accounts.metrics(), message);
    }
    // A canonical recurring resource contains its master plus zero or more
    // modified occurrences.  A standalone occurrence PUT remains supported.
    let masters: Vec<_> = parsed
        .events
        .iter()
        .filter(|event| event.recurrence_id.is_none())
        .collect();
    let (incoming, exceptions): (_, Vec<_>) = if parsed.events.len() == 1 {
        (&parsed.events[0], Vec::new())
    } else {
        if masters.len() != 1 {
            return dav_bad_request(
                accounts.metrics(),
                "a recurring resource requires exactly one master VEVENT",
            );
        }
        let master = masters[0];
        let exceptions: Vec<_> = parsed
            .events
            .iter()
            .filter(|event| event.recurrence_id.is_some())
            .collect();
        if exceptions.len() + 1 != parsed.events.len()
            || exceptions.iter().any(|event| event.uid != master.uid)
        {
            return dav_bad_request(accounts.metrics(), "recurring VEVENTs must share one UID");
        }
        (master, exceptions)
    };
    let is_existing_recurring: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM events WHERE id = ? AND calendar_id = ? AND recurrence_rule IS NOT NULL
         )",
    )
    .bind(resource.event_id)
    .bind(calendar.calendar_id)
    .fetch_one(accounts.pool())
    .await
    .unwrap_or(false);

    if incoming.recurrence_id.is_some() && !is_existing_recurring {
        return dav_bad_request(
            accounts.metrics(),
            "cannot update an occurrence of a non-recurring event",
        );
    }
    if incoming.rrule.is_some() && !is_existing_recurring {
        return dav_bad_request(
            accounts.metrics(),
            "cannot convert a non-recurring event to recurring",
        );
    }
    if is_existing_recurring {
        let current_rule: Option<String> = sqlx::query_scalar(
            "SELECT recurrence_rule FROM events WHERE id = ? AND calendar_id = ?",
        )
        .bind(resource.event_id)
        .bind(calendar.calendar_id)
        .fetch_optional(accounts.pool())
        .await
        .unwrap_or(None);
        if incoming
            .rrule
            .as_ref()
            .is_some_and(|rule| Some(rule) != current_rule.as_ref())
        {
            return dav_bad_request(
                accounts.metrics(),
                "changing RRULE on an existing series is not supported",
            );
        }
    }

    let timing = match &incoming.timing {
        crate::ics::NormalizedTiming::Timed {
            starts_at,
            ends_at,
            timezone,
        } => EventTiming::Timed {
            start_utc: starts_at.timestamp(),
            end_utc: ends_at.timestamp(),
            timezone: timezone.clone().unwrap_or_else(|| "UTC".to_owned()),
        },
        crate::ics::NormalizedTiming::AllDay {
            start_date,
            end_date,
        } => EventTiming::AllDay {
            start_date: start_date.format("%Y-%m-%d").to_string(),
            end_date: end_date.format("%Y-%m-%d").to_string(),
        },
    };
    if timing_end_not_after_start(&timing) {
        return dav_bad_request(accounts.metrics(), "event end must be after start");
    }
    let status = match incoming.status.as_deref() {
        Some("TENTATIVE") => EventStatus::Tentative,
        Some("CANCELLED") => EventStatus::Cancelled,
        _ => EventStatus::Confirmed,
    };
    if incoming.summary.trim().is_empty() {
        return dav_bad_request(accounts.metrics(), "SUMMARY is required");
    }
    let client_properties = client_properties_from(incoming);

    let mutation = EventMutation {
        title: incoming.summary.clone(),
        description: incoming.description.clone(),
        location: incoming.location.clone(),
        status,
        timing,
    };
    let event_service = EventService::new_at(accounts.pool().clone(), accounts.now());
    let mut applied_recurring_exceptions = false;
    let mut projection = if let Some(recurrence_id) = &incoming.recurrence_id {
        match recurrence_id {
            crate::ics::NormalizedDateValue::Timed(dt) => {
                match event_service
                    .update_occurrence(
                        session.user_id,
                        false,
                        calendar.calendar_id,
                        resource.event_id,
                        crate::event::OccurrenceChange {
                            recurrence_id: dt.timestamp(),
                            expected_version: event.version,
                            event: mutation,
                        },
                    )
                    .await
                {
                    Ok(projection) => projection,
                    Err(EventServiceError::Conflict { .. }) => {
                        accounts.metrics().record_precondition_failed();
                        return (StatusCode::PRECONDITION_FAILED, "precondition failed")
                            .into_response();
                    }
                    Err(EventServiceError::NotFound) => {
                        return dav_resource_not_found(accounts.metrics());
                    }
                    Err(EventServiceError::ReadOnly) => {
                        return dav_resource_not_found(accounts.metrics());
                    }
                    Err(_) => return dav_server_error(accounts.metrics()),
                }
            }
            crate::ics::NormalizedDateValue::AllDay(date) => {
                match event_service
                    .update_all_day_occurrence(
                        session.user_id,
                        false,
                        calendar.calendar_id,
                        resource.event_id,
                        crate::event::AllDayOccurrenceChange {
                            recurrence_date: date.format("%Y-%m-%d").to_string(),
                            expected_version: event.version,
                            event: mutation,
                        },
                    )
                    .await
                {
                    Ok(projection) => projection,
                    Err(EventServiceError::Conflict { .. }) => {
                        accounts.metrics().record_precondition_failed();
                        return (StatusCode::PRECONDITION_FAILED, "precondition failed")
                            .into_response();
                    }
                    Err(EventServiceError::NotFound) => {
                        return dav_resource_not_found(accounts.metrics());
                    }
                    Err(EventServiceError::ReadOnly) => {
                        return dav_resource_not_found(accounts.metrics());
                    }
                    Err(_) => return dav_server_error(accounts.metrics()),
                }
            }
        }
    } else {
        let result = if is_existing_recurring {
            let mut changes = Vec::with_capacity(exceptions.len() + incoming.exdates.len());
            for exception in &exceptions {
                let exception_timing = match &exception.timing {
                    crate::ics::NormalizedTiming::Timed {
                        starts_at,
                        ends_at,
                        timezone,
                    } => EventTiming::Timed {
                        start_utc: starts_at.timestamp(),
                        end_utc: ends_at.timestamp(),
                        timezone: timezone.clone().unwrap_or_else(|| "UTC".to_owned()),
                    },
                    crate::ics::NormalizedTiming::AllDay {
                        start_date,
                        end_date,
                    } => EventTiming::AllDay {
                        start_date: start_date.format("%Y-%m-%d").to_string(),
                        end_date: end_date.format("%Y-%m-%d").to_string(),
                    },
                };
                let exception_event = EventMutation {
                    title: exception.summary.clone(),
                    description: exception.description.clone(),
                    location: exception.location.clone(),
                    status: match exception.status.as_deref() {
                        Some("TENTATIVE") => EventStatus::Tentative,
                        Some("CANCELLED") => EventStatus::Cancelled,
                        _ => EventStatus::Confirmed,
                    },
                    timing: exception_timing,
                };
                match exception.recurrence_id.as_ref().expect("filtered above") {
                    crate::ics::NormalizedDateValue::Timed(dt) => {
                        changes.push(RecurringExceptionChange::UpdateTimed(OccurrenceChange {
                            recurrence_id: dt.timestamp(),
                            expected_version: 0,
                            event: exception_event,
                        }))
                    }
                    crate::ics::NormalizedDateValue::AllDay(date) => changes.push(
                        RecurringExceptionChange::UpdateAllDay(AllDayOccurrenceChange {
                            recurrence_date: date.format("%Y-%m-%d").to_string(),
                            expected_version: 0,
                            event: exception_event,
                        }),
                    ),
                }
            }
            for exdate in &incoming.exdates {
                changes.push(match exdate {
                    crate::ics::NormalizedDateValue::Timed(dt) => {
                        RecurringExceptionChange::DeleteTimed(dt.timestamp())
                    }
                    crate::ics::NormalizedDateValue::AllDay(date) => {
                        RecurringExceptionChange::DeleteAllDay(date.format("%Y-%m-%d").to_string())
                    }
                });
            }
            applied_recurring_exceptions = true;
            event_service
                .replace_recurring_series(
                    session.user_id,
                    false,
                    calendar.calendar_id,
                    resource.event_id,
                    RecurringSeriesChange {
                        expected_version: event.version,
                        event: mutation,
                        exceptions: changes,
                    },
                )
                .await
        } else {
            event_service
                .update(
                    session.user_id,
                    false,
                    calendar.calendar_id,
                    resource.event_id,
                    EventChange {
                        expected_version: event.version,
                        target_calendar_id: calendar.calendar_id,
                        event: mutation,
                    },
                )
                .await
        };
        match result {
            Ok(projection) => projection,
            Err(EventServiceError::Conflict { .. }) => {
                accounts.metrics().record_precondition_failed();
                return (StatusCode::PRECONDITION_FAILED, "precondition failed").into_response();
            }
            Err(EventServiceError::NotFound) => return dav_resource_not_found(accounts.metrics()),
            Err(EventServiceError::ReadOnly) => return dav_resource_not_found(accounts.metrics()),
            Err(_) => return dav_server_error(accounts.metrics()),
        }
    };

    if is_existing_recurring && incoming.recurrence_id.is_none() && !applied_recurring_exceptions {
        let mut expected_version = projection.version.unwrap_or(event.version + 1);
        for exception in exceptions {
            let exception_timing = match &exception.timing {
                crate::ics::NormalizedTiming::Timed {
                    starts_at,
                    ends_at,
                    timezone,
                } => EventTiming::Timed {
                    start_utc: starts_at.timestamp(),
                    end_utc: ends_at.timestamp(),
                    timezone: timezone.clone().unwrap_or_else(|| "UTC".to_owned()),
                },
                crate::ics::NormalizedTiming::AllDay {
                    start_date,
                    end_date,
                } => EventTiming::AllDay {
                    start_date: start_date.format("%Y-%m-%d").to_string(),
                    end_date: end_date.format("%Y-%m-%d").to_string(),
                },
            };
            let exception_status = match exception.status.as_deref() {
                Some("TENTATIVE") => EventStatus::Tentative,
                Some("CANCELLED") => EventStatus::Cancelled,
                _ => EventStatus::Confirmed,
            };
            let result = match exception.recurrence_id.as_ref().expect("filtered above") {
                crate::ics::NormalizedDateValue::Timed(dt) => {
                    event_service
                        .update_occurrence(
                            session.user_id,
                            false,
                            calendar.calendar_id,
                            resource.event_id,
                            crate::event::OccurrenceChange {
                                recurrence_id: dt.timestamp(),
                                expected_version,
                                event: EventMutation {
                                    title: exception.summary.clone(),
                                    description: exception.description.clone(),
                                    location: exception.location.clone(),
                                    status: exception_status,
                                    timing: exception_timing,
                                },
                            },
                        )
                        .await
                }
                crate::ics::NormalizedDateValue::AllDay(date) => {
                    event_service
                        .update_all_day_occurrence(
                            session.user_id,
                            false,
                            calendar.calendar_id,
                            resource.event_id,
                            crate::event::AllDayOccurrenceChange {
                                recurrence_date: date.format("%Y-%m-%d").to_string(),
                                expected_version,
                                event: EventMutation {
                                    title: exception.summary.clone(),
                                    description: exception.description.clone(),
                                    location: exception.location.clone(),
                                    status: exception_status,
                                    timing: exception_timing,
                                },
                            },
                        )
                        .await
                }
            };
            match result {
                Ok(next_projection) => {
                    expected_version = next_projection.version.unwrap_or(expected_version);
                    projection = next_projection;
                }
                Err(EventServiceError::Conflict { .. }) => {
                    accounts.metrics().record_precondition_failed();
                    return (StatusCode::PRECONDITION_FAILED, "precondition failed")
                        .into_response();
                }
                Err(EventServiceError::NotFound | EventServiceError::ReadOnly) => {
                    return dav_resource_not_found(accounts.metrics());
                }
                Err(_) => return dav_server_error(accounts.metrics()),
            }
        }
        for exdate in &incoming.exdates {
            let result = match exdate {
                crate::ics::NormalizedDateValue::Timed(dt) => {
                    event_service
                        .delete_occurrence(
                            session.user_id,
                            false,
                            calendar.calendar_id,
                            resource.event_id,
                            dt.timestamp(),
                            expected_version,
                        )
                        .await
                }
                crate::ics::NormalizedDateValue::AllDay(date) => {
                    event_service
                        .delete_all_day_occurrence(
                            session.user_id,
                            false,
                            calendar.calendar_id,
                            resource.event_id,
                            &date.format("%Y-%m-%d").to_string(),
                            expected_version,
                        )
                        .await
                }
            };
            match result {
                Ok(next_projection) => {
                    expected_version = next_projection.version.unwrap_or(expected_version);
                    projection = next_projection;
                }
                Err(_) => return dav_server_error(accounts.metrics()),
            }
        }
    }

    let repository = CaldavRepository::new(accounts.pool().clone());
    if repository
        .save_client_properties(resource.event_id, &client_properties, accounts.now())
        .await
        .is_err()
    {
        return dav_server_error(accounts.metrics());
    }

    let event_record = match EventRepository::new(accounts.pool().clone())
        .event(calendar.calendar_id, projection.id)
        .await
    {
        Ok(Some(event_record)) => event_record,
        _ => return dav_server_error(accounts.metrics()),
    };
    let Some((ical, etag)) = ical_and_etag(
        accounts.pool(),
        calendar.calendar_id,
        resource,
        &event_record,
        DavReadAccess::Details,
    )
    .await
    else {
        return dav_server_error(accounts.metrics());
    };
    (
        StatusCode::OK,
        [
            (header::ETAG, etag.as_str()),
            (header::CONTENT_TYPE, "text/calendar; charset=utf-8"),
        ],
        ical,
    )
        .into_response()
}

/// Conditional `DELETE` that removes one existing supported event through the
/// domain service and retains the DAV mapping as a durable tombstone.
///
/// Requires `If-Match` carrying the current ETag. Fails with 412 when the
/// precondition is missing or stale, without mutating the event. On success the
/// event is deleted and the resource row is tombstoned (event reference
/// cleared, deletion stamped) so the mapping survives for other clients. If the
/// domain delete fails after the tombstone is written, the tombstone is
/// compensated back to the live mapping.
async fn handle_delete_event_resource(
    accounts: &CaldavAccountService,
    session: &DavSession,
    calendar: &CaldavCalendar,
    resource_name: &str,
    request: Request,
) -> Response {
    let Ok(role) = CalendarRole::from_str(&calendar.role) else {
        return dav_server_error(accounts.metrics());
    };
    if authorize_calendar_action(
        UserStatus::Active,
        Some(PlatformRole::User),
        Some(role),
        CalendarAction::EditAnyEvent,
    ) != AuthorizationDecision::Allow
    {
        return dav_resource_not_found(accounts.metrics());
    }

    let if_match = request
        .headers()
        .get(header::IF_MATCH)
        .and_then(|value| value.to_str().ok());
    let Some(if_match) = if_match else {
        accounts.metrics().record_precondition_failed();
        return (
            StatusCode::PRECONDITION_FAILED,
            "If-Match is required to delete a resource",
        )
            .into_response();
    };

    let repository = CaldavRepository::new(accounts.pool().clone());
    let resource = match repository
        .resolve_by_name(calendar.calendar_id, resource_name)
        .await
    {
        Ok(Some(resource)) => resource,
        Ok(None) => return dav_resource_not_found(accounts.metrics()),
        Err(_) => return dav_server_error(accounts.metrics()),
    };
    if event_is_imported(accounts.pool(), resource.event_id).await {
        return dav_resource_not_found(accounts.metrics());
    }

    let event = match EventRepository::new(accounts.pool().clone())
        .event(calendar.calendar_id, resource.event_id)
        .await
    {
        Ok(Some(event)) => event,
        Ok(None) => return dav_resource_not_found(accounts.metrics()),
        Err(_) => return dav_server_error(accounts.metrics()),
    };
    let Some((_, current_etag)) = ical_and_etag(
        accounts.pool(),
        calendar.calendar_id,
        &resource,
        &event,
        DavReadAccess::Details,
    )
    .await
    else {
        return dav_resource_not_found(accounts.metrics());
    };
    if !etag_matches(if_match, &current_etag) {
        accounts.metrics().record_precondition_failed();
        return (StatusCode::PRECONDITION_FAILED, "precondition failed").into_response();
    }

    let now = accounts.now();
    let tombstoned = match repository
        .tombstone_resource(calendar.calendar_id, resource.event_id, now)
        .await
    {
        Ok(tombstoned) => tombstoned,
        Err(_) => return dav_server_error(accounts.metrics()),
    };
    if !tombstoned {
        // The live mapping disappeared between the precondition check and the
        // tombstone write; treat as an already-deleted resource.
        return dav_resource_not_found(accounts.metrics());
    }

    let event_service = EventService::new_at(accounts.pool().clone(), accounts.now());
    let delete_result = event_service
        .delete(
            session.user_id,
            false,
            calendar.calendar_id,
            resource.event_id,
        )
        .await;
    if let Err(error) = delete_result {
        // Compensate the tombstone so the mapping is not left dangling.
        let _ = repository
            .restore_resource(
                calendar.calendar_id,
                resource.event_id,
                resource_name,
                accounts.now(),
            )
            .await;
        return match error {
            EventServiceError::NotFound => dav_resource_not_found(accounts.metrics()),
            EventServiceError::ReadOnly => dav_resource_not_found(accounts.metrics()),
            _ => dav_server_error(accounts.metrics()),
        };
    }

    // The tombstone was applied before the domain delete, so the change-log
    // entry recorded by the domain service could not resolve the resource name
    // from the live mapping. Stamp it now so sync-collection can emit the
    // correct href for this deletion.
    let _ = repository
        .stamp_change_resource_name(calendar.calendar_id, resource.event_id, resource_name)
        .await;

    StatusCode::OK.into_response()
}

/// True when a parsed event's end is not strictly after its start. The parser
/// already enforces this, but the check keeps the HTTP layer fail-safe for both
/// timed and all-day events.
fn timing_end_not_after_start(timing: &EventTiming) -> bool {
    match timing {
        EventTiming::Timed {
            start_utc, end_utc, ..
        } => start_utc >= end_utc,
        EventTiming::AllDay {
            start_date,
            end_date,
        } => start_date >= end_date,
    }
}

/// Validate a parsed calendar for a DAV write (T16). Scheduling properties
/// (`ATTENDEE`, `ORGANIZER`) and the `METHOD` property are unsafe for this
/// surface and must be rejected before any mutation. Unsupported components
/// (`VTODO`, `VJOURNAL`) are already rejected by the parser. Returns `Err`
/// with a client-facing message when the write must be rejected.
fn validate_dav_write(parsed: &crate::ics::NormalizedCalendar) -> Result<(), String> {
    if parsed.has_method {
        return Err("scheduling (METHOD) is not supported".to_owned());
    }
    for event in &parsed.events {
        if !event.scheduling.is_empty() {
            return Err("scheduling properties are not supported".to_owned());
        }
    }
    Ok(())
}

/// Extract the allowlisted client-owned metadata from a parsed event (T16).
/// The incoming value is the source of truth: absent properties yield empty
/// fields, which persist as explicit removals.
fn client_properties_from(event: &crate::ics::NormalizedEvent) -> CaldavClientProperties {
    CaldavClientProperties {
        categories: event.categories.clone(),
        url: event.url.clone(),
        transp: event.transp.clone(),
        alarms: event.alarms.clone(),
        x_properties: event.x_properties.clone(),
    }
}

/// True when an `If-Match` precondition is satisfied by the current ETag.
///
/// Accepts the wildcard and a comma-separated list of strong ETags, comparing
/// each against the current tag.
fn etag_matches(if_match: &str, current_etag: &str) -> bool {
    let if_match = if_match.trim();
    if if_match == "*" {
        return true;
    }
    if_match
        .split(',')
        .map(|tag| tag.trim())
        .any(|tag| tag == current_etag)
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

fn dav_not_found(metrics: &CaldavMetrics) -> Response {
    metrics.record_not_found();
    (StatusCode::NOT_FOUND, "principal not found").into_response()
}

fn dav_server_error(metrics: &CaldavMetrics) -> Response {
    metrics.record_internal_error();
    (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
}

fn depth_is_infinite(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get("depth")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("infinity"))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DavDepth {
    Zero,
    One,
}

fn parse_depth(headers: &axum::http::HeaderMap) -> Option<DavDepth> {
    let value = headers.get("depth").and_then(|value| value.to_str().ok());
    match value {
        None => Some(DavDepth::One),
        Some(value) if value.eq_ignore_ascii_case("0") => Some(DavDepth::Zero),
        Some(value) if value.eq_ignore_ascii_case("1") => Some(DavDepth::One),
        Some(_) => None,
    }
}

fn dav_bad_depth(metrics: &CaldavMetrics) -> Response {
    metrics.record_malformed_request();
    (StatusCode::BAD_REQUEST, "Depth: only 0 and 1 are supported").into_response()
}

fn dav_calendar_not_found(metrics: &CaldavMetrics) -> Response {
    metrics.record_not_found();
    (StatusCode::NOT_FOUND, "calendar not found").into_response()
}

/// 404 for an event resource. Empty body so existence is not leaked.
fn dav_resource_not_found(metrics: &CaldavMetrics) -> Response {
    metrics.record_not_found();
    StatusCode::NOT_FOUND.into_response()
}

fn dav_bad_request(metrics: &CaldavMetrics, message: impl Into<String>) -> Response {
    metrics.record_malformed_request();
    (StatusCode::BAD_REQUEST, message.into()).into_response()
}

fn dav_oversized_body(metrics: &CaldavMetrics) -> Response {
    metrics.record_oversized_body();
    (StatusCode::BAD_REQUEST, "request body too large").into_response()
}

async fn handle_calendar_query(
    accounts: &CaldavAccountService,
    principal_id: &str,
    calendar: &CaldavCalendar,
    cal_query: &CalendarQuery,
) -> Response {
    let Some(access) = read_access_for_role(&calendar.role) else {
        return dav_calendar_not_found(accounts.metrics());
    };
    let repository = CaldavRepository::new(accounts.pool().clone());
    let event_ids = match repository
        .list_events_in_range(
            calendar.calendar_id,
            cal_query.start_utc,
            cal_query.end_utc,
            MAX_RESULTS,
        )
        .await
    {
        Ok(ids) => ids,
        Err(_) => return dav_server_error(accounts.metrics()),
    };
    let events = EventRepository::new(accounts.pool().clone());
    let now = accounts.now();
    let mut responses = Vec::new();
    for event_id in &event_ids {
        let resource = match repository
            .ensure_resource(calendar.calendar_id, *event_id, now)
            .await
        {
            Ok(resource) => resource,
            Err(_) => return dav_server_error(accounts.metrics()),
        };
        match events.event(calendar.calendar_id, *event_id).await {
            Ok(Some(event)) => {
                if let Some((ical, etag)) = ical_and_etag(
                    accounts.pool(),
                    calendar.calendar_id,
                    &resource,
                    &event,
                    access,
                )
                .await
                {
                    let href = format!(
                        "/dav/calendars/{principal_id}/{}/{}.ics",
                        calendar.calendar_id, resource.resource_name
                    );
                    responses.push(calendar_query_response_xml(&href, &etag, &ical));
                }
            }
            Ok(None) => {}
            Err(_) => return dav_server_error(accounts.metrics()),
        }
    }
    let body = if responses.is_empty() {
        String::new()
    } else {
        responses.join("\n")
    };
    let xml = format!(
        r#"<?xml version="1.0" encoding="utf-8" ?>
<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:calendarserver/">
{body}
</D:multistatus>"#
    );
    (
        StatusCode::MULTI_STATUS,
        [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
        xml,
    )
        .into_response()
}

fn calendar_query_response_xml(href: &str, etag: &str, ical: &str) -> String {
    let escaped_ical = xml_escape(ical);
    format!(
        r#"  <D:response>
    <D:href>{href}</D:href>
    <D:propstat>
      <D:prop>
        <D:getetag>{etag}</D:getetag>
        <D:getcontenttype>text/calendar; charset=utf-8</D:getcontenttype>
        <C:calendar-data>{escaped_ical}</C:calendar-data>
      </D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>"#
    )
}

/// Hydrate the requested resources of one authorized collection.
///
/// Each requested href yields exactly one `<D:response>`. Hrefs that do not
/// resolve to an exposed resource in this collection — including foreign
/// paths — return a per-item 404 so existence is never leaked.
async fn handle_calendar_multiget(
    accounts: &CaldavAccountService,
    principal_id: &str,
    calendar: &CaldavCalendar,
    multiget: &CalendarMultiget,
) -> Response {
    let Some(access) = read_access_for_role(&calendar.role) else {
        return dav_calendar_not_found(accounts.metrics());
    };
    let repository = CaldavRepository::new(accounts.pool().clone());
    let events = EventRepository::new(accounts.pool().clone());
    let mut responses = Vec::with_capacity(multiget.hrefs.len());
    for href in &multiget.hrefs {
        let resource_name = match href_within_collection(href, principal_id, calendar.calendar_id) {
            Some(name) => name,
            None => {
                responses.push(multiget_response_xml(href, None));
                continue;
            }
        };
        let resource = match repository
            .resolve_by_name(calendar.calendar_id, &resource_name)
            .await
        {
            Ok(Some(resource)) => resource,
            Ok(None) => {
                responses.push(multiget_response_xml(href, None));
                continue;
            }
            Err(_) => return dav_server_error(accounts.metrics()),
        };
        let event = match events.event(calendar.calendar_id, resource.event_id).await {
            Ok(Some(event)) => event,
            Ok(None) => {
                responses.push(multiget_response_xml(href, None));
                continue;
            }
            Err(_) => return dav_server_error(accounts.metrics()),
        };
        let payload = ical_and_etag(
            accounts.pool(),
            calendar.calendar_id,
            &resource,
            &event,
            access,
        )
        .await
        .map(|(ical, etag)| (etag, ical));
        responses.push(multiget_response_xml(href, payload));
    }
    let body = responses.join("\n");
    let xml = format!(
        r#"<?xml version="1.0" encoding="utf-8" ?>
<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:calendarserver/">
{body}
</D:multistatus>"#
    );
    (
        StatusCode::MULTI_STATUS,
        [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
        xml,
    )
        .into_response()
}

/// Handle a `sync-collection` REPORT: return the changes since the client's
/// opaque token (or the full snapshot when no token is supplied) and mint a
/// new token for the next incremental sync.
///
/// Deleted resources are represented by a response whose `D:status` is
/// `HTTP/1.1 404 Not Found` as a direct child of `D:response`, per RFC
/// 6578. Invalid tokens fail safely with 400. Pagination is bounded by
/// `MAX_RESULTS` and the new token always advances past the last returned
/// revision, so no change is skipped.
async fn handle_sync_collection(
    accounts: &CaldavAccountService,
    principal_id: &str,
    calendar: &CaldavCalendar,
    sync: &query::SyncCollection,
) -> Response {
    let Some(access) = read_access_for_role(&calendar.role) else {
        return dav_calendar_not_found(accounts.metrics());
    };
    let repository = CaldavRepository::new(accounts.pool().clone());
    let events = EventRepository::new(accounts.pool().clone());
    let now = accounts.now();

    let mut responses = Vec::new();
    let new_revision;

    match &sync.sync_token {
        Some(token) => {
            // Incremental sync: decode the opaque token and page the change log.
            let Some(revision) = accounts.decode_sync_token(token) else {
                return dav_bad_request(accounts.metrics(), "invalid sync token");
            };
            let changes = match repository
                .list_changes_since(calendar.calendar_id, revision, MAX_RESULTS)
                .await
            {
                Ok(changes) => changes,
                Err(_) => return dav_server_error(accounts.metrics()),
            };
            new_revision = changes.last().map(|change| change.id).unwrap_or(revision);
            for change in &changes {
                match change.change_type.as_str() {
                    "deleted" => {
                        let resource_name = change.resource_name.clone().unwrap_or_default();
                        let href = format!(
                            "/dav/calendars/{principal_id}/{}/{}.ics",
                            calendar.calendar_id, resource_name
                        );
                        responses.push(sync_deleted_response_xml(&href));
                    }
                    "created" | "updated" => {
                        let Some(event_id) = change.event_id else {
                            continue;
                        };
                        let resource = match repository
                            .ensure_resource(calendar.calendar_id, event_id, now)
                            .await
                        {
                            Ok(resource) => resource,
                            Err(_) => return dav_server_error(accounts.metrics()),
                        };
                        match events.event(calendar.calendar_id, event_id).await {
                            Ok(Some(event)) => {
                                if let Some((ical, etag)) = ical_and_etag(
                                    accounts.pool(),
                                    calendar.calendar_id,
                                    &resource,
                                    &event,
                                    access,
                                )
                                .await
                                {
                                    let href = format!(
                                        "/dav/calendars/{principal_id}/{}/{}.ics",
                                        calendar.calendar_id, resource.resource_name
                                    );
                                    responses.push(sync_changed_response_xml(&href, &etag, &ical));
                                }
                            }
                            Ok(None) => {}
                            Err(_) => return dav_server_error(accounts.metrics()),
                        }
                    }
                    _ => {}
                }
            }
        }
        None => {
            // Initial snapshot: return every currently exposed event and mint a
            // token at the latest revision so the next call is incremental.
            let event_ids = match repository
                .list_exposed_event_ids(calendar.calendar_id)
                .await
            {
                Ok(event_ids) => event_ids,
                Err(_) => return dav_server_error(accounts.metrics()),
            };
            for event_id in &event_ids {
                let resource = match repository
                    .ensure_resource(calendar.calendar_id, *event_id, now)
                    .await
                {
                    Ok(resource) => resource,
                    Err(_) => return dav_server_error(accounts.metrics()),
                };
                match events.event(calendar.calendar_id, *event_id).await {
                    Ok(Some(event)) => {
                        if let Some((ical, etag)) = ical_and_etag(
                            accounts.pool(),
                            calendar.calendar_id,
                            &resource,
                            &event,
                            access,
                        )
                        .await
                        {
                            let href = format!(
                                "/dav/calendars/{principal_id}/{}/{}.ics",
                                calendar.calendar_id, resource.resource_name
                            );
                            responses.push(sync_changed_response_xml(&href, &etag, &ical));
                        }
                    }
                    Ok(None) => {}
                    Err(_) => return dav_server_error(accounts.metrics()),
                }
            }
            new_revision = match repository.latest_revision(calendar.calendar_id).await {
                Ok(revision) => revision,
                Err(_) => return dav_server_error(accounts.metrics()),
            };
        }
    }

    let new_token = accounts.encode_sync_token(new_revision);
    let collection_href = format!("/dav/calendars/{principal_id}/{}/", calendar.calendar_id);
    let collection_response = sync_collection_response_xml(&collection_href, &new_token);
    let body = std::iter::once(collection_response.as_str())
        .chain(responses.iter().map(|response| response.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    let xml = format!(
        r#"<?xml version="1.0" encoding="utf-8" ?>
<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:calendarserver/">
{body}
</D:multistatus>"#
    );
    (
        StatusCode::MULTI_STATUS,
        [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
        xml,
    )
        .into_response()
}

/// The collection's own response element, carrying the new sync token.
fn sync_collection_response_xml(href: &str, sync_token: &str) -> String {
    let escaped_href = xml_escape(href);
    format!(
        r#"  <D:response>
    <D:href>{escaped_href}</D:href>
    <D:propstat>
      <D:prop>
        <D:sync-token>{sync_token}</D:sync-token>
      </D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>"#
    )
}

/// A response element for an added or modified resource.
fn sync_changed_response_xml(href: &str, etag: &str, ical: &str) -> String {
    let escaped_href = xml_escape(href);
    let escaped_ical = xml_escape(ical);
    format!(
        r#"  <D:response>
    <D:href>{escaped_href}</D:href>
    <D:propstat>
      <D:prop>
        <D:getetag>{etag}</D:getetag>
        <D:getcontenttype>text/calendar; charset=utf-8</D:getcontenttype>
        <C:calendar-data>{escaped_ical}</C:calendar-data>
      </D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>"#
    )
}

/// A response element for a deleted resource, per RFC 6578: the `D:status` is
/// a direct child of `D:response` (not inside a `D:propstat`).
fn sync_deleted_response_xml(href: &str) -> String {
    let escaped_href = xml_escape(href);
    format!(
        r#"  <D:response>
    <D:href>{escaped_href}</D:href>
    <D:status>HTTP/1.1 404 Not Found</D:status>
  </D:response>"#
    )
}

/// Extract the resource name from a href that lives inside the given
/// collection. Returns `None` for foreign, malformed, or traversal paths.
fn href_within_collection(href: &str, principal_id: &str, calendar_id: i64) -> Option<String> {
    let prefix = format!("/dav/calendars/{principal_id}/{calendar_id}/");
    let rest = href.strip_prefix(&prefix)?;
    let resource_name = rest.strip_suffix(".ics")?;
    if resource_name.is_empty() || resource_name.contains('/') {
        return None;
    }
    Some(resource_name.to_owned())
}

fn multiget_response_xml(href: &str, payload: Option<(String, String)>) -> String {
    let escaped_href = xml_escape(href);
    match payload {
        Some((etag, ical)) => {
            let escaped_ical = xml_escape(&ical);
            format!(
                r#"  <D:response>
    <D:href>{escaped_href}</D:href>
    <D:propstat>
      <D:prop>
        <D:getetag>{etag}</D:getetag>
        <D:getcontenttype>text/calendar; charset=utf-8</D:getcontenttype>
        <C:calendar-data>{escaped_ical}</C:calendar-data>
      </D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>"#
            )
        }
        None => format!(
            r#"  <D:response>
    <D:href>{escaped_href}</D:href>
    <D:propstat>
      <D:prop>
        <D:getetag/>
        <D:getcontenttype/>
        <C:calendar-data/>
      </D:prop>
      <D:status>HTTP/1.1 404 Not Found</D:status>
    </D:propstat>
  </D:response>"#
        ),
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

fn dav_rate_limited(retry_after: i64) -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(header::RETRY_AFTER, retry_after.to_string())],
        "too many authentication attempts; retry later",
    )
        .into_response()
}

/// Extract the client key (IP) from the request for rate-limit purposes.
fn client_key_from_request(request: &Request) -> String {
    request
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|v| v.trim().to_string())
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Authenticate a DAV request, checking the rate limit first.
/// Returns `Ok((session, request))` on success, or `Err(response)` on failure.
#[allow(clippy::result_large_err)]
async fn authenticate_dav_request(
    accounts: &CaldavAccountService,
    request: Request,
) -> Result<(DavSession, Request), Response> {
    let authorization = request.headers().get(header::AUTHORIZATION).cloned();
    let Some(authorization) = authorization else {
        accounts.metrics().record_unauthorized();
        return Err(dav_unauthorized());
    };
    let client_key = client_key_from_request(&request);
    if let Err((retry_after, _)) = accounts.check_auth_rate_limit(&client_key) {
        return Err(dav_rate_limited(retry_after));
    }
    match accounts.authenticate(&authorization).await {
        Ok(session) => {
            accounts.record_auth_success();
            Ok((session, request))
        }
        Err(_) => {
            accounts.record_auth_failure();
            Err(dav_unauthorized())
        }
    }
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
        <D:current-user-principal>
          <D:href>{principal_url}</D:href>
        </D:current-user-principal>
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

fn privileges_for_role(role: &str) -> Vec<&'static str> {
    let Ok(role) = CalendarRole::from_str(role) else {
        return Vec::new();
    };
    let allows = |action: CalendarAction| {
        authorize_calendar_action(
            UserStatus::Active,
            Some(PlatformRole::User),
            Some(role),
            action,
        ) == AuthorizationDecision::Allow
    };
    let mut privileges = vec!["DAV:read-current-user-privilege-set"];
    let can_read = allows(CalendarAction::ReadDetails) || allows(CalendarAction::ReadFreeBusy);
    if can_read {
        privileges.push("DAV:read");
        privileges.push("DAV:calendar-subscribe");
    }
    if allows(CalendarAction::ReadFreeBusy) {
        privileges.push("DAV:calendar-write-freebusy");
    }
    let writable = allows(CalendarAction::CreateEvent) || allows(CalendarAction::EditAnyEvent);
    if writable {
        privileges.push("DAV:write-content");
        privileges.push("DAV:calendar-write");
    }
    if allows(CalendarAction::ManageSettings) {
        privileges.push("DAV:write-properties");
    }
    if allows(CalendarAction::ManageAcl) {
        privileges.push("DAV:read-acl");
        privileges.push("DAV:write-acl");
    }
    if writable && allows(CalendarAction::ManageSettings) {
        privileges.push("DAV:write");
    }
    if allows(CalendarAction::DeleteCalendar) {
        privileges.push("DAV:all");
    }
    privileges
}

fn privilege_set_xml(privileges: &[&str]) -> String {
    let entries = privileges
        .iter()
        .map(|privilege| format!("        <D:privilege>\n          <D:href>{privilege}</D:href>\n        </D:privilege>"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "      <D:current-user-privilege-set>\n{entries}\n      </D:current-user-privilege-set>"
    )
}

fn calendar_response_xml(
    accounts: &CaldavAccountService,
    principal_id: &str,
    calendar: &CaldavCalendar,
) -> String {
    let href = format!("/dav/calendars/{principal_id}/{}/", calendar.calendar_id);
    let displayname = xml_escape(&calendar.name);
    let color = xml_escape(&calendar.color);
    let description = calendar
        .description
        .as_deref()
        .map(xml_escape)
        .unwrap_or_default();
    let privileges = privileges_for_role(&calendar.role);
    let privilege_set = privilege_set_xml(&privileges);
    let owner_principal = calendar
        .owner_principal_id
        .as_ref()
        .map(|owner_principal_id| {
            format!(
                "      <D:principal-URL>\n        <D:href>{}</D:href>\n      </D:principal-URL>",
                accounts.principal_url(owner_principal_id)
            )
        });
    let owner_block = owner_principal
        .map(|block| format!("\n{block}"))
        .unwrap_or_default();
    format!(
        r#"  <D:response>
    <D:href>{href}</D:href>
    <D:propstat>
      <D:prop>
        <D:resourcetype>
          <D:collection/>
          <C:calendar/>
        </D:resourcetype>
        <D:displayname>{displayname}</D:displayname>
        <D:description>{description}</D:description>
        <C:calendar-color>{color}</C:calendar-color>
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
{privilege_set}{owner_block}
      </D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>"#
    )
}

fn render_calendar_home(
    accounts: &CaldavAccountService,
    principal_id: &str,
    principal: &PrincipalInfo,
    calendars: &[CaldavCalendar],
    depth: DavDepth,
) -> Response {
    let href = format!("/dav/calendars/{principal_id}/");
    let displayname = xml_escape(&principal.display_name);
    let home_privileges = vec!["DAV:read", "DAV:read-current-user-privilege-set"];
    let home_privilege_set = privilege_set_xml(&home_privileges);
    let home = format!(
        r#"  <D:response>
    <D:href>{href}</D:href>
    <D:propstat>
      <D:prop>
        <D:resourcetype>
          <D:collection/>
        </D:resourcetype>
        <D:displayname>{displayname}</D:displayname>
        <D:principal-URL>
          <D:href>{}</D:href>
        </D:principal-URL>
{home_privilege_set}
      </D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>"#,
        accounts.principal_url(principal_id)
    );
    let children = if depth == DavDepth::One {
        calendars
            .iter()
            .map(|calendar| calendar_response_xml(accounts, principal_id, calendar))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        String::new()
    };
    let body = if children.is_empty() {
        format!(
            r#"<?xml version="1.0" encoding="utf-8" ?>
<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:calendarserver/">
{home}
</D:multistatus>"#
        )
    } else {
        format!(
            r#"<?xml version="1.0" encoding="utf-8" ?>
<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:calendarserver/">
{home}
{children}
</D:multistatus>"#
        )
    };
    (
        StatusCode::MULTI_STATUS,
        [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
        body,
    )
        .into_response()
}

/// Canonical iCalendar content and strong ETag for one exposed event, rendered
/// at the caller's read scope. Free-busy readers receive a placeholder that
/// carries only the time window. Recurring events are serialized as a series
/// (master + exceptions) in a single VCALENDAR.
async fn ical_and_etag(
    pool: &SqlitePool,
    calendar_id: i64,
    resource: &CaldavEventResource,
    event: &Event,
    access: DavReadAccess,
) -> Option<(String, String)> {
    let timing = match &event.timing {
        EventTiming::Timed {
            start_utc,
            end_utc,
            timezone,
        } => CaldavIcalTiming::Timed {
            start_utc: *start_utc,
            end_utc: *end_utc,
            timezone: timezone.clone(),
        },
        EventTiming::AllDay {
            start_date,
            end_date,
        } => CaldavIcalTiming::AllDay {
            start_date: start_date.clone(),
            end_date: end_date.clone(),
        },
    };
    let status = match event.status {
        EventStatus::Tentative => "TENTATIVE",
        EventStatus::Confirmed => "CONFIRMED",
        EventStatus::Cancelled => "CANCELLED",
    }
    .to_owned();
    let free_busy = matches!(access, DavReadAccess::FreeBusy);

    // Allowlisted client-owned metadata (T16). Only exposed to detail readers;
    // free-busy placeholders carry only the time window.
    let repository = CaldavRepository::new(pool.clone());
    let client_properties = repository
        .load_client_properties(event.id)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();

    let recurrence_rule: Option<String> =
        sqlx::query_scalar("SELECT recurrence_rule FROM events WHERE id = ? AND calendar_id = ?")
            .bind(event.id)
            .bind(calendar_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .filter(|r: &String| !r.is_empty());

    let ical = if let Some(rrule) = recurrence_rule {
        let exceptions = fetch_recurring_exceptions(pool, event.id, &timing).await;
        let master = CaldavIcalEvent {
            uid: resource.uid.clone(),
            summary: event.title.clone(),
            description: event.description.clone(),
            location: event.location.clone(),
            status: Some(status.clone()),
            timing: timing.clone(),
            dtstamp: event.created_at,
            sequence: event.version as u64,
            free_busy,
            rrule: Some(rrule),
            exdates: exceptions
                .iter()
                .filter(|e| e.is_deleted)
                .map(|e| e.date_value.clone())
                .collect(),
            recurrence_id: None,
            categories: client_properties.categories.clone(),
            url: client_properties.url.clone(),
            transp: client_properties.transp.clone(),
            alarms: client_properties.alarms.clone(),
            x_properties: client_properties.x_properties.clone(),
        };
        let exception_events: Vec<CaldavIcalEvent> = exceptions
            .iter()
            .filter(|e| !e.is_deleted)
            .map(|e| CaldavIcalEvent {
                uid: resource.uid.clone(),
                summary: e.title.clone().unwrap_or_else(|| event.title.clone()),
                description: e.description.clone(),
                location: e.location.clone(),
                status: e
                    .status
                    .clone()
                    .map(|s| s.to_uppercase())
                    .or_else(|| Some(status.clone())),
                timing: e.timing.clone().unwrap_or_else(|| timing.clone()),
                dtstamp: event.created_at,
                sequence: event.version as u64,
                free_busy,
                rrule: None,
                exdates: Vec::new(),
                recurrence_id: Some(e.date_value.clone()),
                categories: Vec::new(),
                url: None,
                transp: None,
                alarms: Vec::new(),
                x_properties: Vec::new(),
            })
            .collect();
        serialize_recurring_series(&master, &exception_events)
    } else {
        serialize_event_resource(&CaldavIcalEvent {
            uid: resource.uid.clone(),
            summary: event.title.clone(),
            description: event.description.clone(),
            location: event.location.clone(),
            status: Some(status),
            timing,
            dtstamp: event.created_at,
            sequence: event.version as u64,
            free_busy,
            rrule: None,
            exdates: Vec::new(),
            recurrence_id: None,
            categories: client_properties.categories,
            url: client_properties.url,
            transp: client_properties.transp,
            alarms: client_properties.alarms,
            x_properties: client_properties.x_properties,
        })
    };
    let etag = etag_for_ical(&ical);
    Some((ical, etag))
}

struct RecurringException {
    is_deleted: bool,
    date_value: CaldavIcalDateValue,
    title: Option<String>,
    description: Option<String>,
    location: Option<String>,
    status: Option<String>,
    timing: Option<CaldavIcalTiming>,
}

#[derive(sqlx::FromRow)]
struct RecurringExceptionRow {
    is_deleted: bool,
    recurrence_id: Option<i64>,
    recurrence_date: Option<String>,
    title: Option<String>,
    description: Option<String>,
    location: Option<String>,
    status: Option<String>,
    timed_start_utc: Option<i64>,
    timed_end_utc: Option<i64>,
    event_timezone: Option<String>,
    all_day_start_date: Option<String>,
    all_day_end_date: Option<String>,
}

async fn fetch_recurring_exceptions(
    pool: &SqlitePool,
    series_id: i64,
    _master_timing: &CaldavIcalTiming,
) -> Vec<RecurringException> {
    let rows: Vec<RecurringExceptionRow> = sqlx::query_as(
        "SELECT is_deleted, recurrence_id, recurrence_date, title, description,
                location, status, timed_start_utc, timed_end_utc, event_timezone,
                all_day_start_date, all_day_end_date
         FROM event_recurrence_exceptions WHERE series_id = ?
         ORDER BY recurrence_id, recurrence_date",
    )
    .bind(series_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    rows.into_iter()
        .filter_map(|row| {
            let RecurringExceptionRow {
                is_deleted,
                recurrence_id,
                recurrence_date,
                title,
                description,
                location,
                status,
                timed_start_utc,
                timed_end_utc,
                event_timezone,
                all_day_start_date,
                all_day_end_date,
            } = row;
            let date_value = if let Some(utc) = recurrence_id {
                CaldavIcalDateValue::Timed(utc)
            } else {
                let date = recurrence_date?;
                CaldavIcalDateValue::AllDay(date)
            };
            let timing = if let (Some(start), Some(end), Some(tz)) =
                (timed_start_utc, timed_end_utc, event_timezone)
            {
                Some(CaldavIcalTiming::Timed {
                    start_utc: start,
                    end_utc: end,
                    timezone: tz,
                })
            } else if let (Some(start), Some(end)) = (all_day_start_date, all_day_end_date) {
                Some(CaldavIcalTiming::AllDay {
                    start_date: start,
                    end_date: end,
                })
            } else {
                None
            };
            Some(RecurringException {
                is_deleted,
                date_value,
                title,
                description,
                location,
                status,
                timing,
            })
        })
        .collect()
}

fn resource_response_xml(
    principal_id: &str,
    calendar_id: i64,
    resource: &CaldavEventResource,
    etag: &str,
) -> String {
    let href = format!(
        "/dav/calendars/{principal_id}/{calendar_id}/{}.ics",
        resource.resource_name
    );
    format!(
        r#"  <D:response>
    <D:href>{href}</D:href>
    <D:propstat>
      <D:prop>
        <D:resourcetype>
          <C:calendar-component/>
          <C:vevent/>
        </D:resourcetype>
        <D:getetag>{etag}</D:getetag>
        <D:getcontenttype>text/calendar; charset=utf-8</D:getcontenttype>
      </D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>"#
    )
}

fn render_calendar(
    accounts: &CaldavAccountService,
    principal_id: &str,
    calendar: &CaldavCalendar,
    depth: DavDepth,
    resources: &[(CaldavEventResource, String)],
) -> Response {
    let response = calendar_response_xml(accounts, principal_id, calendar);
    let children = if depth == DavDepth::One {
        resources
            .iter()
            .map(|(resource, etag)| {
                resource_response_xml(principal_id, calendar.calendar_id, resource, etag)
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        String::new()
    };
    let body = if children.is_empty() {
        format!(
            r#"<?xml version="1.0" encoding="utf-8" ?>
<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:calendarserver/">
{response}
</D:multistatus>"#
        )
    } else {
        format!(
            r#"<?xml version="1.0" encoding="utf-8" ?>
<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:calendarserver/">
{response}
{children}
</D:multistatus>"#
        )
    };
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
        CaldavAuthError::ResourceExists | CaldavAuthError::UidConflict => ApiError::internal(),
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
            sqlx::query(
                "CREATE TABLE calendars (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    owner_user_id INTEGER NOT NULL REFERENCES users(id),
                    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
                    description TEXT,
                    color TEXT NOT NULL CHECK (length(trim(color)) > 0),
                    default_timezone TEXT NOT NULL CHECK (length(trim(default_timezone)) > 0),
                    default_event_visibility TEXT NOT NULL
                        CHECK (default_event_visibility IN ('default', 'public', 'private')),
                    default_notification_rules_json TEXT,
                    archived INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1)),
                    version INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                )",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "CREATE TABLE calendar_acl (
                    calendar_id INTEGER NOT NULL REFERENCES calendars(id) ON DELETE CASCADE,
                    user_id INTEGER NOT NULL REFERENCES users(id),
                    role TEXT NOT NULL CHECK (
                        role IN ('owner', 'manager', 'editor', 'viewer', 'free_busy_viewer')
                    ),
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    PRIMARY KEY (calendar_id, user_id)
                )",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "CREATE TABLE events (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    calendar_id INTEGER NOT NULL REFERENCES calendars(id) ON DELETE CASCADE,
                    title TEXT NOT NULL CHECK (length(trim(title)) > 0),
                    description TEXT,
                    location TEXT,
                    status TEXT NOT NULL CHECK (status IN ('tentative', 'confirmed', 'cancelled')),
                    event_kind TEXT NOT NULL CHECK (event_kind IN ('timed', 'all_day')),
                    timed_start_utc INTEGER,
                    timed_end_utc INTEGER,
                    event_timezone TEXT,
                    all_day_start_date TEXT,
                    all_day_end_date TEXT,
                    created_by_user_id INTEGER NOT NULL REFERENCES users(id),
                    last_edited_by_user_id INTEGER NOT NULL REFERENCES users(id),
                    version INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    recurrence_rule TEXT
                )",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "CREATE TABLE caldav_event_resources (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    event_id INTEGER UNIQUE REFERENCES events(id),
                    calendar_id INTEGER NOT NULL REFERENCES calendars(id),
                    uid TEXT NOT NULL CHECK (length(trim(uid)) > 0),
                    resource_name TEXT NOT NULL CHECK (length(trim(resource_name)) > 0),
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    deleted_at INTEGER
                )",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "CREATE UNIQUE INDEX idx_caldav_resources_calendar_uid_live
                 ON caldav_event_resources(calendar_id, uid) WHERE deleted_at IS NULL",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "CREATE UNIQUE INDEX idx_caldav_resources_calendar_name_live
                 ON caldav_event_resources(calendar_id, resource_name) WHERE deleted_at IS NULL",
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

        async fn insert_calendar(
            &self,
            owner_user_id: i64,
            name: &str,
            color: &str,
            archived: bool,
        ) -> i64 {
            let now = 1000i64;
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO calendars (
                    owner_user_id, name, description, color, default_timezone,
                    default_event_visibility, default_notification_rules_json, archived,
                    version, created_at, updated_at
                 ) VALUES (?, ?, NULL, ?, 'UTC', 'default', NULL, ?, 1, ?, ?)
                 RETURNING id",
            )
            .bind(owner_user_id)
            .bind(name)
            .bind(color)
            .bind(archived as i32)
            .bind(now)
            .bind(now)
            .fetch_one(&self.pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO calendar_acl (calendar_id, user_id, role, created_at, updated_at)
                 VALUES (?, ?, 'owner', ?, ?)",
            )
            .bind(id)
            .bind(owner_user_id)
            .bind(now)
            .bind(now)
            .execute(&self.pool)
            .await
            .unwrap();
            id
        }

        async fn grant_role(&self, calendar_id: i64, user_id: i64, role: &str) {
            let now = 1000i64;
            sqlx::query(
                "INSERT INTO calendar_acl (calendar_id, user_id, role, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(calendar_id)
            .bind(user_id)
            .bind(role)
            .bind(now)
            .bind(now)
            .execute(&self.pool)
            .await
            .unwrap();
        }

        async fn revoke_role(&self, calendar_id: i64, user_id: i64) {
            sqlx::query("DELETE FROM calendar_acl WHERE calendar_id = ? AND user_id = ?")
                .bind(calendar_id)
                .bind(user_id)
                .execute(&self.pool)
                .await
                .unwrap();
        }
    }

    fn basic_header(username: &str, password: &str) -> HeaderValue {
        let encoded = B64.encode(format!("{username}:{password}"));
        HeaderValue::from_str(&format!("Basic {encoded}")).unwrap()
    }

    #[tokio::test]
    async fn propfind_dav_root_returns_current_user_principal_link() {
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
            .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
            .body(Body::from(
                r#"<?xml version="1.0" encoding="utf-8" ?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:current-user-principal/>
  </D:prop>
</D:propfind>"#,
            ))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::MULTI_STATUS);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("<D:current-user-principal>"));
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
            "PROPFIND, OPTIONS, REPORT",
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

    async fn propfind(
        app: Router,
        uri: &str,
        username: &str,
        password: &str,
        depth: Option<&str>,
    ) -> (StatusCode, String) {
        let mut builder = Request::builder()
            .method(Method::from_bytes(b"PROPFIND").unwrap())
            .uri(uri)
            .header(header::AUTHORIZATION, basic_header(username, password));
        if let Some(depth) = depth {
            builder = builder.header("depth", depth);
        }
        let request = builder.body(Body::empty()).unwrap();
        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    async fn setup_owner_with_calendars() -> (
        TestDb,
        CaldavAccountService,
        i64,
        String,
        String,
        i64,
        i64,
        i64,
    ) {
        let db = TestDb::new().await;
        let key = SecretKey::generate();
        let accounts = CaldavAccountService::new_at(
            db.pool.clone(),
            key,
            url::Url::parse("http://127.0.0.1:3000").unwrap(),
            1000,
        );
        let owner_id = db.insert_user("owner@example.test").await;
        let issued = accounts
            .issue_credential(owner_id, "Phone".into())
            .await
            .unwrap();
        let principal_id = accounts
            .status(owner_id)
            .await
            .unwrap()
            .principal_id
            .unwrap();
        let active = db.insert_calendar(owner_id, "Work", "#ff0000", false).await;
        let archived = db.insert_calendar(owner_id, "Old", "#00ff00", true).await;
        let foreign_owner = db.insert_user("foreign@example.test").await;
        let foreign = db
            .insert_calendar(foreign_owner, "Foreign", "#0000ff", false)
            .await;
        (
            db,
            accounts,
            owner_id,
            issued.password.expose().to_owned(),
            principal_id,
            active,
            archived,
            foreign,
        )
    }

    #[tokio::test]
    async fn calendar_home_depth_one_lists_active_authorized_calendars() {
        let (_db, accounts, _owner_id, password, principal_id, active, archived, foreign) =
            setup_owner_with_calendars().await;

        let app = build_caldav_router(accounts);
        let (status, body) = propfind(
            app,
            &format!("/dav/calendars/{principal_id}/"),
            "owner@example.test",
            &password,
            Some("1"),
        )
        .await;

        assert_eq!(status, StatusCode::MULTI_STATUS);
        assert!(body.contains(&format!("/dav/calendars/{principal_id}/{active}/")));
        assert!(body.contains("<D:displayname>Work</D:displayname>"));
        assert!(body.contains("<C:calendar-color>#ff0000</C:calendar-color>"));
        assert!(body.contains("<C:calendar/>"));
        assert!(body.contains("<D:collection/>"));
        // Archived and foreign calendars must not appear.
        assert!(!body.contains(&format!("/dav/calendars/{principal_id}/{archived}/")));
        assert!(!body.contains(&format!("/dav/calendars/{principal_id}/{foreign}/")));
        assert!(!body.contains("<D:displayname>Old</D:displayname>"));
        assert!(!body.contains("<D:displayname>Foreign</D:displayname>"));
    }

    #[tokio::test]
    async fn calendar_home_depth_zero_returns_home_only() {
        let (_db, accounts, _owner_id, password, principal_id, active, _archived, _foreign) =
            setup_owner_with_calendars().await;

        let app = build_caldav_router(accounts);
        let (status, body) = propfind(
            app,
            &format!("/dav/calendars/{principal_id}/"),
            "owner@example.test",
            &password,
            Some("0"),
        )
        .await;

        assert_eq!(status, StatusCode::MULTI_STATUS);
        assert!(body.contains(&format!("/dav/calendars/{principal_id}/")));
        assert!(body.contains("<D:collection/>"));
        // No child calendars at Depth 0.
        assert!(!body.contains(&format!("/dav/calendars/{principal_id}/{active}/")));
        assert!(!body.contains("<C:calendar/>"));
    }

    #[tokio::test]
    async fn calendar_collection_propfind_returns_properties_and_owner_privileges() {
        let (_db, accounts, _owner_id, password, principal_id, active, _archived, _foreign) =
            setup_owner_with_calendars().await;

        let app = build_caldav_router(accounts);
        let (status, body) = propfind(
            app,
            &format!("/dav/calendars/{principal_id}/{active}/"),
            "owner@example.test",
            &password,
            Some("0"),
        )
        .await;

        assert_eq!(status, StatusCode::MULTI_STATUS);
        assert!(body.contains("<C:calendar/>"));
        assert!(body.contains("<D:displayname>Work</D:displayname>"));
        assert!(body.contains("<C:calendar-color>#ff0000</C:calendar-color>"));
        assert!(body.contains("calendar-query"));
        assert!(body.contains("calendar-multiget"));
        assert!(body.contains("sync-collection"));
        // Owner gets the full privilege set.
        assert!(body.contains("<D:href>DAV:all</D:href>"));
        assert!(body.contains("<D:href>DAV:read</D:href>"));
        assert!(body.contains("<D:href>DAV:write</D:href>"));
        assert!(body.contains("<D:href>DAV:write-properties</D:href>"));
        assert!(body.contains("<D:href>DAV:write-content</D:href>"));
        assert!(body.contains("<D:href>DAV:write-acl</D:href>"));
        assert!(body.contains("<D:href>DAV:calendar-write</D:href>"));
    }

    #[tokio::test]
    async fn calendar_privileges_match_editor_viewer_and_freebusy_roles() {
        let (db, accounts, _owner_id, _owner_password, principal_id, active, _archived, _foreign) =
            setup_owner_with_calendars().await;

        let editor_id = db.insert_user("editor@example.test").await;
        let editor_password = accounts
            .issue_credential(editor_id, "Phone".into())
            .await
            .unwrap()
            .password
            .expose()
            .to_owned();
        db.grant_role(active, editor_id, "editor").await;

        let viewer_id = db.insert_user("viewer@example.test").await;
        let viewer_password = accounts
            .issue_credential(viewer_id, "Phone".into())
            .await
            .unwrap()
            .password
            .expose()
            .to_owned();
        db.grant_role(active, viewer_id, "viewer").await;

        let freebusy_id = db.insert_user("freebusy@example.test").await;
        let freebusy_password = accounts
            .issue_credential(freebusy_id, "Phone".into())
            .await
            .unwrap()
            .password
            .expose()
            .to_owned();
        db.grant_role(active, freebusy_id, "free_busy_viewer").await;

        let uri = format!("/dav/calendars/{principal_id}/{active}/");

        let app = build_caldav_router(accounts.clone());
        let (_, editor_body) = propfind(
            app,
            &uri,
            "editor@example.test",
            &editor_password,
            Some("0"),
        )
        .await;
        assert!(editor_body.contains("<D:href>DAV:read</D:href>"));
        assert!(editor_body.contains("<D:href>DAV:write-content</D:href>"));
        assert!(editor_body.contains("<D:href>DAV:calendar-write</D:href>"));
        assert!(!editor_body.contains("<D:href>DAV:all</D:href>"));
        assert!(!editor_body.contains("<D:href>DAV:write-properties</D:href>"));
        assert!(!editor_body.contains("<D:href>DAV:write-acl</D:href>"));

        let app = build_caldav_router(accounts.clone());
        let (_, viewer_body) = propfind(
            app,
            &uri,
            "viewer@example.test",
            &viewer_password,
            Some("0"),
        )
        .await;
        assert!(viewer_body.contains("<D:href>DAV:read</D:href>"));
        assert!(!viewer_body.contains("<D:href>DAV:write-content</D:href>"));
        assert!(!viewer_body.contains("<D:href>DAV:calendar-write</D:href>"));
        assert!(!viewer_body.contains("<D:href>DAV:all</D:href>"));

        let app = build_caldav_router(accounts);
        let (_, freebusy_body) = propfind(
            app,
            &uri,
            "freebusy@example.test",
            &freebusy_password,
            Some("0"),
        )
        .await;
        assert!(freebusy_body.contains("<D:href>DAV:read</D:href>"));
        assert!(freebusy_body.contains("<D:href>DAV:calendar-write-freebusy</D:href>"));
        assert!(!freebusy_body.contains("<D:href>DAV:write-content</D:href>"));
        assert!(!freebusy_body.contains("<D:href>DAV:all</D:href>"));
    }

    #[tokio::test]
    async fn revoked_calendar_does_not_appear_and_propfind_is_404() {
        let (db, accounts, _owner_id, _owner_password, principal_id, active, _archived, _foreign) =
            setup_owner_with_calendars().await;

        let revoked_id = db.insert_user("revoked@example.test").await;
        let revoked_password = accounts
            .issue_credential(revoked_id, "Phone".into())
            .await
            .unwrap()
            .password
            .expose()
            .to_owned();
        db.grant_role(active, revoked_id, "editor").await;
        db.revoke_role(active, revoked_id).await;

        let app = build_caldav_router(accounts);
        let (status, _body) = propfind(
            app,
            &format!("/dav/calendars/{principal_id}/{active}/"),
            "revoked@example.test",
            &revoked_password,
            Some("0"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn cross_user_calendar_is_hidden_and_propfind_is_404() {
        let (db, accounts, _owner_id, _owner_password, principal_id, _active, _archived, foreign) =
            setup_owner_with_calendars().await;

        let intruder_id = db.insert_user("intruder@example.test").await;
        let intruder_password = accounts
            .issue_credential(intruder_id, "Phone".into())
            .await
            .unwrap()
            .password
            .expose()
            .to_owned();

        let app = build_caldav_router(accounts);
        let (status, _body) = propfind(
            app,
            &format!("/dav/calendars/{principal_id}/{foreign}/"),
            "intruder@example.test",
            &intruder_password,
            Some("0"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn calendar_home_cross_user_principal_is_404() {
        let (db, accounts, _owner_id, _owner_password, principal_id, _active, _archived, _foreign) =
            setup_owner_with_calendars().await;

        let intruder_id = db.insert_user("intruder@example.test").await;
        let intruder_password = accounts
            .issue_credential(intruder_id, "Phone".into())
            .await
            .unwrap()
            .password
            .expose()
            .to_owned();

        let app = build_caldav_router(accounts);
        let (status, _body) = propfind(
            app,
            &format!("/dav/calendars/{principal_id}/"),
            "intruder@example.test",
            &intruder_password,
            Some("1"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn calendar_home_unauthenticated_is_401() {
        let (_db, accounts, _owner_id, _password, principal_id, _a, _b, _c) =
            setup_owner_with_calendars().await;

        let app = build_caldav_router(accounts);
        let request = Request::builder()
            .method(Method::from_bytes(b"PROPFIND").unwrap())
            .uri(format!("/dav/calendars/{principal_id}/"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn unknown_calendar_key_is_404() {
        let (_db, accounts, _owner_id, password, principal_id, _a, _b, _c) =
            setup_owner_with_calendars().await;

        let app = build_caldav_router(accounts);
        let (status, _body) = propfind(
            app,
            &format!("/dav/calendars/{principal_id}/not-a-number/"),
            "owner@example.test",
            &password,
            Some("0"),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn shared_calendar_appears_in_grantee_calendar_home() {
        let (
            db,
            accounts,
            _owner_id,
            _owner_password,
            _owner_principal,
            active,
            _archived,
            _foreign,
        ) = setup_owner_with_calendars().await;

        // Grant editor access to a second user (a non-owner grantee).
        let editor_id = db.insert_user("editor@example.test").await;
        let editor_password = accounts
            .issue_credential(editor_id, "Phone".into())
            .await
            .unwrap()
            .password
            .expose()
            .to_owned();
        db.grant_role(active, editor_id, "editor").await;

        let editor_principal_id = accounts
            .status(editor_id)
            .await
            .unwrap()
            .principal_id
            .unwrap();

        let app = build_caldav_router(accounts.clone());
        let (status, body) = propfind(
            app,
            &format!("/dav/calendars/{editor_principal_id}/"),
            "editor@example.test",
            &editor_password,
            Some("1"),
        )
        .await;

        assert_eq!(status, StatusCode::MULTI_STATUS);
        // The shared calendar must appear in the grantee's calendar home.
        let href = format!("/dav/calendars/{editor_principal_id}/{active}/");
        assert!(
            body.contains(&href),
            "shared calendar must appear in the grantee's calendar home"
        );

        // The listed href must resolve for the grantee with editor privileges.
        let app = build_caldav_router(accounts);
        let (href_status, href_body) = propfind(
            app,
            &href,
            "editor@example.test",
            &editor_password,
            Some("0"),
        )
        .await;
        assert_eq!(href_status, StatusCode::MULTI_STATUS);
        assert!(href_body.contains("<C:calendar/>"));
        assert!(href_body.contains("<D:displayname>Work</D:displayname>"));
        assert!(href_body.contains("<C:calendar-color>#ff0000</C:calendar-color>"));
        assert!(href_body.contains("<D:href>DAV:write-content</D:href>"));
        assert!(!href_body.contains("<D:href>DAV:all</D:href>"));
    }

    async fn insert_timed_event(
        db: &TestDb,
        calendar_id: i64,
        user_id: i64,
        title: &str,
        start_utc: i64,
        end_utc: i64,
    ) -> i64 {
        let now = 1000i64;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO events (
                calendar_id, title, description, location, status, event_kind,
                timed_start_utc, timed_end_utc, event_timezone,
                all_day_start_date, all_day_end_date, created_by_user_id,
                last_edited_by_user_id, version, created_at, updated_at
             ) VALUES (?, ?, NULL, NULL, 'confirmed', 'timed', ?, ?, 'UTC',
                NULL, NULL, ?, ?, 1, ?, ?)
             RETURNING id",
        )
        .bind(calendar_id)
        .bind(title)
        .bind(start_utc)
        .bind(end_utc)
        .bind(user_id)
        .bind(user_id)
        .bind(now)
        .bind(now)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        id
    }

    async fn insert_all_day_event(
        db: &TestDb,
        calendar_id: i64,
        user_id: i64,
        title: &str,
        start_date: &str,
        end_date: &str,
    ) -> i64 {
        let now = 1000i64;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO events (
                calendar_id, title, description, location, status, event_kind,
                timed_start_utc, timed_end_utc, event_timezone,
                all_day_start_date, all_day_end_date, created_by_user_id,
                last_edited_by_user_id, version, created_at, updated_at
             ) VALUES (?, ?, NULL, NULL, 'confirmed', 'all_day',
                NULL, NULL, NULL, ?, ?, ?, ?, 1, ?, ?)
             RETURNING id",
        )
        .bind(calendar_id)
        .bind(title)
        .bind(start_date)
        .bind(end_date)
        .bind(user_id)
        .bind(user_id)
        .bind(now)
        .bind(now)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        id
    }

    async fn report_request(
        app: Router,
        uri: &str,
        username: &str,
        password: &str,
        body: &str,
    ) -> (StatusCode, String) {
        let request = Request::builder()
            .method(Method::from_bytes(b"REPORT").unwrap())
            .uri(uri)
            .header(header::AUTHORIZATION, basic_header(username, password))
            .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
            .body(Body::from(body.to_owned()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    fn calendar_query_xml(start: &str, end: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:calendarserver/">
  <D:prop>
    <D:getetag/>
    <D:getcontenttype/>
    <C:calendar-data/>
  </D:prop>
  <C:filter>
    <C:time-range start="{start}" end="{end}"/>
  </C:filter>
</C:calendar-query>"#
        )
    }

    #[tokio::test]
    async fn report_calendar_query_returns_events_in_range() {
        let (db, accounts, owner_id, password, principal_id, active, _archived, _foreign) =
            setup_owner_with_calendars().await;

        // Event in range: 2026-01-15 00:00 UTC to 01:00 UTC
        insert_timed_event(
            &db,
            active,
            owner_id,
            "In Range",
            1_768_435_200,
            1_768_438_800,
        )
        .await;
        // Event outside range: 2025-06-01
        insert_timed_event(
            &db,
            active,
            owner_id,
            "Out of Range",
            1_748_736_000,
            1_748_739_600,
        )
        .await;

        let app = build_caldav_router(accounts);
        let body = calendar_query_xml("20260101T000000Z", "20260131T235959Z");
        let (status, resp) = report_request(
            app,
            &format!("/dav/calendars/{principal_id}/{active}/"),
            "owner@example.test",
            &password,
            &body,
        )
        .await;

        assert_eq!(status, StatusCode::MULTI_STATUS);
        assert!(resp.contains("In Range"), "should contain in-range event");
        assert!(
            !resp.contains("Out of Range"),
            "should not contain out-of-range event"
        );
        assert!(resp.contains("<D:getetag>"));
        assert!(resp.contains("<C:calendar-data>"));
    }

    /// T13: a calendar-query REPORT returns all-day events whose dates fall in
    /// the requested range, rendered as DATE values.
    #[tokio::test]
    async fn report_calendar_query_returns_all_day_events_in_range() {
        let (db, accounts, owner_id, password, principal_id, active, _archived, _foreign) =
            setup_owner_with_calendars().await;

        // All-day event in range: 2026-01-15 through 2026-01-17 (exclusive).
        insert_all_day_event(
            &db,
            active,
            owner_id,
            "All Day In Range",
            "2026-01-15",
            "2026-01-17",
        )
        .await;
        // All-day event outside range: 2025-06-01 through 2025-06-03.
        insert_all_day_event(
            &db,
            active,
            owner_id,
            "All Day Out Of Range",
            "2025-06-01",
            "2025-06-03",
        )
        .await;

        let app = build_caldav_router(accounts);
        let body = calendar_query_xml("20260101T000000Z", "20260131T235959Z");
        let (status, resp) = report_request(
            app,
            &format!("/dav/calendars/{principal_id}/{active}/"),
            "owner@example.test",
            &password,
            &body,
        )
        .await;

        assert_eq!(status, StatusCode::MULTI_STATUS);
        assert!(
            resp.contains("All Day In Range"),
            "should contain in-range all-day event"
        );
        assert!(
            !resp.contains("All Day Out Of Range"),
            "should not contain out-of-range all-day event"
        );
        // The in-range all-day event must be rendered as DATE values.
        assert!(resp.contains("DTSTART;VALUE=DATE:20260115"));
        assert!(resp.contains("DTEND;VALUE=DATE:20260117"));
    }

    #[tokio::test]
    async fn report_calendar_query_empty_range_returns_207_no_events() {
        let (db, accounts, owner_id, password, principal_id, active, _archived, _foreign) =
            setup_owner_with_calendars().await;

        insert_timed_event(
            &db,
            active,
            owner_id,
            "Some Event",
            1_768_435_200,
            1_768_438_800,
        )
        .await;

        let app = build_caldav_router(accounts);
        let body = calendar_query_xml("20200101T000000Z", "20200102T000000Z");
        let (status, resp) = report_request(
            app,
            &format!("/dav/calendars/{principal_id}/{active}/"),
            "owner@example.test",
            &password,
            &body,
        )
        .await;

        assert_eq!(status, StatusCode::MULTI_STATUS);
        assert!(!resp.contains("Some Event"));
        assert!(!resp.contains("<D:response>"));
    }

    #[tokio::test]
    async fn report_calendar_query_malformed_body_returns_400() {
        let (_db, accounts, _owner_id, password, principal_id, active, _archived, _foreign) =
            setup_owner_with_calendars().await;

        let app = build_caldav_router(accounts);
        let (status, _resp) = report_request(
            app,
            &format!("/dav/calendars/{principal_id}/{active}/"),
            "owner@example.test",
            &password,
            "this is not xml",
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn report_calendar_query_unsupported_filter_returns_400() {
        let (_db, accounts, _owner_id, password, principal_id, active, _archived, _foreign) =
            setup_owner_with_calendars().await;

        let body = r#"<C:calendar-query xmlns:C="urn:ietf:params:xml:ns:calendarserver/"><C:filter><C:comp-filter name="VEVENT"><C:prop-filter name="SUMMARY"><C:is-text>test</C:is-text></C:prop-filter></C:comp-filter></C:filter></C:calendar-query>"#;
        let app = build_caldav_router(accounts);
        let (status, _resp) = report_request(
            app,
            &format!("/dav/calendars/{principal_id}/{active}/"),
            "owner@example.test",
            &password,
            body,
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn report_calendar_query_unauthenticated_returns_401() {
        let (_db, accounts, _owner_id, _password, principal_id, active, _archived, _foreign) =
            setup_owner_with_calendars().await;

        let app = build_caldav_router(accounts);
        let request = Request::builder()
            .method(Method::from_bytes(b"REPORT").unwrap())
            .uri(format!("/dav/calendars/{principal_id}/{active}/"))
            .header(header::CONTENT_TYPE, "application/xml")
            .body(Body::from("<C:calendar-query/>".to_owned()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn report_calendar_query_cross_user_returns_404() {
        let (db, accounts, _owner_id, _owner_password, principal_id, active, _archived, _foreign) =
            setup_owner_with_calendars().await;

        let intruder_id = db.insert_user("intruder@example.test").await;
        let intruder_password = accounts
            .issue_credential(intruder_id, "Phone".into())
            .await
            .unwrap()
            .password
            .expose()
            .to_owned();

        let body = calendar_query_xml("20260101T000000Z", "20260131T235959Z");
        let app = build_caldav_router(accounts);
        let (status, _resp) = report_request(
            app,
            &format!("/dav/calendars/{principal_id}/{active}/"),
            "intruder@example.test",
            &intruder_password,
            &body,
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn report_calendar_query_range_too_large_returns_400() {
        let (_db, accounts, _owner_id, password, principal_id, active, _archived, _foreign) =
            setup_owner_with_calendars().await;

        let body = calendar_query_xml("20200101T000000Z", "20260101T000000Z");
        let app = build_caldav_router(accounts);
        let (status, _resp) = report_request(
            app,
            &format!("/dav/calendars/{principal_id}/{active}/"),
            "owner@example.test",
            &password,
            &body,
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn report_calendar_query_overlapping_event_included() {
        let (db, accounts, owner_id, password, principal_id, active, _archived, _foreign) =
            setup_owner_with_calendars().await;

        // Event: 2026-01-14 23:00 to 2026-01-15 01:00 UTC
        // Query range: 2026-01-15 00:00 to 2026-01-15 12:00 UTC
        // Event overlaps the range (starts before, ends after start)
        insert_timed_event(
            &db,
            active,
            owner_id,
            "Overlapping",
            1_768_431_600,
            1_768_438_800,
        )
        .await;

        let app = build_caldav_router(accounts);
        let body = calendar_query_xml("20260115T000000Z", "20260115T120000Z");
        let (status, resp) = report_request(
            app,
            &format!("/dav/calendars/{principal_id}/{active}/"),
            "owner@example.test",
            &password,
            &body,
        )
        .await;

        assert_eq!(status, StatusCode::MULTI_STATUS);
        assert!(resp.contains("Overlapping"));
    }

    async fn ensure_resource_name(db: &TestDb, calendar_id: i64, event_id: i64) -> String {
        let repo = CaldavRepository::new(db.pool.clone());
        let resource = repo
            .ensure_resource(calendar_id, event_id, 1000)
            .await
            .unwrap();
        resource.resource_name
    }

    fn multiget_xml(hrefs: &[String]) -> String {
        let href_elements = hrefs
            .iter()
            .map(|href| format!("<D:href>{href}</D:href>"))
            .collect::<Vec<_>>()
            .join("");
        format!(
            r#"<?xml version="1.0" encoding="utf-8" ?>
<C:calendar-multiget xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:calendarserver/">
  <D:prop>
    <D:getetag/>
    <D:getcontenttype/>
    <C:calendar-data/>
  </D:prop>
  {href_elements}
</C:calendar-multiget>"#
        )
    }

    fn resource_href(principal_id: &str, calendar_id: i64, name: &str) -> String {
        format!("/dav/calendars/{principal_id}/{calendar_id}/{name}.ics")
    }

    #[tokio::test]
    async fn report_calendar_multiget_returns_data_and_etags() {
        let (db, accounts, owner_id, password, principal_id, active, _archived, _foreign) =
            setup_owner_with_calendars().await;

        let event_id = insert_timed_event(
            &db,
            active,
            owner_id,
            "Multiget Event",
            1_768_435_200,
            1_768_438_800,
        )
        .await;
        let name = ensure_resource_name(&db, active, event_id).await;

        let app = build_caldav_router(accounts);
        let body = multiget_xml(&[resource_href(&principal_id, active, &name)]);
        let (status, resp) = report_request(
            app,
            &format!("/dav/calendars/{principal_id}/{active}/"),
            "owner@example.test",
            &password,
            &body,
        )
        .await;

        assert_eq!(status, StatusCode::MULTI_STATUS);
        assert!(resp.contains(&resource_href(&principal_id, active, &name)));
        assert!(resp.contains("Multiget Event"), "should contain event data");
        assert!(resp.contains("<D:getetag>"), "should contain an ETag");
        assert!(
            resp.contains("<C:calendar-data>"),
            "should contain calendar data"
        );
        assert!(resp.contains("HTTP/1.1 200 OK"));
    }

    #[tokio::test]
    async fn report_calendar_multiget_missing_href_returns_per_item_404() {
        let (db, accounts, owner_id, password, principal_id, active, _archived, _foreign) =
            setup_owner_with_calendars().await;

        let event_id = insert_timed_event(
            &db,
            active,
            owner_id,
            "Present Event",
            1_768_435_200,
            1_768_438_800,
        )
        .await;
        let name = ensure_resource_name(&db, active, event_id).await;
        let present_href = resource_href(&principal_id, active, &name);
        let missing_href = resource_href(&principal_id, active, "does-not-exist");

        let app = build_caldav_router(accounts);
        let body = multiget_xml(&[present_href.clone(), missing_href.clone()]);
        let (status, resp) = report_request(
            app,
            &format!("/dav/calendars/{principal_id}/{active}/"),
            "owner@example.test",
            &password,
            &body,
        )
        .await;

        // The report as a whole succeeds (207) even though one item is missing.
        assert_eq!(status, StatusCode::MULTI_STATUS);
        // The present item returns data.
        assert!(resp.contains("Present Event"));
        // The missing item yields a per-item 404, not a whole-report failure.
        assert!(resp.contains(&missing_href));
        assert!(resp.contains("HTTP/1.1 404 Not Found"));
    }

    #[tokio::test]
    async fn report_calendar_multiget_foreign_path_leaks_nothing() {
        let (db, accounts, owner_id, password, principal_id, active, _archived, foreign) =
            setup_owner_with_calendars().await;

        // A real event exists in the owner's active calendar.
        let event_id = insert_timed_event(
            &db,
            active,
            owner_id,
            "Real Event",
            1_768_435_200,
            1_768_438_800,
        )
        .await;
        let name = ensure_resource_name(&db, active, event_id).await;

        // A foreign href points at a different (foreign-owned) calendar. It must
        // not resolve and must not leak that the resource exists elsewhere.
        let foreign_href = resource_href(&principal_id, foreign, &name);

        let app = build_caldav_router(accounts);
        let body = multiget_xml(std::slice::from_ref(&foreign_href));
        let (status, resp) = report_request(
            app,
            &format!("/dav/calendars/{principal_id}/{active}/"),
            "owner@example.test",
            &password,
            &body,
        )
        .await;

        assert_eq!(status, StatusCode::MULTI_STATUS);
        assert!(resp.contains(&foreign_href));
        assert!(resp.contains("HTTP/1.1 404 Not Found"));
        assert!(
            !resp.contains("Real Event"),
            "foreign path must not leak data"
        );
    }

    #[tokio::test]
    async fn report_calendar_multiget_mixed_success_and_404() {
        let (db, accounts, owner_id, password, principal_id, active, _archived, _foreign) =
            setup_owner_with_calendars().await;

        let first_id =
            insert_timed_event(&db, active, owner_id, "First", 1_768_435_200, 1_768_438_800).await;
        let second_id = insert_timed_event(
            &db,
            active,
            owner_id,
            "Second",
            1_768_435_200,
            1_768_438_800,
        )
        .await;
        let first_name = ensure_resource_name(&db, active, first_id).await;
        let second_name = ensure_resource_name(&db, active, second_id).await;
        let first_href = resource_href(&principal_id, active, &first_name);
        let second_href = resource_href(&principal_id, active, &second_name);
        let missing_href = resource_href(&principal_id, active, "missing");

        let app = build_caldav_router(accounts);
        let body = multiget_xml(&[
            first_href.clone(),
            second_href.clone(),
            missing_href.clone(),
        ]);
        let (status, resp) = report_request(
            app,
            &format!("/dav/calendars/{principal_id}/{active}/"),
            "owner@example.test",
            &password,
            &body,
        )
        .await;

        assert_eq!(status, StatusCode::MULTI_STATUS);
        assert!(resp.contains("First"));
        assert!(resp.contains("Second"));
        assert!(resp.contains(&missing_href));
        assert!(resp.contains("HTTP/1.1 404 Not Found"));
        // Exactly one 404 (the missing item) and two 200s.
        assert_eq!(resp.matches("HTTP/1.1 404 Not Found").count(), 1);
        assert_eq!(resp.matches("HTTP/1.1 200 OK").count(), 2);
    }

    #[tokio::test]
    async fn report_calendar_multiget_unauthenticated_returns_401() {
        let (_db, accounts, _owner_id, _password, principal_id, active, _archived, _foreign) =
            setup_owner_with_calendars().await;

        let app = build_caldav_router(accounts);
        let request = Request::builder()
            .method(Method::from_bytes(b"REPORT").unwrap())
            .uri(format!("/dav/calendars/{principal_id}/{active}/"))
            .header(header::CONTENT_TYPE, "application/xml")
            .body(Body::from(
                r#"<C:calendar-multiget xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:calendarserver/"><D:href>/x/</D:href></C:calendar-multiget>"#
                    .to_owned(),
            ))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn report_calendar_multiget_cross_user_returns_404() {
        let (db, accounts, _owner_id, _owner_password, principal_id, active, _archived, _foreign) =
            setup_owner_with_calendars().await;

        let intruder_id = db.insert_user("intruder@example.test").await;
        let intruder_password = accounts
            .issue_credential(intruder_id, "Phone".into())
            .await
            .unwrap()
            .password
            .expose()
            .to_owned();

        let body = multiget_xml(&[resource_href(&principal_id, active, "x")]);
        let app = build_caldav_router(accounts);
        let (status, _resp) = report_request(
            app,
            &format!("/dav/calendars/{principal_id}/{active}/"),
            "intruder@example.test",
            &intruder_password,
            &body,
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    // ── T18: Production hardening tests ──────────────────────────────────────

    #[tokio::test]
    async fn dav_auth_rate_limiter_blocks_after_max_attempts() {
        let db = TestDb::new().await;
        let key = SecretKey::generate();
        let accounts = CaldavAccountService::new_at(
            db.pool.clone(),
            key,
            url::Url::parse("http://127.0.0.1:3000").unwrap(),
            1000,
        );
        let user_id = db.insert_user("ratelimit@example.test").await;
        let _issued = accounts
            .issue_credential(user_id, "Phone".into())
            .await
            .unwrap();

        // Exhaust the rate limit with wrong passwords.
        for _ in 0..crate::caldav::auth::DAV_AUTH_MAX_ATTEMPTS {
            let app = build_caldav_router(accounts.clone());
            let request = Request::builder()
                .method(Method::from_bytes(b"PROPFIND").unwrap())
                .uri("/dav/")
                .header(
                    header::AUTHORIZATION,
                    basic_header("ratelimit@example.test", "wrong-password"),
                )
                .body(Body::empty())
                .unwrap();
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        // Next attempt should be rate-limited (429), even with correct password.
        let correct_password = accounts
            .issue_credential(user_id, "Phone2".into())
            .await
            .unwrap()
            .password
            .expose()
            .to_owned();
        let app = build_caldav_router(accounts.clone());
        let request = Request::builder()
            .method(Method::from_bytes(b"PROPFIND").unwrap())
            .uri("/dav/")
            .header(
                header::AUTHORIZATION,
                basic_header("ratelimit@example.test", &correct_password),
            )
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(response.headers().get(header::RETRY_AFTER).is_some());
    }

    #[tokio::test]
    async fn dav_auth_rate_limiter_independent_per_client_key() {
        let db = TestDb::new().await;
        let key = SecretKey::generate();
        let accounts = CaldavAccountService::new_at(
            db.pool.clone(),
            key,
            url::Url::parse("http://127.0.0.1:3000").unwrap(),
            1000,
        );
        let user_id = db.insert_user("multi@example.test").await;
        let password = accounts
            .issue_credential(user_id, "Phone".into())
            .await
            .unwrap()
            .password
            .expose()
            .to_owned();

        // Exhaust rate limit for client "1.1.1.1".
        for _ in 0..crate::caldav::auth::DAV_AUTH_MAX_ATTEMPTS {
            let app = build_caldav_router(accounts.clone());
            let request = Request::builder()
                .method(Method::from_bytes(b"PROPFIND").unwrap())
                .uri("/dav/")
                .header("x-forwarded-for", "1.1.1.1")
                .header(
                    header::AUTHORIZATION,
                    basic_header("multi@example.test", "wrong"),
                )
                .body(Body::empty())
                .unwrap();
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        // Different client "2.2.2.2" should still be allowed.
        let app = build_caldav_router(accounts.clone());
        let request = Request::builder()
            .method(Method::from_bytes(b"PROPFIND").unwrap())
            .uri("/dav/")
            .header("x-forwarded-for", "2.2.2.2")
            .header(
                header::AUTHORIZATION,
                basic_header("multi@example.test", &password),
            )
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::MULTI_STATUS);
    }

    #[tokio::test]
    async fn dav_metrics_classify_auth_failures() {
        let db = TestDb::new().await;
        let key = SecretKey::generate();
        let accounts = CaldavAccountService::new_at(
            db.pool.clone(),
            key,
            url::Url::parse("http://127.0.0.1:3000").unwrap(),
            1000,
        );
        let user_id = db.insert_user("metrics@example.test").await;
        let _issued = accounts
            .issue_credential(user_id, "Phone".into())
            .await
            .unwrap();

        // Trigger an auth failure.
        let app = build_caldav_router(accounts.clone());
        let request = Request::builder()
            .method(Method::from_bytes(b"PROPFIND").unwrap())
            .uri("/dav/")
            .header(
                header::AUTHORIZATION,
                basic_header("metrics@example.test", "wrong-password"),
            )
            .body(Body::empty())
            .unwrap();
        let _ = app.oneshot(request).await.unwrap();

        let metrics = accounts.metrics();
        assert_eq!(metrics.auth_failures(), 1);
        assert_eq!(metrics.total_failures(), 1);
    }

    #[tokio::test]
    async fn dav_metrics_classify_successful_auth() {
        let db = TestDb::new().await;
        let key = SecretKey::generate();
        let accounts = CaldavAccountService::new_at(
            db.pool.clone(),
            key,
            url::Url::parse("http://127.0.0.1:3000").unwrap(),
            1000,
        );
        let user_id = db.insert_user("success@example.test").await;
        let password = accounts
            .issue_credential(user_id, "Phone".into())
            .await
            .unwrap()
            .password
            .expose()
            .to_owned();

        let app = build_caldav_router(accounts.clone());
        let request = Request::builder()
            .method(Method::from_bytes(b"PROPFIND").unwrap())
            .uri("/dav/")
            .header(
                header::AUTHORIZATION,
                basic_header("success@example.test", &password),
            )
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::MULTI_STATUS);

        let metrics = accounts.metrics();
        assert_eq!(metrics.successful_requests(), 1);
        assert_eq!(metrics.auth_failures(), 0);
    }

    #[tokio::test]
    async fn dav_metrics_classify_rate_limited() {
        let db = TestDb::new().await;
        let key = SecretKey::generate();
        let accounts = CaldavAccountService::new_at(
            db.pool.clone(),
            key,
            url::Url::parse("http://127.0.0.1:3000").unwrap(),
            1000,
        );
        let user_id = db.insert_user("rl@example.test").await;
        let _issued = accounts
            .issue_credential(user_id, "Phone".into())
            .await
            .unwrap();

        // Exhaust the rate limit.
        for _ in 0..crate::caldav::auth::DAV_AUTH_MAX_ATTEMPTS {
            let app = build_caldav_router(accounts.clone());
            let request = Request::builder()
                .method(Method::from_bytes(b"PROPFIND").unwrap())
                .uri("/dav/")
                .header(
                    header::AUTHORIZATION,
                    basic_header("rl@example.test", "wrong"),
                )
                .body(Body::empty())
                .unwrap();
            let _ = app.oneshot(request).await.unwrap();
        }

        // Trigger a rate-limited response.
        let app = build_caldav_router(accounts.clone());
        let request = Request::builder()
            .method(Method::from_bytes(b"PROPFIND").unwrap())
            .uri("/dav/")
            .header(
                header::AUTHORIZATION,
                basic_header("rl@example.test", "wrong"),
            )
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);

        let metrics = accounts.metrics();
        assert!(metrics.rate_limited() >= 1);
    }

    #[tokio::test]
    async fn dav_oversized_body_fails_before_any_write() {
        let db = TestDb::new().await;
        let key = SecretKey::generate();
        let accounts = CaldavAccountService::new_at(
            db.pool.clone(),
            key,
            url::Url::parse("http://127.0.0.1:3000").unwrap(),
            1000,
        );
        let user_id = db.insert_user("oversized@example.test").await;
        let password = accounts
            .issue_credential(user_id, "Phone".into())
            .await
            .unwrap()
            .password
            .expose()
            .to_owned();
        let calendar_id = db.insert_calendar(user_id, "Cal", "#fff", false).await;
        let principal_id: String =
            sqlx::query_scalar("SELECT principal_id FROM caldav_accounts WHERE user_id = ?")
                .bind(user_id)
                .fetch_one(&db.pool)
                .await
                .unwrap();

        // Build an oversized REPORT body.
        let huge_body = format!(
            r#"<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:calendarserver/"><C:filter><C:time-range start="20260101T000000Z" end="20260131T235959Z"/></C:filter><!-- {} --></C:calendar-query>"#,
            "x".repeat(crate::caldav::query::MAX_BODY_BYTES)
        );

        let app = build_caldav_router(accounts.clone());
        let request = Request::builder()
            .method(Method::from_bytes(b"REPORT").unwrap())
            .uri(format!("/dav/calendars/{principal_id}/{calendar_id}/"))
            .header(
                header::AUTHORIZATION,
                basic_header("oversized@example.test", &password),
            )
            .header(header::CONTENT_TYPE, "application/xml")
            .body(Body::from(huge_body))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // No event should have been created.
        let event_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(event_count, 0);
    }

    #[tokio::test]
    async fn dav_malformed_report_fails_before_any_write() {
        let db = TestDb::new().await;
        let key = SecretKey::generate();
        let accounts = CaldavAccountService::new_at(
            db.pool.clone(),
            key,
            url::Url::parse("http://127.0.0.1:3000").unwrap(),
            1000,
        );
        let user_id = db.insert_user("malformed@example.test").await;
        let password = accounts
            .issue_credential(user_id, "Phone".into())
            .await
            .unwrap()
            .password
            .expose()
            .to_owned();
        let calendar_id = db.insert_calendar(user_id, "Cal", "#fff", false).await;
        let principal_id: String =
            sqlx::query_scalar("SELECT principal_id FROM caldav_accounts WHERE user_id = ?")
                .bind(user_id)
                .fetch_one(&db.pool)
                .await
                .unwrap();

        // Send malformed XML.
        let app = build_caldav_router(accounts.clone());
        let request = Request::builder()
            .method(Method::from_bytes(b"REPORT").unwrap())
            .uri(format!("/dav/calendars/{principal_id}/{calendar_id}/"))
            .header(
                header::AUTHORIZATION,
                basic_header("malformed@example.test", &password),
            )
            .header(header::CONTENT_TYPE, "application/xml")
            .body(Body::from("<C:calendar-query><broken"))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // No event should have been created.
        let event_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(event_count, 0);
    }

    #[test]
    fn production_config_rejects_http_caldav_origin() {
        use crate::config::{AppConfig, Environment};
        let config = AppConfig::with_database_path_and_origin(
            Environment::Production,
            "127.0.0.1:3000",
            Some("secret".into()),
            "test.sqlite",
            "https://app.example",
        )
        .unwrap();
        let result = config.with_caldav_public_origin(Some("http://dav.example".into()));
        assert!(result.is_err());
    }

    #[test]
    fn production_config_accepts_https_caldav_origin() {
        use crate::config::{AppConfig, Environment};
        let config = AppConfig::with_database_path_and_origin(
            Environment::Production,
            "127.0.0.1:3000",
            Some("secret".into()),
            "test.sqlite",
            "https://app.example",
        )
        .unwrap();
        let config = config
            .with_caldav_public_origin(Some("https://dav.example".into()))
            .unwrap();
        assert_eq!(config.caldav_public_origin(), "https://dav.example");
    }

    #[test]
    fn caldav_metrics_default_state() {
        use crate::caldav::types::CaldavMetrics;
        let metrics = CaldavMetrics::new();
        assert_eq!(metrics.auth_failures(), 0);
        assert_eq!(metrics.rate_limited(), 0);
        assert_eq!(metrics.malformed_requests(), 0);
        assert_eq!(metrics.oversized_bodies(), 0);
        assert_eq!(metrics.unauthorized(), 0);
        assert_eq!(metrics.not_found(), 0);
        assert_eq!(metrics.precondition_failed(), 0);
        assert_eq!(metrics.internal_errors(), 0);
        assert_eq!(metrics.successful_requests(), 0);
        assert_eq!(metrics.total_failures(), 0);
    }

    #[test]
    fn caldav_metrics_record_and_read() {
        use crate::caldav::types::CaldavMetrics;
        let metrics = CaldavMetrics::new();
        metrics.record_auth_failure();
        metrics.record_auth_failure();
        metrics.record_rate_limited();
        metrics.record_malformed_request();
        metrics.record_oversized_body();
        metrics.record_unauthorized();
        metrics.record_not_found();
        metrics.record_precondition_failed();
        metrics.record_internal_error();
        metrics.record_success();
        metrics.record_success();

        assert_eq!(metrics.auth_failures(), 2);
        assert_eq!(metrics.rate_limited(), 1);
        assert_eq!(metrics.malformed_requests(), 1);
        assert_eq!(metrics.oversized_bodies(), 1);
        assert_eq!(metrics.unauthorized(), 1);
        assert_eq!(metrics.not_found(), 1);
        assert_eq!(metrics.precondition_failed(), 1);
        assert_eq!(metrics.internal_errors(), 1);
        assert_eq!(metrics.successful_requests(), 2);
        assert_eq!(metrics.total_failures(), 9);
    }
}
