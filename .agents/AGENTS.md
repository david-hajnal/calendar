# Agent Rules

## Docker

Use `scripts/dev.sh` for all local Docker operations. Never run raw `docker compose` commands.

```
./scripts/dev.sh start     # build + up, wait for healthcheck
./scripts/dev.sh stop      # stop containers (keeps volumes)
./scripts/dev.sh rebuild   # stop, rebuild (no cache), start
./scripts/dev.sh logs      # show logs
./scripts/dev.sh logs -f   # follow logs
./scripts/dev.sh status    # container + volume status
./scripts/dev.sh clean     # stop, remove volumes, prune cache
```

### Dev login

In `APP_ENV=development`, use `/dev/login` page to sign in directly (no email link). Enter any email, click "Sign in".

### Ports

- 3000 — backend API
- 5173 — Vite dev server (frontend)
