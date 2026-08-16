#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "Usage: $0 <tag>" >&2
  exit 1
fi

TAG="$1"
REGISTRY="ghcr.io/david-hajnal"

if [[ -z "${GHCR_TOKEN:-}" ]]; then
  echo "ERROR: GHCR_TOKEN is not set" >&2
  exit 1
fi

echo "==> Logging in to GHCR..."
echo "$GHCR_TOKEN" | docker login ghcr.io -u david-hajnal --password-stdin

echo "==> Building calendar-core:$TAG..."
docker build -t "$REGISTRY/calendar-core:$TAG" -f Dockerfile .
docker push "$REGISTRY/calendar-core:$TAG"

echo "==> Building calendar-mcp:$TAG..."
docker build -t "$REGISTRY/calendar-mcp:$TAG" -f Dockerfile -f Dockerfile.mcp .
docker push "$REGISTRY/calendar-mcp:$TAG"

echo "==> Done. Published: $TAG"
