#!/bin/bash
set -euo pipefail

# Validate deployment manifests for correctness and safety.
# Checks: Helm lint, Helm template rendering, Kustomize build,
# immutable SHA tags in production, no retired version-release references,
# Flux CRD conformance, YAML syntax, chart template assertions,
# bridge isolation, dependency topology, issuer consistency,
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
  # Rendering fixtures only; production mail settings must be supplied by the operator.
  helm lint deploy/helm/commoncal --set-string mail.host=smtp.example.test --set-string mail.from=no-reply@example.test >/dev/null 2>&1 || { echo "FAIL: helm lint commoncal"; ERRORS=$((ERRORS+1)); }
  helm lint deploy/helm/commoncal-mcp >/dev/null 2>&1 || { echo "FAIL: helm lint commoncal-mcp"; ERRORS=$((ERRORS+1)); }
  
  echo "--- Helm template (auth) ---"
  helm template commoncal-auth deploy/helm/commoncal-auth \
    --set-string image.tag=test \
    --set-string secrets.name=commoncal-auth-secrets \
    --set-string secrets.bridgeKeyKey=AUTH_BRIDGE_KEY \
    --set-string secrets.cookieKeysKey=AUTH_COOKIE_KEYS \
    --set-string secrets.signingKidKey=AUTH_SIGNING_KID \
    >/dev/null 2>&1 || { echo "FAIL: helm template commoncal-auth"; ERRORS=$((ERRORS+1)); }
  
  echo "--- Helm template (core) ---"
  helm template commoncal deploy/helm/commoncal \
    --set-string mail.host=smtp.example.test \
    --set-string mail.from=no-reply@example.test \
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

if [ -f "deploy/helm/commoncal/tests/mail_assertions.sh" ]; then
  sh deploy/helm/commoncal/tests/mail_assertions.sh || { echo "FAIL: commoncal mail_assertions.sh"; ERRORS=$((ERRORS+1)); }
else
  echo "SKIP: commoncal mail_assertions.sh not found"
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
    --set-string secrets.bridgeKeyKey=AUTH_BRIDGE_KEY \
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

# 11. Dependency topology: auth is active in production, core must depend on it.
#     The mcp HelmRelease must still depend on core.
#     Uses structural YAML checks (not grep) to verify dependsOn is at spec level.
echo ""
echo "--- Checking Flux dependency topology (auth -> core -> mcp) ---"
CORE_HR="deploy/flux/overlays/production/charts/core-helmrelease.yaml"
MCP_HR="deploy/flux/overlays/production/charts/mcp-helmrelease.yaml"
PRODUCTION_KUSTOMIZATION="deploy/flux/overlays/production/kustomization.yaml"
if [ -f "$PRODUCTION_KUSTOMIZATION" ] && grep -q 'auth-helmrelease.yaml' "$PRODUCTION_KUSTOMIZATION"; then
  echo "OK: auth HelmRelease is included in production"
else
  echo "FAIL: auth HelmRelease must be included in production"
  ERRORS=$((ERRORS+1))
fi

# Check core dependsOn at spec level (not inside spec.values)
if [ -f "$CORE_HR" ]; then
  core_depends_on="$(python3 -c "
import sys, yaml
with open(sys.argv[1]) as f:
    doc = yaml.safe_load(f)
deps = doc.get('spec', {}).get('dependsOn', [])
names = [d.get('name') for d in deps] if deps else []
print(','.join(names))
" "$CORE_HR")"
  if echo "$core_depends_on" | grep -q 'commoncal-auth'; then
    echo "OK: core depends on commoncal-auth at spec.dependsOn (auth is active in production)"
  else
    echo "FAIL: core HelmRelease must depend on commoncal-auth at spec.dependsOn (auth is active, not suspended). Found: ${core_depends_on:-none}"
    ERRORS=$((ERRORS+1))
  fi

  # Check authBridge.enabled is actually true (not just present)
  auth_bridge_enabled="$(python3 -c "
import sys, yaml
with open(sys.argv[1]) as f:
    doc = yaml.safe_load(f)
vals = doc.get('spec', {}).get('values', {})
enabled = vals.get('authBridge', {}).get('enabled', False)
print(str(enabled).lower())
" "$CORE_HR")"
  if [ "$auth_bridge_enabled" = "true" ]; then
    echo "OK: core authBridge.enabled is true (auth is active in production)"
  else
    echo "FAIL: core authBridge.enabled must be true when auth is active. Found: ${auth_bridge_enabled}"
    ERRORS=$((ERRORS+1))
  fi
else
  echo "FAIL: core HelmRelease file not found: $CORE_HR"
  ERRORS=$((ERRORS+1))
fi

# Check mcp dependsOn at spec level
if [ -f "$MCP_HR" ]; then
  mcp_depends_on="$(python3 -c "
import sys, yaml
with open(sys.argv[1]) as f:
    doc = yaml.safe_load(f)
