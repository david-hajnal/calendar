#!/usr/bin/env bash
set -euo pipefail

# docker-build-push.sh — Build Docker image and push to GHCR.
#
# Env vars:
#   IMAGE_TAG       - Git describe tag (default: git describe --tags --always --dirty)
#   DOCKER_REGISTRY - Registry URL (default: ghcr.io/david-hajnal)
#   IMAGE_NAME      - Image name (default: calendar-core)
#   GHCR_TOKEN      - GHCR auth token (fallback: gh auth token)
#   DRY_RUN         - set to 1 for dry-run mode
#
# Usage:
#   scripts/docker-build-push.sh [OPTIONS]
#   scripts/docker-build-push.sh --help
#   scripts/docker-build-push.sh --dry-run

usage() {
  cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Build Docker image and push to GHCR.

Options:
  --dry-run       Print commands without executing
  --build-only    Build image without pushing
  --help          Show this help message

Env vars:
  IMAGE_TAG       Git tag for the image (default: git describe --tags --always --dirty)
  DOCKER_REGISTRY Registry URL (default: ghcr.io/david-hajnal)
  IMAGE_NAME      Image name (default: calendar-core)
  GHCR_TOKEN      GHCR auth token (fallback: gh auth token)
  DRY_RUN         Set to 1 for dry-run mode

Examples:
  $(basename "$0")                          # build and push
  $(basename "$0") --dry-run                # preview commands
  GHCR_TOKEN=ghp_xxx $(basename "$0")       # with explicit token
  IMAGE_TAG=v1.2.3 $(basename "$0")         # with explicit tag
EOF
}

# Parse args
DRY_RUN=0
BUILD_ONLY=0
for arg in "$@"; do
  case "$arg" in
    --help)
      usage
      exit 0
      ;;
    --dry-run)
      DRY_RUN=1
      ;;
    --build-only)
      BUILD_ONLY=1
      ;;
    *)
      echo "ERROR: unknown argument: $arg" >&2
      usage >&2
      exit 1
      ;;
  esac
done

# Defaults
DOCKER_REGISTRY="${DOCKER_REGISTRY:-ghcr.io/david-hajnal}"
IMAGE_NAME="${IMAGE_NAME:-calendar-core}"

# Derive IMAGE_TAG from git if not provided
if [[ -z "${IMAGE_TAG:-}" ]]; then
  IMAGE_TAG="$(git describe --tags --always --dirty 2>/dev/null || git rev-parse --short HEAD)"
fi

# Full image reference
IMAGE_REF="${DOCKER_REGISTRY}/${IMAGE_NAME}:${IMAGE_TAG}"

# Local dev tag
LOCAL_TAG="commoncal:local"

# Resolve GHCR token
resolve_token() {
  if [[ -n "${GHCR_TOKEN:-}" ]]; then
    echo "${GHCR_TOKEN}"
  elif command -v gh >/dev/null 2>&1; then
    gh auth token 2>/dev/null
  else
    echo "ERROR: GHCR_TOKEN not set and 'gh' CLI not found" >&2
    exit 1
  fi
}

# Build image
build() {
  echo "==> Building image ${IMAGE_REF}"
  docker build -t "${IMAGE_REF}" -t "${LOCAL_TAG}" -f Dockerfile .
}

# Push image to registry
push() {
  local token
  token="$(resolve_token)"

  echo "==> Logging in to ${DOCKER_REGISTRY}..."
  docker login "${DOCKER_REGISTRY}" -u _token --password-stdin <<< "${token}"

  echo "==> Pushing ${IMAGE_REF}..."
  docker push "${IMAGE_REF}"

  # If a git tag exists, also push with that tag name
  local git_tag
  git_tag="$(git describe --tags --exact-match 2>/dev/null || true)"
  if [[ -n "$git_tag" ]]; then
    local tag_with_name="${DOCKER_REGISTRY}/${IMAGE_NAME}:${git_tag}"
    echo "==> Also tagging as ${tag_with_name}"
    docker tag "${IMAGE_REF}" "${tag_with_name}"
    docker push "${tag_with_name}"
  fi

  # Always tag and push latest
  local latest_ref="${DOCKER_REGISTRY}/${IMAGE_NAME}:latest"
  echo "==> Tagging as ${latest_ref}"
  docker tag "${IMAGE_REF}" "${latest_ref}"
  docker push "${latest_ref}"
}

# Cleanup handler
cleanup() {
  docker logout "${DOCKER_REGISTRY}" >/dev/null 2>&1 || true
}
trap cleanup EXIT

# Dry-run mode
if ((DRY_RUN)); then
  echo "[dry-run] Would build image: ${IMAGE_REF}"
  echo "[dry-run] Would build image: ${LOCAL_TAG}"
  echo "[dry-run] Would login to ${DOCKER_REGISTRY}"
  echo "[dry-run] Would push ${IMAGE_REF}"

  git_tag="$(git describe --tags --exact-match 2>/dev/null || true)"
  if [[ -n "$git_tag" ]]; then
    echo "[dry-run] Would also tag as ${DOCKER_REGISTRY}/${IMAGE_NAME}:${git_tag}"
  fi
  echo "[dry-run] Would tag as ${DOCKER_REGISTRY}/${IMAGE_NAME}:latest"
  echo "[dry-run] Would logout from ${DOCKER_REGISTRY}"
  exit 0
fi

# Execute
if ((BUILD_ONLY)); then
  build
else
  build
  push
fi

echo "==> Done. Image: ${IMAGE_REF}"
exit 0
