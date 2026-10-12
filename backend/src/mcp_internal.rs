// MCP Server internal API handlers.
//
// These endpoints are called by the MCP server via the x-mcp-api-key header.
// They provide the MCP server with access to user data, calendar data,
// and grant management without exposing the database directly.

use axum::Json;
use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Deserialize)]
pub struct TokenExchangeQuery {
    pub grant_type: String,
    pub subject_token: String,
    pub subject_type: String,
    pub actor_token: Option<String>,
    pub actor_type: Option<String>,
    pub scope: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TokenExchangeResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: i64,
    pub scope: String,
}

#[derive(Debug, Deserialize)]
pub struct UserIdPath {
    pub user_id: i64,
}

#[derive(Debug, Serialize)]
pub struct UserStatusResponse {
    pub user_id: i64,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct CalendarInfoResponse {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub acl_role: String,
}

#[derive(Debug, Deserialize)]
pub struct CalendarIdPath {
    pub calendar_id: i64,
}

#[derive(Debug, Deserialize)]
pub struct UserIdQuery {
    pub user_id: i64,
}

#[derive(Debug, Serialize)]
pub struct CalendarRoleResponse {
    pub role: String,
}

#[derive(Debug, Deserialize)]
pub struct EventIdPath {
    pub event_id: i64,
}

#[derive(Debug, Serialize)]
pub struct EventInfoResponse {
    pub id: i64,
    pub calendar_id: i64,
    pub title: Option<String>,
    pub description: Option<String>,
    pub location: Option<String>,
    pub status: String,
    pub event_kind: String,
    pub start_utc: Option<String>,
    pub end_utc: Option<String>,
    pub version: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct SearchEventsQuery {
    pub from: String,
    pub to: String,
    pub page_token: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SearchEventsResponse {
    pub events: Vec<EventInfoResponse>,
    pub next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateEventPayload {
    pub title: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub start_utc: String,
    pub end_utc: String,
    pub status: Option<String>,
    pub timezone: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateEventResponse {
    pub id: i64,
    pub calendar_id: i64,
    pub title: Option<String>,
    pub description: Option<String>,
    pub location: Option<String>,
    pub status: String,
    pub event_kind: String,
    pub start_utc: Option<String>,
    pub end_utc: Option<String>,
    pub version: i64,
}

#[derive(Debug, Deserialize)]
pub struct UpdateEventPayload {
    pub title: Option<String>,
    pub description: Option<String>,
    pub location: Option<String>,
    pub start_utc: Option<String>,
    pub end_utc: Option<String>,
    pub status: Option<String>,
    pub version: i64,
}

#[derive(Debug, Deserialize)]
pub struct DeleteIntentPayload {
    pub user_id: i64,
    pub oauth_client_id: String,
    pub event_id: i64,
    pub calendar_id: i64,
    pub event_version: i64,
    pub expires_at: i64,
}

#[derive(Debug, Serialize)]
pub struct DeleteIntentResponse {
    pub intent_id: String,
    pub event_id: i64,
    pub calendar_id: i64,
    pub event_version: i64,
    pub expires_at: i64,
    pub confirmation_state: String,
}

#[derive(Debug, Deserialize)]
pub struct IntentIdPath {
    pub intent_id: String,
}

#[derive(Debug, Serialize)]
pub struct McpGrantResponse {
    pub grant_id: String,
    pub user_id: i64,
    pub oauth_client_id: String,
    pub allowed_calendar_ids: Vec<i64>,
    pub allow_availability: bool,
    pub allow_event_titles: bool,
    pub allow_event_details: bool,
    pub allow_create: bool,
    pub allow_update: bool,
    pub allow_delete: bool,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct McpGrantsQuery {
    pub user_id: i64,
    pub client_id: String,
}

#[derive(Debug, Deserialize)]
pub struct IdempotencyKeyPath {
    pub operation_id: String,
}

#[derive(Debug, Deserialize)]
pub struct RecordIdempotencyPayload {
    pub operation_id: String,
    pub user_id: i64,
    pub payload: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct CheckIdempotencyQuery {
    pub user_id: i64,
}

#[derive(Debug, Deserialize)]
pub struct CreateReminderPayload {
    pub user_id: i64,
    pub oauth_client_id: String,
    pub event_id: i64,
    pub calendar_id: i64,
    pub reminder_minutes: i64,
}

#[derive(Debug, Serialize)]
pub struct ReminderResponse {
    pub reminder_id: String,
}

/// Shared state for the MCP internal API key middleware.
#[derive(Clone)]
pub struct McpApiKeyAuth {
    expected_key: String,
}

impl McpApiKeyAuth {
    pub fn new(expected_key: impl Into<String>) -> Self {
        Self {
            expected_key: expected_key.into(),
        }
    }
}

/// Constant-time byte comparison to avoid timing side channels.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn unauthorized_response() -> Response {
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::json!({ "error": "unauthorized" }).to_string(),
        ))
        .expect("static response cannot fail to build")
}

/// Middleware enforcing the `x-mcp-api-key` header on MCP internal routes.
///
/// Fails closed: when no key is configured, every request is rejected.
/// Returns 401 for missing or invalid keys and never logs key material.
pub async fn mcp_api_key_middleware(
    State(auth): State<McpApiKeyAuth>,
    request: Request,
    next: Next,
) -> Response {
    if auth.expected_key.is_empty() {
        return unauthorized_response();
    }
    let provided = request
        .headers()
        .get("x-mcp-api-key")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if constant_time_eq(provided.as_bytes(), auth.expected_key.as_bytes()) {
        return next.run(request).await;
    }
    unauthorized_response()
}

/// Build the MCP internal API router with every protected route and the
/// API-key middleware applied.
pub fn build_mcp_internal_router(pool: SqlitePool, auth: McpApiKeyAuth) -> axum::Router {
    axum::Router::new()
        .route(
            "/internal/token-exchange",
            axum::routing::post(token_exchange),
        )
        .route(
            "/internal/mcp/users/:user_id/status",
            axum::routing::get(get_user_status),
        )
        .route(
            "/internal/mcp/users/:user_id/calendars",
            axum::routing::get(list_calendars_for_mcp),
        )
        .route(
            "/internal/mcp/calendars/:calendar_id/role/:user_id",
            axum::routing::get(get_calendar_role),
        )
        .route(
            "/internal/mcp/calendars/:calendar_id/events/:event_id",
            axum::routing::get(get_event),
        )
        .route(
            "/internal/mcp/calendars/:calendar_id/events/search",
            axum::routing::get(search_events),
        )
        .route(
            "/internal/mcp/events/:calendar_id",
            axum::routing::post(create_event),
        )
        .route(
            "/internal/mcp/events/:calendar_id/:event_id",
            axum::routing::patch(update_event),
        )
        .route(
            "/internal/mcp/delete-intents",
            axum::routing::post(create_delete_intent),
        )
        .route(
            "/internal/mcp/delete-intents/:intent_id",
            axum::routing::get(get_delete_intent),
        )
        .route(
            "/internal/mcp/delete-intents/:intent_id/commit",
            axum::routing::post(commit_delete_intent),
        )
        .route(
            "/internal/mcp/mcp-grants",
            axum::routing::get(get_mcp_grants),
        )
        .route(
            "/internal/mcp/idempotency/:operation_id",
            axum::routing::get(check_idempotency),
        )
        .route(
            "/internal/mcp/idempotency",
            axum::routing::post(record_idempotency),
        )
        .route(
            "/api/v1/calendars/:calendar_id/reminders",
            axum::routing::post(create_reminder),
        )
        .layer(axum::middleware::from_fn_with_state(
            auth,
            mcp_api_key_middleware,
        ))
        .with_state(pool)
}

/// RFC 8693 token exchange endpoint.
pub async fn token_exchange(
    State(pool): State<SqlitePool>,
    Query(params): Query<TokenExchangeQuery>,
    headers: axum::http::HeaderMap,
) -> Result<Json<TokenExchangeResponse>, (StatusCode, String)> {
    let _ = pool;
    let _ = headers;

    if params.grant_type != "urn:ietf:params:oauth:grant-type:token-exchange" {
        return Err((
            StatusCode::BAD_REQUEST,
            "unsupported grant type".to_string(),
        ));
    }

    if params.subject_token.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "missing subject_token".to_string()));
    }

