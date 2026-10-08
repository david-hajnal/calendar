// McpGrant management API handlers.
//
// These endpoints allow users to manage their MCP grant permissions
// through the frontend. All routes are session-bound: the authenticated
// user id comes from the session (never a browser-supplied value), and
// ownership is enforced on every grant-scoped operation.

use axum::Json;
use axum::Router;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, patch, post};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::sessions::{AuthenticatedSession, SessionManager};

/// Build the MCP grant management router with session middleware applied.
///
/// All routes are session-bound: the authenticated user id comes from the
/// session (never a browser-supplied value), and ownership is enforced on
/// every grant-scoped operation.
pub fn build_mcp_grant_router(pool: SqlitePool, session_manager: SessionManager) -> Router {
    Router::new()
        .route("/api/v1/mcp-grants", get(list_mcp_grants))
        .route("/api/v1/mcp-grants", post(create_mcp_grant))
        .route("/api/v1/mcp-grants/:id", patch(update_mcp_grant))
        .route("/api/v1/mcp-grants/:id", delete(revoke_mcp_grant))
        .route(
            "/api/v1/mcp-grants/:id/resend",
            post(resend_mcp_grant_confirmation),
        )
        .layer(axum::middleware::from_fn_with_state(
            session_manager,
            crate::http::authenticated_session,
        ))
        .with_state(pool)
}

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

