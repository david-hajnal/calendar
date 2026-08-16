// event_search tool handler.
//
// Searches events in a calendar within a time range.
// Requires:
// - Valid OAuth token
// - McpGrant with allow_event_titles
// - Time range within max 31 days
// - Max 100 events returned

use axum::http::{Response, StatusCode};
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::error::ToolError;
use crate::internal_client::{EventInfo, InternalClient};
use crate::mcp_grant::{check_calendar_access, get_grant};
use crate::oauth::TokenValidationResult;
use crate::output_schema::{
    ContentBlock, EventDescription, EventOutput, EventSearchOutput, EventSummary, ToolOutput,
};

/// Maximum number of events to return.
const MAX_EVENTS: usize = 100;

#[derive(Debug, Deserialize)]
pub struct EventSearchParams {
    pub calendar_id: i64,
    pub from: String,
    pub to: String,
    pub query: Option<String>,
}

/// Handle the event_search tool call.
///
/// Full authorization pipeline:
/// 1. Validate OAuth token → TokenValidationResult
/// 2. Load McpGrant → check allow_event_titles
/// 3. Check calendar access
/// 4. Validate time range (max 31 days)
/// 5. Call internal API to search events
/// 6. Limit to MAX_EVENTS
/// 7. Return structured response with access level control
pub async fn handle(
    token: &TokenValidationResult,
    db_pool: &SqlitePool,
    internal_client: &InternalClient,
    params: EventSearchParams,
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
    if !crate::mcp_grant::check_tool_permission(&grant, "event_search") {
        return Err(ToolError::Forbidden(
            "event_search requires event titles permission".to_string(),
        ));
    }

    // Step 4: Validate time range (reuse availability_find validation).
    let from_ts = crate::tools::availability_find::parse_utc_timestamp(&params.from)
        .map_err(|e| ToolError::BadRequest(format!("invalid 'from' timestamp: {}", e)))?;
    let to_ts = crate::tools::availability_find::parse_utc_timestamp(&params.to)
        .map_err(|e| ToolError::BadRequest(format!("invalid 'to' timestamp: {}", e)))?;

    if to_ts <= from_ts {
        return Err(ToolError::BadRequest(
            "'to' must be after 'from'".to_string(),
        ));
    }

    let range_secs = to_ts - from_ts;
    let max_secs = 31 * 24 * 3600;

    if range_secs > max_secs {
        return Err(ToolError::BadRequest(format!(
            "time range exceeds maximum of 31 days ({} seconds)",
            max_secs
        )));
    }

    // Step 5: Search events via internal API.
    let events = internal_client
        .search_events(params.calendar_id, &params.from, &params.to)
        .await
        .map_err(|e| ToolError::Internal(format!("event search failed: {}", e)))?;

    let has_more = events.len() > MAX_EVENTS;

    // Step 6: Limit to MAX_EVENTS.
    let limited: Vec<EventInfo> = events.into_iter().take(MAX_EVENTS).collect();

    // Step 7: Build structured response.
    let access_level = if grant.allow_event_details {
        "full"
    } else {
        "basic"
    };

    let event_summaries: Vec<EventSummary> = limited
        .iter()
        .map(|e| {
            if access_level == "full" {
                EventSummary {
                    id: e.id,
                    calendar_id: e.calendar_id,
                    title: e.title.clone(),
                    description: e.description.as_ref().map(|d| EventDescription {
                        value: d.clone(),
                        trust: "user_supplied_untrusted",
                    }),
                    location: e.location.clone(),
                    status: e.status.clone(),
                    event_kind: e.event_kind.clone(),
                    start_utc: e.start_utc.map(|t| t.to_string()),
                    end_utc: e.end_utc.map(|t| t.to_string()),
                    version: e.version.unwrap_or(0),
                }
            } else {
                EventSummary {
                    id: e.id,
                    calendar_id: e.calendar_id,
                    title: e.title.clone(),
                    description: None,
                    location: None,
                    status: e.status.clone(),
                    event_kind: e.event_kind.clone(),
                    start_utc: e.start_utc.map(|t| t.to_string()),
                    end_utc: e.end_utc.map(|t| t.to_string()),
                    version: e.version.unwrap_or(0),
                }
            }
        })
        .collect();

    let output = EventSearchOutput {
        events: event_summaries,
        next_page: if has_more {
            Some("page_token_placeholder".to_string())
        } else {
            None
        },
    };

    let tool_output = ToolOutput {
        content: vec![ContentBlock::Text {
            text: serde_json::to_string_pretty(&output).unwrap_or_else(|_| "[]".to_string()),
        }],
    };

    let body = serde_json::to_string_pretty(&tool_output)
        .unwrap_or_else(|_| r#"{"content":[{"text":"[]"}]}"#.to_string());

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
    fn event_search_params_deserializes_with_query() {
        let json = r#"{"calendar_id": 1, "from": "2024-01-01T00:00:00Z", "to": "2024-01-02T00:00:00Z", "query": "meeting"}"#;
        let params: EventSearchParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.calendar_id, 1);
        assert_eq!(params.query, Some("meeting".to_string()));
    }

    #[test]
    fn event_search_params_deserializes_without_query() {
        let json =
            r#"{"calendar_id": 1, "from": "2024-01-01T00:00:00Z", "to": "2024-01-02T00:00:00Z"}"#;
        let params: EventSearchParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.calendar_id, 1);
        assert_eq!(params.query, None);
    }

    #[test]
    fn event_search_output_serializes_with_next_page() {
        let output = EventSearchOutput {
            events: vec![],
            next_page: Some("abc123".to_string()),
        };
        let json = serde_json::to_string_pretty(&output).unwrap();
        assert!(json.contains("\"next_page\""));
        assert!(json.contains("abc123"));
    }

    #[test]
    fn event_search_output_serializes_without_next_page() {
        let output = EventSearchOutput {
            events: vec![],
            next_page: None,
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"events\""));
        assert!(!json.contains("next_page"));
    }

    #[test]
    fn max_events_constant_is_100() {
        assert_eq!(MAX_EVENTS, 100);
    }
}
