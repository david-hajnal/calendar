# Plan: Align MCP Internal API Configuration

## Finding

The MCP server reads `MCP_INTERNAL_API_BASE`. The client appends `/internal/...`, so a configured base ending in `/api` produces incorrect request paths.

## Desired state

- One canonical environment variable identifies the backend origin.
- Its path contract is documented and validated at startup.
- Every deployment method produces the same internal Service address.

## Implementation

1. Standardize on `MCP_INTERNAL_API_BASE` as the canonical internal API variable.
2. Define the value as an origin without `/api` or a trailing slash, such as `http://commoncal-core:3000`.
3. Parse it as a URL and reject credentials, queries, fragments, and unexpected paths.
4. Use URL joining in `InternalClient` rather than string concatenation.
5. Update Helm, Flux, deploy scripts, schema, and setup documentation together.
6. Use namespace-qualified Kubernetes DNS if backend and MCP run in different namespaces.

## Verification

- Rendered deployments contain `MCP_INTERNAL_API_BASE` with HTTPS origin.
- Unit tests verify exact token-exchange and `/internal/mcp/...` URLs.
- Deploy-script tests require the canonical variable.
- An integration test performs token exchange and one read-only backend request through the Service.

## Acceptance criteria

- All MCP deployment paths and documentation use `MCP_INTERNAL_API_BASE`.
- Generated internal requests contain exactly one `/internal/...` prefix.
- Production fails closed when the backend base is missing or malformed.