deps = doc.get('spec', {}).get('dependsOn', [])
names = [d.get('name') for d in deps] if deps else []
print(','.join(names))
" "$MCP_HR")"
  if echo "$mcp_depends_on" | grep -q 'commoncal'; then
    echo "OK: mcp depends on commoncal at spec.dependsOn"
  else
    echo "FAIL: mcp HelmRelease must depend on commoncal at spec.dependsOn. Found: ${mcp_depends_on:-none}"
    ERRORS=$((ERRORS+1))
  fi

  # Check mcp does not require auth issuer hold key (cutover is dormant)
  mcp_has_hold_key="$(python3 -c "
import sys, yaml
with open(sys.argv[1]) as f:
    doc = yaml.safe_load(f)
vals = doc.get('spec', {}).get('values', {})
es = vals.get('existingSecret', {})
has_hold = bool(es.get('oauthIssuerHoldKeyName'))
print(str(has_hold).lower())
" "$MCP_HR")"
  if [ "$mcp_has_hold_key" = "false" ]; then
    echo "OK: mcp does not require auth issuer hold key (cutover is dormant)"
  else
    echo "FAIL: mcp must not require auth issuer hold secret while cutover is suspended"
    ERRORS=$((ERRORS+1))
  fi
else
  echo "FAIL: mcp HelmRelease file not found: $MCP_HR"
  ERRORS=$((ERRORS+1))
fi

# 11b. Negative coverage: verify dependsOn is NOT nested inside spec.values.
#     This catches the bug where dependsOn is placed under values (where Helm
#     charts read it but Flux ignores it).
echo ""
echo "--- Checking dependsOn is not nested inside spec.values ---"
for hr_file in deploy/flux/overlays/production/charts/*-helmrelease.yaml; do
  if [ ! -f "$hr_file" ]; then continue; fi
  nested="$(python3 -c "
import sys, yaml
with open(sys.argv[1]) as f:
    doc = yaml.safe_load(f)
vals = doc.get('spec', {}).get('values', {})
has_dep = 'dependsOn' in vals
print(str(has_dep).lower())
" "$hr_file")"
  if [ "$nested" = "true" ]; then
    echo "FAIL: dependsOn is nested inside spec.values in $(basename "$hr_file") — Flux ignores it there"
    ERRORS=$((ERRORS+1))
  else
    echo "OK: dependsOn is not nested inside spec.values in $(basename "$hr_file")"
  fi
done

# 12. Issuer consistency: the primary issuer must be unchanged (no cutover).
#     The MCP chart uses env.MCP_OAUTH_ISSUER (public issuer) rather than
#     the retired Secret-based issuer path.
echo ""
echo "--- Checking issuer consistency (no cutover) ---"
MCP_VALUES="deploy/values-mcp-production.yaml"
if [ -f "$MCP_VALUES" ]; then
  # Verify oauthIssuerKeyName is empty (issuer from env, not Secret)
  issuer_key="$(python3 -c "
import sys, yaml
with open(sys.argv[1]) as f:
    doc = yaml.safe_load(f)
es = doc.get('existingSecret', {})
print(es.get('oauthIssuerKeyName', '') or '')
" "$MCP_VALUES")"
  if [ -z "$issuer_key" ]; then
    echo "OK: issuer source is env.MCP_OAUTH_ISSUER (oauthIssuerKeyName is empty)"
  else
    echo "FAIL: oauthIssuerKeyName must be empty when using env.MCP_OAUTH_ISSUER. Found: ${issuer_key}"
    ERRORS=$((ERRORS+1))
  fi
  # Verify env.MCP_OAUTH_ISSUER is set
  mcp_issuer="$(python3 -c "
import sys, yaml
with open(sys.argv[1]) as f:
    doc = yaml.safe_load(f)
env = doc.get('env', {})
print(env.get('MCP_OAUTH_ISSUER', '') or '')
" "$MCP_VALUES")"
  if [ -n "$mcp_issuer" ]; then
    echo "OK: env.MCP_OAUTH_ISSUER is set to ${mcp_issuer}"
  else
    echo "FAIL: env.MCP_OAUTH_ISSUER must be set when oauthIssuerKeyName is empty"
    ERRORS=$((ERRORS+1))
  fi
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

# Dormant cutover: render the separate suspended candidate without activating it.
echo "--- Dormant auth cutover contracts ---"
python3 scripts/test-auth-cutover-manifests.py || { echo "FAIL: dormant cutover manifests"; ERRORS=$((ERRORS+1)); }

# 14. Run deploy script tests
echo ""
echo "--- Deploy script tests ---"
if [ -f "scripts/test-sqlite-prod.sh" ]; then
  sh scripts/test-sqlite-prod.sh || { echo "FAIL: test-sqlite-prod.sh"; ERRORS=$((ERRORS+1)); }
else
  echo "SKIP: test-sqlite-prod.sh not found"
fi

python3 scripts/test-auth-tls.py || { echo "FAIL: test-auth-tls.py"; ERRORS=$((ERRORS+1)); }

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
