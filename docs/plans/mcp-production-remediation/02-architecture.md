# Architecture: MCP Production Remediation

## Fit

Touches 3 layers:
- **Rust MCP server** (`mcp-server/src/config.rs`, `mcp-server/src/oauth/`): config parsing, OAuth protected-resource metadata, bind address.
- **Helm charts + Flux** (`deploy/helm/commoncal-mcp/`, `deploy/flux/overlays/production/charts/`): chart defaults, HelmRelease values, ingress templates.
- **Deploy tooling** (`deploy/deploy-*.sh`, `deploy/.env.example`, `deploy/values-production.yaml`): shell defaults, env vars, production overrides.

No changes to core backend (`deploy/helm/commoncal/`) beyond domain variable passing.

## Endpoints

- `GET /.well-known/oauth-protected-resource` — new public path on MCP ingress (Gate 6)
- `POST /mcp` — existing MCP Streamable HTTP endpoint (Gate 6 ingress routing)
- No new application endpoints

## Data

### New config fields (MCP server)
- `MCP_DOMAIN` — env var, the public MCP hostname (e.g. `mcal.hajnal.space`)
- `MCP_PUBLIC_RESOURCE_URL` — env var, the public MCP resource identifier (e.g. `https://mcal.hajnal.space/mcp`)
- `APP_ENV` — env var, `development` | `production` (Gate 2)

### Changed config fields
- `BIND_ADDRESS` — parsed as `SocketAddr`, default `127.0.0.1:3001` (dev), `0.0.0.0:3001` (prod via Helm)
- `MCP_INTERNAL_API_BASE` — canonical env var (replaces `CALENDAR_API_URL`); origin-only URL, no trailing path
- `MCP_OAUTH_ISSUER`, `MCP_SESSION_SECRET`, `MCP_INTERNAL_API_KEY`, `MCP_DATABASE_PATH` — required in production

### Helm values changes
- `values.yaml` defaults: `domain` → `mcp.example.com` (RFC 2606)
- `values.schema.json`: add `existingSecret.name` as required for production
- Flux `HelmRelease` manifests: remove hardcoded `cal.hajnal.space`, use `MCP_DOMAIN` / `CORE_DOMAIN` substitution
- New ingress paths in MCP chart: `/.well-known/oauth-protected-resource` (Exact) + `/mcp` (Prefix)

### Kubernetes Secret
- MCP workload: `secretKeyRef` for `MCP_INTERNAL_API_KEY`, `MCP_SESSION_SECRET` (Gate 7)
- Existing `commoncal-session` Secret reused by core

## Flow

### Deployment (Gate 1 + Gate 4)
```
deploy/.env          → DOMAIN, MCP_DOMAIN, IMAGE_TAG (operator-managed)
deploy/deploy-prod.sh    → --set-string domain="$DOMAIN" (core)
deploy/deploy-mcp-prod.sh → --set-string domain="$MCP_DOMAIN" (MCP)
deploy/flux/.../mcp-helmrelease.yaml → domain from HelmRelease values
deploy/flux/.../core-helmrelease.yaml → domain from HelmRelease values
```

### Runtime (Gate 2 + Gate 4 + Gate 5)
```
Env vars → config.rs parsing → validation (fail if missing/placeholder in production)
  → MCP server binds on BIND_ADDRESS
  → OAuth handler uses MCP_PUBLIC_RESOURCE_URL for metadata + token validation
  → InternalClient uses MCP_INTERNAL_API_BASE for backend calls
```

### Ingress routing (Gate 6)
```
mcal.hajnal.space/.well-known/oauth-protected-resource → MCP service (Exact path)
mcal.hajnal.space/mcp → MCP service (Prefix path)
cal.hajnal.space/ → core service (unchanged)
```

## External

- **Env var names** (not values): `MCP_DOMAIN`, `MCP_PUBLIC_RESOURCE_URL`, `APP_ENV`, `MCP_INTERNAL_API_BASE`, `MCP_OAUTH_ISSUER`, `MCP_SESSION_SECRET`, `MCP_INTERNAL_API_KEY`, `MCP_DATABASE_PATH`, `BIND_ADDRESS`
- **cert-manager** cluster issuer `letsencrypt-prod` (unchanged)
- **Traefik** ingress class (unchanged)
- **Cloudflare/WAF** — must permit MCP POST + OAuth discovery headers (DPoP)