    // Generate a short-lived internal access token.
    let internal_token = uuid::Uuid::new_v4().to_string();

    Ok(Json(TokenExchangeResponse {
        access_token: internal_token,
        token_type: "mcp_internal".to_string(),
        expires_in: 300,
        scope: params.scope.unwrap_or_default(),
    }))
}

/// Get user status from the database.
pub async fn get_user_status(
    State(pool): State<SqlitePool>,
    Path(user_id): Path<i64>,
) -> Result<Json<UserStatusResponse>, (StatusCode, String)> {
    let user = sqlx::query_as::<_, (i64, String)>("SELECT id, status FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(&pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    match user {
        Some((id, status)) => Ok(Json(UserStatusResponse {
            user_id: id,
            status: match status.as_str() {
                "registered" => "active".to_owned(),
                "inactive" => "suspended".to_owned(),
                _ => status,
            },
        })),
        None => Err((StatusCode::NOT_FOUND, "user not found".to_string())),
    }
}

/// List calendars for a user (MCP-specific, returns only granted calendars).
pub async fn list_calendars_for_mcp(
    State(pool): State<SqlitePool>,
    Path(user_id): Path<i64>,
) -> Result<Json<Vec<CalendarInfoResponse>>, (StatusCode, String)> {
    let calendars = sqlx::query_as::<_, (i64, String, Option<String>, String)>(
        "SELECT c.id, c.name, c.description, ca.role
         FROM calendars c
         JOIN calendar_acl ca ON c.id = ca.calendar_id
         WHERE ca.user_id = ? AND c.archived = 0",
    )
    .bind(user_id)
    .fetch_all(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let response: Vec<CalendarInfoResponse> = calendars
        .into_iter()
        .map(|(id, name, description, acl_role)| CalendarInfoResponse {
            id,
            name,
            description,
            acl_role,
        })
        .collect();

    Ok(Json(response))
}

/// Get calendar role for a user.
pub async fn get_calendar_role(
    State(pool): State<SqlitePool>,
    Path((calendar_id, user_id)): Path<(i64, i64)>,
) -> Result<Json<CalendarRoleResponse>, (StatusCode, String)> {
    let role = sqlx::query_as::<_, (String,)>(
        "SELECT ca.role FROM calendar_acl ca JOIN calendars c ON c.id = ca.calendar_id WHERE ca.calendar_id = ? AND ca.user_id = ? AND c.archived = 0",
    )
    .bind(calendar_id)
    .bind(user_id)
    .fetch_optional(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    match role {
        Some((role,)) => Ok(Json(CalendarRoleResponse { role })),
        None => Err((StatusCode::NOT_FOUND, "calendar role not found".to_string())),
    }
}

/// Parse a UTC timestamp string (Unix epoch seconds or RFC 3339) to an i64 epoch.
fn parse_utc_timestamp(s: &str) -> Result<i64, String> {
    if let Ok(ts) = s.parse::<i64>() {
        return Ok(ts);
    }
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.timestamp())
        .map_err(|_| format!("cannot parse timestamp: {}", s))
}

/// Stored event row shape (matches the SELECT column order used below).
type EventRow = (
    i64,            // id
    i64,            // calendar_id
    Option<String>, // title
    Option<String>, // description
    Option<String>, // location
    String,         // status
    String,         // event_kind
    Option<i64>,    // timed_start_utc
    Option<i64>,    // timed_end_utc
    Option<i64>,    // version
);

/// Convert a stored event row into the wire response, formatting stored
/// epoch timestamps as RFC 3339 UTC strings.
fn event_row_to_response(row: EventRow) -> EventInfoResponse {
    let (
        id,
        calendar_id,
        title,
        description,
        location,
        status,
        event_kind,
        timed_start,
        timed_end,
        version,
    ) = row;
    let format_ts = |t: Option<i64>| -> Option<String> {
        t.and_then(|ts| {
            chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0)
                .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        })
    };
    EventInfoResponse {
        id,
        calendar_id,
        title,
        description,
        location,
        status,
        event_kind,
        start_utc: format_ts(timed_start),
        end_utc: format_ts(timed_end),
        version,
    }
}

/// Fetch a single event row by calendar + event id.
async fn fetch_event_row(
    pool: &SqlitePool,
    calendar_id: i64,
    event_id: i64,
) -> Result<Option<EventRow>, (StatusCode, String)> {
    sqlx::query_as::<_, EventRow>(
        "SELECT id, calendar_id, title, description, location, status, event_kind, timed_start_utc, timed_end_utc, version FROM events WHERE calendar_id = ? AND id = ?",
    )
    .bind(calendar_id)
    .bind(event_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// Get event by ID.
pub async fn get_event(
    State(pool): State<SqlitePool>,
    Path((calendar_id, event_id)): Path<(i64, i64)>,
) -> Result<Json<EventInfoResponse>, (StatusCode, String)> {
    match fetch_event_row(&pool, calendar_id, event_id).await? {
        Some(row) => Ok(Json(event_row_to_response(row))),
        None => Err((StatusCode::NOT_FOUND, "event not found".to_string())),
    }
}

/// Search events by time range.
///
/// Overlap semantics: an event matches when it starts before `to` and ends
/// after `from`. `from`/`to` accept Unix epoch seconds or RFC 3339 strings.
pub async fn search_events(
    State(pool): State<SqlitePool>,
    Path(calendar_id): Path<i64>,
    Query(params): Query<SearchEventsQuery>,
) -> Result<Json<SearchEventsResponse>, (StatusCode, String)> {
    let from_ts = parse_utc_timestamp(&params.from)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let to_ts = parse_utc_timestamp(&params.to)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    if to_ts <= from_ts {
        return Err((
            StatusCode::BAD_REQUEST,
            "'to' must be after 'from'".to_string(),
        ));
    }

    let rows = sqlx::query_as::<_, EventRow>(
        "SELECT id, calendar_id, title, description, location, status, event_kind, timed_start_utc, timed_end_utc, version FROM events WHERE calendar_id = ? AND event_kind = 'timed' AND timed_start_utc < ? AND timed_end_utc > ? ORDER BY timed_start_utc LIMIT 100"
    )
    .bind(calendar_id)
    .bind(to_ts)
    .bind(from_ts)
    .fetch_all(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(SearchEventsResponse {
        events: rows.into_iter().map(event_row_to_response).collect(),
        next_page_token: None,
    }))
}

/// Create event.
///
/// Validates timestamps (epoch or RFC 3339), requires `end_utc > start_utc`,
/// and validates the timezone against IANA names before writing anything.
/// The response is built from the stored row, not the request payload.
pub async fn create_event(
    State(pool): State<SqlitePool>,
    Path(calendar_id): Path<i64>,
    Json(payload): Json<CreateEventPayload>,
) -> Result<Json<CreateEventResponse>, (StatusCode, String)> {
    let timed_start = parse_utc_timestamp(&payload.start_utc)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let timed_end = parse_utc_timestamp(&payload.end_utc)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    if timed_end <= timed_start {
        return Err((
            StatusCode::BAD_REQUEST,
            "end_utc must be after start_utc".to_string(),
        ));
    }

    let event_timezone = payload.timezone.as_deref().unwrap_or("UTC");
    let _tz: chrono_tz::Tz = std::str::FromStr::from_str(event_timezone).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            format!("unknown timezone: {}", event_timezone),
        )
    })?;

