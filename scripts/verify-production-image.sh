#!/bin/sh
set -eu

image=${1:-commoncal:local}
temporary_directory=$(mktemp -d)
container_id=
export_container_id=

cleanup() {
  [ -z "$container_id" ] || docker rm -f "$container_id" >/dev/null 2>&1 || true
  [ -z "$export_container_id" ] || docker rm -f "$export_container_id" >/dev/null 2>&1 || true
  rm -rf "$temporary_directory"
}
trap cleanup EXIT HUP INT TERM

image_user=$(docker image inspect --format '{{.Config.User}}' "$image")
if [ -z "$image_user" ] || [ "$image_user" = "root" ] || [ "$image_user" = "0" ]; then
  echo "image must declare a non-root USER" >&2
  exit 1
fi

runtime_uid=$(docker run --rm --entrypoint id "$image" -u)
if [ "$runtime_uid" = "0" ]; then
  echo "container ran as root" >&2
  exit 1
fi

if docker run --rm -e APP_ENV=production -e SESSION_SECRET= "$image" 2>&1 \
  | grep -F 'SESSION_SECRET is required in production' >/dev/null; then
  :
else
  echo "missing production configuration did not fail clearly" >&2
  exit 1
fi

mkdir "$temporary_directory/data"
chmod 777 "$temporary_directory/data"
container_id=$(docker run -d --rm --read-only --tmpfs /app/tmp \
  --mount "type=bind,src=$temporary_directory/data,dst=/app/data" \
  -e APP_ENV=production \
  -e SESSION_SECRET=acceptance-test-secret \
  -e APP_ORIGIN=http://127.0.0.1 \
  -p 127.0.0.1::3000 "$image")

port=$(docker port "$container_id" 3000/tcp | sed -n '1s/.*://p')
if [ -z "$port" ]; then
  echo "container did not publish port 3000" >&2
  exit 1
fi

for attempt in $(seq 1 20); do
  if curl --fail --silent --show-error --connect-timeout 1 --max-time 2 \
    "http://127.0.0.1:$port/health/ready" | grep -Fx '{"status":"ok"}' >/dev/null; then
    break
  fi
  if [ "$attempt" = 20 ]; then
    docker logs "$container_id" >&2 || true
    echo "health endpoint did not become ready" >&2
    exit 1
  fi
  sleep 1
done

if ! curl --fail --silent --show-error --connect-timeout 1 --max-time 2 \
  "http://127.0.0.1:$port/" | grep -qi '<!doctype html'; then
  echo "static frontend did not load" >&2
  exit 1
fi

export_container_id=$(docker create "$image")
if docker export "$export_container_id" | tar -tf - | grep -E '(^|/)(\.env(\.|/|$)|Cargo\.toml|Cargo\.lock|package\.json|pnpm-lock\.yaml|yarn\.lock|node_modules|src|tests|migrations)(/|$)' >/dev/null; then
  echo "runtime image contains source or development environment files" >&2
  exit 1
fi

if docker image inspect --format '{{range .Config.Env}}{{println .}}{{end}}' "$image" \
  | grep -Ei '(^|_)(token|secret|password|api_key)=' >/dev/null; then
  echo "runtime image declares a token-like environment variable" >&2
  exit 1
fi

echo "production image acceptance checks passed: $image"
