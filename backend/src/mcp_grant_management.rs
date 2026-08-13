// McpGrant management API handlers.
//
// These endpoints allow users to manage their MCP grant permissions
// through the frontend.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Deserialize)]
pub struct CreateMcpGrantPayload {
    pub oauth_client_id: String,
    pub calendar_ids: Vec<i64>,
    pub allow_availability: bool,
    pub allow_event_titles: bool,
    pub allow_event_details: bool,
    pub allow_create: bool,
    pub allow_update: bool,
    pub allow_delete: bool,
    pub expires_at: Option<i64>,
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
pub struct UpdateMcpGrantPayload {
    pub calendar_ids: Option<Vec<i64>>,
    pub allow_availability: Option<bool>,
    pub allow_event_titles: Option<bool>,
    pub allow_event_details: Option<bool>,
    pub allow_create: Option<bool>,
    pub allow_update: Option<bool>,
    pub allow_delete: Option<bool>,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct GrantIdPath {
    pub id: String,
}

/// List all MCP grants for the current user.
pub async fn list_mcp_grants(
    State(pool): State<SqlitePool>,
    // In production, user_id comes from session middleware.
    // This is a placeholder — the real handler would extract user_id from session.
) -> Result<Json<Vec<McpGrantResponse>>, (StatusCode, String)> {
    // Placeholder: return empty list.
    // Real implementation would require user_id from session.
    let _ = pool;
    Ok(Json(vec![]))
}

/// Create a new MCP grant.
pub async fn create_mcp_grant(
    State(pool): State<SqlitePool>,
    // user_id from session
    Json(payload): Json<CreateMcpGrantPayload>,
) -> Result<Json<McpGrantResponse>, (StatusCode, String)> {
    let grant_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();

    let calendar_ids_json = serde_json::to_string(&payload.calendar_ids)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    sqlx::query(
        "INSERT INTO mcp_grant (id, user_id, oauth_client_id, allowed_calendar_ids, allow_availability, allow_event_titles, allow_event_details, allow_create, allow_update, allow_delete, created_at, last_used_at, expires_at, revoked_at) VALUES (?, 0, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, NULL)"
    )
    .bind(&grant_id)
    .bind(&payload.oauth_client_id)
    .bind(&calendar_ids_json)
    .bind(if payload.allow_availability { 1 } else { 0 })
    .bind(if payload.allow_event_titles { 1 } else { 0 })
    .bind(if payload.allow_event_details { 1 } else { 0 })
    .bind(if payload.allow_create { 1 } else { 0 })
    .bind(if payload.allow_update { 1 } else { 0 })
    .bind(if payload.allow_delete { 1 } else { 0 })
    .bind(now)
    .bind(payload.expires_at)
    .execute(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(McpGrantResponse {
        grant_id,
        user_id: 0,
        oauth_client_id: payload.oauth_client_id,
        allowed_calendar_ids: payload.calendar_ids,
        allow_availability: payload.allow_availability,
        allow_event_titles: payload.allow_event_titles,
        allow_event_details: payload.allow_event_details,
        allow_create: payload.allow_create,
        allow_update: payload.allow_update,
        allow_delete: payload.allow_delete,
        created_at: now,
        last_used_at: None,
        expires_at: payload.expires_at,
        revoked_at: None,
    }))
}

/// Update an existing MCP grant.
pub async fn update_mcp_grant(
    State(pool): State<SqlitePool>,
    Path(id): Path<String>,
    Json(payload): Json<UpdateMcpGrantPayload>,
) -> Result<Json<McpGrantResponse>, (StatusCode, String)> {
    let now = chrono::Utc::now().timestamp();

    // Build dynamic update query with placeholders.
    let mut sets = Vec::new();

    if payload.calendar_ids.is_some() {
        sets.push("allowed_calendar_ids = ?".to_string());
    }
    if payload.allow_availability.is_some() {
        sets.push("allow_availability = ?".to_string());
    }
    if payload.allow_event_titles.is_some() {
        sets.push("allow_event_titles = ?".to_string());
    }
    if payload.allow_event_details.is_some() {
        sets.push("allow_event_details = ?".to_string());
    }
    if payload.allow_create.is_some() {
        sets.push("allow_create = ?".to_string());
    }
    if payload.allow_update.is_some() {
        sets.push("allow_update = ?".to_string());
    }
    if payload.allow_delete.is_some() {
        sets.push("allow_delete = ?".to_string());
    }
    if payload.expires_at.is_some() {
        sets.push("expires_at = ?".to_string());
    }

    if sets.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "no fields to update".to_string()));
    }

    sets.push("updated_at = ?".to_string());
    sets.push("id = ?".to_string());

    let query_str = format!(
        "UPDATE mcp_grant SET {} WHERE id = ?",
        sets.join(", ")
    );

    let result = sqlx::query(&query_str)
        .bind(payload.calendar_ids.map(|c| serde_json::to_string(&c).unwrap_or_default()))
        .bind(payload.allow_availability.map(|v| if v { 1i32 } else { 0i32 }))
        .bind(payload.allow_event_titles.map(|v| if v { 1i32 } else { 0i32 }))
        .bind(payload.allow_event_details.map(|v| if v { 1i32 } else { 0i32 }))
        .bind(payload.allow_create.map(|v| if v { 1i32 } else { 0i32 }))
        .bind(payload.allow_update.map(|v| if v { 1i32 } else { 0i32 }))
        .bind(payload.allow_delete.map(|v| if v { 1i32 } else { 0i32 }))
        .bind(payload.expires_at)
        .bind(now)
        .bind(&id)
        .execute(&pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if result.rows_affected() == 0 {
        return Err((StatusCode::NOT_FOUND, "grant not found".to_string()));
    }

    // Fetch the updated grant.
    let grant = sqlx::query_as::<_, (String, i64, String, String, i32, i32, i32, i32, i32, i32, i64, Option<i64>, Option<i64>, Option<i64>)>(
        "SELECT id, user_id, oauth_client_id, allowed_calendar_ids, allow_availability, allow_event_titles, allow_event_details, allow_create, allow_update, allow_delete, created_at, last_used_at, expires_at, revoked_at FROM mcp_grant WHERE id = ?"
    )
    .bind(&id)
    .fetch_optional(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    match grant {
        Some((id, user_id, client_id, calendar_ids, avail, titles, details, create, update, delete, created_at, last_used, expires, revoked)) => {
            let calendars: Vec<i64> = serde_json::from_str(&calendar_ids).unwrap_or_default();
            Ok(Json(McpGrantResponse {
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
            }))
        }
        None => Err((StatusCode::NOT_FOUND, "grant not found".to_string())),
    }
}

/// Revoke an MCP grant.
pub async fn revoke_mcp_grant(
    State(pool): State<SqlitePool>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let now = chrono::Utc::now().timestamp();

    let result = sqlx::query(
        "UPDATE mcp_grant SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL"
    )
    .bind(now)
    .bind(&id)
    .execute(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if result.rows_affected() == 0 {
        Err((StatusCode::NOT_FOUND, "grant not found or already revoked".to_string()))
    } else {
        Ok(StatusCode::OK)
    }
}

/// Resend confirmation for an MCP grant.
pub async fn resend_mcp_grant_confirmation(
    State(pool): State<SqlitePool>,
    Path(id): Path<String>,
) -> Result<Json<McpGrantResponse>, (StatusCode, String)> {
    // In production, this would trigger an email/SMS confirmation.
    // For now, just return the grant.
    let grant = sqlx::query_as::<_, (String, i64, String, String, i32, i32, i32, i32, i32, i32, i64, Option<i64>, Option<i64>, Option<i64>)>(
        "SELECT id, user_id, oauth_client_id, allowed_calendar_ids, allow_availability, allow_event_titles, allow_event_details, allow_create, allow_update, allow_delete, created_at, last_used_at, expires_at, revoked_at FROM mcp_grant WHERE id = ?"
    )
    .bind(&id)
    .fetch_optional(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    match grant {
        Some((id, user_id, client_id, calendar_ids, avail, titles, details, create, update, delete, created_at, last_used, expires, revoked)) => {
            let calendars: Vec<i64> = serde_json::from_str(&calendar_ids).unwrap_or_default();
            Ok(Json(McpGrantResponse {
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
            }))
        }
        None => Err((StatusCode::NOT_FOUND, "grant not found".to_string())),
    }
}
