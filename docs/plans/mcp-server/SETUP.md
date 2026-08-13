# MCP Server Setup Guide

## Prerequisites

- Rust 1.82+
- SQLite 3.35+
- Docker 24+ (for containerized deployment)
- k3s 1.28+ (for Kubernetes deployment)
- CommonCal backend running with OAuth issuer configured

## Local Development

### 1. Clone and build

```bash
cd mcp-server
cargo build
```

### 2. Configure environment

```bash
# Required
export MCP_INTERNAL_API_KEY="<strong-random-key>"
export MCP_OAUTH_ISSUER="https://auth.cal.hajnal.space"
export MCP_OAUTH_RESOURCE="commoncal-mcp"
export MCP_JWKS_URL="https://auth.cal.hajnal.space/.well-known/oauth-jwks"

# Optional (defaults shown)
export MCP_LISTEN_ADDR="0.0.0.0:8080"
export MCP_DB_DIR="./data"
export MCP_RATE_LIMIT_WINDOW=60
export MCP_RATE_LIMIT_MAX_REQUESTS=100
```

### 3. Run

```bash
cargo run
```

Server starts on `0.0.0.0:8080` by default.

### 4. Verify

```bash
curl http://localhost:8080/health
# {"status":"healthy"}
```

## Database Setup

The first run auto-creates the database and runs migrations. Tables created:

| Table | Purpose |
|---|---|
| `mcp_grant` | OAuth client permission grants |
| `delete_intent` | Two-phase delete pending intents |
| `idempotency_key` | Request deduplication |
| `mcp_audit` | Security audit log |

Migration file: `migrations/0001_initial.sql`

For manual migration:

```bash
sqlx migrate run --source migrations
```

## Docker Deployment

### Build

```bash
docker build -t mcp-server:latest .
```

### Run

```bash
docker run -d \
  --name mcp-server \
  -p 8080:8080 \
  -e MCP_INTERNAL_API_KEY="<key>" \
  -e MCP_OAUTH_ISSUER="https://auth.cal.hajnal.space" \
  -e MCP_JWKS_URL="https://auth.cal.hajnal.space/.well-known/oauth-jwks" \
  -v mcp-data:/data \
  mcp-server:latest
```

## Kubernetes (k3s) Deployment

### 1. Apply namespace and resources

```bash
kubectl apply -f k8s/namespace.yaml
kubectl apply -f k8s/pvc.yaml
```

### 2. Create secrets

```bash
kubectl create secret generic mcp-server-secrets \
  --from-literal=MCP_INTERNAL_API_KEY="<key>" \
  --from-literal=MCP_OAUTH_ISSUER="https://auth.cal.hajnal.space" \
  --from-literal=MCP_OAUTH_RESOURCE="commoncal-mcp" \
  --from-literal=MCP_JWKS_URL="https://auth.cal.hajnal.space/.well-known/oauth-jwks" \
  --from-literal=MCP_DB_PATH="/data/mcp.db" \
  --from-literal=MCP_RATE_LIMIT_WINDOW="60" \
  --from-literal=MCP_RATE_LIMIT_MAX_REQUESTS="100" \
  -n mcp-server
```

### 3. Deploy

```bash
kubectl apply -f k8s/deployment.yaml
kubectl apply -f k8s/service.yaml
kubectl apply -f k8s/network-policy.yaml
```

### 4. Verify

```bash
kubectl rollout status deployment/mcp-server -n mcp-server
kubectl logs -l app=mcp-server -n mcp-server --tail=50
```

## Backend Integration

The MCP server communicates with the CommonCal backend via internal API. Ensure these routes are available:

### Internal API (x-mcp-api-key auth)

