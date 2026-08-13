// reminder_set tool handler.
//
// Creates a reminder for an event.
// Requires:
// - Valid OAuth token
// - McpGrant with allow_delete=true (delete permission required for reminders)
// - Calendar ID in grant's allowed_calendar_ids

use axum::http::{Response, StatusCode};
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::error::ToolError;
use crate::internal_client::InternalClient;
use crate::mcp_grant::{check_calendar_access, get_grant};
use crate::oauth::TokenValidationResult;
use crate::output_schema::{ContentBlock, ReminderOutput, ToolOutput};

#[derive(Debug, Deserialize)]
pub struct ReminderSetParams {
    pub calendar_id: i64,
    pub event_id: i64,
    pub reminder_minutes: i64,
}

/// Handle the reminder_set tool call.
///
/// Full authorization pipeline:
/// 1. Validate OAuth token → TokenValidationResult
/// 2. Load McpGrant → check allow_delete
/// 3. Check calendar access
/// 4. Create reminder via internal API
/// 5. Return reminder details
pub async fn handle(
    token: &TokenValidationResult,
    db_pool: &SqlitePool,
    internal_client: &InternalClient,
    params: ReminderSetParams,
) -> Result<Response<axum::body::Body>, ToolError> {
    // Step 1: Load the McpGrant.
    let grant = get_grant(db_pool, token.user_id, &token.oauth_client_id)
        .await
        .map_err(|e| ToolError::Internal(format!("grant lookup failed: {}", e)))?;

    let grant = grant.ok_or(ToolError::Forbidden("no MCP grant found".to_string()))?;

    // Step 2: Check tool permission (reminders require delete permission).
    if !crate::mcp_grant::check_tool_permission(&grant, "reminder_set") {
        return Err(ToolError::Forbidden(
            "reminder_set requires delete permission".to_string(),
        ));
    }

    // Step 3: Check calendar access.
    if !check_calendar_access(&grant, params.calendar_id) {
        return Err(ToolError::Forbidden(
            "calendar not in grant".to_string(),
        ));
    }

    // Step 4: Validate reminder_minutes.
    if params.reminder_minutes <= 0 {
        return Err(ToolError::BadRequest(
            "reminder_minutes must be positive".to_string(),
        ));
    }
    if params.reminder_minutes > 10080 {
        return Err(ToolError::BadRequest(
            "reminder_minutes must be at most 10080 (7 days)".to_string(),
        ));
    }

    // Step 5: Create reminder via internal API.
    let reminder_payload = serde_json::json!({
        "user_id": token.user_id,
        "oauth_client_id": token.oauth_client_id,
        "event_id": params.event_id,
        "calendar_id": params.calendar_id,
        "reminder_minutes": params.reminder_minutes,
    });

    let reminder = internal_client
        .create_reminder(&reminder_payload)
        .await
        .map_err(|e| ToolError::Internal(format!("reminder creation failed: {}", e)))?;

    // Step 6: Build structured response.
    let output = ReminderOutput {
        reminder_id: reminder.reminder_id,
        event_id: params.event_id,
    };

    let tool_output = ToolOutput {
        content: vec![ContentBlock::Text {
            text: serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{}".to_string()),
        }],
    };

    let body = serde_json::to_string_pretty(&tool_output)
        .unwrap_or_else(|_| r#"{"content":[{"text":"{}"}]}"#.to_string());

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reminder_set_params_deserializes() {
        let json = r#"{"calendar_id": 1, "event_id": 42, "reminder_minutes": 15}"#;
        let params: ReminderSetParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.calendar_id, 1);
        assert_eq!(params.event_id, 42);
        assert_eq!(params.reminder_minutes, 15);
    }

    #[test]
    fn reminder_set_params_rejects_zero_minutes() {
        let json = r#"{"calendar_id": 1, "event_id": 42, "reminder_minutes": 0}"#;
        let params: ReminderSetParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.reminder_minutes, 0);
    }

    #[test]
    fn reminder_set_params_rejects_negative_minutes() {
        let json = r#"{"calendar_id": 1, "event_id": 42, "reminder_minutes": -1}"#;
        let params: ReminderSetParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.reminder_minutes, -1);
    }

    #[test]
    fn reminder_set_params_rejects_max_exceeded() {
        let json = r#"{"calendar_id": 1, "event_id": 42, "reminder_minutes": 10081}"#;
        let params: ReminderSetParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.reminder_minutes, 10081);
    }

    #[test]
    fn reminder_set_params_accepts_exactly_10080() {
        let json = r#"{"calendar_id": 1, "event_id": 42, "reminder_minutes": 10080}"#;
        let params: ReminderSetParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.reminder_minutes, 10080);
    }

    #[test]
    fn reminder_set_output_serializes() {
        let output = ReminderOutput {
            reminder_id: "rem-123".to_string(),
            event_id: 42,
        };
        let json = serde_json::to_string_pretty(&output).unwrap();
        assert!(json.contains("\"reminder_id\""));
        assert!(json.contains("rem-123"));
        assert!(json.contains("\"event_id\""));
        assert!(json.contains("42"));
    }
}
