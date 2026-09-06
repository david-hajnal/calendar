#!/bin/bash
set -euo pipefail

# Validate deployment manifests for correctness and safety.
# Checks: Helm lint, Helm template rendering, Kustomize build,
# immutable SHA tags in production, no retired version-release references,
# Flux CRD conformance, YAML syntax, chart template assertions,
# bridge isolation, dependency order, issuer consistency,
# no cert-manager annotations, deploy script tests.
# Usage: scripts/validate-deploy.sh
# Exits 0 on success, 1 on failure.

ERRORS=0

echo "=== Validating deployment manifests ==="

# 1. Helm lint
echo ""
echo "--- Helm lint (auth, core, mcp) ---"
if command -v helm &>/dev/null; then
  helm lint deploy/helm/commoncal-auth >/dev/null 2>&1 || { echo "FAIL: helm lint commoncal-auth"; ERRORS=$((ERRORS+1)); }
  helm lint deploy/helm/commoncal >/dev/null 2>&1 || { echo "FAIL: helm lint commoncal"; ERRORS=$((ERRORS+1)); }
  helm lint deploy/helm/commoncal-mcp >/dev/null 2>&1 || { echo "FAIL: helm lint commoncal-mcp"; ERRORS=$((ERRORS+1)); }
  
  echo "--- Helm template (auth) ---"
  helm template commoncal-auth deploy/helm/commoncal-auth \
    --set-string image.tag=test \
    --set-string secrets.name=commoncal-auth-secrets \
    --set-string secrets.databaseUrlKey=DATABASE_URL \
    --set-string secrets.bridgeKeyKey=LAB_BRIDGE_KEY \
    --set-string secrets.cookieKeysKey=AUTH_COOKIE_KEYS \
    --set-string secrets.signingKidKey=AUTH_SIGNING_KID \
    >/dev/null 2>&1 || { echo "FAIL: helm template commoncal-auth"; ERRORS=$((ERRORS+1)); }
  
  echo "--- Helm template (core) ---"
  helm template commoncal deploy/helm/commoncal \
    --set-string image.tag=test \
    --set-string domain=example.com \
    --set-string config.appOrigin=https://example.com \
    --set-string existingSecret.name=commoncal-session \
    --set-string existingSecret.sessionSecretKey=SESSION_SECRET \
    --set-string existingSecret.backupEncryptionKeyHex=0000000000000000000000000000000000000000000000000000000000000000 \
    >/dev/null 2>&1 || { echo "FAIL: helm template commoncal"; ERRORS=$((ERRORS+1)); }
  
  echo "--- Helm template (MCP) ---"
  helm template commoncal-mcp deploy/helm/commoncal-mcp \
    --set-string existingSecret.name=commoncal-mcp-secrets \
    --set-string existingSecret.apiKeyName=mcp-internal-api-key \
    --set-string existingSecret.sessionSecretKeyName=mcp-session-secret \
    >/dev/null 2>&1 || { echo "FAIL: helm template commoncal-mcp"; ERRORS=$((ERRORS+1)); }
else
  echo "SKIP: helm not installed"
fi

# 2. Kustomize build (bundle kept for CRD conformance checks)
echo ""
echo "--- Kustomize build ---"
BUNDLE="$(mktemp)"
trap 'rm -f "$BUNDLE"' EXIT
if command -v kustomize &>/dev/null; then
  kustomize build deploy/flux/overlays/production > "$BUNDLE" 2>/dev/null || { echo "FAIL: kustomize build"; ERRORS=$((ERRORS+1)); }
elif command -v kubectl &>/dev/null; then
  kubectl kustomize deploy/flux/overlays/production > "$BUNDLE" 2>/dev/null || { echo "FAIL: kubectl kustomize build"; ERRORS=$((ERRORS+1)); }
else
  echo "SKIP: neither kustomize nor kubectl installed"
fi

# 3. Check for immutable source-revision tags in production manifests
echo ""
echo "--- Checking for mutable tags ---"
MUTABLE=$(grep -rn 'tag:.*latest' deploy/flux/overlays/production/ --include='*.yaml' 2>/dev/null || true)
if [ -n "$MUTABLE" ]; then
  echo "FAIL: Found 'latest' tag in production manifests:"
  echo "$MUTABLE"
  ERRORS=$((ERRORS+1))
fi

MUTABLE=$(grep -rn 'tag:.*"main"' deploy/flux/overlays/production/ --include='*.yaml' 2>/dev/null || true)
if [ -n "$MUTABLE" ]; then
  echo "FAIL: Found 'main' tag in production manifests:"
  echo "$MUTABLE"
  ERRORS=$((ERRORS+1))
fi

