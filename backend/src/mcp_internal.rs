// MCP Server internal API handlers.
//
// These endpoints are called by the MCP server via the x-mcp-api-key header.
// They provide the MCP server with access to user data, calendar data,
// and grant management without exposing the database directly.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
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
    pub payload: serde_json::Value,
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

/// Validate the x-mcp-api-key header.
#[allow(dead_code)]
fn validate_mcp_api_key(headers: &axum::http::HeaderMap) -> Result<(), (StatusCode, &'static str)> {
    let api_key = headers
        .get("x-mcp-api-key")
        .ok_or((StatusCode::UNAUTHORIZED, "missing API key"))?;

    let api_key_str = api_key
        .to_str()
        .map_err(|_| (StatusCode::UNAUTHORIZED, "invalid API key encoding"))?;

    let expected = std::env::var("MCP_INTERNAL_API_KEY").unwrap_or_default();
    if expected.is_empty() || api_key_str == expected {
        Ok(())
    } else {
        Err((StatusCode::UNAUTHORIZED, "invalid API key"))
    }
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
            status,
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
         JOIN calendar_owners co ON c.id = co.calendar_id 
         WHERE co.user_id = ? 
         UNION 
         SELECT c.id, c.name, c.description, ca.role 
         FROM calendars c 
         JOIN calendar_acl ca ON c.id = ca.calendar_id 
         WHERE ca.user_id = ?",
    )
    .bind(user_id)
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
        "SELECT role FROM calendar_acl WHERE calendar_id = ? AND user_id = ?",
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

