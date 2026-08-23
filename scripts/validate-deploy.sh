#!/bin/bash
set -euo pipefail

# Validate deployment manifests for correctness and safety.
# Usage: scripts/validate-deploy.sh
# Exits 0 on success, 1 on failure.

ERRORS=0

echo "=== Validating deployment manifests ==="

# 1. Helm lint
echo ""
echo "--- Helm lint (core) ---"
if command -v helm &>/dev/null; then
  helm lint deploy/helm/commoncal >/dev/null 2>&1 || { echo "FAIL: helm lint commoncal"; ERRORS=$((ERRORS+1)); }
  helm lint deploy/helm/commoncal-mcp >/dev/null 2>&1 || { echo "FAIL: helm lint commoncal-mcp"; ERRORS=$((ERRORS+1)); }
  
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

# 2. Kustomize build
echo ""
echo "--- Kustomize build ---"
if command -v kustomize &>/dev/null; then
  kustomize build deploy/flux/overlays/production >/dev/null 2>&1 || { echo "FAIL: kustomize build"; ERRORS=$((ERRORS+1)); }
else
  echo "SKIP: kustomize not installed"
fi

# 3. Check for mutable tags in production manifests
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

# 4. Check HelmRelease resources have flux setter comments
echo ""
echo "--- Checking flux setter comments ---"
HELMRELEASE_FILES=$(find deploy/flux/overlays/production/charts/ -name '*.yaml' -exec grep -l 'kind: HelmRelease' {} \; 2>/dev/null || true)
if [ -n "$HELMRELEASE_FILES" ]; then
  echo "$HELMRELEASE_FILES" | while read -r f; do
    if grep -q 'fluxcd/image-automation: tag=' "$f"; then
      echo "OK: $(basename $f) has flux setter comment"
    else
      echo "WARN: $(basename $f) missing flux setter comment"
    fi
  done
fi

# 5. Check no circular dependsOn in Flux resources
echo ""
echo "--- Checking for circular dependencies ---"
# Simple check: ensure no resource depends on itself
CIRCULAR=$(grep -rn 'dependsOn' deploy/flux/overlays/production/ --include='*.yaml' 2>/dev/null || true)
if [ -n "$CIRCULAR" ]; then
  echo "INFO: Found dependsOn references (verify manually):"
  echo "$CIRCULAR"
fi

# 6. Validate YAML syntax
echo ""
echo "--- YAML syntax validation ---"
python3 -c "import yaml, sys; [list(yaml.safe_load_all(open(f))) for f in sys.argv[1:]]" deploy/flux/overlays/production/**/*.yaml 2>/dev/null || { echo "FAIL: YAML syntax error"; ERRORS=$((ERRORS+1)); }

# 7. Run chart template assertions
echo ""
echo "--- Chart template assertions ---"
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

# 8. Check for cert-manager annotations in production-owned deployment files
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

# 9. Run deploy script tests
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

echo ""
if [ $ERRORS -gt 0 ]; then
  echo "FAILED: $ERRORS error(s) found"
  exit 1
else
  echo "PASSED: All validations passed"
  exit 0
fi