    let now = chrono::Utc::now().timestamp();

    let result = sqlx::query(
        "INSERT INTO events (calendar_id, title, description, location, status, event_kind,
         timed_start_utc, timed_end_utc, event_timezone, all_day_start_date, all_day_end_date,
         created_by_user_id, last_edited_by_user_id, version, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, 'timed', ?, ?, ?, NULL, NULL, 1, 1, 1, ?, ?)",
    )
    .bind(calendar_id)
    .bind(&payload.title)
    .bind(&payload.description)
    .bind(&payload.location)
    .bind(payload.status.as_deref().unwrap_or("confirmed"))
    .bind(timed_start)
    .bind(timed_end)
    .bind(event_timezone)
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let event_id = result.last_insert_rowid();

    // Respond with the stored row so the client sees exactly what was written.
    let row = fetch_event_row(&pool, calendar_id, event_id)
        .await?
        .ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "event vanished after insert".to_string(),
            )
        })?;
    let stored = event_row_to_response(row);

    Ok(Json(CreateEventResponse {
        id: stored.id,
        calendar_id: stored.calendar_id,
        title: stored.title,
        description: stored.description,
        location: stored.location,
        status: stored.status,
        event_kind: stored.event_kind,
        start_utc: stored.start_utc,
        end_utc: stored.end_utc,
        version: stored.version.unwrap_or(1),
    }))
}

/// Update event.
///
/// Fetches the current row once for the version check and the stored
/// timestamps, validates the effective `end > start` (provided-or-stored
/// values), and responds with the stored row after the UPDATE.
pub async fn update_event(
    State(pool): State<SqlitePool>,
    Path((calendar_id, event_id)): Path<(i64, i64)>,
    Json(payload): Json<UpdateEventPayload>,
) -> Result<Json<EventInfoResponse>, (StatusCode, String)> {
    // Fetch the current row: version check + stored timestamps in one query.
    let current = fetch_event_row(&pool, calendar_id, event_id).await?;
    let current = match current {
        Some(row) => row,
        None => return Err((StatusCode::NOT_FOUND, "event not found".to_string())),
    };
    let current_version = current.9.unwrap_or(1);
    if current_version != payload.version {
        return Err((StatusCode::CONFLICT, "event version conflict".to_string()));
    }

    let now = chrono::Utc::now().timestamp();
    let new_version = payload.version + 1;

    // Parse provided timestamps (if any) to Unix epoch.
    let timed_start = payload
        .start_utc
        .as_ref()
        .map(|s| parse_utc_timestamp(s))
        .transpose()
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let timed_end = payload
        .end_utc
        .as_ref()
        .map(|s| parse_utc_timestamp(s))
        .transpose()
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    // Validate the effective range: provided values fall back to stored ones.
    let effective_start = timed_start.or(current.7);
    let effective_end = timed_end.or(current.8);
    match (effective_start, effective_end) {
        (Some(start), Some(end)) if end <= start => {
            return Err((
                StatusCode::BAD_REQUEST,
                "end_utc must be after start_utc".to_string(),
            ));
        }
        _ => {}
    }

    sqlx::query(
        "UPDATE events SET title = COALESCE(?, title), description = COALESCE(?, description), location = COALESCE(?, location), timed_start_utc = COALESCE(?, timed_start_utc), timed_end_utc = COALESCE(?, timed_end_utc), status = COALESCE(?, status), version = ?, updated_at = ? WHERE id = ? AND calendar_id = ?"
    )
    .bind(&payload.title)
    .bind(&payload.description)
    .bind(&payload.location)
    .bind(timed_start)
    .bind(timed_end)
    .bind(&payload.status)
    .bind(new_version)
    .bind(now)
    .bind(event_id)
    .bind(calendar_id)
    .execute(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Respond with the stored row so the client sees exactly what was written.
    let row = fetch_event_row(&pool, calendar_id, event_id)
        .await?
        .ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "event vanished after update".to_string(),
            )
        })?;
    Ok(Json(event_row_to_response(row)))
}

