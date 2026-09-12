// event_delete_commit tool handler.
//
// Commits a pending deletion after the user has confirmed via the confirmation URL.
// Requires:
// - Valid OAuth token
// - McpGrant with allow_delete=true
// - Valid delete intent (not expired, not already committed)

use axum::http::{Response, StatusCode};
use serde::Deserialize;

use crate::error::ToolError;
use crate::output_schema::{ContentBlock, DeleteCommitOutput, ToolOutput};
use crate::tools::AuthorizedToolContext;

#[derive(Debug, Deserialize)]
pub struct EventDeleteCommitParams {
    pub intent_id: String,
}

/// Handle the event_delete_commit tool call.
///
/// Authorization pipeline:
/// 1. Gateway validates the OAuth token and resolves the authoritative grant.
/// 2. Check the grant's delete permission.
/// 3. Get the delete intent from CommonCal core.
/// 4. Verify the intent is not expired and not already committed.
/// 5. Commit the deletion via CommonCal core.
/// 6. Return the result.
pub async fn handle(
    context: &AuthorizedToolContext<'_>,
    params: EventDeleteCommitParams,
) -> Result<Response<axum::body::Body>, ToolError> {
    let grant = context.grant;

    // Check the grant's delete permission.
    if !crate::mcp_grant::check_tool_permission(grant, "event_delete_commit") {
        return Err(ToolError::Forbidden(
            "event_delete_commit requires delete permission".to_string(),
        ));
    }

    // Get the delete intent from CommonCal core.
    let delete_intent = context
        .internal_client
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

    // Commit the deletion via CommonCal core.
    context
        .internal_client
        .commit_delete_intent(&params.intent_id)
        .await
        .map_err(|e| ToolError::Internal(format!("delete commit failed: {}", e)))?;

    // Step 7: Build structured response.
    let output = DeleteCommitOutput { deleted: true };

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
