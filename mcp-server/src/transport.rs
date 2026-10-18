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
    // Enforce same-origin requests: only the resource's own origin may send an
    // `Origin` header. rmcp still accepts requests without an `Origin` header
    // (CLI / non-browser clients), so this blocks cross-origin browser requests
    // without breaking headless clients.
    let origin = url.origin().ascii_serialization();
    Ok(
        StreamableHttpServerConfig::default()
            .with_allowed_hosts([host])
            .with_allowed_origins([origin]),
    )
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

    #[test]
    fn server_config_enforces_same_origin_and_host() {
        let config = server_config("https://mcal.hajnal.space/mcp").unwrap();
        assert_eq!(config.allowed_hosts, vec!["mcal.hajnal.space".to_string()]);
        assert_eq!(
            config.allowed_origins,
            vec!["https://mcal.hajnal.space".to_string()]
        );
    }

    #[test]
    fn server_config_preserves_non_default_port_in_origin() {
        let config = server_config("https://mcal.hajnal.space:8443/mcp").unwrap();
        assert_eq!(config.allowed_hosts, vec!["mcal.hajnal.space".to_string()]);
        assert_eq!(
            config.allowed_origins,
            vec!["https://mcal.hajnal.space:8443".to_string()]
        );
    }
}
