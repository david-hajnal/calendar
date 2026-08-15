# Plan: Correct OAuth Protected Resource Metadata

## Finding

The protected-resource handler hard-codes `https://mcp.commoncal.tld/`, independently of the deployed domain and `/mcp` resource URL. This can cause OAuth token audiences to differ from the resource enforced by the MCP server.

## Desired state

One canonical public MCP resource identifier drives metadata, authorization requests, token validation, and documentation.

## Implementation

1. Add required `MCP_PUBLIC_RESOURCE_URL`; configure the current production value as `https://mcal.hajnal.space/mcp` outside reusable source.
2. Require an absolute HTTPS URL in production with no credentials, query, or fragment.
3. Generate protected-resource metadata from this setting.
4. Use the same value as the expected JWT audience/resource and remove conflicting literals.
5. Ensure the advertised authorization server exactly matches `MCP_OAUTH_ISSUER`.
6. Advertise DPoP-bound tokens only when the complete issuer/client flow supports them.

## Verification

- Handler tests assert configuration-derived resource and authorization-server values.
- OAuth tests reject tokens for the core API, old hostname, and wrong path.
- A contract test proves discovery metadata matches runtime token validation.
- External discovery returns the intended resource identifier.

## Acceptance criteria

- No placeholder MCP hostname remains in runtime code.
- Metadata and token validation use the identical resource identifier.
- Tokens issued for another CommonCal service cannot authenticate to MCP.

