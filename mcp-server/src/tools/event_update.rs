// event_update tool handler.
//
// Updates an existing event in a calendar.
// Requires:
// - Valid OAuth token
// - McpGrant with allow_update=true
// - Calendar ID in grant's allowed_calendar_ids
// - Event version for optimistic concurrency

use axum::http::{Response, StatusCode};
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::error::ToolError;
use crate::internal_client::{EventInfo, InternalClient};
use crate::mcp_grant::{check_calendar_access, get_grant};
use crate::oauth::TokenValidationResult;
use crate::output_schema::{ContentBlock, EventDescription, EventOutput, EventSummary, ToolOutput};

#[derive(Debug, Deserialize)]
pub struct EventUpdateParams {
    pub calendar_id: i64,
    pub event_id: i64,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<Option<String>>,
    #[serde(default)]
    pub location: Option<Option<String>>,
    #[serde(default)]
    pub start_utc: Option<Option<String>>,
    #[serde(default)]
    pub end_utc: Option<Option<String>>,
    pub version: i64,
    #[serde(default)]
    pub operation_id: Option<String>,
}

/// Validate the event_update input.
pub fn validate_update_input(params: &EventUpdateParams) -> Result<(), ToolError> {
    if let Some(ref title) = params.title {
        if title.is_empty() {
            return Err(ToolError::BadRequest(
                "event title cannot be empty".to_string(),
            ));
        }
        if title.len() > 256 {
            return Err(ToolError::BadRequest(
                "event title exceeds 256 character limit".to_string(),
            ));
        }
    }

    if let Some(ref desc) = params.description {
        if let Some(ref d) = *desc {
            if d.len() > 10000 {
                return Err(ToolError::BadRequest(
                    "event description exceeds 10000 character limit".to_string(),
                ));
            }
        }
    }

    if let Some(ref loc) = params.location {
        if let Some(ref l) = *loc {
            if l.len() > 1024 {
                return Err(ToolError::BadRequest(
                    "event location exceeds 1024 character limit".to_string(),
                ));
            }
        }
    }

    Ok(())
}

