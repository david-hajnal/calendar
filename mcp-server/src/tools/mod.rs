// Tools module entry point.
//
// Dispatches MCP tool calls to the appropriate handler.
// Each tool handler performs its own authorization checks against the
// authoritative grant supplied by the gateway.

pub mod availability_find;
pub mod calendar_list;
pub mod event_create;
pub mod event_delete_commit;
pub mod event_delete_prepare;
pub mod event_get;
pub mod event_search;
pub mod event_update;
pub mod list;
pub mod reminder_set;

use axum::http::Response;

use crate::error::ToolError;
use crate::internal_client::InternalClient;
use crate::mcp_grant::McpGrant;
use crate::oauth::TokenValidationResult;

/// Authorization context supplied to every tool handler.
///
/// The gateway resolves the authoritative grant once per tool call and passes
/// it here. Tools perform calendar and capability checks against the grant and
/// call CommonCal core for the actual calendar operation.
pub struct AuthorizedToolContext<'a> {
    pub token: &'a TokenValidationResult,
    pub grant: &'a McpGrant,
    pub internal_client: &'a InternalClient,
}

/// Dispatch a tool call to the appropriate handler.
///
/// All nine MCP tools are wired here. Each handler performs its own
/// grant-based scope and calendar enforcement against the supplied grant.
pub async fn dispatch(
    context: &AuthorizedToolContext<'_>,
    tool_name: &str,
    params: serde_json::Value,
) -> Result<Response<axum::body::Body>, ToolError> {
    match tool_name {
        // Read tools
        "availability_find" => {
            let params: availability_find::AvailabilityFindParams = serde_json::from_value(params)
                .map_err(|e| ToolError::BadRequest(format!("invalid params: {e}")))?;
            availability_find::handle(context, params).await
        }
        "calendar_list" => {
            let params: calendar_list::CalendarListParams = serde_json::from_value(params)
                .map_err(|e| ToolError::BadRequest(format!("invalid params: {e}")))?;
            calendar_list::handle(context, params).await
        }
        "event_get" => {
            let params: event_get::EventGetParams = serde_json::from_value(params)
                .map_err(|e| ToolError::BadRequest(format!("invalid params: {e}")))?;
            event_get::handle(context, params).await
        }
        "event_search" => {
            let params: event_search::EventSearchParams = serde_json::from_value(params)
                .map_err(|e| ToolError::BadRequest(format!("invalid params: {e}")))?;
            event_search::handle(context, params).await
        }
        // Mutation tools
        "event_create" => {
            let params: event_create::EventCreateParams = serde_json::from_value(params)
                .map_err(|e| ToolError::BadRequest(format!("invalid params: {e}")))?;
            event_create::handle(context, params).await
        }
        "event_update" => {
            let params: event_update::EventUpdateParams = serde_json::from_value(params)
                .map_err(|e| ToolError::BadRequest(format!("invalid params: {e}")))?;
            event_update::handle(context, params).await
        }
        "reminder_set" => {
            let params: reminder_set::ReminderSetParams = serde_json::from_value(params)
                .map_err(|e| ToolError::BadRequest(format!("invalid params: {e}")))?;
            reminder_set::handle(context, params).await
        }
        // Deletion tools (two-phase)
        "event_delete_prepare" => {
            let params: event_delete_prepare::EventDeletePrepareParams =
                serde_json::from_value(params)
                    .map_err(|e| ToolError::BadRequest(format!("invalid params: {e}")))?;
            event_delete_prepare::handle(context, params).await
        }
        "event_delete_commit" => {
            let params: event_delete_commit::EventDeleteCommitParams =
                serde_json::from_value(params)
                    .map_err(|e| ToolError::BadRequest(format!("invalid params: {e}")))?;
            event_delete_commit::handle(context, params).await
        }
        _ => Err(ToolError::BadRequest(format!(
            "Unknown tool: {}",
            tool_name
        ))),
    }
}

/// Return the list of available MCP tool names.
pub fn list_tools() -> &'static [&'static str] {
    &[
        "availability_find",
        "calendar_list",
        "event_get",
        "event_search",
        "event_create",
        "event_update",
        "reminder_set",
        "event_delete_prepare",
        "event_delete_commit",
    ]
}
