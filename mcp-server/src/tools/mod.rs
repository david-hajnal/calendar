// Tools module entry point.
//
// Dispatches MCP tool calls to the appropriate handler.
// Each tool handler performs its own authorization checks.

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
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::error::ToolError;
use crate::internal_client::InternalClient;
use crate::oauth::TokenValidationResult;

/// Dispatch a tool call to the appropriate handler.
///
/// All nine MCP tools are wired here. Each handler performs its own
/// grant-based scope and calendar enforcement.
pub async fn dispatch(
    token: &TokenValidationResult,
    db_pool: &SqlitePool,
    internal_client: &InternalClient,
    tool_name: &str,
    params: serde_json::Value,
) -> Result<Response<axum::body::Body>, ToolError> {
    match tool_name {
        // Read tools
        "availability_find" => {
            let params: availability_find::AvailabilityFindParams =
                serde_json::from_value(params).map_err(|e| ToolError::BadRequest(format!("invalid params: {e}")))?;
            availability_find::handle(token, db_pool, internal_client, params).await
        }
        "calendar_list" => {
            let params: calendar_list::CalendarListParams =
                serde_json::from_value(params).map_err(|e| ToolError::BadRequest(format!("invalid params: {e}")))?;
            calendar_list::handle(token, db_pool, internal_client, params).await
        }
        "event_get" => {
            let params: event_get::EventGetParams =
                serde_json::from_value(params).map_err(|e| ToolError::BadRequest(format!("invalid params: {e}")))?;
            event_get::handle(token, db_pool, internal_client, params).await
        }
        "event_search" => {
            let params: event_search::EventSearchParams =
                serde_json::from_value(params).map_err(|e| ToolError::BadRequest(format!("invalid params: {e}")))?;
            event_search::handle(token, db_pool, internal_client, params).await
        }
        // Mutation tools
        "event_create" => {
            let params: event_create::EventCreateParams =
                serde_json::from_value(params).map_err(|e| ToolError::BadRequest(format!("invalid params: {e}")))?;
            event_create::handle(token, db_pool, internal_client, params).await
        }
        "event_update" => {
            let params: event_update::EventUpdateParams =
                serde_json::from_value(params).map_err(|e| ToolError::BadRequest(format!("invalid params: {e}")))?;
            event_update::handle(token, db_pool, internal_client, params).await
        }
        "reminder_set" => {
            let params: reminder_set::ReminderSetParams =
                serde_json::from_value(params).map_err(|e| ToolError::BadRequest(format!("invalid params: {e}")))?;
            reminder_set::handle(token, db_pool, internal_client, params).await
        }
        // Deletion tools (two-phase)
        "event_delete_prepare" => {
            let params: event_delete_prepare::EventDeletePrepareParams =
                serde_json::from_value(params).map_err(|e| ToolError::BadRequest(format!("invalid params: {e}")))?;
            event_delete_prepare::handle(token, db_pool, internal_client, params).await
        }
        "event_delete_commit" => {
            let params: event_delete_commit::EventDeleteCommitParams =
                serde_json::from_value(params).map_err(|e| ToolError::BadRequest(format!("invalid params: {e}")))?;
            event_delete_commit::handle(token, db_pool, internal_client, params).await
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
