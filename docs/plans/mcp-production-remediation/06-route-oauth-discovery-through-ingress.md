# Plan: Route OAuth Discovery Through MCP Ingress

## Finding

The server implements both `/mcp` and `/.well-known/oauth-protected-resource`, but the ingress has historically exposed only `/mcp`. During inspection, the corrected MCP hostname returned `404` for both discovery and a harmless MCP initialize request.

## Desired state

The MCP transport and OAuth metadata reach the same MCP workload over TLS, while health and internal endpoints remain private.

## Implementation

1. Add an Exact ingress path for `/.well-known/oauth-protected-resource` beside the `/mcp` Prefix path.
2. Route both through the dedicated MCP host and Service.
3. Do not expose `/health/*`, `/internal/*`, or database-related routes.
4. Disable path rewriting for both public MCP routes.
5. Return `application/json` metadata with conservative caching behavior.
6. Verify Cloudflare/WAF preserves authorization and DPoP headers and permits MCP POST requests.

## Verification

- Helm tests assert both paths share the MCP Service backend.
- Negative tests prove health and internal paths are absent from ingress.
- Post-deploy discovery returns `200 application/json` with the configured resource.
- A credential-free initialize request reaches MCP and returns a protocol/auth response, never ingress `404` or upstream `502`.

## Acceptance criteria

- OAuth discovery returns valid JSON from the MCP service.
- `/mcp` reaches the MCP handler on the dedicated domain.
- No operational or internal route becomes public.

