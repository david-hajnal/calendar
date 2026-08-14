# Slice Plan: MCP Production Remediation

## Slice 1 — Tracer bullet: Externalize chart defaults + Flux hostname correction
- `deploy/helm/commoncal-mcp/values.yaml`: `domain` default → `mcp.example.com`
- `deploy/flux/overlays/production/charts/mcp-helmrelease.yaml`: `cal.hajnal.space` → `mcal.hajnal.space` (hostname correction)
- `deploy/flux/overlays/production/charts/core-helmrelease.yaml`: leave as `cal.hajnal.space` (core domain, unchanged)
- `deploy/.env.example`: `DOMAIN=calendar.example.com` (example)
- Verify: `helm template` renders `mcp.example.com` in ingress; Flux manifest has `mcal.hajnal.space`

## Slice 2 — APP_ENV enum + config validation (fail-closed)
- `mcp-server/src/config.rs`: Add `AppEnv` enum; split `from_env()` → `parse_env()` + `validate()`; reject placeholders in production
- `mcp-server/src/main.rs`: Add `APP_ENV=production` to deployment env
- `mcp-server/src/config.rs`: `ConfigError` → `Vec<ConfigError>` on validation failure
- Tests: production missing vars → startup failure; dev mode → accepts defaults

## Slice 3 — Fix OAuth resource URL
- `mcp-server/src/config.rs`: Add `public_resource_url` field to `Config`; add `mcp_domain` field
- `mcp-server/src/config.rs`: `OauthProtectedResourceMetadata::new()` already accepts `resource_url` param — no change needed there
- `mcp-server/src/main.rs`: Pass `config.public_resource_url` to metadata handler (replace `"https://mcp.commoncal.tld/"`)
- `mcp-server/src/oauth.rs`: Fix `extract_audience()` → return configured resource URL (remove `"commoncal-mcp"` hardcoded)
- `mcp-server/src/gateway.rs`: Pass `config.public_resource_url` as `resource` to `validate_access_token()`
- Tests: token for wrong audience rejected; metadata returns configured URL

## Slice 4 — Align internal API config
- `mcp-server/src/internal_client.rs`: `api_base: String` → `url::Url`; validate no credentials/query/fragment in `new()`
- `mcp-server/src/internal_client.rs`: Fix `create_reminder()` URL from `/api/v1/` → `/internal/`
- `mcp-server/src/config.rs`: Add `MCP_INTERNAL_API_BASE` validation in production (required, valid URL)
- `deploy/deploy-mcp-prod.sh`: Pass `MCP_INTERNAL_API_BASE` explicitly
- Tests: invalid URL rejected; `create_reminder()` uses `/internal/` path

## Slice 5 — Fix bind address + deploy scripts
- `mcp-server/src/config.rs`: `bind_address: String` → `SocketAddr`; parse in `validate()`
- `deploy/helm/commoncal-mcp/values.yaml`: Add `bindAddress: "0.0.0.0:3001"` default
- `deploy/deploy-mcp-prod.sh`: `DOMAIN` → `MCP_DOMAIN`; add `MCP_DOMAIN` to required vars; fail if absent
- `deploy/deploy-prod.sh`: Add `CORE_DOMAIN` alias
- `deploy/.env.example`: Add `MCP_DOMAIN=mcal.hajnal.space`
- Tests: malformed bind address rejected; deploy fails without `MCP_DOMAIN`

## Slice 6 — OAuth discovery ingress + K8s secret hygiene
- `deploy/helm/commoncal-mcp/templates/ingress.yaml`: Add `/.well-known/oauth-protected-resource` (Exact) path
- `deploy/helm/commoncal-mcp/values.schema.json`: Add `existingSecret.name` as required
- `mcp-server/k8s/secret.yaml` → `mcp-server/k8s/secret.yaml.example`: `CHANGE_ME` placeholders
- `deploy/helm/commoncal-mcp/values.yaml`: Add `existingSecret.name: ""` placeholder
- Tests: helm assertions verify ingress paths; schema requires secret name

## Slice 7 — Verification + cleanup
- `rg` confirms no `hajnal.space` in tracked reusable config/docs
- `helm template` with `MCP_DOMAIN=mcal.hajnal.space` renders correct ingress + TLS
- Deploy script tests prove missing domain → fail before Helm
- Final: update `00-status.md` slice checklist