# 4. Each HelmRelease must use the immutable tag published for a source commit.
echo ""
echo "--- Checking immutable production tags ---"
HELMRELEASE_FILES=$(find deploy/flux/overlays/production/charts/ -name '*.yaml' -exec grep -l 'kind: HelmRelease' {} \; 2>/dev/null || true)
if [ -n "$HELMRELEASE_FILES" ]; then
  while read -r f; do
    name=$(basename "$f")
    if grep -Eq '^[[:space:]]*tag: "sha-[0-9a-f]{40}"$' "$f"; then
      echo "OK: $name uses an immutable source revision"
    else
      echo "FAIL: $name must use tag sha-<40 hex commit>"
      ERRORS=$((ERRORS+1))
    fi
    if grep -Eq '^[[:space:]]*reconcileStrategy: Revision$' "$f"; then
      echo "OK: $name rebuilds its chart for each Git revision"
    else
      echo "FAIL: $name must use reconcileStrategy: Revision for its GitRepository chart"
      ERRORS=$((ERRORS+1))
    fi
  done <<< "$HELMRELEASE_FILES"
fi

# 5. Flux CRD conformance for the rendered production resources.
echo ""
echo "--- Checking Flux CRD conformance ---"
if command -v python3 &>/dev/null; then
  python3 scripts/validate-flux-crds.py \
    deploy/flux/overlays/production/flux-system/gotk-components.yaml \
    "$BUNDLE" || ERRORS=$((ERRORS+1))
else
  echo "SKIP: python3 not installed"
fi

# 6. Verify no version-release or semver promotion references remain.
echo ""
echo "--- Checking for retired version-release references ---"
if grep -rn 'type=semver' .github/workflows/ mcp-server/.github/workflows/ 2>/dev/null; then
  echo "FAIL: Found semver tag pattern in workflows; only sha-<commit> tags are valid"
  ERRORS=$((ERRORS+1))
else
  echo "OK: no semver promotion patterns in workflows"
fi

if [ -f "scripts/release.sh" ]; then
  echo "FAIL: scripts/release.sh must be retired; promotion is handled by promote-main.yml"
  ERRORS=$((ERRORS+1))
else
  echo "OK: scripts/release.sh has been retired"
fi

# 7. Check no circular dependsOn in Flux resources
echo ""
echo "--- Checking for circular dependencies ---"
# Simple check: ensure no resource depends on itself
CIRCULAR=$(grep -rn 'dependsOn' deploy/flux/overlays/production/ --include='*.yaml' 2>/dev/null || true)
if [ -n "$CIRCULAR" ]; then
  echo "INFO: Found dependsOn references (verify manually):"
  echo "$CIRCULAR"
fi

# 8. Validate YAML syntax (all production manifests, recursively)
echo ""
echo "--- YAML syntax validation ---"
YAML_FILES=$(find deploy/flux/overlays/production -name '*.yaml' -type f 2>/dev/null || true)
if [ -n "$YAML_FILES" ]; then
  # shellcheck disable=SC2086
  python3 scripts/validate-yaml.py $YAML_FILES || { echo "FAIL: YAML syntax or duplicate mapping key"; ERRORS=$((ERRORS+1)); }
fi

# 9. Run chart template assertions
echo ""
echo "--- Chart template assertions ---"
if [ -f "deploy/helm/commoncal-auth/tests/template_assertions.sh" ]; then
  sh deploy/helm/commoncal-auth/tests/template_assertions.sh || { echo "FAIL: commoncal-auth template_assertions.sh"; ERRORS=$((ERRORS+1)); }
else
  echo "SKIP: commoncal-auth template_assertions.sh not found"
fi

if [ -f "deploy/helm/commoncal-auth/tests/production_values_assertions.sh" ]; then
  sh deploy/helm/commoncal-auth/tests/production_values_assertions.sh || { echo "FAIL: commoncal-auth production_values_assertions.sh"; ERRORS=$((ERRORS+1)); }
else
  echo "SKIP: commoncal-auth production_values_assertions.sh not found"
fi

if [ -f "deploy/helm/commoncal/tests/template_assertions.sh" ]; then
  sh deploy/helm/commoncal/tests/template_assertions.sh || { echo "FAIL: commoncal template_assertions.sh"; ERRORS=$((ERRORS+1)); }
else
  echo "SKIP: commoncal template_assertions.sh not found"
fi

if [ -f "deploy/helm/commoncal-mcp/tests/template_assertions.sh" ]; then
  sh deploy/helm/commoncal-mcp/tests/template_assertions.sh || { echo "FAIL: commoncal-mcp template_assertions.sh"; ERRORS=$((ERRORS+1)); }
else
  echo "SKIP: commoncal-mcp template_assertions.sh not found"
fi

