#!/usr/bin/env bash
# Forward shutdown to PostgreSQL and reload TLS after Kubernetes Secret updates.
set -euo pipefail
/usr/local/bin/docker-entrypoint.sh "$@" &
database_pid=$!
monitor_pid=
shutdown() {
  [[ -z "$monitor_pid" ]] || kill "$monitor_pid" 2>/dev/null || true
  kill -INT "$database_pid" 2>/dev/null || true
  wait "$database_pid" || true
  exit 0
}
trap shutdown INT TERM
if [[ "${1:-}" == postgres ]]; then
  (
    previous=$(sha256sum /etc/postgres-tls/tls.crt /etc/postgres-tls/tls.key 2>/dev/null || true)
    while kill -0 "$database_pid" 2>/dev/null; do
      sleep 30
      current=$(sha256sum /etc/postgres-tls/tls.crt /etc/postgres-tls/tls.key 2>/dev/null || true)
      # The temporary initdb server has a different PID. Signal only the final
      # postmaster, after init scripts complete and its PID file agrees.
      postmaster=$(head -1 "${PGDATA:-/var/lib/postgresql/data}/postmaster.pid" 2>/dev/null || true)
      if [[ -n "$current" && "$current" != "$previous" && "$postmaster" == "$database_pid" ]]; then
        kill -HUP "$database_pid"
        previous=$current
      fi
    done
  ) &
  monitor_pid=$!
fi
set +e
wait "$database_pid"
status=$?
[[ -z "$monitor_pid" ]] || kill "$monitor_pid" 2>/dev/null || true
exit "$status"
