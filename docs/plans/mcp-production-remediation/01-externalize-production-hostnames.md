# Plan: Externalize Production Hostnames

## Finding

Production hostnames are committed in Helm defaults, Flux resources, deployment scripts, and documentation. The MCP Flux release also points at `cal.hajnal.space`, while the intended MCP endpoint is `mcal.hajnal.space`.

This is primarily configuration disclosure and environment coupling, not a credential leak. Public service names remain discoverable through DNS and certificate-transparency logs, so authentication and authorization must not depend on keeping them secret.

## Evidence

- `deploy/flux/overlays/production/charts/mcp-helmchart.yaml` embeds the MCP domain in `domain`, ingress, and TLS values.
- `deploy/helm/commoncal-mcp/values.yaml` embeds a production hostname as a chart default.
- `deploy/values-production.yaml`, `deploy/deploy-prod.sh`, and `deploy/deploy-mcp-prod.sh` embed the core production hostname.
- `deploy/flux/overlays/production/charts/mcp-helmchart.yaml.env` already demonstrates `${MCP_DOMAIN}` substitution but is not the applied manifest.

## Desired state

- Reusable charts contain non-production example values only.
- Production hostnames enter the deployment through the existing environment-specific Flux substitution or secret/config mechanism.
- Core and MCP domains are separate required settings (`CORE_DOMAIN` and `MCP_DOMAIN`).
- The rendered MCP ingress consistently uses `mcal.hajnal.space` in the current production environment.

## Implementation

1. Replace production domains in chart defaults with RFC 2606 examples such as `calendar.example.com` and `mcp.example.com`.
2. Convert the applied Flux chart manifests to substituted templates, using `MCP_DOMAIN` for the MCP release and `CORE_DOMAIN` for the core release.
3. Store non-secret environment domains in a cluster-local Flux ConfigMap or deployment environment. Use a Secret only if hiding operational metadata is an explicit policy requirement.
4. Remove default production domains from deploy scripts and fail with a clear error when the corresponding domain variable is absent.
5. Update documentation examples to reserved example domains, while documenting required variables.
6. Do not rewrite Git history unless there is a separately approved operational requirement; hostname removal does not rotate or protect credentials.

## Verification

- `rg` finds no `hajnal.space` value in tracked reusable configuration or documentation.
- Helm rendering with explicit core and MCP domains places each hostname in the correct ingress and TLS entries.
- Deployment-script tests prove missing domain values fail before invoking Helm.
- Flux reconciliation produces an MCP ingress for `mcal.hajnal.space` and does not modify the core ingress.

## Rollout and rollback

Apply the MCP hostname correction before removing the old ingress. Keep the old hostname routed only for a short, explicitly bounded migration period if clients may already use it. Roll back by restoring the previous Flux variable, not by restoring hard-coded chart values.

## Acceptance criteria

- No production hostname is required in the public repository.
- MCP and core domains cannot accidentally inherit one another.
- The production MCP certificate and ingress use the intended MCP domain.

