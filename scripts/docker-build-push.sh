#!/usr/bin/env bash
set -euo pipefail

# docker-build-push.sh — Build Docker image and push to GHCR.
#
# Env vars:
#   IMAGE_TAG       - Git describe tag (default: git describe --tags --always --dirty)
#   DOCKER_REGISTRY - Registry URL (default: ghcr.io/david-hajnal)
#   IMAGE_NAME      - Image name (default: calendar-core)
#   DOCKERFILE      - Dockerfile to build (default: Dockerfile)
#   LOCAL_TAG       - Local dev tag (default: commoncal:local)
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
  DOCKERFILE      Dockerfile to build (default: Dockerfile)
  LOCAL_TAG       Local dev tag (default: commoncal:local)
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
DOCKERFILE="${DOCKERFILE:-Dockerfile}"

# Derive IMAGE_TAG from git if not provided
if [[ -z "${IMAGE_TAG:-}" ]]; then
  IMAGE_TAG="$(git describe --tags --always --dirty 2>/dev/null || git rev-parse --short HEAD)"
fi

# Full image reference
IMAGE_REF="${DOCKER_REGISTRY}/${IMAGE_NAME}:${IMAGE_TAG}"

# Local dev tag
LOCAL_TAG="${LOCAL_TAG:-commoncal:local}"

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

# Collect all tags to apply
collect_tags() {
  TAGS=("${IMAGE_REF}" "${LOCAL_TAG}")
  local git_tag
  git_tag="$(git describe --tags --exact-match 2>/dev/null || true)"
  if [[ -n "$git_tag" && "${git_tag}" != "${IMAGE_TAG}" ]]; then
    TAGS+=("${DOCKER_REGISTRY}/${IMAGE_NAME}:${git_tag}")
  fi
  TAGS+=("${DOCKER_REGISTRY}/${IMAGE_NAME}:latest")
}

# Build-only: load into local docker (single platform only)
build_local() {
  local platforms="${PLATFORMS:-linux/amd64,linux/arm64}"
  local tag_args=()
  local tag
  if [[ "${platforms}" == *","* ]]; then
    echo "WARNING: multi-platform build with --build-only falls back to linux/amd64" >&2
    echo "         (multi-platform images cannot be loaded into local docker)" >&2
    platforms="linux/amd64"
  fi
  for tag in "${TAGS[@]}"; do
    tag_args+=("-t" "$tag")
  done
  echo "==> Building image ${IMAGE_REF} (platform: ${platforms})"
  docker buildx build \
    --platform "${platforms}" \
    --load \
    "${tag_args[@]}" \
    -f "${DOCKERFILE}" .
}

# Build and push in one step (supports multi-platform)
build_push() {
  local platforms="${PLATFORMS:-linux/amd64,linux/arm64}"
  local tag_args=()
  local tag
  local token
  token="$(resolve_token)"

  echo "==> Logging in to ${DOCKER_REGISTRY}..."
  docker login "${DOCKER_REGISTRY}" -u _token --password-stdin <<< "${token}"

  for tag in "${TAGS[@]}"; do
    case "${tag}" in
      "${DOCKER_REGISTRY}"/*) tag_args+=("-t" "$tag") ;;
      *) echo "==> Skipping non-registry tag: ${tag}" ;;
    esac
  done
  echo "==> Building and pushing ${IMAGE_REF} (platforms: ${platforms})"
  docker buildx build \
    --platform "${platforms}" \
    --push \
    "${tag_args[@]}" \
    -f "${DOCKERFILE}" .

  docker logout "${DOCKER_REGISTRY}" >/dev/null 2>&1 || true
}

# Dry-run mode
if ((DRY_RUN)); then
  collect_tags
  platforms="${PLATFORMS:-linux/amd64,linux/arm64}"
  echo "[dry-run] Would build image (platforms: ${platforms})"
  for tag in "${TAGS[@]}"; do
    echo "[dry-run]   tag: ${tag}"
  done
  if ((!BUILD_ONLY)); then
    echo "[dry-run] Would login to ${DOCKER_REGISTRY}"
    echo "[dry-run] Would push to ${DOCKER_REGISTRY}"
    echo "[dry-run] Would logout from ${DOCKER_REGISTRY}"
  fi
  exit 0
fi

# Execute
collect_tags
if ((BUILD_ONLY)); then
  build_local
else
  build_push
fi

echo "==> Done. Image: ${IMAGE_REF}"
exit 0
