# Plan: Make MCP Production Configuration Fail Closed

## Finding

`mcp-server/src/config.rs` silently supplies development values for the OAuth issuer, internal API key, session secret, internal API base, and bind address. The MCP process has no parsed `APP_ENV`, so it cannot reject these defaults in production.

Known fallback credentials are a high-severity production risk because a missing deployment value can become a valid credential instead of preventing startup.

## Desired state

- Production startup fails before opening a listener when any security-critical setting is absent, empty, malformed, or still equal to a development placeholder.
- Development retains explicit, convenient defaults where safe.
- Secret values are redacted from debug output and logs.

## Implementation

1. Add an MCP environment enum parsed from `APP_ENV`, matching the core backend's `development`/`production` behavior.
2. Split configuration construction into parsing and validation so unit tests can cover both without mutating global environment state.
3. In production, require non-empty values for `MCP_OAUTH_ISSUER`, `MCP_INTERNAL_API_BASE`, `MCP_INTERNAL_API_KEY`, `MCP_SESSION_SECRET`, `MCP_DATABASE_PATH`, and `BIND_ADDRESS`.
4. Reject known placeholder values and require HTTPS for externally resolved OAuth endpoints.
5. Parse `BIND_ADDRESS` as `SocketAddr` rather than storing an unchecked string.
6. Implement a redacted `Debug` representation for `Config`; never include key or secret contents in errors.
7. Set `APP_ENV=production` in the MCP chart explicitly.

## Tests

- Unit tests cover every missing, empty, malformed, and placeholder production value.
- A development configuration test documents which defaults remain supported.
- A production startup test asserts failure happens before database connection and listener binding.
- A log-safety test or review assertion confirms secrets cannot appear through `Debug` formatting.

## Deployment

Populate and validate all required production values before shipping the fail-closed image. Deploy configuration first if it is backward-compatible, then deploy the validating binary. Observe restart counts and startup errors during rollout.

## Acceptance criteria

- No production execution path uses a built-in credential or placeholder endpoint.
- Missing production configuration results in a clear non-zero startup failure.
- Tests prevent reintroduction of security-sensitive production defaults.

