# Plan: Align MCP Internal API Configuration

## Finding

The deployed MCP chart sets `CALENDAR_API_URL`, but the Rust MCP server reads `MCP_INTERNAL_API_BASE`. The unused variable leaves the server using the placeholder `https://commoncal-core.internal`, so token exchange and tool calls cannot reliably reach the backend.

There is also path ambiguity: `InternalClient` appends `/internal/...`, while the configured value currently ends in `/api`.

## Desired state

- One canonical environment variable names the backend origin.
- The base URL has a documented path contract and is validated at startup.
- MCP and backend deployments share a tested internal service address.

## Implementation

1. Standardize on `MCP_INTERNAL_API_BASE` and remove `CALENDAR_API_URL` from MCP deployment paths.
2. Define the value as an origin without `/api` or a trailing slash, for example `http://commoncal-core:3000`.
3. Parse the value as a URL and reject credentials, query strings, fragments, and unexpected path components.
4. Use URL joining in `InternalClient` instead of string concatenation.
5. Update `deploy/deploy-mcp-prod.sh`, Flux values, Helm defaults, schema, and setup documentation together.
6. Confirm the backend Service name and namespace; use a namespace-qualified DNS name if the workloads are not co-located.

## Tests

- Unit tests cover URL validation and joining for every internal endpoint family.
- Deployment-script tests require `MCP_INTERNAL_API_BASE` and prove the legacy variable is not passed.
- Helm tests assert the canonical variable and reject a base ending in `/api`.
- An integration test performs token exchange and one read-only MCP backend request over the cluster Service.

## Acceptance criteria

- No MCP deployment or documentation refers to `CALENDAR_API_URL`.
- Every generated internal request has exactly one `/internal/...` path prefix.
- Production startup fails when the backend base is absent or malformed.

