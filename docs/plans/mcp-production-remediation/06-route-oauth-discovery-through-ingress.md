# Plan: Route OAuth Discovery Through MCP Ingress

## Finding

The MCP ingress exposes only `/mcp`. The server also implements `/.well-known/oauth-protected-resource`, but that path currently reaches another route or returns `404` depending on the hostname. On `mcal.hajnal.space`, both the discovery path and a harmless MCP initialize request returned `404` during the 2026-08-14 inspection.

## Desired state

Both the Streamable HTTP endpoint and OAuth protected-resource metadata reach the same MCP workload over TLS, without exposing health or internal routes publicly.

## Implementation

1. Add an Exact ingress path for `/.well-known/oauth-protected-resource` alongside the `/mcp` Prefix path.
2. Ensure both paths use the dedicated `mcal.hajnal.space` host and MCP Service backend.
3. Keep `/health/*`, `/internal/*`, and the MCP database inaccessible through public ingress.
4. Check for path rewrite middleware and disable rewriting for these routes.
5. Add appropriate metadata response content type and conservative caching behavior.
6. Verify Cloudflare/WAF permits MCP POST requests and OAuth discovery while preserving request headers required for authorization and DPoP.

## Tests

- Helm rendering tests assert both public paths and their shared Service backend.
- Negative assertions prove health and internal paths are not in the ingress.
- A post-deploy smoke test requires metadata to return `200 application/json` with the expected resource.
- A credential-free MCP initialize request must reach the MCP application and return an MCP/OAuth response, not an ingress `404` or upstream `502`.

## Acceptance criteria

- OAuth discovery returns valid JSON from the MCP service.
- `/mcp` reaches the MCP handler on the dedicated domain.
- No internal or operational endpoint is added to public ingress.

