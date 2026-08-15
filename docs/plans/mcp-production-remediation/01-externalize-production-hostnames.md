# Plan: Externalize Production Hostnames

## Finding

Production hostnames are committed in Helm defaults, Flux resources, deployment scripts, and documentation. The MCP release has also historically pointed at the core calendar domain instead of the intended `mcal.hajnal.space` endpoint.

This is configuration disclosure and environment coupling, not a credential leak. Public service names remain discoverable through DNS and certificate-transparency logs, so security must not depend on hostname secrecy.

## Desired state

- Reusable charts contain reserved example domains only.
- Production receives `CORE_DOMAIN` and `MCP_DOMAIN` through environment-specific Flux configuration.
- Core and MCP domains cannot accidentally inherit one another.
- The rendered MCP ingress and certificate consistently use the intended MCP domain.

## Implementation

1. Replace production domains in chart defaults and documentation with RFC 2606 examples.
2. Parameterize the applied Flux HelmRelease with distinct `CORE_DOMAIN` and `MCP_DOMAIN` values.
3. Store domains in cluster-local configuration; use a Secret only if hiding operational metadata is an explicit policy requirement.
4. Remove production-domain defaults from deploy scripts and fail clearly when required values are absent.
5. Do not rewrite Git history unless separately approved; hostname removal does not rotate credentials.

## Verification

- `rg` finds no real production hostname in reusable tracked configuration.
- Helm rendering places injected test domains in the correct ingress and TLS entries.
- Deploy-script tests prove missing domain values fail before Helm is invoked.
- Flux reconciliation produces the expected MCP ingress without modifying the core ingress.

## Acceptance criteria

- Production hostnames are not required in the public repository.
- MCP and core domains are independent required settings.
- The production MCP certificate and ingress use the intended MCP domain.