/// Handle the event_update tool call.
///
/// Full authorization pipeline:
/// 1. Validate OAuth token → TokenValidationResult
/// 2. Load McpGrant → check allow_update
/// 3. Check calendar access
/// 4. Validate input
/// 5. Call internal API to update event (with version check)
/// 6. Return structured response
pub async fn handle(
    token: &TokenValidationResult,
    db_pool: &SqlitePool,
    internal_client: &InternalClient,
    params: EventUpdateParams,
) -> Result<Response<axum::body::Body>, ToolError> {
    // Step 1: Load the McpGrant.
    let grant = get_grant(db_pool, token.user_id, &token.oauth_client_id)
        .await
        .map_err(|e| ToolError::Internal(format!("grant lookup failed: {}", e)))?;

    let grant = grant.ok_or(ToolError::Forbidden("no MCP grant found".to_string()))?;

    // Step 2: Check calendar access.
    if !check_calendar_access(&grant, params.calendar_id) {
        return Err(ToolError::Forbidden("calendar not in grant".to_string()));
    }

    // Step 3: Check tool permission.
    if !crate::mcp_grant::check_tool_permission(&grant, "event_update") {
        return Err(ToolError::Forbidden(
            "event_update requires update permission".to_string(),
        ));
    }

    // Step 4: Validate input.
    validate_update_input(&params)?;

    // Step 5: Build the update payload.
    let mut payload = serde_json::Map::new();
    if let Some(ref title) = params.title {
        payload.insert("title".to_string(), serde_json::json!(title));
    }
    if let Some(ref desc) = params.description {
        payload.insert("description".to_string(), serde_json::json!(desc));
    }
    if let Some(ref loc) = params.location {
        payload.insert("location".to_string(), serde_json::json!(loc));
    }
    if let Some(ref start) = params.start_utc {
        payload.insert("start_utc".to_string(), serde_json::json!(start));
    }
    if let Some(ref end) = params.end_utc {
        payload.insert("end_utc".to_string(), serde_json::json!(end));
    }
    payload.insert("version".to_string(), serde_json::json!(params.version));

    let payload = serde_json::Value::Object(payload);

    // Step 6: Update event via internal API.
    let event_info = internal_client
        .update_event(params.calendar_id, params.event_id, &payload)
        .await
        .map_err(|e| match e {
            crate::internal_client::InternalError::Http(409, _) => {
                ToolError::Conflict("event version conflict — event was modified".to_string())
            }
            crate::internal_client::InternalError::Http(404, _) => ToolError::NotFound,
            _ => ToolError::Internal(format!("event update failed: {}", e)),
        })?;

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
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_update_params_deserializes() {
        let json = r#"{"calendar_id": 1, "event_id": 42, "title": "Updated Meeting", "description": "New description", "location": null, "start_utc": null, "end_utc": null, "version": 3}"#;
        let params: EventUpdateParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.calendar_id, 1);
        assert_eq!(params.event_id, 42);
        assert_eq!(params.title, Some("Updated Meeting".to_string()));
        assert_eq!(params.version, 3);
    }

    #[test]
    fn event_update_params_clears_description() {
        let json = r#"{"calendar_id": 1, "event_id": 42, "description": null, "version": 3}"#;
        let params: EventUpdateParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.description, None);
    }

    #[test]
    fn validate_update_input_accepts_valid_update() {
        let params = EventUpdateParams {
            calendar_id: 1,
            event_id: 42,
            title: Some("Updated".to_string()),
            description: Some(Some("New desc".to_string())),
            location: Some(Some("Room 1".to_string())),
            start_utc: Some(Some("2024-01-01T10:00:00Z".to_string())),
            end_utc: Some(Some("2024-01-01T11:00:00Z".to_string())),
            version: 3,
            operation_id: None,
        };
        assert!(validate_update_input(&params).is_ok());
    }

    #[test]
    fn validate_update_input_rejects_empty_title() {
        let params = EventUpdateParams {
            calendar_id: 1,
            event_id: 42,
            title: Some("".to_string()),
            description: None,
            location: None,
            start_utc: None,
            end_utc: None,
            version: 3,
            operation_id: None,
        };
        assert!(validate_update_input(&params).is_err());
    }

    #[test]
    fn validate_update_input_accepts_none_title() {
        let params = EventUpdateParams {
            calendar_id: 1,
            event_id: 42,
            title: None,
            description: None,
            location: None,
            start_utc: None,
            end_utc: None,
            version: 3,
            operation_id: None,
        };
        assert!(validate_update_input(&params).is_ok());
    }

    #[test]
    fn validate_update_input_rejects_long_title() {
        let params = EventUpdateParams {
            calendar_id: 1,
            event_id: 42,
            title: Some("a".repeat(257)),
            description: None,
            location: None,
            start_utc: None,
            end_utc: None,
            version: 3,
            operation_id: None,
        };
        assert!(validate_update_input(&params).is_err());
    }

    #[test]
    fn validate_update_input_rejects_long_location() {
        let params = EventUpdateParams {
            calendar_id: 1,
            event_id: 42,
            title: Some("Meeting".to_string()),
            description: None,
            location: Some(Some("a".repeat(1025))),
            start_utc: None,
            end_utc: None,
            version: 3,
            operation_id: None,
        };
        assert!(validate_update_input(&params).is_err());
    }

    #[test]
    fn validate_update_input_accepts_1024_char_location() {
        let params = EventUpdateParams {
            calendar_id: 1,
            event_id: 42,
            title: Some("Meeting".to_string()),
            description: None,
            location: Some(Some("a".repeat(1024))),
            start_utc: None,
            end_utc: None,
            version: 3,
            operation_id: None,
        };
        assert!(validate_update_input(&params).is_ok());
    }
}
