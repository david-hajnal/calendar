// event_delete_commit tool handler.
//
// Commits a pending deletion after the user has confirmed via the confirmation URL.
// Requires:
// - Valid OAuth token
// - McpGrant with allow_delete=true
// - Valid delete intent (not expired, not already committed)

use axum::http::{Response, StatusCode};
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::error::ToolError;
use crate::internal_client::{InternalClient};
use crate::mcp_grant::{get_grant};
use crate::oauth::TokenValidationResult;
use crate::output_schema::{ContentBlock, DeleteCommitOutput, ToolOutput};

#[derive(Debug, Deserialize)]
pub struct EventDeleteCommitParams {
    pub intent_id: String,
}

/// Handle the event_delete_commit tool call.
///
/// Full authorization pipeline:
/// 1. Validate OAuth token → TokenValidationResult
/// 2. Load McpGrant → check allow_delete
/// 3. Get delete intent from internal API
/// 4. Verify intent is not expired and not already committed
/// 5. Commit deletion via internal API
/// 6. Return result
pub async fn handle(
    token: &TokenValidationResult,
    db_pool: &SqlitePool,
    internal_client: &InternalClient,
    params: EventDeleteCommitParams,
) -> Result<Response<axum::body::Body>, ToolError> {
    // Step 1: Load the McpGrant.
    let grant = get_grant(db_pool, token.user_id, &token.oauth_client_id)
        .await
        .map_err(|e| ToolError::Internal(format!("grant lookup failed: {}", e)))?;

    let grant = grant.ok_or(ToolError::Forbidden("no MCP grant found".to_string()))?;

    // Step 2: Check tool permission.
    if !crate::mcp_grant::check_tool_permission(&grant, "event_delete_commit") {
        return Err(ToolError::Forbidden(
            "event_delete_commit requires delete permission".to_string(),
        ));
    }

    // Step 3: Get delete intent from internal API.
    let delete_intent = internal_client
        .get_delete_intent(&params.intent_id)
        .await
        .map_err(|e| match e {
            crate::internal_client::InternalError::Http(404, _) => {
                ToolError::BadRequest("delete intent not found".to_string())
            }
            _ => ToolError::Internal(format!("delete intent fetch failed: {}", e)),
        })?;

    // Step 4: Verify intent is not expired.
    if delete_intent.expires_at <= crate::mcp_grant::current_time_secs() {
        return Err(ToolError::BadRequest(
            "delete intent has expired".to_string(),
        ));
    }

    // Step 5: Verify intent is not already committed.
    if delete_intent.confirmation_state == "committed" {
        return Err(ToolError::Conflict(
            "delete intent already committed".to_string(),
        ));
    }

    // Step 6: Commit deletion via internal API.
    internal_client
        .commit_delete_intent(&params.intent_id)
        .await
        .map_err(|e| ToolError::Internal(format!("delete commit failed: {}", e)))?;

    // Step 7: Build structured response.
    let output = DeleteCommitOutput {
        deleted: true,
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
    fn event_delete_commit_params_deserializes() {
        let json = r#"{"intent_id": "intent-abc-123"}"#;
        let params: EventDeleteCommitParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.intent_id, "intent-abc-123");
    }

    #[test]
    fn delete_commit_output_serializes() {
        let output = DeleteCommitOutput { deleted: true };
        let json = serde_json::to_string_pretty(&output).unwrap();
        assert!(json.contains("\"deleted\""));
        assert!(json.contains("true"));
    }

    #[test]
    fn delete_commit_output_serializes_false() {
        let output = DeleteCommitOutput { deleted: false };
        let json = serde_json::to_string_pretty(&output).unwrap();
        assert!(json.contains("\"deleted\""));
        assert!(json.contains("false"));
    }
}
