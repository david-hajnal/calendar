// event_get tool handler.
//
// Returns event details for a specific event ID.
// Requires:
// - Valid OAuth token
// - McpGrant with allow_event_titles (basic) or allow_event_details (full)
// - Calendar ID in grant's allowed_calendar_ids

use axum::http::{Response, StatusCode};
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::error::ToolError;
use crate::internal_client::{EventInfo, InternalClient};
use crate::mcp_grant::{check_calendar_access, get_grant};
use crate::oauth::TokenValidationResult;
use crate::output_schema::{ContentBlock, EventDescription, EventOutput, EventSummary, ToolOutput};

#[derive(Debug, Deserialize)]
pub struct EventGetParams {
    pub calendar_id: i64,
    pub event_id: i64,
}

/// Handle the event_get tool call.
///
/// Full authorization pipeline:
/// 1. Validate OAuth token → TokenValidationResult
/// 2. Load McpGrant → check allow_event_titles or allow_event_details
/// 3. Check calendar access
/// 4. Call internal API to get event
/// 5. Return structured response with access level control
pub async fn handle(
    token: &TokenValidationResult,
    db_pool: &SqlitePool,
    internal_client: &InternalClient,
    params: EventGetParams,
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
    if !crate::mcp_grant::check_tool_permission(&grant, "event_get") {
        return Err(ToolError::Forbidden(
            "event_get requires event titles permission".to_string(),
        ));
    }

    // Step 4: Determine access level.
    let access_level = if grant.allow_event_details {
        "full"
    } else if grant.allow_event_titles {
        "basic"
    } else {
        return Err(ToolError::Forbidden(
            "event_get requires event titles permission".to_string(),
        ));
    };

    // Step 5: Fetch event from internal API.
    let event_info = internal_client
        .get_event(params.calendar_id, params.event_id)
        .await
        .map_err(|e| match e {
            crate::internal_client::InternalError::Http(404, _) => ToolError::NotFound,
            _ => ToolError::Internal(format!("event fetch failed: {}", e)),
        })?;

    // Step 6: Build structured response based on access level.
    let event_summary = if access_level == "full" {
        EventSummary {
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
        }
    } else {
        // Basic access: only title, status, event_kind.
        EventSummary {
            id: event_info.id,
            calendar_id: event_info.calendar_id,
            title: event_info.title.clone(),
            description: None,
            location: None,
            status: event_info.status,
            event_kind: event_info.event_kind,
            start_utc: event_info.start_utc.map(|t| t.to_string()),
            end_utc: event_info.end_utc.map(|t| t.to_string()),
            version: event_info.version.unwrap_or(0),
        }
    };

    let output = EventOutput {
        event: event_summary,
        access: access_level.to_string(),
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
    fn event_get_params_deserializes() {
        let json = r#"{"calendar_id": 1, "event_id": 42}"#;
        let params: EventGetParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.calendar_id, 1);
        assert_eq!(params.event_id, 42);
    }

    #[test]
    fn event_summary_serializes_with_all_fields() {
        let summary = EventSummary {
            id: 100,
            calendar_id: 1,
            title: Some("Meeting".to_string()),
            description: Some(EventDescription {
                value: "Discuss Q1".to_string(),
                trust: "user_supplied_untrusted",
            }),
            location: Some("Room 5".to_string()),
            status: "confirmed".to_string(),
            event_kind: "default".to_string(),
            start_utc: Some("1704067200".to_string()),
            end_utc: Some("1704070800".to_string()),
            version: 3,
        };
        let json = serde_json::to_string_pretty(&summary).unwrap();
        assert!(json.contains("\"id\""));
        assert!(json.contains("100"));
        assert!(json.contains("\"title\""));
        assert!(json.contains("\"Meeting\""));
        assert!(json.contains("\"status\""));
        assert!(json.contains("\"confirmed\""));
    }

    #[test]
    fn event_summary_serializes_with_null_fields() {
        let summary = EventSummary {
            id: 101,
            calendar_id: 1,
            title: None,
            description: None,
            location: None,
            status: "cancelled".to_string(),
            event_kind: "default".to_string(),
            start_utc: None,
            end_utc: None,
            version: 0,
        };
        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("\"id\":101"));
        assert!(json.contains("\"status\":\"cancelled\""));
    }

    #[test]
    fn event_output_serializes() {
        let output = EventOutput {
            event: EventSummary {
                id: 100,
                calendar_id: 1,
                title: Some("Test".to_string()),
                description: None,
                location: None,
                status: "confirmed".to_string(),
                event_kind: "default".to_string(),
                start_utc: None,
                end_utc: None,
                version: 1,
            },
            access: "full".to_string(),
        };
        let json = serde_json::to_string_pretty(&output).unwrap();
        assert!(json.contains("\"event\""));
        assert!(json.contains("\"access\""));
        assert!(json.contains("full"));
    }

    #[test]
    fn event_description_serializes() {
        let desc = EventDescription {
            value: "Injected <script>".to_string(),
            trust: "user_supplied_untrusted",
        };
        let json = serde_json::to_string(&desc).unwrap();
        assert!(json.contains("\"value\":\"Injected <script>\""));
        assert!(json.contains("\"trust\":\"user_supplied_untrusted\""));
    }

    #[test]
    fn event_summary_skips_null_optionals() {
        let summary = EventSummary {
            id: 1,
            calendar_id: 1,
            title: None,
            description: None,
            location: None,
            status: "confirmed".to_string(),
            event_kind: "default".to_string(),
            start_utc: None,
            end_utc: None,
            version: 0,
        };
        let json = serde_json::to_string(&summary).unwrap();
        assert!(!json.contains("description"));
        assert!(!json.contains("location"));
    }
}
