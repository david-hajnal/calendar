# Production Diagnosis Guide

Quick reference for diagnosing CommonCal production issues without production cluster access.

## Offline Diagnosis

```bash
# Show configuration state (no cluster access needed)
make prod-doctor

# Render effective production workloads
make prod-render

# Validate production configuration
make prod-check
```

## Canonical Source

All production configuration lives in:
```
deploy/flux/overlays/production/
├── kustomization.yaml          # Bundle: core + auth + MCP HelmReleases
└── charts/
    ├── core-helmrelease.yaml   # Core app (StatefulSet)
    ├── auth-helmrelease.yaml   # Auth server (Deployment, SQLite)
    └── mcp-helmrelease.yaml    # MCP server (Deployment)
```

## Secret Interfaces

| Secret | Keys | Consumer |
|--------|------|----------|
| `commoncal-session` | `SESSION_SECRET`, `BACKUP_ENCRYPTION_KEY_HEX` | Core |
| `commoncal-mail` | `SMTP_USERNAME`, `SMTP_PASSWORD` | Core mail |
| `commoncal-mcp-secrets` | `mcp-internal-api-key`, `mcp-session-secret`, `mcp-oauth-issuer` | MCP + Core→MCP |
| `commoncal-auth-secrets` | `AUTH_BRIDGE_KEY`, `AUTH_COOKIE_KEYS`, `AUTH_SIGNING_KID` | Auth server + Core bridge |
| `commoncal-tls` | `tls.crt`, `tls.key` | All ingresses |

## Key Configuration Values (Non-Secret)

| Value | Source | Value |
|-------|--------|-------|
| Core domain | `core-helmrelease.yaml` | `cal.hajnal.space` |
| MCP domain | `mcp-helmrelease.yaml` | `mcal.hajnal.space` |
| Auth issuer | `auth-helmrelease.yaml` | `https://auth.hajnal.space` |
| MCP issuer (env) | `mcp-helmrelease.yaml` | `https://auth.hajnal.space` |
| Bridge URL | `core-helmrelease.yaml` | `http://commoncal-auth-internal.commoncal.svc:80` |
| Mail host | `core-helmrelease.yaml` | `smtp.zoho.com:587` |
| Mail from | `core-helmrelease.yaml` | `cal.endar@hajnal.space` |

## Deployment Mode

The deploy script auto-detects the mode:
- **Flux mode**: All 3 HelmReleases active → `flux reconcile kustomization flux-system`
- **Direct Helm mode**: All 3 suspended → `deploy/deploy-prod.sh`

## External Unknowns

These require operator action to verify:
- Traefik controller namespace (expected: `traefik`)
- StorageClass name (default)
- Cloudflare DNS records
- Cloudflare TLS mode (Full origin)
- TLS certificate expiry

## Staged Migration Plan

See `docs/plans/production-reproducibility-implementation/00-status.md` for implementation progress.

## Local Reproducibility

```bash
# Start local stack
./scripts/dev.sh start

# Or with Makefile:
make prod-local-up
```

Local stack runs backend + frontend only. Full stack (auth + MCP) requires additional setup.
