// Tools module entry point.
//
// Dispatches MCP tool calls to the appropriate handler.
// Each tool handler performs its own authorization checks.

pub mod availability_find;
pub mod calendar_list;
pub mod event_delete_commit;
pub mod event_delete_prepare;
pub mod event_create;
pub mod reminder_set;
pub mod event_get;
pub mod event_search;
pub mod event_update;
pub mod list;

/// Dispatch a tool call to the appropriate handler.
///
/// For the tracer bullet, only tools/list is supported (handled in gateway.rs).
/// All other tools return "method not found".
pub async fn dispatch(
    tool_name: &str,
) -> Result<serde_json::Value, crate::error::ToolError> {
    match tool_name {
        "calendar_list" => calendar_list::handle_empty().await,
        _ => Err(crate::error::ToolError::BadRequest(format!(
            "Unknown tool: {}",
            tool_name
        ))),
    }
}
