#!/usr/bin/env bash
set -euo pipefail

# Usage: ./scripts/release.sh [major|minor|patch]
# Example: ./scripts/release.sh minor  ->  v1.1.0

BUMP="${1:-}"
if [[ -z "$BUMP" ]]; then
  echo "Usage: $0 [major|minor|patch]" >&2
  exit 1
fi

if [[ ! "$BUMP" =~ ^(major|minor|patch)$ ]]; then
  echo "Error: bump must be major, minor, or patch" >&2
  exit 1
fi

# Read current versions
CORE_VERSION=$(grep '^version = ' backend/Cargo.toml | head -1 | cut -d'"' -f2)
MCP_VERSION=$(grep '^version = ' mcp-server/Cargo.toml | head -1 | cut -d'"' -f2)
FRONTEND_VERSION=$(grep '"version":' frontend/package.json | head -1 | sed 's/.*"version": *"\([^"]*\)".*/\1/')
CORE_CHART_VERSION=$(grep '^version:' deploy/helm/commoncal/Chart.yaml | awk '{print $2}')
MCP_CHART_VERSION=$(grep '^version:' deploy/helm/commoncal-mcp/Chart.yaml | awk '{print $2}')

# Validate all versions are the same (monorepo semver)
if [[ "$CORE_VERSION" != "$MCP_VERSION" ]] || [[ "$MCP_VERSION" != "$FRONTEND_VERSION" ]]; then
  echo "Error: versions are out of sync:" >&2
  echo "  backend: $CORE_VERSION" >&2
  echo "  mcp-server: $MCP_VERSION" >&2
  echo "  frontend: $FRONTEND_VERSION" >&2
  exit 1
fi

# Parse current version
CURRENT="$CORE_VERSION"
MAJOR=$(echo "$CURRENT" | cut -d. -f1)
MINOR=$(echo "$CURRENT" | cut -d. -f2)
PATCH=$(echo "$CURRENT" | cut -d. -f3)

case "$BUMP" in
  major) MAJOR=$((MAJOR + 1)); MINOR=0; PATCH=0 ;;
  minor) MINOR=$((MINOR + 1)); PATCH=0 ;;
  patch) PATCH=$((PATCH + 1)) ;;
esac

NEW_VERSION="${MAJOR}.${MINOR}.${PATCH}"
TAG="v${NEW_VERSION}"

# Check if tag already exists
if git tag -l "$TAG" | grep -q "^${TAG}$"; then
  echo "Error: tag $TAG already exists" >&2
  exit 1
fi

echo "Bumping version: $CURRENT -> $NEW_VERSION"
echo "Tag: $TAG"
echo ""

# Bump Cargo.toml versions
sed -i '' "s/^version = \"${CURRENT}\"/version = \"${NEW_VERSION}\"/" backend/Cargo.toml
sed -i '' "s/^version = \"${CURRENT}\"/version = \"${NEW_VERSION}\"/" mcp-server/Cargo.toml

# Sync Cargo.lock with bumped versions (CI and Dockerfiles build with --locked)
cargo update --manifest-path backend/Cargo.toml -w
cargo update --manifest-path mcp-server/Cargo.toml -w

# Bump frontend package version
sed -i '' "s/\"version\": \"${CURRENT}\"/\"version\": \"${NEW_VERSION}\"/" frontend/package.json

# Bump Helm chart versions and appVersions
if [[ "$(uname)" == "Darwin" ]]; then
  sed -i '' "s/^version: ${CORE_CHART_VERSION}/version: ${NEW_VERSION}/" deploy/helm/commoncal/Chart.yaml
  sed -i '' "s/^appVersion: \".*\"/appVersion: \"${TAG}\"/" deploy/helm/commoncal/Chart.yaml
  sed -i '' "s/^version: ${MCP_CHART_VERSION}/version: ${NEW_VERSION}/" deploy/helm/commoncal-mcp/Chart.yaml
  sed -i '' "s/^appVersion: \".*\"/appVersion: \"${TAG}\"/" deploy/helm/commoncal-mcp/Chart.yaml
else
  sed -i "s/^version: ${CORE_CHART_VERSION}/version: ${NEW_VERSION}/" deploy/helm/commoncal/Chart.yaml
  sed -i "s/^appVersion: \".*\"/appVersion: \"${TAG}\"/" deploy/helm/commoncal/Chart.yaml
  sed -i "s/^version: ${MCP_CHART_VERSION}/version: ${NEW_VERSION}/" deploy/helm/commoncal-mcp/Chart.yaml
  sed -i "s/^appVersion: \".*\"/appVersion: \"${TAG}\"/" deploy/helm/commoncal-mcp/Chart.yaml
fi

# Bump HelmRelease image tags (CI checks that production tags are not latest/main)
if [[ "$(uname)" == "Darwin" ]]; then
  sed -i '' "s/tag: \"v[0-9]*\\.[0-9]*\\.[0-9]*\"/tag: \"${TAG}\"/" deploy/flux/overlays/production/charts/core-helmrelease.yaml
  sed -i '' "s/tag: \"v[0-9]*\\.[0-9]*\\.[0-9]*\"/tag: \"${TAG}\"/" deploy/flux/overlays/production/charts/mcp-helmrelease.yaml
else
  sed -i "s/tag: \"v[0-9]*\\.[0-9]*\\.[0-9]*\"/tag: \"${TAG}\"/" deploy/flux/overlays/production/charts/core-helmrelease.yaml
  sed -i "s/tag: \"v[0-9]*\\.[0-9]*\\.[0-9]*\"/tag: \"${TAG}\"/" deploy/flux/overlays/production/charts/mcp-helmrelease.yaml
fi

# Stage and commit
git add backend/Cargo.toml mcp-server/Cargo.toml frontend/package.json deploy/helm/commoncal/Chart.yaml deploy/helm/commoncal-mcp/Chart.yaml deploy/flux/overlays/production/charts/core-helmrelease.yaml deploy/flux/overlays/production/charts/mcp-helmrelease.yaml
git commit -m "chore: bump version to ${NEW_VERSION}"

# Create tag and push
git tag -a "$TAG" -m "Release ${TAG}"
git push origin main
git push origin "$TAG"

echo ""
echo "Done: ${TAG} pushed"
echo "CI will build and push images to ghcr.io/david-hajnal/calendar-core:${TAG} and ghcr.io/david-hajnal/calendar-mcp:${TAG}"
echo "Flux will auto-promote the new version to production"
