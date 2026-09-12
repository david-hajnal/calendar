// event_delete_prepare tool handler.
//
// Initiates the two-phase deletion flow by creating a delete intent.
// Requires:
// - Valid OAuth token
// - McpGrant with allow_delete=true
// - Calendar ID in grant's allowed_calendar_ids

use axum::http::{Response, StatusCode};
use serde::Deserialize;

use crate::error::ToolError;
use crate::mcp_grant::check_calendar_access;
use crate::output_schema::{ContentBlock, DeletePrepareOutput, EventSummary, ToolOutput};
use crate::tools::AuthorizedToolContext;

/// Deletion intent expiry in seconds (24 hours).
const DELETE_INTENT_EXPIRY: i64 = 86400;

#[derive(Debug, Deserialize)]
pub struct EventDeletePrepareParams {
    pub calendar_id: i64,
    pub event_id: i64,
}

/// Handle the event_delete_prepare tool call.
///
/// Authorization pipeline:
/// 1. Gateway validates the OAuth token and resolves the authoritative grant.
/// 2. Check calendar access against the grant.
/// 3. Check the grant's delete permission.
/// 4. Fetch the event to build the event_summary.
/// 5. Create the delete intent via CommonCal core.
/// 6. Return the intent_id and confirmation URL.
pub async fn handle(
    context: &AuthorizedToolContext<'_>,
    params: EventDeletePrepareParams,
) -> Result<Response<axum::body::Body>, ToolError> {
    let grant = context.grant;

    // Check calendar access against the authoritative grant.
    if !check_calendar_access(grant, params.calendar_id) {
        return Err(ToolError::Forbidden("calendar not in grant".to_string()));
    }

    // Check the grant's delete permission.
    if !crate::mcp_grant::check_tool_permission(grant, "event_delete_prepare") {
        return Err(ToolError::Forbidden(
            "event_delete_prepare requires delete permission".to_string(),
        ));
    }

    // Fetch the event for the event_summary.
    let event_info = context
        .internal_client
        .get_event(params.calendar_id, params.event_id)
        .await
        .map_err(|e| match e {
            crate::internal_client::InternalError::Http(404, _) => ToolError::NotFound,
            _ => ToolError::Internal(format!("event fetch failed: {}", e)),
        })?;

    let event_summary = EventSummary {
        id: event_info.id,
        calendar_id: event_info.calendar_id,
        title: event_info.title.clone(),
        description: None,
        location: None,
        status: event_info.status.clone(),
        event_kind: event_info.event_kind.clone(),
        start_utc: event_info.start_utc.map(|t| t.to_string()),
        end_utc: event_info.end_utc.map(|t| t.to_string()),
        version: event_info.version.unwrap_or(0),
    };

    // Create the delete intent via CommonCal core.
    let intent_payload = serde_json::json!({
        "user_id": context.token.user_id,
        "oauth_client_id": context.token.oauth_client_id,
        "event_id": params.event_id,
        "calendar_id": params.calendar_id,
        "event_version": event_info.version.unwrap_or(0),
        "expires_at": context.token.auth_time + DELETE_INTENT_EXPIRY,
    });

    let delete_intent = context
        .internal_client
        .create_delete_intent(&intent_payload)
        .await
        .map_err(|e| ToolError::Internal(format!("delete intent creation failed: {}", e)))?;

    // Generate the confirmation URL.
    let confirmation_url = format!(
        "{}/confirm-delete/{}",
        context.internal_client.api_base(),
        delete_intent.intent_id
    );

    // Step 7: Build structured response.
    let output = DeletePrepareOutput {
        intent_id: delete_intent.intent_id,
        event_summary,
        expires_at: delete_intent.expires_at,
        confirmation_required: true,
        confirmation_url,
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
    fn delete_intent_expiry_is_24_hours() {
        assert_eq!(DELETE_INTENT_EXPIRY, 86400);
    }

    #[test]
    fn event_delete_prepare_params_deserializes() {
        let json = r#"{"calendar_id": 1, "event_id": 42}"#;
        let params: EventDeletePrepareParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.calendar_id, 1);
        assert_eq!(params.event_id, 42);
    }

    #[test]
    fn delete_prepare_output_serializes() {
        let output = DeletePrepareOutput {
            intent_id: "intent-abc".to_string(),
            event_summary: EventSummary {
                id: 42,
                calendar_id: 1,
                title: Some("Meeting".to_string()),
                description: None,
                location: None,
                status: "confirmed".to_string(),
                event_kind: "default".to_string(),
                start_utc: Some("1704067200".to_string()),
                end_utc: Some("1704070800".to_string()),
                version: 1,
            },
            expires_at: 1704153600,
            confirmation_required: true,
            confirmation_url: "https://commoncal.example.com/confirm-delete/intent-abc".to_string(),
        };
        let json = serde_json::to_string_pretty(&output).unwrap();
        assert!(json.contains("\"intent_id\""));
        assert!(json.contains("intent-abc"));
        assert!(json.contains("\"confirmation_required\""));
        assert!(json.contains("true"));
    }
}
