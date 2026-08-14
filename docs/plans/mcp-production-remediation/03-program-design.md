# Program Design: MCP Production Remediation

## Files

### Rust MCP server (mcp-server/src/)

**config.rs** — Add `APP_ENV` enum; add `mcp_domain`, `public_resource_url` fields to `Config`; split `from_env()` into `parse()` (returns raw `RawConfig`) + `validate()` (returns `Config`); change `bind_address: String` → `bind_address: SocketAddr`; add `redacted_debug()` impl; fix `OauthProtectedResourceMetadata::new()` to accept `resource_url` param.

**oauth.rs** — Fix `extract_audience()` to return configured resource URL instead of `"commoncal-mcp"`; update `validate_access_token()` caller to pass `resource` separately from `issuer`.

**internal_client.rs** — Change `api_base: String` → `api_base: Url` in `InternalClient`; validate URL has no credentials/query/fragment; fix `create_reminder()` URL from `{api_base}/api/v1/...` → `{api_base}/internal/...` for consistency.

**gateway.rs** — Fix `validate_token()` to pass `config.public_resource_url` as `resource` (not `config.oauth_issuer`).

**main.rs** — Update `/.well-known/oauth-protected-resource` route to use `config.public_resource_url` (not hardcoded `"https://mcp.commoncal.tld/"`); add `APP_ENV=production` env var to deployment.

### Helm chart (deploy/helm/commoncal-mcp/)

**values.yaml** — `domain: cal.hajnal.space` → `domain: mcp.example.com`; add `existingSecret.name: ""` placeholder; add `bindAddress: "0.0.0.0:3001"` default; add `mcpDomain: ""` and `publicResourceUrl: ""` fields.

**values.schema.json** — Add `existingSecret.name` as required property for production; add `bindAddress`, `mcpDomain`, `publicResourceUrl` properties.

**templates/ingress.yaml** — Add `/.well-known/oauth-protected-resource` path (Exact) alongside `/mcp` (Prefix).

### Flux manifests (deploy/flux/overlays/production/charts/)

**mcp-helmrelease.yaml** — `domain: cal.hajnal.space` → `domain: mcal.hajnal.space`; add `existingSecret.name` reference; add `bindAddress`; add `mcpDomain: mcal.hajnal.space`; add `publicResourceUrl: https://mcal.hajnal.space/mcp`.

**core-helmrelease.yaml** — `domain` stays `cal.hajnal.space` (unchanged, core domain); add `coreDomain` variable for clarity.

### Deploy tooling

**deploy/deploy-prod.sh** — Keep `DOMAIN` for core; add `CORE_DOMAIN` alias.

**deploy/deploy-mcp-prod.sh** — Rename `DOMAIN` default to `MCP_DOMAIN`; add `MCP_DOMAIN` to required vars; pass `--set-string domain="$MCP_DOMAIN"`, `--set-string mcpDomain="$MCP_DOMAIN"`, `--set-string publicResourceUrl="https://$MCP_DOMAIN/mcp"`.

**deploy/.env.example** — Add `MCP_DOMAIN=mcal.hajnal.space`; change `DOMAIN=cal.hajnal.space` to `DOMAIN=calendar.example.com` (example).

### K8s manifests

**mcp-server/k8s/secret.yaml** — Convert to `.example` suffix with `CHANGE_ME` placeholders; document it is not deployed.

## Types & signatures

```rust
// config.rs — new enum
#[derive(Clone, Debug, PartialEq)]
pub enum AppEnv { Development, Production }

// config.rs — new raw config (pre-validation)
pub struct RawConfig {
    pub app_env: AppEnv,
    pub oauth_issuer: String,
    pub internal_api_base: String,
    pub internal_api_key: String,
    pub session_secret: String,
    pub database_path: PathBuf,
    pub mcp_domain: Option<String>,
    pub public_resource_url: Option<String>,
    pub bind_address: String,
    // ... existing fields ...
}

// config.rs — Config field changes
pub struct Config {
    // existing fields ...
    pub bind_address: std::net::SocketAddr,  // was String
    pub mcp_domain: String,                  // NEW
    pub public_resource_url: String,         // NEW
    pub app_env: AppEnv,                     // NEW
}

// config.rs — new split API
impl Config {
    pub fn parse_env() -> Result<RawConfig, ConfigError>;
    pub fn validate(raw: RawConfig) -> Result<Self, Vec<ConfigError>>;
}

// config.rs — metadata constructor change
impl OauthProtectedResourceMetadata {
    pub fn new(resource_url: &str, auth_issuer: &str) -> Self { ... }
    // existing signature unchanged — caller now passes config.public_resource_url
}

// internal_client.rs — field change
pub struct InternalClient {
    api_base: url::Url,       // was String
    api_key: String,
    http_client: reqwest::Client,
}

impl InternalClient {
    pub fn new(api_base: url::Url, api_key: String) -> Result<Self, ConfigError>;
    // ... existing methods unchanged ...
}
```

## Call stack