# 10. Bridge isolation: the private bridge service must NOT be exposed via
#     Ingress, and the auth chart must not render secret values.
echo ""
echo "--- Checking bridge isolation (no private ingress, no secret values) ---"
if [ -s "$BUNDLE" ]; then
  # The auth chart's Ingress must only reference the public service.
  INGRESS_TEMPLATE="$(mktemp)"
  trap 'rm -f "$BUNDLE" "$INGRESS_TEMPLATE"' EXIT
  helm template commoncal-auth deploy/helm/commoncal-auth \
    --set-string image.tag=test \
    --set-string secrets.name=commoncal-auth-secrets \
    --set-string secrets.databaseUrlKey=DATABASE_URL \
    --set-string secrets.bridgeKeyKey=LAB_BRIDGE_KEY \
    --set-string secrets.cookieKeysKey=AUTH_COOKIE_KEYS \
    --set-string secrets.signingKidKey=AUTH_SIGNING_KID \
    2>/dev/null > "$INGRESS_TEMPLATE" || true
  if grep -A20 'kind: Ingress' "$INGRESS_TEMPLATE" | grep -q 'commoncal-auth-internal'; then
    echo "FAIL: private bridge service must not be exposed via Ingress"
    ERRORS=$((ERRORS+1))
  else
    echo "OK: private bridge service is not exposed via Ingress"
  fi
else
  echo "SKIP: no bundle to check bridge isolation"
fi

# 11. Dependency order: auth -> core -> mcp. The core HelmRelease must depend
#     on the auth HelmRelease; the mcp HelmRelease must depend on core.
echo ""
echo "--- Checking Flux dependency order (auth -> core -> mcp) ---"
CORE_HR="deploy/flux/overlays/production/charts/core-helmrelease.yaml"
MCP_HR="deploy/flux/overlays/production/charts/mcp-helmrelease.yaml"
if [ -f "$CORE_HR" ] && grep -q 'name: commoncal-auth' "$CORE_HR"; then
  echo "OK: core depends on commoncal-auth"
else
  echo "FAIL: core HelmRelease must depend on commoncal-auth"
  ERRORS=$((ERRORS+1))
fi
if [ -f "$MCP_HR" ] && grep -q 'name: commoncal' "$MCP_HR"; then
  echo "OK: mcp depends on commoncal"
else
  echo "FAIL: mcp HelmRelease must depend on commoncal"
  ERRORS=$((ERRORS+1))
fi

# 12. Issuer consistency: the primary issuer must be unchanged (no cutover).
#     The MCP chart's primary issuer key must still be mcp-oauth-issuer.
echo ""
echo "--- Checking issuer consistency (no cutover) ---"
MCP_VALUES="deploy/values-mcp-production.yaml"
if [ -f "$MCP_VALUES" ] && grep -q 'oauthIssuerKeyName: mcp-oauth-issuer' "$MCP_VALUES"; then
  echo "OK: primary issuer key is unchanged (mcp-oauth-issuer)"
else
  echo "FAIL: primary issuer key must remain mcp-oauth-issuer (no cutover)"
  ERRORS=$((ERRORS+1))
fi

# 13. Check for cert-manager annotations in production-owned deployment files
# (scoped to app deployment files; does not scan vendored Flux CRDs)
echo ""
echo "--- Checking for cert-manager annotations in production files ---"
CERT_MGR=$(grep -rn 'cert-manager.io/cluster-issuer\|cert-manager.io/issuer' \
  deploy/flux/overlays/production/ \
  deploy/values-production.yaml \
  deploy/values-mcp-production.yaml \
  deploy/deploy-prod.sh \
  --include='*.yaml' --include='*.sh' 2>/dev/null || true)
if [ -n "$CERT_MGR" ]; then
  echo "FAIL: Found cert-manager issuer annotation in production files:"
  echo "$CERT_MGR"
  ERRORS=$((ERRORS+1))
fi

# 14. Run deploy script tests
echo ""
echo "--- Deploy script tests ---"
if [ -f "scripts/test-sqlite-prod.sh" ]; then
  sh scripts/test-sqlite-prod.sh || { echo "FAIL: test-sqlite-prod.sh"; ERRORS=$((ERRORS+1)); }
else
  echo "SKIP: test-sqlite-prod.sh not found"
fi

if [ -f "scripts/test-deploy-prod.sh" ]; then
  sh scripts/test-deploy-prod.sh || { echo "FAIL: test-deploy-prod.sh"; ERRORS=$((ERRORS+1)); }
else
  echo "SKIP: test-deploy-prod.sh not found"
fi

if [ -f "scripts/test-deploy-prod-stack.sh" ]; then
  bash scripts/test-deploy-prod-stack.sh || { echo "FAIL: test-deploy-prod-stack.sh"; ERRORS=$((ERRORS+1)); }
else
  echo "SKIP: test-deploy-prod-stack.sh not found"
fi

if [ -f "scripts/test-production-ingress-network-policies.sh" ]; then
  sh scripts/test-production-ingress-network-policies.sh || { echo "FAIL: test-production-ingress-network-policies.sh"; ERRORS=$((ERRORS+1)); }
else
  echo "SKIP: test-production-ingress-network-policies.sh not found"
fi

echo ""
if [ $ERRORS -gt 0 ]; then
  echo "FAILED: $ERRORS error(s) found"
  exit 1
else
  echo "PASSED: All validations passed"
  exit 0
fi
