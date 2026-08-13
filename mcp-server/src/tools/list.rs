// tools/list handler.
//
// Returns the list of available MCP tools.
// For the tracer bullet, returns an empty list.

pub async fn handle() -> Result<serde_json::Value, crate::error::ToolError> {
    // Tracer bullet: empty tool catalog.
    // Slice 5 will return the real tool list with schemas.
    Ok(serde_json::json!({
        "tools": []
    }))
}