/// Create delete intent.
pub async fn create_delete_intent(
    State(pool): State<SqlitePool>,
    Json(payload): Json<DeleteIntentPayload>,
) -> Result<Json<DeleteIntentResponse>, (StatusCode, String)> {
    let intent_id = uuid::Uuid::new_v4().to_string();

    sqlx::query(
        "INSERT INTO delete_intent (id, user_id, oauth_client_id, event_id, calendar_id, event_version, confirmation_state, expires_at) VALUES (?, ?, ?, ?, ?, ?, 'pending', ?)"
    )
    .bind(&intent_id)
    .bind(payload.user_id)
    .bind(&payload.oauth_client_id)
    .bind(payload.event_id)
    .bind(payload.calendar_id)
    .bind(payload.event_version)
    .bind(payload.expires_at)
    .execute(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(DeleteIntentResponse {
        intent_id,
        event_id: payload.event_id,
        calendar_id: payload.calendar_id,
        event_version: payload.event_version,
        expires_at: payload.expires_at,
        confirmation_state: "pending".to_string(),
    }))
}

/// Get delete intent.
pub async fn get_delete_intent(
    State(pool): State<SqlitePool>,
    Path(intent_id): Path<String>,
) -> Result<Json<DeleteIntentResponse>, (StatusCode, String)> {
    let intent = sqlx::query_as::<_, (String, i64, String, i64, i64, i64, String, i64)>(
        "SELECT id, user_id, oauth_client_id, event_id, calendar_id, event_version, confirmation_state, expires_at FROM delete_intent WHERE id = ?"
    )
    .bind(&intent_id)
    .fetch_optional(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    match intent {
        Some((
            id,
            _user_id,
            _client_id,
            event_id,
            calendar_id,
            event_version,
            confirmation_state,
            expires_at,
        )) => Ok(Json(DeleteIntentResponse {
            intent_id: id,
            event_id,
            calendar_id,
            event_version,
            expires_at,
            confirmation_state,
        })),
        None => Err((StatusCode::NOT_FOUND, "delete intent not found".to_string())),
    }
}

/// Commit delete intent.
pub async fn commit_delete_intent(
    State(pool): State<SqlitePool>,
    Path(intent_id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let result = sqlx::query(
        "UPDATE delete_intent SET confirmation_state = 'committed' WHERE id = ? AND confirmation_state = 'pending'"
    )
    .bind(&intent_id)
    .execute(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if result.rows_affected() == 0 {
        Err((
            StatusCode::CONFLICT,
            "delete intent already committed or not found".to_string(),
        ))
    } else {
        Ok(StatusCode::OK)
    }
}

/// Get the single authoritative active MCP grant for a user + OAuth client.
///
/// Returns only the active grant (revoked_at IS NULL and not expired). This
/// guarantees the MCP server resolves at most one grant, and that revocation
/// takes effect immediately on the next tool invocation.
pub async fn get_mcp_grants(
    State(pool): State<SqlitePool>,
    Query(params): Query<McpGrantsQuery>,
) -> Result<Json<Vec<McpGrantResponse>>, (StatusCode, String)> {
    let now = chrono::Utc::now().timestamp();
    let grants = sqlx::query_as::<_, (String, i64, String, String, i32, i32, i32, i32, i32, i32, i64, Option<i64>, Option<i64>, Option<i64>)>(
        "SELECT id, user_id, oauth_client_id, allowed_calendar_ids, allow_availability, allow_event_titles, allow_event_details, allow_create, allow_update, allow_delete, created_at, last_used_at, expires_at, revoked_at FROM mcp_grant WHERE user_id = ? AND oauth_client_id = ? AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > ?) LIMIT 1"
    )
    .bind(params.user_id)
    .bind(&params.client_id)
    .bind(now)
    .fetch_all(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Grants store consent, while current membership remains authoritative.
    // Intersect on every lookup so all tools lose access immediately when a
    // calendar is archived or its membership is removed.
    let live_calendars: Vec<i64> = sqlx::query_scalar(
        "SELECT c.id FROM calendars c JOIN calendar_acl ca ON ca.calendar_id = c.id WHERE ca.user_id = ? AND c.archived = 0",
    )
    .bind(params.user_id)
    .fetch_all(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let response: Vec<McpGrantResponse> = grants
        .into_iter()
        .map(
            |(
                id,
                user_id,
                client_id,
                calendar_ids,
                avail,
                titles,
                details,
                create,
                update,
                delete,
                created_at,
                last_used,
                expires,
                revoked,
            )| {
                let calendars: Vec<i64> = serde_json::from_str(&calendar_ids).unwrap_or_default();
                McpGrantResponse {
                    grant_id: id,
                    user_id,
                    oauth_client_id: client_id,
                    allowed_calendar_ids: calendars
                        .into_iter()
                        .filter(|id| live_calendars.contains(id))
                        .collect(),
                    allow_availability: avail != 0,
                    allow_event_titles: titles != 0,
                    allow_event_details: details != 0,
                    allow_create: create != 0,
                    allow_update: update != 0,
                    allow_delete: delete != 0,
                    created_at,
                    last_used_at: last_used,
                    expires_at: expires,
                    revoked_at: revoked,
                }
            },
        )
        .collect();

    Ok(Json(response))
}

/// Check idempotency key.
pub async fn check_idempotency(
    State(pool): State<SqlitePool>,
    Path(operation_id): Path<String>,
    Query(params): Query<CheckIdempotencyQuery>,
) -> Result<Json<Option<serde_json::Value>>, (StatusCode, String)> {
    let result = sqlx::query_as::<_, (i64, String, String)>(
        "SELECT response_status, response_headers, response_body FROM idempotency_key WHERE key = ? AND user_id = ?"
    )
    .bind(&operation_id)
    .bind(params.user_id)
    .fetch_optional(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    match result {
        Some((_status, _headers, body)) => {
            let value: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            Ok(Json(Some(value)))
        }
        None => Ok(Json(None)),
    }
}

/// Record idempotency key.
pub async fn record_idempotency(
    State(pool): State<SqlitePool>,
    Json(payload): Json<RecordIdempotencyPayload>,
) -> Result<StatusCode, (StatusCode, String)> {
    let now = chrono::Utc::now().timestamp();

    sqlx::query(
        "INSERT OR REPLACE INTO idempotency_key (key, user_id, response_status, response_headers, response_body, created_at) VALUES (?, ?, 0, '[]', ?, ?)"
    )
    .bind(&payload.operation_id)
    .bind(payload.user_id)
    .bind(payload.payload.to_string())
    .bind(now)
    .execute(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::OK)
}

/// Create reminder.
pub async fn create_reminder(
    State(pool): State<SqlitePool>,
    Path(calendar_id): Path<i64>,
    Json(payload): Json<CreateReminderPayload>,
) -> Result<Json<ReminderResponse>, (StatusCode, String)> {
    let reminder_id = uuid::Uuid::new_v4().to_string();

    let now = chrono::Utc::now().timestamp();

    sqlx::query(
        "INSERT INTO reminders (id, calendar_id, event_id, reminder_minutes, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)"
    )
    .bind(&reminder_id)
    .bind(calendar_id)
    .bind(payload.event_id)
    .bind(payload.reminder_minutes)
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(ReminderResponse { reminder_id }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use tower::ServiceExt;

    const TEST_KEY: &str = "test-mcp-internal-key";

    async fn test_pool() -> SqlitePool {
        let options = SqliteConnectOptions::new()
            .filename(":memory:")
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        pool
    }

    fn build_app(pool: SqlitePool) -> axum::Router {
        build_mcp_internal_router(pool, McpApiKeyAuth::new(TEST_KEY))
    }

    fn make_request(method: &str, path: &str, key: Option<&str>) -> axum::extract::Request {
        let mut builder = axum::http::Request::builder().method(method).uri(path);
        if let Some(key) = key {
            builder = builder.header("x-mcp-api-key", key);
        }
        builder.body(axum::body::Body::empty()).unwrap()
    }

    /// Every protected route: (method, path).
    fn protected_routes() -> Vec<(&'static str, &'static str)> {
        vec![
            ("POST", "/internal/token-exchange"),
            ("GET", "/internal/mcp/users/1/status"),
            ("GET", "/internal/mcp/users/1/calendars"),
            ("GET", "/internal/mcp/calendars/1/role/1"),
            ("GET", "/internal/mcp/calendars/1/events/1"),
            ("GET", "/internal/mcp/calendars/1/events/search"),
            ("POST", "/internal/mcp/events/1"),
            ("PATCH", "/internal/mcp/events/1/1"),
            ("POST", "/internal/mcp/delete-intents"),
            ("GET", "/internal/mcp/delete-intents/abc"),
            ("POST", "/internal/mcp/delete-intents/abc/commit"),
            ("GET", "/internal/mcp/mcp-grants"),
            ("GET", "/internal/mcp/idempotency/op1"),
            ("POST", "/internal/mcp/idempotency"),
            ("POST", "/api/v1/calendars/1/reminders"),
        ]
    }

    #[tokio::test]
    async fn missing_key_rejected_on_all_routes() {
        let pool = test_pool().await;
        for (method, path) in protected_routes() {
            let app = build_app(pool.clone());
            let response = app.oneshot(make_request(method, path, None)).await.unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {path} must be 401 without a key"
            );
        }
    }

    #[tokio::test]
    async fn invalid_key_rejected_on_all_routes() {
        let pool = test_pool().await;
        for (method, path) in protected_routes() {
            let app = build_app(pool.clone());
            let response = app
                .oneshot(make_request(method, path, Some("wrong-key")))
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {path} must be 401 with an invalid key"
            );
        }
    }

    #[tokio::test]
    async fn valid_key_passes_auth_on_all_routes() {
        let pool = test_pool().await;
        for (method, path) in protected_routes() {
            let app = build_app(pool.clone());
            let response = app
                .oneshot(make_request(method, path, Some(TEST_KEY)))
                .await
                .unwrap();
            assert_ne!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {path} must not be 401 with a valid key"
            );
        }
    }

    #[tokio::test]
    async fn empty_configured_key_rejects_all_requests() {
        let pool = test_pool().await;
        let app = build_mcp_internal_router(pool, McpApiKeyAuth::new(""));
        for (method, path) in protected_routes() {
            let app = app.clone();
            let response = app
                .oneshot(make_request(method, path, Some("anything")))
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {path} must be 401 when no key is configured"
            );
        }
    }

    #[test]
    fn constant_time_eq_matches_and_mismatches() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
    }

    #[tokio::test]
    async fn get_mcp_grants_returns_only_active_grant() {
        let pool = test_pool().await;
        let now = chrono::Utc::now().timestamp();

        // Insert an active grant and a revoked grant for the same (user, client).
        sqlx::query(
            "INSERT INTO mcp_grant (id, user_id, oauth_client_id, allowed_calendar_ids, created_at, revoked_at) VALUES ('active', 1, 'client', '[]', ?, NULL)",
        )
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO mcp_grant (id, user_id, oauth_client_id, allowed_calendar_ids, created_at, revoked_at) VALUES ('revoked', 1, 'client', '[]', ?, ?)",
        )
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();

        // The internal API must return exactly one grant (the active one).
        let grants = get_mcp_grants(
            State(pool.clone()),
            Query(McpGrantsQuery {
                user_id: 1,
                client_id: "client".to_string(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(grants.len(), 1, "must return exactly one active grant");
        assert_eq!(grants[0].grant_id, "active");
        assert!(grants[0].revoked_at.is_none());
    }

    #[tokio::test]
    async fn get_mcp_grants_returns_empty_when_all_revoked() {
        let pool = test_pool().await;
        let now = chrono::Utc::now().timestamp();

        // Insert only a revoked grant.
        sqlx::query(
            "INSERT INTO mcp_grant (id, user_id, oauth_client_id, allowed_calendar_ids, created_at, revoked_at) VALUES ('revoked', 1, 'client', '[]', ?, ?)",
        )
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();

        // The internal API must return no grants (immediate revocation).
        let grants = get_mcp_grants(
            State(pool.clone()),
            Query(McpGrantsQuery {
                user_id: 1,
                client_id: "client".to_string(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert!(
            grants.is_empty(),
            "must return no grants when all are revoked"
        );
    }

    #[tokio::test]
    async fn get_mcp_grants_excludes_expired_grant() {
        let pool = test_pool().await;
        let now = chrono::Utc::now().timestamp();

        // Insert an expired grant.
        sqlx::query(
            "INSERT INTO mcp_grant (id, user_id, oauth_client_id, allowed_calendar_ids, created_at, expires_at, revoked_at) VALUES ('expired', 1, 'client', '[]', ?, ?, NULL)",
        )
        .bind(now - 1000)
        .bind(now - 100)
        .execute(&pool)
        .await
        .unwrap();

        // The internal API must return no grants (expired).
        let grants = get_mcp_grants(
            State(pool.clone()),
            Query(McpGrantsQuery {
                user_id: 1,
                client_id: "client".to_string(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert!(
            grants.is_empty(),
            "must return no grants when the grant is expired"
        );
    }
    #[tokio::test]
    async fn grant_lookup_removes_lost_and_archived_calendar_access() {
        let pool = test_pool().await;
        let user: i64 = sqlx::query_scalar("INSERT INTO users (normalized_email, status, created_at, is_superadmin) VALUES ('live@example.com', 'registered', 1, 0) RETURNING id").fetch_one(&pool).await.unwrap();
        let mut calendars = Vec::new();
        for name in ["kept", "removed", "archived"] {
            let id: i64 = sqlx::query_scalar("INSERT INTO calendars (owner_user_id, name, color, default_timezone, default_event_visibility, created_at, updated_at) VALUES (?, ?, '#fff', 'UTC', 'default', 1, 1) RETURNING id")
                .bind(user).bind(name).fetch_one(&pool).await.unwrap();
            sqlx::query("INSERT INTO calendar_acl (calendar_id, user_id, role, created_at, updated_at) VALUES (?, ?, 'owner', 1, 1)").bind(id).bind(user).execute(&pool).await.unwrap();
            calendars.push(id);
        }
        sqlx::query("INSERT INTO mcp_grant (id, user_id, oauth_client_id, allowed_calendar_ids, created_at) VALUES ('live', ?, 'client', ?, 1)")
            .bind(user).bind(serde_json::to_string(&calendars).unwrap()).execute(&pool).await.unwrap();
        sqlx::query("DELETE FROM calendar_acl WHERE calendar_id = ?")
            .bind(calendars[1])
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE calendars SET archived = 1 WHERE id = ?")
            .bind(calendars[2])
            .execute(&pool)
            .await
            .unwrap();
        let grants = get_mcp_grants(
            State(pool.clone()),
            Query(McpGrantsQuery {
                user_id: user,
                client_id: "client".into(),
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(grants[0].allowed_calendar_ids, vec![calendars[0]]);
        let visible = list_calendars_for_mcp(State(pool.clone()), Path(user))
            .await
            .unwrap()
            .0;
        assert_eq!(
            visible.iter().map(|c| c.id).collect::<Vec<_>>(),
            vec![calendars[0]]
        );
        let role = get_calendar_role(State(pool), Path((calendars[2], user))).await;
        assert_eq!(role.err().map(|e| e.0), Some(StatusCode::NOT_FOUND));
    }

    // ------------------------------------------------------------------
    // Event operation regression tests (create / search / get / update).
    // ------------------------------------------------------------------

    /// Create a calendar owned by the seeded admin user (id 1).
    async fn test_calendar(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO calendars (owner_user_id, name, color, default_timezone, default_event_visibility, created_at, updated_at) VALUES (1, 'test-cal', '#fff', 'UTC', 'default', 1, 1) RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn count_events(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM events")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[test]
    fn parse_utc_timestamp_accepts_epoch_and_rfc3339() {
        assert_eq!(parse_utc_timestamp("1760248800").unwrap(), 1760248800);
        assert_eq!(
            parse_utc_timestamp("2025-10-12T06:00:00Z").unwrap(),
            1760248800
        );
        assert_eq!(
            parse_utc_timestamp("2025-10-12T08:00:00+02:00").unwrap(),
            1760248800
        );
        assert!(parse_utc_timestamp("not-a-time").is_err());
    }

    #[tokio::test]
    async fn create_event_rejects_end_before_start_without_writing() {
        let pool = test_pool().await;
        let cal = test_calendar(&pool).await;
        let payload = CreateEventPayload {
            title: "Bad range".to_string(),
            description: None,
            location: None,
            start_utc: "2026-10-12T06:30:00Z".to_string(),
            end_utc: "2026-10-12T06:00:00Z".to_string(),
            status: None,
            timezone: Some("Europe/Budapest".to_string()),
        };
        let result = create_event(State(pool.clone()), Path(cal), Json(payload)).await;
        assert_eq!(result.err().map(|e| e.0), Some(StatusCode::BAD_REQUEST));
        assert_eq!(count_events(&pool).await, 0, "rejected create must not write");
    }

    #[tokio::test]
    async fn create_event_rejects_equal_start_and_end_without_writing() {
        let pool = test_pool().await;
        let cal = test_calendar(&pool).await;
        let payload = CreateEventPayload {
            title: "Zero length".to_string(),
            description: None,
            location: None,
            start_utc: "2026-10-12T06:00:00Z".to_string(),
            end_utc: "2026-10-12T06:00:00Z".to_string(),
            status: None,
            timezone: None,
        };
        let result = create_event(State(pool.clone()), Path(cal), Json(payload)).await;
        assert_eq!(result.err().map(|e| e.0), Some(StatusCode::BAD_REQUEST));
        assert_eq!(count_events(&pool).await, 0, "rejected create must not write");
    }

    #[tokio::test]
    async fn create_event_rejects_invalid_timestamp_without_writing() {
        let pool = test_pool().await;
        let cal = test_calendar(&pool).await;
        let payload = CreateEventPayload {
            title: "Bad ts".to_string(),
            description: None,
            location: None,
            start_utc: "not-a-time".to_string(),
            end_utc: "2026-10-12T06:30:00Z".to_string(),
            status: None,
            timezone: None,
        };
        let result = create_event(State(pool.clone()), Path(cal), Json(payload)).await;
        assert_eq!(result.err().map(|e| e.0), Some(StatusCode::BAD_REQUEST));
        assert_eq!(count_events(&pool).await, 0, "rejected create must not write");
    }

    #[tokio::test]
    async fn create_event_rejects_unknown_timezone_without_writing() {
        let pool = test_pool().await;
        let cal = test_calendar(&pool).await;
        let payload = CreateEventPayload {
            title: "Bad tz".to_string(),
            description: None,
            location: None,
            start_utc: "2026-10-12T06:00:00Z".to_string(),
            end_utc: "2026-10-12T06:30:00Z".to_string(),
            status: None,
            timezone: Some("Mars/Olympus".to_string()),
        };
        let result = create_event(State(pool.clone()), Path(cal), Json(payload)).await;
        assert_eq!(result.err().map(|e| e.0), Some(StatusCode::BAD_REQUEST));
        assert_eq!(count_events(&pool).await, 0, "rejected create must not write");
    }

    #[tokio::test]
    async fn create_event_stores_timezone_and_returns_stored_row() {
        let pool = test_pool().await;
        let cal = test_calendar(&pool).await;
        let payload = CreateEventPayload {
            title: "MCP test — morning walk".to_string(),
            description: Some("Created for local MCP validation".to_string()),
            location: None,
            start_utc: "2026-10-12T06:00:00Z".to_string(),
            end_utc: "2026-10-12T06:30:00Z".to_string(),
            status: None,
            timezone: Some("Europe/Budapest".to_string()),
        };
        let response = create_event(State(pool.clone()), Path(cal), Json(payload))
            .await
            .unwrap()
            .0;

        // The response must be RFC 3339 UTC, not the raw input string.
        assert_eq!(response.start_utc.as_deref(), Some("2026-10-12T06:00:00Z"));
        assert_eq!(response.end_utc.as_deref(), Some("2026-10-12T06:30:00Z"));
        assert_eq!(response.event_kind, "timed");
        assert_eq!(response.version, 1);

        // The timezone must be persisted in the DB.
        let stored_tz: String = sqlx::query_scalar(
            "SELECT event_timezone FROM events WHERE id = ?",
        )
        .bind(response.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(stored_tz, "Europe/Budapest");
    }

    #[tokio::test]
    async fn search_events_finds_only_overlapping_events() {
        let pool = test_pool().await;
        let cal = test_calendar(&pool).await;

        // Event A: 06:00–06:30 UTC. Event B: 10:00–11:00 UTC.
        for (title, start, end) in [
            ("A", "2026-10-12T06:00:00Z", "2026-10-12T06:30:00Z"),
            ("B", "2026-10-12T10:00:00Z", "2026-10-12T11:00:00Z"),
        ] {
            let payload = CreateEventPayload {
                title: title.to_string(),
                description: None,
                location: None,
                start_utc: start.to_string(),
                end_utc: end.to_string(),
                status: None,
                timezone: None,
            };
            let _ = create_event(State(pool.clone()), Path(cal), Json(payload))
                .await
                .unwrap();
        }

        // Range 06:15–06:45 overlaps only event A.
        let response = search_events(
            State(pool.clone()),
            Path(cal),
            Query(SearchEventsQuery {
                from: "2026-10-12T06:15:00Z".to_string(),
                to: "2026-10-12T06:45:00Z".to_string(),
                page_token: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(response.events.len(), 1);
        assert_eq!(response.events[0].title.as_deref(), Some("A"));
        assert_eq!(response.next_page_token, None);

        // Epoch-second inputs must work too (integer binding).
        let response = search_events(
            State(pool.clone()),
            Path(cal),
            Query(SearchEventsQuery {
                from: "1791784800".to_string(), // 2026-10-12T06:00:00Z
                to: "1791788400".to_string(),   // 2026-10-12T07:00:00Z
                page_token: None,
            }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(response.events.len(), 1);
        assert_eq!(response.events[0].title.as_deref(), Some("A"));
    }

    #[tokio::test]
    async fn search_events_rejects_invalid_range() {
        let pool = test_pool().await;
        let cal = test_calendar(&pool).await;

        let result = search_events(
            State(pool.clone()),
            Path(cal),
            Query(SearchEventsQuery {
                from: "2026-10-12T07:00:00Z".to_string(),
                to: "2026-10-12T06:00:00Z".to_string(),
                page_token: None,
            }),
        )
        .await;
        assert_eq!(result.err().map(|e| e.0), Some(StatusCode::BAD_REQUEST));

        let result = search_events(
            State(pool.clone()),
            Path(cal),
            Query(SearchEventsQuery {
                from: "garbage".to_string(),
                to: "2026-10-12T06:00:00Z".to_string(),
                page_token: None,
            }),
        )
        .await;
        assert_eq!(result.err().map(|e| e.0), Some(StatusCode::BAD_REQUEST));
    }

    #[tokio::test]
    async fn search_events_returns_envelope_shape() {
        let pool = test_pool().await;
        let cal = test_calendar(&pool).await;
        let response = search_events(
            State(pool.clone()),
            Path(cal),
            Query(SearchEventsQuery {
                from: "2026-10-12T00:00:00Z".to_string(),
                to: "2026-10-13T00:00:00Z".to_string(),
                page_token: None,
            }),
        )
        .await
        .unwrap()
        .0;
        let json = serde_json::to_value(&response).unwrap();
        assert!(json.get("events").is_some(), "envelope must have events");
        assert!(
            json.get("next_page_token").is_some(),
            "envelope must have next_page_token"
        );
        assert_eq!(json["events"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn get_event_returns_rfc3339_timestamps() {
        let pool = test_pool().await;
        let cal = test_calendar(&pool).await;
        let payload = CreateEventPayload {
            title: "Get me".to_string(),
            description: Some("desc".to_string()),
            location: Some("loc".to_string()),
            start_utc: "2026-10-12T06:00:00Z".to_string(),
            end_utc: "2026-10-12T06:30:00Z".to_string(),
            status: None,
            timezone: Some("Europe/Budapest".to_string()),
        };
        let created = create_event(State(pool.clone()), Path(cal), Json(payload))
            .await
            .unwrap()
            .0;

        let fetched = get_event(State(pool.clone()), Path((cal, created.id)))
            .await
            .unwrap()
            .0;
        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.title.as_deref(), Some("Get me"));
        assert_eq!(fetched.description.as_deref(), Some("desc"));
        assert_eq!(fetched.location.as_deref(), Some("loc"));
        assert_eq!(fetched.status, "confirmed");
        assert_eq!(fetched.event_kind, "timed");
        assert_eq!(fetched.start_utc.as_deref(), Some("2026-10-12T06:00:00Z"));
        assert_eq!(fetched.end_utc.as_deref(), Some("2026-10-12T06:30:00Z"));
        assert_eq!(fetched.version, Some(1));
    }

    #[tokio::test]
    async fn update_event_returns_stored_values_not_payload() {
        let pool = test_pool().await;
        let cal = test_calendar(&pool).await;
        let payload = CreateEventPayload {
            title: "Original".to_string(),
            description: Some("keep me".to_string()),
            location: Some("keep me too".to_string()),
            start_utc: "2026-10-12T06:00:00Z".to_string(),
            end_utc: "2026-10-12T06:30:00Z".to_string(),
            status: None,
            timezone: Some("Europe/Budapest".to_string()),
        };
        let created = create_event(State(pool.clone()), Path(cal), Json(payload))
            .await
            .unwrap()
            .0;

        // Update only the title; the response must still carry the stored
        // description, location, times, and the real event_kind.
        let updated = update_event(
            State(pool.clone()),
            Path((cal, created.id)),
            Json(UpdateEventPayload {
                title: Some("Renamed".to_string()),
                description: None,
                location: None,
                start_utc: None,
                end_utc: None,
                status: None,
                version: 1,
            }),
        )
        .await
        .unwrap()
        .0;

        assert_eq!(updated.title.as_deref(), Some("Renamed"));
        assert_eq!(updated.description.as_deref(), Some("keep me"));
        assert_eq!(updated.location.as_deref(), Some("keep me too"));
        assert_eq!(updated.event_kind, "timed", "must not hardcode event_kind");
        assert_eq!(updated.start_utc.as_deref(), Some("2026-10-12T06:00:00Z"));
        assert_eq!(updated.end_utc.as_deref(), Some("2026-10-12T06:30:00Z"));
        assert_eq!(updated.version, Some(2));

        // Read-back must agree with the update response.
        let fetched = get_event(State(pool.clone()), Path((cal, created.id)))
            .await
            .unwrap()
            .0;
        assert_eq!(fetched.title.as_deref(), Some("Renamed"));
        assert_eq!(fetched.version, Some(2));
    }

    #[tokio::test]
    async fn update_event_rejects_end_before_start() {
        let pool = test_pool().await;
        let cal = test_calendar(&pool).await;
        let payload = CreateEventPayload {
            title: "Move me".to_string(),
            description: None,
            location: None,
            start_utc: "2026-10-12T06:00:00Z".to_string(),
            end_utc: "2026-10-12T06:30:00Z".to_string(),
            status: None,
            timezone: None,
        };
        let created = create_event(State(pool.clone()), Path(cal), Json(payload))
            .await
            .unwrap()
            .0;

        // New start after the stored end → effective end <= start.
        let result = update_event(
            State(pool.clone()),
            Path((cal, created.id)),
            Json(UpdateEventPayload {
                title: None,
                description: None,
                location: None,
                start_utc: Some("2026-10-12T07:00:00Z".to_string()),
                end_utc: None,
                status: None,
                version: 1,
            }),
        )
        .await;
        assert_eq!(result.err().map(|e| e.0), Some(StatusCode::BAD_REQUEST));

        // Stored row must be unchanged.
        let fetched = get_event(State(pool.clone()), Path((cal, created.id)))
            .await
            .unwrap()
            .0;
        assert_eq!(fetched.version, Some(1));
        assert_eq!(fetched.start_utc.as_deref(), Some("2026-10-12T06:00:00Z"));
    }

    #[tokio::test]
    async fn update_event_version_conflict_rejected() {
        let pool = test_pool().await;
        let cal = test_calendar(&pool).await;
        let payload = CreateEventPayload {
            title: "Conflict".to_string(),
            description: None,
            location: None,
            start_utc: "2026-10-12T06:00:00Z".to_string(),
            end_utc: "2026-10-12T06:30:00Z".to_string(),
            status: None,
            timezone: None,
        };
        let created = create_event(State(pool.clone()), Path(cal), Json(payload))
            .await
            .unwrap()
            .0;

        let result = update_event(
            State(pool.clone()),
            Path((cal, created.id)),
            Json(UpdateEventPayload {
                title: Some("Stale write".to_string()),
                description: None,
                location: None,
                start_utc: None,
                end_utc: None,
                status: None,
                version: 99,
            }),
        )
        .await;
        assert_eq!(result.err().map(|e| e.0), Some(StatusCode::CONFLICT));
    }
}