| Method | Path | Purpose |
|---|---|---|
| POST | `/internal/token-exchange` | RFC 8693 token exchange |
| GET | `/internal/mcp/users/:user_id/status` | User status check |
| GET | `/internal/mcp/users/:user_id/calendars` | List user calendars |
| GET | `/internal/mcp/calendars/:calendar_id/role/:user_id` | Calendar role check |
| GET | `/internal/mcp/calendars/:calendar_id/events/:event_id` | Get event |
| GET | `/internal/mcp/calendars/:calendar_id/events/search` | Search events |
| POST | `/internal/mcp/events/:calendar_id` | Create event |
| PATCH | `/internal/mcp/events/:calendar_id/:event_id` | Update event |
| POST | `/internal/mcp/delete-intents` | Create delete intent |
| GET | `/internal/mcp/delete-intents/:intent_id` | Get delete intent |
| POST | `/internal/mcp/delete-intents/:intent_id/commit` | Commit delete |
| GET | `/internal/mcp/mcp-grants` | List MCP grants |
| GET | `/internal/mcp/idempotency/:operation_id` | Check idempotency |
| POST | `/internal/mcp/idempotency` | Record idempotency |

### Public API (for MCP server to call)

| Method | Path | Purpose |
|---|---|---|
| POST | `/api/v1/calendars/:calendar_id/reminders` | Create reminder |

## Security Configuration

### MCP Internal API Key

Generate a strong key:

```bash
openssl rand -hex 32
```

Set the same value in both backend and MCP server:

```bash
# Backend (env or k8s secret)
MCP_INTERNAL_API_KEY="<key>"

# MCP server (env or k8s secret)
MCP_INTERNAL_API_KEY="<key>"
```

### OAuth Configuration

The MCP server acts as an OAuth resource server. Configure:

1. **JWKS URL** — OAuth issuer's public key endpoint
2. **Issuer** — OAuth issuer URL (for audience validation)
3. **Resource** — MCP server's resource identifier

### DPoP (Sender-Constrained Tokens)

The MCP server validates DPoP proofs:
- `typ` must be `dpop+jwt`
- `jwk` must be present in proof header
- `htm`/`htu` must match request method/URL

### NetworkPolicy

The provided `k8s/network-policy.yaml` restricts:
- **Ingress**: Only from ingress-nginx or traefik namespaces
- **Egress**: DNS (53), OAuth issuer (443), CommonCal backend (8080)
- **Blocked**: Direct internet access, other namespaces

## Troubleshooting

### "JWKS fetch failed"

Verify the JWKS URL is reachable:

```bash
curl -v $MCP_JWKS_URL
```

### "invalid API key"

Ensure `MCP_INTERNAL_API_KEY` matches between MCP server and backend.

### "grant not found"

The MCP server looks up grants from the backend's `mcp_grant` table. Ensure:
1. The grant exists for the user/client combination
2. The grant hasn't expired or been revoked

### Database errors

Check the database directory is writable:

```bash
ls -la /data/
```

For SQLite WAL mode issues:

```bash
sqlite3 /data/mcp.db "PRAGMA journal_mode=WAL;"
```

## Monitoring

### Health checks

```bash
# Liveness
curl http://localhost:8080/health/live

# Readiness
curl http://localhost:8080/health/ready
```

### Audit log

Audit entries are written to the `mcp_audit` table. Query recent entries:

```sql
SELECT timestamp, user_id, tool, auth_result, result_type, latency_ms
FROM mcp_audit
ORDER BY timestamp DESC
LIMIT 50;
```

### Rate limiting

Check rate limiter status via logs. The fixed-window limiter logs when limits are exceeded.

## CI/CD

The provided `.github/workflows/ci-cd.yaml` runs:

1. **Test** — `cargo test --all`
2. **Clippy** — `cargo clippy -- -D warnings`
3. **Build** — Multi-stage Docker build, push to GHCR
4. **Deploy** — kubectl set image + rollout status

Push to `main` triggers full pipeline. PRs run test + clippy only.

## Running Tests

```bash
# All tests
cargo test

# Unit tests only
cargo test --lib

# Integration tests
cargo test --test integration

# E2E tests
cargo test --test e2e
```

Expected: 174 tests passing (151 unit + 11 integration + 12 e2e).
