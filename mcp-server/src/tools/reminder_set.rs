// reminder_set tool handler.
//
// Creates a reminder for an event.
// Requires:
// - Valid OAuth token
// - McpGrant with allow_delete=true (delete permission required for reminders)
// - Calendar ID in grant's allowed_calendar_ids

use axum::http::{Response, StatusCode};
use serde::Deserialize;

use crate::error::ToolError;
use crate::mcp_grant::check_calendar_access;
use crate::output_schema::{ContentBlock, ReminderOutput, ToolOutput};
use crate::tools::AuthorizedToolContext;

#[derive(Debug, Deserialize)]
pub struct ReminderSetParams {
    pub calendar_id: i64,
    pub event_id: i64,
    pub reminder_minutes: i64,
}

/// Handle the reminder_set tool call.
///
/// Authorization pipeline:
/// 1. Gateway validates the OAuth token and resolves the authoritative grant.
/// 2. Check the grant's reminder permission.
/// 3. Check calendar access against the grant.
/// 4. Create the reminder via CommonCal core.
/// 5. Return the reminder details.
pub async fn handle(
    context: &AuthorizedToolContext<'_>,
    params: ReminderSetParams,
) -> Result<Response<axum::body::Body>, ToolError> {
    let grant = context.grant;

    // Check the grant's reminder permission.
    if !crate::mcp_grant::check_tool_permission(grant, "reminder_set") {
        return Err(ToolError::Forbidden(
            "reminder_set requires delete permission".to_string(),
        ));
    }

    // Check calendar access against the authoritative grant.
    if !check_calendar_access(grant, params.calendar_id) {
        return Err(ToolError::Forbidden("calendar not in grant".to_string()));
    }

    // Validate reminder_minutes.
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

    // Create the reminder via CommonCal core.
    let reminder_payload = serde_json::json!({
        "user_id": context.token.user_id,
        "oauth_client_id": context.token.oauth_client_id,
        "event_id": params.event_id,
        "calendar_id": params.calendar_id,
        "reminder_minutes": params.reminder_minutes,
    });

    let reminder = context
        .internal_client
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