### Startup (Gate 2 + Gate 4)
```
main()
  → Config::parse_env()        // reads all env vars, returns RawConfig
  → Config::validate(raw)      // fails fast if production + missing/placeholder
  → connect_and_migrate()      // DB connection
  → Gateway::new(config, db)   // creates InternalClient with validated api_base
  → TcpListener::bind(config.bind_address)  // SocketAddr, not string
  → axum::serve()
```

### OAuth metadata (Gate 4 + Gate 6)
```
GET /.well-known/oauth-protected-resource
  → config.public_resource_url  // NOT hardcoded
  → OauthProtectedResourceMetadata::new(resource_url, oauth_issuer)
  → serialize + return JSON
```

### Token validation (Gate 4 + Gate 5)
```
POST /mcp
  → Gateway::handle_mcp_request()
  → Gateway::validate_token(token)
    → oauth::validate_access_token(token, issuer, resource)
      // resource = config.public_resource_url (NOT config.oauth_issuer)
      → load_jwks(issuer)
      → decode JWT
      → validate issuer, audience, expiry
        // audience = resource (NOT "commoncal-mcp")
      → extract claims
```

### Internal API calls (Gate 4)
```
InternalClient::exchange_token(subject_token, resource)
  → POST {api_base}/internal/token-exchange   // url-joined, not format!
  → InternalClient::get_user_status(user_id)
  → POST {api_base}/internal/mcp/users/{id}/status
  → InternalClient::create_reminder(payload)
  → POST {api_base}/internal/mcp/reminders    // FIXED: was /api/v1/
```

## Test plan

### config.rs tests
- `test_parse_env_default_app_env` — no APP_ENV → `AppEnv::Development`
- `test_parse_env_production` — `APP_ENV=production` → `AppEnv::Production`
- `test_validate_production_missing_oauth_issuer` — fails with error
- `test_validate_production_missing_internal_api_key` — fails with error
- `test_validate_production_placeholder_secret` — `mcp-session-dev-secret-change-in-production` rejected
- `test_validate_production_placeholder_api_base` — `https://commoncal-core.internal` rejected
- `test_validate_development_accepts_defaults` — dev mode accepts all defaults
- `test_validate_production_missing_bind_address` — fails
- `test_validate_production_bind_address_malformed` — `not-a-port` rejected
- `test_validate_production_missing_mcp_domain` — fails
- `test_validate_production_missing_public_resource_url` — fails
- `test_parse_bind_address_valid` — `0.0.0.0:3001` → `SocketAddr`
- `test_redacted_debug_excludes_secrets` — Config Debug output contains no secret values

### oauth.rs tests
- `test_extract_audience_uses_resource_url` — returns configured resource, not `"commoncal-mcp"`
- `test_validate_token_rejects_wrong_audience` — token for `cal.hajnal.space` rejected for MCP

### internal_client.rs tests
- `test_new_client_validates_url_no_credentials` — `http://user:pass@host` rejected
- `test_new_client_validates_url_no_query_string` — `http://host?key=val` rejected
- `test_create_reminder_uses_internal_path` — URL starts with `/internal/`, not `/api/v1/`

### Helm tests (deploy/helm/commoncal-mcp/tests/template_assertions.sh)
- `test_domain_default_is_example` — default domain is `mcp.example.com`
- `test_domain_override_works` — `--set domain=mcal.hajnal.space` renders correctly
- `test_ingress_has_oauth_discovery_path` — Exact path `/.well-known/oauth-protected-resource` present
- `test_ingress_no_health_paths` — `/health/*` not in public ingress
- `test_existing_secret_required_in_schema` — schema requires `existingSecret.name`

### Deploy script tests
- `test_mcp_deploy_requires_mcp_domain` — missing `MCP_DOMAIN` exits non-zero
- `test_core_deploy_requires_core_domain` — missing `CORE_DOMAIN` exits non-zero
- `test_env_example_has_example_domains` — `.env.example` uses `calendar.example.com`

### Integration
- `test_helm_render_mcp_with_domain` — `helm template` produces ingress for `mcal.hajnal.space`
- `test_flux_helmrelease_has_correct_domain` — `mcp-helmrelease.yaml` has `mcal.hajnal.space`

## Least confident decisions

1. **DPoP advertising**: `dpop_bound_access_tokens: true` in metadata — is the authorization flow fully DPoP-compatible for the initial Codex client? Should we default to `false` until confirmed?
2. **Secret storage mechanism**: Flux ConfigMap vs. SOPS/Sealed Secrets vs. external secret controller. The plan says "cluster-local mechanism" but doesn't commit to one. Which does this repo already use for core?
3. **Migration period for old hostname**: `cal.hajnal.space` → `mcal.hajnal.space` for MCP. How long should both be accepted? Is there a DNS/CNAME migration in progress?
4. **`create_reminder()` path**: Currently `{api_base}/api/v1/calendars/{id}/reminders` — is this intentional (different endpoint family) or a bug? The plan assumes it's a bug.
5. **Git history rewrite**: Plan says no rewrite. If hostname removal is a compliance requirement, this decision needs separate operational approval.
