// event_create tool handler.
//
// Creates a new event in a calendar.
// Requires:
// - Valid OAuth token
// - McpGrant with allow_create=true
// - Calendar ID in grant's allowed_calendar_ids
// - Non-empty title

use axum::http::{Response, StatusCode};
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::error::ToolError;
use crate::internal_client::{EventInfo, InternalClient};
use crate::mcp_grant::{check_calendar_access, get_grant};
use crate::oauth::TokenValidationResult;
use crate::output_schema::{ContentBlock, EventDescription, EventOutput, EventSummary, ToolOutput};

#[derive(Debug, Deserialize)]
pub struct EventCreateParams {
    pub calendar_id: i64,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub start_utc: Option<String>,
    #[serde(default)]
    pub end_utc: Option<String>,
    #[serde(default)]
    pub operation_id: Option<String>,
}

/// Validate the event_create input.
pub fn validate_create_input(params: &EventCreateParams) -> Result<(), ToolError> {
    if params.title.is_empty() {
        return Err(ToolError::BadRequest(
            "event title cannot be empty".to_string(),
        ));
    }

    if params.title.len() > 256 {
        return Err(ToolError::BadRequest(
            "event title exceeds 256 character limit".to_string(),
        ));
    }

    if let Some(ref desc) = params.description {
        if desc.len() > 10000 {
            return Err(ToolError::BadRequest(
                "event description exceeds 10000 character limit".to_string(),
            ));
        }
    }

    Ok(())
}

/// Handle the event_create tool call.
///
/// Full authorization pipeline:
/// 1. Validate OAuth token → TokenValidationResult
/// 2. Load McpGrant → check allow_create
/// 3. Check calendar access
/// 4. Validate input (non-empty title, length limits)
/// 5. Call internal API to create event
/// 6. Return structured response
pub async fn handle(
    token: &TokenValidationResult,
    db_pool: &SqlitePool,
    internal_client: &InternalClient,
    params: EventCreateParams,
) -> Result<Response<axum::body::Body>, ToolError> {
    // Step 1: Load the McpGrant.
    let grant = get_grant(db_pool, token.user_id, &token.oauth_client_id)
        .await
        .map_err(|e| ToolError::Internal(format!("grant lookup failed: {}", e)))?;

    let grant = grant.ok_or(ToolError::Forbidden("no MCP grant found".to_string()))?;

    // Step 2: Check calendar access.
    if !check_calendar_access(&grant, params.calendar_id) {
        return Err(ToolError::Forbidden(
            "calendar not in grant".to_string(),
        ));
    }

    // Step 3: Check tool permission.
    if !crate::mcp_grant::check_tool_permission(&grant, "event_create") {
        return Err(ToolError::Forbidden(
            "event_create requires create permission".to_string(),
        ));
    }

    // Step 4: Validate input.
    validate_create_input(&params)?;

    // Step 5: Build the event payload for the internal API.
    let payload = serde_json::json!({
        "title": params.title,
        "description": params.description,
        "location": params.location,
        "start_utc": params.start_utc,
        "end_utc": params.end_utc,
    });

    // Step 6: Create event via internal API.
    let event_info = internal_client
        .create_event(params.calendar_id, &payload)
        .await
        .map_err(|e| ToolError::Internal(format!("event create failed: {}", e)))?;

    // Step 7: Build structured response.
    let event_summary = EventSummary {
        id: event_info.id,
        calendar_id: event_info.calendar_id,
        title: event_info.title.clone(),
        description: event_info.description.as_ref().map(|d| EventDescription {
            value: d.clone(),
            trust: "user_supplied_untrusted",
        }),
        location: event_info.location.clone(),
        status: event_info.status,
        event_kind: event_info.event_kind,
        start_utc: event_info.start_utc.map(|t| t.to_string()),
        end_utc: event_info.end_utc.map(|t| t.to_string()),
        version: event_info.version.unwrap_or(0),
    };

    let output = EventOutput {
        event: event_summary,
        access: "full".to_string(),
    };

    let tool_output = ToolOutput {
        content: vec![ContentBlock::Text {
            text: serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{}".to_string()),
        }],
    };

    let body = serde_json::to_string_pretty(&tool_output)
        .unwrap_or_else(|_| r#"{"content":[{"text":"{}"}]}"#.to_string());

    Ok(Response::builder()
        .status(StatusCode::CREATED)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_create_params_deserializes() {
        let json = r#"{"calendar_id": 1, "title": "Meeting", "description": "Discuss Q1", "location": "Room 5", "start_utc": "2024-01-01T10:00:00Z", "end_utc": "2024-01-01T11:00:00Z", "operation_id": "op-123"}"#;
        let params: EventCreateParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.calendar_id, 1);
        assert_eq!(params.title, "Meeting");
        assert_eq!(params.description, Some("Discuss Q1".to_string()));
        assert_eq!(params.location, Some("Room 5".to_string()));
        assert_eq!(params.operation_id, Some("op-123".to_string()));
    }

    #[test]
    fn event_create_params_defaults() {
        let json = r#"{"calendar_id": 1, "title": "Meeting"}"#;
        let params: EventCreateParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.description, None);
        assert_eq!(params.location, None);
        assert_eq!(params.start_utc, None);
        assert_eq!(params.end_utc, None);
        assert_eq!(params.operation_id, None);
    }

    #[test]
    fn validate_create_input_rejects_empty_title() {
        let params = EventCreateParams {
            calendar_id: 1,
            title: "".to_string(),
            description: None,
            location: None,
            start_utc: None,
            end_utc: None,
            operation_id: None,
        };
        let result = validate_create_input(&params);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(format!("{}", err).contains("empty"));
    }

    #[test]
    fn validate_create_input_rejects_long_title() {
        let params = EventCreateParams {
            calendar_id: 1,
            title: "a".repeat(257),
            description: None,
            location: None,
            start_utc: None,
            end_utc: None,
            operation_id: None,
        };
        let result = validate_create_input(&params);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(format!("{}", err).contains("256"));
    }

    #[test]
    fn validate_create_input_accepts_256_char_title() {
        let params = EventCreateParams {
            calendar_id: 1,
            title: "a".repeat(256),
            description: None,
            location: None,
            start_utc: None,
            end_utc: None,
            operation_id: None,
        };
        assert!(validate_create_input(&params).is_ok());
    }

    #[test]
    fn validate_create_input_rejects_long_description() {
        let params = EventCreateParams {
            calendar_id: 1,
            title: "Meeting".to_string(),
            description: Some("a".repeat(10001)),
            location: None,
            start_utc: None,
            end_utc: None,
            operation_id: None,
        };
        let result = validate_create_input(&params);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(format!("{}", err).contains("10000"));
    }

    #[test]
    fn validate_create_input_accepts_10000_char_description() {
        let params = EventCreateParams {
            calendar_id: 1,
            title: "Meeting".to_string(),
            description: Some("a".repeat(10000)),
            location: None,
            start_utc: None,
            end_utc: None,
            operation_id: None,
        };
        assert!(validate_create_input(&params).is_ok());
    }
}
