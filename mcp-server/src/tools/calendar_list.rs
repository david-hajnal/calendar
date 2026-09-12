// calendar_list tool handler.
//
// Returns the user's calendars filtered by McpGrant permissions.
// Requires:
// - Valid OAuth token
// - McpGrant with allow_availability=true (metadata read)
// - Calendar IDs in grant's allowed_calendar_ids

use axum::http::{Response, StatusCode};
use serde::Deserialize;

use crate::error::ToolError;
use crate::mcp_grant::check_calendar_access;
use crate::output_schema::{CalendarListOutput, CalendarSummary, ContentBlock, ToolOutput};
use crate::tools::AuthorizedToolContext;

#[derive(Debug, Deserialize)]
pub struct CalendarListParams {
    #[serde(default)]
    pub include_access: bool,
}

/// Handle the calendar_list tool call.
///
/// Authorization pipeline:
/// 1. Gateway validates the OAuth token and resolves the authoritative grant.
/// 2. Check the grant's tool permission.
/// 3. Call CommonCal core to list calendars.
/// 4. Filter by the grant's allowed_calendar_ids.
/// 5. Return the structured response.
pub async fn handle(
    context: &AuthorizedToolContext<'_>,
    _params: CalendarListParams,
) -> Result<Response<axum::body::Body>, ToolError> {
    let grant = context.grant;

    // Check tool permission against the authoritative grant.
    if !crate::mcp_grant::check_tool_permission(grant, "availability_find") {
        return Err(ToolError::Forbidden(
            "calendar_list requires availability permission".to_string(),
        ));
    }

    // Fetch calendars from CommonCal core.
    let calendars = context
        .internal_client
        .list_calendars(context.token.user_id)
        .await
        .map_err(|e| ToolError::Internal(format!("calendar fetch failed: {}", e)))?;

    // Filter by the grant's allowed calendars.
    let filtered: Vec<CalendarSummary> = calendars
        .into_iter()
        .filter(|c| check_calendar_access(grant, c.id))
        .map(|c| CalendarSummary {
            id: c.id,
            name: c.name,
            color: String::new(),
            access: c.access,
        })
        .collect();

    // Step 5: Build structured response.
    let output = ToolOutput {
        content: vec![ContentBlock::Text {
            text: serde_json::to_string_pretty(&CalendarListOutput {
                calendars: filtered,
            })
            .unwrap_or_else(|_| "[]".to_string()),
        }],
    };

    let body = serde_json::to_string_pretty(&output)
        .unwrap_or_else(|_| r#"{"content":[{"text":"[]"}]}"#.to_string());

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap())
}

/// Handle calendar_list for the tracer bullet — returns empty tool catalog.
/// Slice 5 will wire this to the real tool list.
pub async fn handle_empty() -> Result<serde_json::Value, crate::error::ToolError> {
    Ok(serde_json::json!({
        "tools": []
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_list_params_defaults() {
        let json = r#"{}"#;
        let params: CalendarListParams = serde_json::from_str(json).unwrap();
        assert!(!params.include_access);
    }

    #[test]
    fn calendar_list_params_include_access_true() {
        let json = r#"{"include_access": true}"#;
        let params: CalendarListParams = serde_json::from_str(json).unwrap();
        assert!(params.include_access);
    }

    #[test]
    fn calendar_summary_serializes() {
        let summary = CalendarSummary {
            id: 1,
            name: "Work".to_string(),
            color: "#ff0000".to_string(),
            access: "full".to_string(),
        };
        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("\"id\":1"));
        assert!(json.contains("\"name\":\"Work\""));
        assert!(json.contains("\"access\":\"full\""));
    }

    #[test]
    fn calendar_list_output_serializes() {
        let output = CalendarListOutput {
            calendars: vec![
                CalendarSummary {
                    id: 1,
                    name: "Work".to_string(),
                    color: "#ff0000".to_string(),
                    access: "full".to_string(),
                },
                CalendarSummary {
                    id: 2,
                    name: "Personal".to_string(),
                    color: "#00ff00".to_string(),
                    access: "read".to_string(),
                },
            ],
        };
        let json = serde_json::to_string_pretty(&output).unwrap();
        assert!(json.contains("\"calendars\""));
        assert!(json.contains("\"Work\""));
        assert!(json.contains("\"Personal\""));
    }

    #[test]
    fn calendar_list_output_serializes_empty() {
        let output = CalendarListOutput { calendars: vec![] };
        let json = serde_json::to_string_pretty(&output).unwrap();
        assert!(json.contains("\"calendars\""));
        assert!(json.contains("[]"));
    }

    #[test]
    fn content_block_text_serializes() {
        let block = ContentBlock::Text {
            text: "hello".to_string(),
        };
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains("\"Text\""));
        assert!(json.contains("\"text\":\"hello\""));
    }

    #[test]
    fn content_block_image_serializes() {
        let block = ContentBlock::Image {
            data: "iVBOR".to_string(),
            mime_type: "image/png".to_string(),
        };
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains("\"Image\""));
        assert!(json.contains("\"mime_type\":\"image/png\""));
    }

    #[test]
    fn tool_output_serializes() {
        let output = ToolOutput {
            content: vec![ContentBlock::Text {
                text: r#"{"calendars":[]}"#.to_string(),
            }],
        };
        let json = serde_json::to_string_pretty(&output).unwrap();
        assert!(json.contains("\"content\""));
        assert!(json.contains("\"text\""));
    }
}
