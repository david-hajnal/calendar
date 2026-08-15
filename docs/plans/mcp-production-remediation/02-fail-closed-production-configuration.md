# Plan: Make MCP Production Configuration Fail Closed

## Finding

`mcp-server/src/config.rs` supplies development defaults for the OAuth issuer, internal API base, internal API key, session secret, and bind address. Without production-aware validation, missing deployment configuration can silently activate known placeholder credentials.

## Desired state

- Production startup fails before opening a listener when security-critical configuration is missing, empty, malformed, or a known placeholder.
- Development retains explicit safe defaults.
- Secret values never appear in logs or debug output.

## Implementation status: DONE

Implemented in `mcp-server/src/config.rs` (651 lines). All 7 items below are complete.

### What's implemented

1. **APP_ENV parsing** — `AppEnv::from_env()` (line 18): parses `APP_ENV` into `Development`/`Production` enum. Falls back to `Development`.

2. **Parse/validation separation** — `Config::parse_env()` (line 82) returns `RawConfig`; `Config::validate()` (line 158) returns `Result<Config, Vec<ConfigError>>`. `Config::from_env()` (line 257) wires them together.

3. **Production-required fields** — validation at line 162–222 requires all of:
   - `MCP_OAUTH_ISSUER` (non-empty, HTTPS, no `commoncal.tld`)
   - `MCP_INTERNAL_API_BASE` (non-empty, no `commoncal-core.internal`)
   - `MCP_INTERNAL_API_KEY` (non-empty, not `mcp-internal-dev-key`)
   - `MCP_SESSION_SECRET` (non-empty, no `dev-secret`)
   - `MCP_DATABASE_PATH` (non-empty)
   - `MCP_DOMAIN` (non-empty)
   - `MCP_PUBLIC_RESOURCE_URL` (non-empty)
   - `BIND_ADDRESS` (valid socket address, validated in all envs)

4. **Placeholder rejection** — lines 165–203 reject:
   - `commoncal.tld` in `MCP_OAUTH_ISSUER`
   - `commoncal-core.internal` in `MCP_INTERNAL_API_BASE`
   - `mcp-internal-dev-key` in `MCP_INTERNAL_API_KEY`
   - `dev-secret` in `MCP_SESSION_SECRET`
   - `http://` prefix on `MCP_OAUTH_ISSUER`

5. **Rate limiting** — `MCP_RATE_LIMIT_ENABLED` env var parsed (line 135–136), wired to `RateLimiter` in gateway (line 29–39). Production validation requires `MCP_RATE_LIMIT_ENABLED=1` (line 223–227). HelmRelease overlay sets it explicitly. Default values.yaml has `MCP_RATE_LIMIT_ENABLED: "0"`.

6. **Redacted Debug** — `impl Debug for Config` (line 60–77): `oauth_issuer`, `internal_api_key`, `session_secret` all show `[redacted]`.

7. **Startup failure** — `main.rs` line 40–48: `Config::from_env()` errors cause `eprintln!` + `std::process::exit(1)` **before** any DB connection or listener binding.

### Tests (lines 323–677)

| Test | Covers |
|------|--------|
| `test_parse_env_default_app_env` | Default env = Development |
| `test_parse_env_production` | `APP_ENV=production` |
| `test_validate_production_missing_oauth_issuer` | Empty issuer rejected |
| `test_validate_production_placeholder_oauth_issuer` | `commoncal.tld` rejected |
| `test_validate_production_oauth_issuer_http_rejected` | `http://` rejected |
| `test_validate_production_missing_api_key` | Empty key rejected |
| `test_validate_production_placeholder_api_key` | `mcp-internal-dev-key` rejected |
| `test_validate_production_placeholder_secret` | `dev-secret` rejected |
| `test_validate_production_placeholder_api_base` | `commoncal-core.internal` rejected |
| `test_validate_development_accepts_defaults` | Dev env accepts defaults |
| `test_validate_production_missing_bind_address` | Default bind `0.0.0.0:3001` in prod |
| `test_validate_production_bind_address_malformed` | Invalid socket rejected |
| `test_validate_production_rate_limit_required` | Missing rate limit rejected in prod |
| `test_validate_production_rate_limit_enabled_accepted` | Rate limit = `1` accepted in prod |
| `test_validate_production_missing_mcp_domain` | Empty domain rejected |
| `test_validate_production_missing_public_resource_url` | Empty resource URL rejected |
| `test_validate_production_missing_database_path` | Empty db path rejected |
| `test_redacted_debug_excludes_secrets` | `[redacted]` in Debug output |

## Acceptance criteria

- No production path uses a built-in credential or placeholder endpoint. ✅
- Invalid production configuration causes a clear non-zero startup failure. ✅
- Regression tests prevent security-sensitive defaults from returning. ✅
- Rate limiting required in production. ✅

