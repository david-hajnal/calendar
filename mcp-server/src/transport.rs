//! Host validation for the public Streamable HTTP endpoint.

use rmcp::transport::StreamableHttpServerConfig;

/// Keep rmcp's DNS-rebinding protection enabled for exactly the resource host.
/// rmcp's hostname-only entry also matches explicit ports, including HTTPS :443.
pub fn server_config(public_resource_url: &str) -> Result<StreamableHttpServerConfig, String> {
    let url = url::Url::parse(public_resource_url)
        .map_err(|_| "MCP public resource must be an absolute HTTP(S) URL".to_string())?;
    if !matches!(url.scheme(), "https" | "http") {
        return Err("MCP public resource must use HTTP(S)".to_string());
    }
    let host = url
        .host_str()
        .filter(|host| !host.is_empty() && !host.contains('*'))
        .ok_or_else(|| "MCP public resource must have an exact hostname".to_string())?;
    Ok(StreamableHttpServerConfig::default().with_allowed_hosts([host]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_or_wildcard_resource_fails_closed() {
        for resource in [
            "",
            "/mcp",
            "https://",
            "https://*.hajnal.space/mcp",
            "file:///mcp",
        ] {
            assert!(server_config(resource).is_err());
        }
    }
}