/// List all active MCP grants for the authenticated user.
pub async fn list_mcp_grants(
    State(pool): State<SqlitePool>,
    Extension(session): Extension<AuthenticatedSession>,
) -> Result<Json<Vec<McpGrantResponse>>, (StatusCode, String)> {
    let user_id = session.user.id;
    let now = chrono::Utc::now().timestamp();

    let grants = sqlx::query_as::<_, (String, i64, String, String, i32, i32, i32, i32, i32, i32, i64, Option<i64>, Option<i64>, Option<i64>)>(
        "SELECT id, user_id, oauth_client_id, allowed_calendar_ids, allow_availability, allow_event_titles, allow_event_details, allow_create, allow_update, allow_delete, created_at, last_used_at, expires_at, revoked_at FROM mcp_grant WHERE user_id = ? AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > ?) ORDER BY created_at DESC"
    )
    .bind(user_id)
    .bind(now)
    .fetch_all(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let response: Vec<McpGrantResponse> = grants.into_iter().map(row_to_response).collect();

    Ok(Json(response))
}

/// Create (upsert) an MCP grant for the authenticated user.
///
/// The allowed calendars are intersected with the user's live calendar
/// membership so the grant never references calendars the user no longer
/// has access to.
pub async fn create_mcp_grant(
    State(pool): State<SqlitePool>,
    Extension(session): Extension<AuthenticatedSession>,
    Json(payload): Json<CreateMcpGrantPayload>,
) -> Result<Json<McpGrantResponse>, (StatusCode, String)> {
    let user_id = session.user.id;
    let now = chrono::Utc::now().timestamp();

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Intersect requested calendars with the live allowed set.
    let live_calendars: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT c.id FROM calendars c
         JOIN calendar_acl ca ON c.id = ca.calendar_id
         WHERE ca.user_id = ? AND c.archived = 0",
    )
    .bind(user_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let allowed_ids: Vec<i64> = payload
        .calendar_ids
        .iter()
        .filter(|id| live_calendars.contains(id))
        .cloned()
        .collect();

    // Revoke any existing active grant for this (user, client) pair.
    sqlx::query(
        "UPDATE mcp_grant SET revoked_at = ? WHERE user_id = ? AND oauth_client_id = ? AND revoked_at IS NULL",
    )
    .bind(now)
    .bind(user_id)
    .bind(&payload.oauth_client_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let grant_id = uuid::Uuid::new_v4().to_string();
    let calendar_ids_json = serde_json::to_string(&allowed_ids)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    sqlx::query(
        "INSERT INTO mcp_grant (id, user_id, oauth_client_id, allowed_calendar_ids, allow_availability, allow_event_titles, allow_event_details, allow_create, allow_update, allow_delete, created_at, last_used_at, expires_at, revoked_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, NULL)"
    )
    .bind(&grant_id)
    .bind(user_id)
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
    .execute(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(McpGrantResponse {
        grant_id,
        user_id,
        oauth_client_id: payload.oauth_client_id,
        allowed_calendar_ids: allowed_ids,
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

/// Update (narrow) an existing MCP grant.
///
/// Ownership is enforced: a grant belonging to another user is not found.
/// Calendars, permissions, and expiration may only be narrowed.
pub async fn update_mcp_grant(
    State(pool): State<SqlitePool>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(id): Path<String>,
    Json(payload): Json<UpdateMcpGrantPayload>,
) -> Result<Json<McpGrantResponse>, (StatusCode, String)> {
    let user_id = session.user.id;

    let mut tx = pool
        .begin()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Fetch the current grant, enforcing ownership.
    let current = sqlx::query_as::<_, (String, i64, String, String, i32, i32, i32, i32, i32, i32, i64, Option<i64>, Option<i64>, Option<i64>)>(
        "SELECT id, user_id, oauth_client_id, allowed_calendar_ids, allow_availability, allow_event_titles, allow_event_details, allow_create, allow_update, allow_delete, created_at, last_used_at, expires_at, revoked_at FROM mcp_grant WHERE id = ? AND user_id = ? AND revoked_at IS NULL"
    )
    .bind(&id)
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let Some(current) = current else {
        return Err((StatusCode::NOT_FOUND, "grant not found".to_string()));
    };

    let current_calendars: Vec<i64> = serde_json::from_str(&current.3).unwrap_or_default();

    // No broadening: every requested calendar must already be allowed.
    if let Some(new_calendars) = &payload.calendar_ids
        && !new_calendars
            .iter()
            .all(|id| current_calendars.contains(id))
    {
        return Err((StatusCode::FORBIDDEN, "cannot_broaden_grant".to_string()));
    }

    // Updates may remove authority, but adding a permission or extending its
    // lifetime requires fresh OAuth consent.
    let permissions = [
        (payload.allow_availability, current.4),
        (payload.allow_event_titles, current.5),
        (payload.allow_event_details, current.6),
        (payload.allow_create, current.7),
        (payload.allow_update, current.8),
        (payload.allow_delete, current.9),
    ];
    if permissions
        .iter()
        .any(|(requested, existing)| *requested == Some(true) && *existing == 0)
        || matches!((payload.expires_at, current.12), (Some(requested), Some(existing)) if requested > existing)
    {
        return Err((StatusCode::FORBIDDEN, "cannot_broaden_grant".to_string()));
    }

    if payload.calendar_ids.is_none()
        && payload.allow_availability.is_none()
        && payload.allow_event_titles.is_none()
        && payload.allow_event_details.is_none()
        && payload.allow_create.is_none()
        && payload.allow_update.is_none()
        && payload.allow_delete.is_none()
        && payload.expires_at.is_none()
    {
        return Err((StatusCode::BAD_REQUEST, "no fields to update".to_string()));
    }

    let calendars_json = payload
        .calendar_ids
        .map(|c| serde_json::to_string(&c).unwrap_or_default());

    sqlx::query(
        "UPDATE mcp_grant SET
            allowed_calendar_ids = COALESCE(?, allowed_calendar_ids),
            allow_availability = COALESCE(?, allow_availability),
            allow_event_titles = COALESCE(?, allow_event_titles),
            allow_event_details = COALESCE(?, allow_event_details),
            allow_create = COALESCE(?, allow_create),
            allow_update = COALESCE(?, allow_update),
            allow_delete = COALESCE(?, allow_delete),
            expires_at = COALESCE(?, expires_at)
         WHERE id = ? AND user_id = ?",
    )
    .bind(calendars_json)
    .bind(
        payload
            .allow_availability
            .map(|v| if v { 1i32 } else { 0i32 }),
    )
    .bind(
        payload
            .allow_event_titles
            .map(|v| if v { 1i32 } else { 0i32 }),
    )
    .bind(
        payload
            .allow_event_details
            .map(|v| if v { 1i32 } else { 0i32 }),
    )
    .bind(payload.allow_create.map(|v| if v { 1i32 } else { 0i32 }))
    .bind(payload.allow_update.map(|v| if v { 1i32 } else { 0i32 }))
    .bind(payload.allow_delete.map(|v| if v { 1i32 } else { 0i32 }))
    .bind(payload.expires_at)
    .bind(&id)
    .bind(user_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Fetch the updated grant.
    let grant = sqlx::query_as::<_, (String, i64, String, String, i32, i32, i32, i32, i32, i32, i64, Option<i64>, Option<i64>, Option<i64>)>(
        "SELECT id, user_id, oauth_client_id, allowed_calendar_ids, allow_availability, allow_event_titles, allow_event_details, allow_create, allow_update, allow_delete, created_at, last_used_at, expires_at, revoked_at FROM mcp_grant WHERE id = ? AND user_id = ?"
    )
    .bind(&id)
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    tx.commit()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    match grant {
        Some(row) => Ok(Json(row_to_response(row))),
        None => Err((StatusCode::NOT_FOUND, "grant not found".to_string())),
    }
}

/// Revoke an MCP grant. Ownership is enforced.
pub async fn revoke_mcp_grant(
    State(pool): State<SqlitePool>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let user_id = session.user.id;
    let now = chrono::Utc::now().timestamp();

    let result = sqlx::query(
        "UPDATE mcp_grant SET revoked_at = ? WHERE id = ? AND user_id = ? AND revoked_at IS NULL",
    )
    .bind(now)
    .bind(&id)
    .bind(user_id)
    .execute(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if result.rows_affected() == 0 {
        Err((
            StatusCode::NOT_FOUND,
            "grant not found or already revoked".to_string(),
        ))
    } else {
        Ok(StatusCode::OK)
    }
}

/// Resend confirmation for an MCP grant. Ownership is enforced.
pub async fn resend_mcp_grant_confirmation(
    State(pool): State<SqlitePool>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(id): Path<String>,
) -> Result<Json<McpGrantResponse>, (StatusCode, String)> {
    let user_id = session.user.id;

    let grant = sqlx::query_as::<_, (String, i64, String, String, i32, i32, i32, i32, i32, i32, i64, Option<i64>, Option<i64>, Option<i64>)>(
        "SELECT id, user_id, oauth_client_id, allowed_calendar_ids, allow_availability, allow_event_titles, allow_event_details, allow_create, allow_update, allow_delete, created_at, last_used_at, expires_at, revoked_at FROM mcp_grant WHERE id = ? AND user_id = ?"
    )
    .bind(&id)
    .bind(user_id)
    .fetch_optional(&pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    match grant {
        Some(row) => Ok(Json(row_to_response(row))),
        None => Err((StatusCode::NOT_FOUND, "grant not found".to_string())),
    }
}

type GrantRow = (
    String,
    i64,
    String,
    String,
    i32,
    i32,
    i32,
    i32,
    i32,
    i32,
    i64,
    Option<i64>,
    Option<i64>,
    Option<i64>,
);

fn row_to_response(
    (
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
    ): GrantRow,
) -> McpGrantResponse {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    fn make_session(user_id: i64) -> AuthenticatedSession {
        let secret_key = crate::security::SecretKey::generate();
        let token = secret_key.generate_token();
        let csrf_token = secret_key.generate_csrf_token(&token).expose().to_owned();
        AuthenticatedSession::new_for_test(
            user_id,
            token,
            csrf_token,
            crate::invitations::ActiveUser {
                id: user_id,
                email: "test@example.com".into(),
                display_name: Some("Test User".into()),
                status: "registered",
                is_superadmin: false,
            },
            1000,
            1000,
            4600,
        )
    }

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

    async fn seed_user_and_calendar(pool: &SqlitePool, email: &str) -> (i64, i64) {
        let now = chrono::Utc::now().timestamp();
        let user_id: i64 = sqlx::query_scalar(
            "INSERT INTO users (normalized_email, display_name, status, created_at, is_superadmin) VALUES (?, 'Test', 'registered', ?, 0) RETURNING id",
        )
        .bind(email)
        .bind(now)
        .fetch_one(pool)
        .await
        .unwrap();
        let cal_id: i64 = sqlx::query_scalar(
            "INSERT INTO calendars (owner_user_id, name, color, default_timezone, default_event_visibility, created_at, updated_at) VALUES (?, 'Cal', '#fff', 'UTC', 'default', ?, ?) RETURNING id",
        )
        .bind(user_id)
        .bind(now)
        .bind(now)
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO calendar_acl (calendar_id, user_id, role, created_at, updated_at) VALUES (?, ?, 'owner', ?, ?)",
        )
        .bind(cal_id)
        .bind(user_id)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
        (user_id, cal_id)
    }

    #[tokio::test]
    async fn list_returns_only_own_active_grants() {
        let pool = test_pool().await;
        let (user1, _cal1) = seed_user_and_calendar(&pool, "user1@example.com").await;
        let (user2, _cal2) = seed_user_and_calendar(&pool, "user2@example.com").await;
        let now = chrono::Utc::now().timestamp();

        // Create grants for both users.
        for user in [user1, user2] {
            sqlx::query(
                "INSERT INTO mcp_grant (id, user_id, oauth_client_id, allowed_calendar_ids, created_at) VALUES (?, ?, 'client', '[]', ?)",
            )
            .bind(format!("grant-{user}"))
            .bind(user)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        }

        // User1 sees only their own grant.
        let grants = list_mcp_grants(State(pool.clone()), Extension(make_session(user1)))
            .await
            .unwrap()
            .0;
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].user_id, user1);

        // User2 sees only their own grant.
        let grants = list_mcp_grants(State(pool.clone()), Extension(make_session(user2)))
            .await
            .unwrap()
            .0;
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].user_id, user2);
    }

    #[tokio::test]
    async fn revoke_is_ownership_checked() {
        let pool = test_pool().await;
        let (user1, _cal1) = seed_user_and_calendar(&pool, "user1@example.com").await;
        let (user2, _cal2) = seed_user_and_calendar(&pool, "user2@example.com").await;
        let now = chrono::Utc::now().timestamp();

        // Create a grant for user1.
        sqlx::query(
            "INSERT INTO mcp_grant (id, user_id, oauth_client_id, allowed_calendar_ids, created_at) VALUES ('grant-1', ?, 'client', '[]', ?)",
        )
        .bind(user1)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();

        // User2 cannot revoke user1's grant.
        let result = revoke_mcp_grant(
            State(pool.clone()),
            Extension(make_session(user2)),
            Path("grant-1".to_string()),
        )
        .await;
        assert!(result.is_err(), "cross-user revoke must fail");
        let (status, _) = result.unwrap_err();
        assert_eq!(status, StatusCode::NOT_FOUND);

        // The grant is still active.
        let revoked: Option<i64> =
            sqlx::query_scalar("SELECT revoked_at FROM mcp_grant WHERE id = 'grant-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(revoked.is_none(), "grant must still be active");

        // User1 can revoke their own grant.
        let result = revoke_mcp_grant(
            State(pool.clone()),
            Extension(make_session(user1)),
            Path("grant-1".to_string()),
        )
        .await;
        assert!(result.is_ok(), "own revoke must succeed");
    }

    #[tokio::test]
    async fn update_is_ownership_checked_and_no_broadening() {
        let pool = test_pool().await;
        let (user1, cal1) = seed_user_and_calendar(&pool, "user1@example.com").await;
        let (user2, _cal2) = seed_user_and_calendar(&pool, "user2@example.com").await;
        let now = chrono::Utc::now().timestamp();

        // Create a grant for user1 with calendar cal1.
        let cal_json = serde_json::to_string(&vec![cal1]).unwrap();
        sqlx::query(
            "INSERT INTO mcp_grant (id, user_id, oauth_client_id, allowed_calendar_ids, created_at) VALUES ('grant-1', ?, 'client', ?, ?)",
        )
        .bind(user1)
        .bind(&cal_json)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();

        // User2 cannot update user1's grant.
        let result = update_mcp_grant(
            State(pool.clone()),
            Extension(make_session(user2)),
            Path("grant-1".to_string()),
            Json(UpdateMcpGrantPayload {
                calendar_ids: Some(vec![cal1]),
                allow_availability: None,
                allow_event_titles: None,
                allow_event_details: None,
                allow_create: None,
                allow_update: None,
                allow_delete: None,
                expires_at: None,
            }),
        )
        .await;
        assert!(result.is_err(), "cross-user update must fail");
        let (status, _) = result.unwrap_err();
        assert_eq!(status, StatusCode::NOT_FOUND);

        // User1 cannot broaden the grant (add a calendar not in the grant).
        let foreign_cal: i64 = sqlx::query_scalar(
            "INSERT INTO calendars (owner_user_id, name, color, default_timezone, default_event_visibility, created_at, updated_at) VALUES (?, 'Foreign', '#fff', 'UTC', 'default', ?, ?) RETURNING id",
        )
        .bind(user2)
        .bind(now)
        .bind(now)
        .fetch_one(&pool)
        .await
        .unwrap();

        let result = update_mcp_grant(
            State(pool.clone()),
            Extension(make_session(user1)),
            Path("grant-1".to_string()),
            Json(UpdateMcpGrantPayload {
                calendar_ids: Some(vec![cal1, foreign_cal]),
                allow_availability: None,
                allow_event_titles: None,
                allow_event_details: None,
                allow_create: None,
                allow_update: None,
                allow_delete: None,
                expires_at: None,
            }),
        )
        .await;
        assert!(result.is_err(), "broadening must fail");
        let (status, _) = result.unwrap_err();
        assert_eq!(status, StatusCode::FORBIDDEN);

        // User1 can narrow the grant (remove a calendar).
        let result = update_mcp_grant(
            State(pool.clone()),
            Extension(make_session(user1)),
            Path("grant-1".to_string()),
            Json(UpdateMcpGrantPayload {
                calendar_ids: Some(vec![]),
                allow_availability: None,
                allow_event_titles: None,
                allow_event_details: None,
                allow_create: None,
                allow_update: None,
                allow_delete: None,
                expires_at: None,
            }),
        )
        .await;
        assert!(
            result.is_ok(),
            "narrowing must succeed: {:?}",
            result.as_ref().err()
        );
        let grant = result.unwrap().0;
        assert!(grant.allowed_calendar_ids.is_empty());
    }

    #[tokio::test]
    async fn create_intersects_with_live_calendars() {
        let pool = test_pool().await;
        let (user1, cal1) = seed_user_and_calendar(&pool, "user1@example.com").await;
        let (user2, foreign_cal) = seed_user_and_calendar(&pool, "user2@example.com").await;
        let _ = user2;

        let result = create_mcp_grant(
            State(pool.clone()),
            Extension(make_session(user1)),
            Json(CreateMcpGrantPayload {
                oauth_client_id: "client".to_string(),
                calendar_ids: vec![cal1, foreign_cal],
                allow_availability: true,
                allow_event_titles: false,
                allow_event_details: false,
                allow_create: false,
                allow_update: false,
                allow_delete: false,
                expires_at: None,
            }),
        )
        .await;
        assert!(result.is_ok());
        let grant = result.unwrap().0;
        // Only the user's own calendar is in the grant.
        assert_eq!(grant.allowed_calendar_ids, vec![cal1]);
        assert!(!grant.allowed_calendar_ids.contains(&foreign_cal));
    }
    #[tokio::test]
    async fn update_rejects_permission_and_expiry_broadening() {
        let pool = test_pool().await;
        let (user, _) = seed_user_and_calendar(&pool, "narrow@example.com").await;
        let now = chrono::Utc::now().timestamp();
        sqlx::query("INSERT INTO mcp_grant (id, user_id, oauth_client_id, allowed_calendar_ids, created_at, expires_at) VALUES ('narrow', ?, 'client', '[]', ?, ?)")
            .bind(user).bind(now).bind(now + 3600).execute(&pool).await.unwrap();
        for field in [
            "allow_availability",
            "allow_event_titles",
            "allow_event_details",
            "allow_create",
            "allow_update",
            "allow_delete",
            "expires_at",
        ] {
            let value = if field == "expires_at" {
                serde_json::json!(now + 7200)
            } else {
                serde_json::json!(true)
            };
            let payload = serde_json::from_value(serde_json::json!({field: value})).unwrap();
            let result = update_mcp_grant(
                State(pool.clone()),
                Extension(make_session(user)),
                Path("narrow".into()),
                Json(payload),
            )
            .await;
            assert_eq!(
                result.err().map(|e| e.0),
                Some(StatusCode::FORBIDDEN),
                "must reject {field} broadening"
            );
        }
        let grant = update_mcp_grant(
            State(pool.clone()),
            Extension(make_session(user)),
            Path("narrow".into()),
            Json(
                serde_json::from_value(
                    serde_json::json!({"expires_at": now + 1800, "allow_create": false}),
                )
                .unwrap(),
            ),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(grant.expires_at, Some(now + 1800));
        assert!(!grant.allow_create);
    }

    #[tokio::test]
    async fn failed_grant_replacement_preserves_existing_grant() {
        let pool = test_pool().await;
        let (user, calendar) = seed_user_and_calendar(&pool, "atomic@example.com").await;
        sqlx::query("INSERT INTO mcp_grant (id, user_id, oauth_client_id, allowed_calendar_ids, created_at) VALUES ('original', ?, 'client', '[]', 1)")
            .bind(user).execute(&pool).await.unwrap();
        sqlx::query("CREATE TRIGGER reject_grant BEFORE INSERT ON mcp_grant BEGIN SELECT RAISE(ABORT, 'injected failure'); END")
            .execute(&pool).await.unwrap();
        let result = create_mcp_grant(
            State(pool.clone()),
            Extension(make_session(user)),
            Json(CreateMcpGrantPayload {
                oauth_client_id: "client".into(),
                calendar_ids: vec![calendar],
                allow_availability: true,
                allow_event_titles: false,
                allow_event_details: false,
                allow_create: false,
                allow_update: false,
                allow_delete: false,
                expires_at: None,
            }),
        )
        .await;
        assert!(result.is_err());
        let revoked: Option<i64> =
            sqlx::query_scalar("SELECT revoked_at FROM mcp_grant WHERE id = 'original'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            revoked.is_none(),
            "failed replacement must not revoke original grant"
        );
    }
}
