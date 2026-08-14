# Plan: Correct OAuth Protected Resource Metadata

## Finding

The metadata handler hard-codes `https://mcp.commoncal.tld/`, independent of the configured production domain and `/mcp` resource URL. This can make requested token audiences differ from the resource enforced by the MCP server.

## Desired state

The public MCP resource identifier has one canonical configured value and is used consistently for metadata, token validation, OAuth authorization, and documentation.

## Implementation

1. Add required `MCP_PUBLIC_RESOURCE_URL` configuration, set to `https://mcal.hajnal.space/mcp` for the current production environment.
2. Parse and validate it as an absolute HTTPS URL in production, with no credentials, query, or fragment.
3. Generate protected-resource metadata from this setting instead of a literal.
4. Use the same value as the expected JWT audience/resource during OAuth validation; remove fallback audience literals such as `commoncal-mcp` where they conflict.
5. Ensure the advertised authorization server exactly matches the validated `MCP_OAUTH_ISSUER` value.
6. Decide whether DPoP is mandatory for the initial supported Codex client. Advertise `dpop_bound_access_tokens` only when the complete authorization flow issues and validates such tokens compatibly.

## Tests

- Handler tests assert the metadata `resource` and authorization server are configuration-derived.
- OAuth tests reject tokens for the core API, old hostname, or a different path.
- OAuth tests accept only tokens whose audience/resource matches the configured MCP resource.
- A contract test compares discovery metadata with runtime validation configuration.

## Rollout and rollback

Register the final resource identifier with the authorization server before deploying this change. Existing grants or tokens for the old audience may require reauthorization; do not accept both audiences indefinitely.

## Acceptance criteria

- No placeholder MCP hostname remains in runtime source.
- Metadata and token validation use the identical canonical resource identifier.
- Tokens issued for other CommonCal services cannot authenticate to MCP.

