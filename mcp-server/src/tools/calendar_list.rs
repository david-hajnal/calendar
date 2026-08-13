// calendar_list tool handler.
//
// Returns the user's calendars filtered by McpGrant permissions.
// Requires:
// - Valid OAuth token
// - McpGrant with allow_availability=true (metadata read)
// - Calendar IDs in grant's allowed_calendar_ids

use axum::http::{Response, StatusCode};
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::error::ToolError;
use crate::internal_client::{CalendarInfo, InternalClient};
use crate::mcp_grant::{check_calendar_access, get_grant};
use crate::oauth::TokenValidationResult;
use crate::output_schema::{CalendarListOutput, CalendarSummary, ContentBlock, ToolOutput};

#[derive(Debug, Deserialize)]
pub struct CalendarListParams {
    #[serde(default)]
    pub include_access: bool,
}

/// Handle the calendar_list tool call.
///
/// Full authorization pipeline:
/// 1. Validate OAuth token → TokenValidationResult
/// 2. Load McpGrant from DB → check allow_availability
/// 3. Call internal API to get calendars
/// 4. Filter by grant's allowed_calendar_ids
/// 5. Return structured response
pub async fn handle(
    token: &TokenValidationResult,
    db_pool: &SqlitePool,
    internal_client: &InternalClient,
    params: CalendarListParams,
) -> Result<Response<axum::body::Body>, ToolError> {
    // Step 1: Load the McpGrant for this user + client.
    let grant = get_grant(db_pool, token.user_id, &token.oauth_client_id)
        .await
        .map_err(|e| ToolError::Internal(format!("grant lookup failed: {}", e)))?;

    let grant = grant.ok_or(ToolError::Forbidden("no MCP grant found".to_string()))?;

    // Step 2: Check tool permission.
    if !crate::mcp_grant::check_tool_permission(&grant, "availability_find") {
        return Err(ToolError::Forbidden("calendar_list requires availability permission".to_string()));
    }

    // Step 3: Fetch calendars from internal API.
    let calendars = internal_client
        .list_calendars(token.user_id)
        .await
        .map_err(|e| ToolError::Internal(format!("calendar fetch failed: {}", e)))?;

    // Step 4: Filter by grant's allowed calendars.
    let filtered: Vec<CalendarSummary> = calendars
        .into_iter()
        .filter(|c| check_calendar_access(&grant, c.id))
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
