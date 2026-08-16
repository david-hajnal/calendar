// McpGrant persistence and enforcement module.
//
// Handles loading and validating MCP grants from the MCP service's local SQLite.
// In production, McpGrant is also stored in CommonCal DB as source of truth.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::error::GrantError;

/// Raw database row for mcp_grant.
#[derive(Debug, Deserialize, sqlx::FromRow)]
struct McpGrantRow {
    grant_id: String,
    user_id: i64,
    oauth_client_id: String,
    allowed_calendar_ids: String,
    allow_availability: i64,
    allow_event_titles: i64,
    allow_event_details: i64,
    allow_create: i64,
    allow_update: i64,
    allow_delete: i64,
    created_at: i64,
    last_used_at: Option<i64>,
    expires_at: Option<i64>,
    revoked_at: Option<i64>,
}

impl McpGrantRow {
    fn to_grant(self) -> McpGrant {
        let allowed_calendar_ids: Vec<i64> =
            serde_json::from_str(&self.allowed_calendar_ids).unwrap_or_default();

        McpGrant {
            grant_id: self.grant_id,
            user_id: self.user_id,
            oauth_client_id: self.oauth_client_id,
            allowed_calendar_ids,
            allow_availability: self.allow_availability != 0,
            allow_event_titles: self.allow_event_titles != 0,
            allow_event_details: self.allow_event_details != 0,
            allow_create: self.allow_create != 0,
            allow_update: self.allow_update != 0,
            allow_delete: self.allow_delete != 0,
            created_at: self.created_at,
            last_used_at: self.last_used_at,
            expires_at: self.expires_at,
            revoked_at: self.revoked_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpGrant {
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

/// Load McpGrant for a user + OAuth client pair from the DB.
///
/// Returns None if no grant exists (not an error — the client may have no grants).
pub async fn get_grant(
    pool: &SqlitePool,
    user_id: i64,
    client_id: &str,
) -> Result<Option<McpGrant>, GrantError> {
    let row = sqlx::query_as::<_, McpGrantRow>(
        r#"
        SELECT
            grant_id,
            user_id,
            oauth_client_id,
            allowed_calendar_ids,
            allow_availability,
            allow_event_titles,
            allow_event_details,
            allow_create,
            allow_update,
            allow_delete,
            created_at,
            last_used_at,
            expires_at,
            revoked_at
        FROM mcp_grant
        WHERE user_id = ? AND oauth_client_id = ?
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .bind(client_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| GrantError::Db(format!("failed to load mcp_grant: {}", e)))?;

    Ok(row.map(|r| r.to_grant()))
}

/// Check if a calendar is accessible under the grant.
///
/// Returns true if:
/// - Grant is not revoked
/// - Grant is not expired (if expires_at is set)
/// - Calendar ID is in the allowed list
pub fn check_calendar_access(grant: &McpGrant, calendar_id: i64) -> bool {
    if grant.revoked_at.is_some() {
        return false;
    }
    if let Some(expires_at) = grant.expires_at {
        if current_time_secs() > expires_at {
            return false;
        }
    }
    grant.allowed_calendar_ids.contains(&calendar_id)
}

/// Check if a tool is permitted under the grant.
///
/// Returns true if:
/// - Grant is not revoked
/// - Grant is not expired
/// - The tool name maps to a permission the grant allows
pub fn check_tool_permission(grant: &McpGrant, tool_name: &str) -> bool {
    if grant.revoked_at.is_some() {
        return false;
    }
    if let Some(expires_at) = grant.expires_at {
        if current_time_secs() > expires_at {
            return false;
        }
    }
    match tool_name {
        "availability_find" | "availability_get" => grant.allow_availability,
        "event_get" | "event_search" => grant.allow_event_titles,
        "event_create" => grant.allow_create,
        "event_update" => grant.allow_update,
        "event_delete_prepare" | "event_delete_commit" => grant.allow_delete,
        "reminder_set" => grant.allow_create,
        _ => false,
    }
}

/// Revoke an McpGrant by setting revoked_at.
pub async fn revoke_grant(pool: &SqlitePool, grant_id: &str) -> Result<(), GrantError> {
    sqlx::query("UPDATE mcp_grant SET revoked_at = ? WHERE grant_id = ?")
        .bind(current_time_secs())
        .bind(grant_id)
        .execute(pool)
        .await
        .map_err(|e| GrantError::Db(format!("failed to revoke grant: {}", e)))?;

    Ok(())
}

/// Get current time as Unix seconds.
///
/// In production this uses system time. For testing, set MCP_TEST_TIME
/// to a fixed timestamp.
pub fn current_time_secs() -> i64 {
    if let Ok(test_time) = std::env::var("MCP_TEST_TIME") {
        return test_time
            .parse()
            .unwrap_or_else(|_| current_time_secs_real());
    }
    current_time_secs_real()
}

fn current_time_secs_real() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_else(|_| 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_grant() -> McpGrant {
        let test_time = current_time_secs();
        McpGrant {
            grant_id: "test-grant".to_string(),
            user_id: 42,
            oauth_client_id: "client-1".to_string(),
            allowed_calendar_ids: vec![1, 2, 3],
            allow_availability: true,
            allow_event_titles: true,
            allow_event_details: true,
            allow_create: true,
            allow_update: true,
            allow_delete: true,
            created_at: test_time - 86400,
            last_used_at: Some(test_time - 3600),
            expires_at: None,
            revoked_at: None,
        }
    }

    // check_calendar_access tests

    #[test]
    fn calendar_access_allowed_when_in_list() {
        let grant = mock_grant();
        assert!(check_calendar_access(&grant, 1));
        assert!(check_calendar_access(&grant, 2));
        assert!(check_calendar_access(&grant, 3));
    }

    #[test]
    fn calendar_access_denied_when_not_in_list() {
        let grant = mock_grant();
        assert!(!check_calendar_access(&grant, 4));
        assert!(!check_calendar_access(&grant, 0));
        assert!(!check_calendar_access(&grant, -1));
    }

    #[test]
    fn calendar_access_denied_when_revoked() {
        let mut grant = mock_grant();
        grant.revoked_at = Some(current_time_secs());
        assert!(!check_calendar_access(&grant, 1));
    }

    #[test]
    fn calendar_access_denied_when_expired() {
        let mut grant = mock_grant();
        grant.expires_at = Some(current_time_secs() - 100);
        assert!(!check_calendar_access(&grant, 1));
    }

    #[test]
    fn calendar_access_allowed_when_not_yet_expired() {
        let mut grant = mock_grant();
        grant.expires_at = Some(current_time_secs() + 100);
        assert!(check_calendar_access(&grant, 1));
    }

    #[test]
    fn calendar_access_denied_when_revoked_overrides_expiry() {
        let mut grant = mock_grant();
        grant.revoked_at = Some(current_time_secs());
        grant.expires_at = Some(current_time_secs() + 100);
        assert!(!check_calendar_access(&grant, 1));
    }

    // check_tool_permission tests

    #[test]
    fn availability_find_permitted() {
        let grant = mock_grant();
        assert!(check_tool_permission(&grant, "availability_find"));
    }

    #[test]
    fn availability_get_permitted() {
        let grant = mock_grant();
        assert!(check_tool_permission(&grant, "availability_get"));
    }

    #[test]
    fn event_get_permitted() {
        let grant = mock_grant();
        assert!(check_tool_permission(&grant, "event_get"));
    }

    #[test]
    fn event_search_permitted() {
        let grant = mock_grant();
        assert!(check_tool_permission(&grant, "event_search"));
    }

    #[test]
    fn event_create_permitted() {
        let grant = mock_grant();
        assert!(check_tool_permission(&grant, "event_create"));
    }

    #[test]
    fn event_update_permitted() {
        let grant = mock_grant();
        assert!(check_tool_permission(&grant, "event_update"));
    }

    #[test]
    fn event_delete_prepare_permitted() {
        let grant = mock_grant();
        assert!(check_tool_permission(&grant, "event_delete_prepare"));
    }

    #[test]
    fn event_delete_commit_permitted() {
        let grant = mock_grant();
        assert!(check_tool_permission(&grant, "event_delete_commit"));
    }

    #[test]
    fn reminder_set_permitted() {
        let grant = mock_grant();
        assert!(check_tool_permission(&grant, "reminder_set"));
    }

    #[test]
    fn unknown_tool_denied() {
        let grant = mock_grant();
        assert!(!check_tool_permission(&grant, "unknown_tool"));
        assert!(!check_tool_permission(&grant, ""));
    }

    #[test]
    fn availability_denied_when_allow_availability_false() {
        let mut grant = mock_grant();
        grant.allow_availability = false;
        assert!(!check_tool_permission(&grant, "availability_find"));
        assert!(!check_tool_permission(&grant, "availability_get"));
    }

    #[test]
    fn event_read_denied_when_allow_event_titles_false() {
        let mut grant = mock_grant();
        grant.allow_event_titles = false;
        assert!(!check_tool_permission(&grant, "event_get"));
        assert!(!check_tool_permission(&grant, "event_search"));
    }

    #[test]
    fn create_denied_when_allow_create_false() {
        let mut grant = mock_grant();
        grant.allow_create = false;
        assert!(!check_tool_permission(&grant, "event_create"));
        assert!(!check_tool_permission(&grant, "reminder_set"));
    }

    #[test]
    fn update_denied_when_allow_update_false() {
        let mut grant = mock_grant();
        grant.allow_update = false;
        assert!(!check_tool_permission(&grant, "event_update"));
    }

    #[test]
    fn delete_denied_when_allow_delete_false() {
        let mut grant = mock_grant();
        grant.allow_delete = false;
        assert!(!check_tool_permission(&grant, "event_delete_prepare"));
        assert!(!check_tool_permission(&grant, "event_delete_commit"));
    }

    #[test]
    fn tool_permission_denied_when_revoked() {
        let mut grant = mock_grant();
        grant.revoked_at = Some(current_time_secs());
        assert!(!check_tool_permission(&grant, "event_create"));
        assert!(!check_tool_permission(&grant, "event_get"));
    }

    #[test]
    fn tool_permission_denied_when_expired() {
        let mut grant = mock_grant();
        grant.expires_at = Some(current_time_secs() - 100);
        assert!(!check_tool_permission(&grant, "event_create"));
        assert!(!check_tool_permission(&grant, "event_get"));
    }

    // McpGrant serialization tests

    #[test]
    fn grant_serializes_and_deserializes() {
        let grant = mock_grant();
        let json = serde_json::to_string(&grant).unwrap();
        let restored: McpGrant = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.grant_id, grant.grant_id);
        assert_eq!(restored.user_id, grant.user_id);
        assert_eq!(restored.oauth_client_id, grant.oauth_client_id);
        assert_eq!(restored.allowed_calendar_ids, grant.allowed_calendar_ids);
        assert_eq!(restored.allow_availability, grant.allow_availability);
        assert_eq!(restored.allow_event_titles, grant.allow_event_titles);
        assert_eq!(restored.allow_event_details, grant.allow_event_details);
        assert_eq!(restored.allow_create, grant.allow_create);
        assert_eq!(restored.allow_update, grant.allow_update);
        assert_eq!(restored.allow_delete, grant.allow_delete);
    }

    #[test]
    fn grant_clone_works() {
        let grant = mock_grant();
        let cloned = grant.clone();
        assert_eq!(cloned.grant_id, grant.grant_id);
        assert_eq!(cloned.user_id, grant.user_id);
        assert_eq!(cloned.allowed_calendar_ids, grant.allowed_calendar_ids);
    }

    #[test]
    fn grant_with_null_optional_fields_serializes() {
        let grant = McpGrant {
            grant_id: "test".to_string(),
            user_id: 1,
            oauth_client_id: "c1".to_string(),
            allowed_calendar_ids: vec![],
            allow_availability: false,
            allow_event_titles: false,
            allow_event_details: false,
            allow_create: false,
            allow_update: false,
            allow_delete: false,
            created_at: 0,
            last_used_at: None,
            expires_at: None,
            revoked_at: None,
        };
        let json = serde_json::to_string(&grant).unwrap();
        let restored: McpGrant = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.last_used_at, None);
        assert_eq!(restored.expires_at, None);
        assert_eq!(restored.revoked_at, None);
    }

    // GrantError tests

    #[test]
    fn grant_error_no_grant_display() {
        let err = GrantError::NoGrant;
        assert_eq!(format!("{}", err), "no MCP grant found");
    }

    #[test]
    fn grant_error_grant_expired_display() {
        let err = GrantError::GrantExpired;
        assert_eq!(format!("{}", err), "MCP grant has expired");
    }

    #[test]
    fn grant_error_grant_revoked_display() {
        let err = GrantError::GrantRevoked;
        assert_eq!(format!("{}", err), "MCP grant has been revoked");
    }

    #[test]
    fn grant_error_calendar_not_in_grant_display() {
        let err = GrantError::CalendarNotInGrant;
        assert_eq!(format!("{}", err), "calendar not in grant");
    }

    #[test]
    fn grant_error_tool_permission_denied_display() {
        let err = GrantError::ToolPermissionDenied;
        assert_eq!(format!("{}", err), "tool permission denied");
    }

    // Grant with empty calendar list

    #[test]
    fn calendar_access_denied_when_no_calendars_allowed() {
        let mut grant = mock_grant();
        grant.allowed_calendar_ids.clear();
        assert!(!check_calendar_access(&grant, 1));
        assert!(!check_calendar_access(&grant, 999));
    }
}