/// Get event by ID.
pub async fn get_event(
    State(pool): State<SqlitePool>,
    Path((calendar_id, event_id)): Path<(i64, i64)>,
) -> Result<Json<EventInfoResponse>, (StatusCode, String)> {
    let event = sqlx::query_as::<_, (i64, i64, Option<String>, Option<String>, Option<String>, String, String, Option<String>, Option<String>, Option<i64>)>(
        "SELECT id, calendar_id, title, description, location, status, event_kind, start_utc, end_utc, version FROM events WHERE calendar_id = ? AND id = ?"
    )
    .bind(calendar_id)
    .bind(event_id)
    .fetch_optional(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    match event {
        Some((
            id,
            cal_id,
            title,
            description,
            location,
            status,
            event_kind,
            start_utc,
            end_utc,
            version,
        )) => Ok(Json(EventInfoResponse {
            id,
            calendar_id: cal_id,
            title,
            description,
            location,
            status,
            event_kind,
            start_utc: start_utc.map(|t| t.to_string()),
            end_utc: end_utc.map(|t| t.to_string()),
            version,
        })),
        None => Err((StatusCode::NOT_FOUND, "event not found".to_string())),
    }
}

/// Search events by time range.
pub async fn search_events(
    State(pool): State<SqlitePool>,
    Path(calendar_id): Path<i64>,
    Query(params): Query<SearchEventsQuery>,
) -> Result<Json<SearchEventsResponse>, (StatusCode, String)> {
    let events = sqlx::query_as::<_, (i64, i64, Option<String>, Option<String>, Option<String>, String, String, Option<String>, Option<String>, Option<i64>)>(
        "SELECT id, calendar_id, title, description, location, status, event_kind, start_utc, end_utc, version FROM events WHERE calendar_id = ? AND start_utc >= ? AND end_utc <= ? LIMIT 100"
    )
    .bind(calendar_id)
    .bind(&params.from)
    .bind(&params.to)
    .fetch_all(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let response: Vec<EventInfoResponse> = events
        .into_iter()
        .map(
            |(
                id,
                cal_id,
                title,
                description,
                location,
                status,
                event_kind,
                start_utc,
                end_utc,
                version,
            )| {
                EventInfoResponse {
                    id,
                    calendar_id: cal_id,
                    title,
                    description,
                    location,
                    status,
                    event_kind,
                    start_utc: start_utc.map(|t| t.to_string()),
                    end_utc: end_utc.map(|t| t.to_string()),
                    version,
                }
            },
        )
        .collect();

    Ok(Json(SearchEventsResponse {
        events: response,
        next_page_token: params.page_token,
    }))
}

/// Create event.
pub async fn create_event(
    State(pool): State<SqlitePool>,
    Path(calendar_id): Path<i64>,
    Json(payload): Json<CreateEventPayload>,
) -> Result<Json<CreateEventResponse>, (StatusCode, String)> {
    let now = chrono::Utc::now().timestamp();

    let result = sqlx::query(
        "INSERT INTO events (calendar_id, title, description, location, status, event_kind, start_utc, end_utc, version, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)"
    )
    .bind(calendar_id)
    .bind(&payload.title)
    .bind(&payload.description)
    .bind(&payload.location)
    .bind(payload.status.as_deref().unwrap_or("confirmed"))
    .bind("default")
    .bind(&payload.start_utc)
    .bind(&payload.end_utc)
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let event_id = result.last_insert_rowid();

    Ok(Json(CreateEventResponse {
        id: event_id,
        calendar_id,
        title: Some(payload.title),
        description: payload.description,
        location: payload.location,
        status: payload.status.unwrap_or_else(|| "confirmed".to_string()),
        event_kind: "default".to_string(),
        start_utc: Some(payload.start_utc),
        end_utc: Some(payload.end_utc),
        version: 1,
    }))
}

/// Update event.
pub async fn update_event(
    State(pool): State<SqlitePool>,
    Path((calendar_id, event_id)): Path<(i64, i64)>,
    Json(payload): Json<UpdateEventPayload>,
) -> Result<Json<EventInfoResponse>, (StatusCode, String)> {
    // Check version.
    let current_version: Option<i64> =
        sqlx::query_scalar("SELECT version FROM events WHERE id = ? AND calendar_id = ?")
            .bind(event_id)
            .bind(calendar_id)
            .fetch_optional(&pool)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    match current_version {
        Some(v) if v != payload.version => {
            return Err((StatusCode::CONFLICT, "event version conflict".to_string()));
        }
        None => {
            return Err((StatusCode::NOT_FOUND, "event not found".to_string()));
        }
        _ => {}
    }

    let now = chrono::Utc::now().timestamp();
    let new_version = payload.version + 1;

    sqlx::query(
        "UPDATE events SET title = COALESCE(?, title), description = COALESCE(?, description), location = COALESCE(?, location), start_utc = COALESCE(?, start_utc), end_utc = COALESCE(?, end_utc), status = COALESCE(?, status), version = ?, updated_at = ? WHERE id = ? AND calendar_id = ?"
    )
    .bind(&payload.title)
    .bind(&payload.description)
    .bind(&payload.location)
    .bind(&payload.start_utc)
    .bind(&payload.end_utc)
    .bind(&payload.status)
    .bind(new_version)
    .bind(now)
    .bind(event_id)
    .bind(calendar_id)
    .execute(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(EventInfoResponse {
        id: event_id,
        calendar_id,
        title: payload.title,
        description: payload.description,
        location: payload.location,
        status: payload.status.unwrap_or_else(|| "confirmed".to_string()),
        event_kind: "default".to_string(),
        start_utc: payload.start_utc,
        end_utc: payload.end_utc,
        version: Some(new_version),
    }))
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

/// Get MCP grants.
pub async fn get_mcp_grants(
    State(pool): State<SqlitePool>,
    Query(params): Query<McpGrantsQuery>,
) -> Result<Json<Vec<McpGrantResponse>>, (StatusCode, String)> {
    let grants = sqlx::query_as::<_, (String, i64, String, String, i32, i32, i32, i32, i32, i32, i64, Option<i64>, Option<i64>, Option<i64>)>(
        "SELECT id, user_id, oauth_client_id, allowed_calendar_ids, allow_availability, allow_event_titles, allow_event_details, allow_create, allow_update, allow_delete, created_at, last_used_at, expires_at, revoked_at FROM mcp_grant WHERE user_id = ? AND oauth_client_id = ?"
    )
    .bind(params.user_id)
    .bind(&params.client_id)
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
                    allowed_calendar_ids: calendars,
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
) -> Result<Json<Option<serde_json::Value>>, (StatusCode, String)> {
    let result = sqlx::query_as::<_, (i64, String, String)>(
        "SELECT response_status, response_headers, response_body FROM idempotency_key WHERE key = ?"
    )
    .bind(&operation_id)
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
    .bind(0)
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
