#!/bin/bash
# E2E verification script for flux-image-automation workflow.
# Run this after pushing a v* tag to verify the complete chain.
# Usage: scripts/e2e-verify-deploy.sh
set -euo pipefail

echo "=== E2E Verification: Flux Image Automation ==="
echo ""

# Step 1: Verify images appear in GHCR
echo "Step 1: Verify images in GHCR"
echo "  Check: ghcr.io/david-hajnal/calendar-core:<tag>"
echo "  Check: ghcr.io/david-hajnal/calendar-mcp:<tag>"
echo "  Run: gh cr list david-hajnal/calendar-core --limit 5"
echo "  Run: gh cr list david-hajnal/calendar-mcp --limit 5"
echo ""

# Step 2: Verify Flux ImageRepository detected the tag
echo "Step 2: Verify Flux ImageRepository"
echo "  Run: kubectl get imgrepo -n flux-system"
echo "  Expected: LastImage shows the new tag"
echo "  Run: kubectl describe imgrepo image-repository-core -n flux-system | grep LastImage"
echo "  Run: kubectl describe imgrepo image-repository-mcp -n flux-system | grep LastImage"
echo ""

# Step 3: Verify ImagePolicy selected the tag
echo "Step 3: Verify Flux ImagePolicy"
echo "  Run: kubectl get imgpolicy -n flux-system"
echo "  Expected: LatestImage matches the new tag"
echo "  Run: kubectl describe imgpolicy image-policy-core -n flux-system | grep LatestImage"
echo "  Run: kubectl describe imgpolicy image-policy-mcp -n flux-system | grep LatestImage"
echo ""

# Step 4: Verify ImageUpdateAutomation committed the tags (both releases)
echo "Step 4: Verify ImageUpdateAutomation commit"
echo "  Run: kubectl get imageupdateautomation -n flux-system"
echo "  Expected: LastPushCommit shows recent commit"
echo "  Run: kubectl describe imageupdateautomation image-update-automation -n flux-system | grep LastPushCommit"
echo "  Run: git log --oneline -5 | grep 'chore(deploy): update.*image'"
echo "  Expected: commit(s) updating the core and MCP tags (may be 1 or 2 commits)"
echo ""

# Step 5: Verify HelmRelease upgraded
echo "Step 5: Verify HelmRelease upgrade"
echo "  Run: kubectl get hr -n flux-system"
echo "  Expected: RECONCILING -> READY"
echo "  Run: kubectl describe hr commoncal -n flux-system | grep -A5 'Release Metadata'"
echo "  Run: kubectl describe hr commoncal-mcp -n flux-system | grep -A5 'Release Metadata'"
echo ""

# Step 6: Verify pods rolled out
echo "Step 6: Verify pod rollout"
echo "  Run: kubectl get statefulset -n commoncal"
echo "  Run: kubectl get deployment -n commoncal"
echo "  Run: kubectl get pods -n commoncal -l app.kubernetes.io/instance=commoncal"
echo "  Run: kubectl get pods -n commoncal -l app.kubernetes.io/instance=commoncal-mcp"
echo "  Expected: Pods running with new image digest"
echo "  Run: kubectl get pod -n commoncal -o jsonpath='{.items[*].spec.containers[0].image}'"
echo ""

# Step 7: Verify PVCs preserved
echo "Step 7: Verify PVCs preserved"
echo "  Run: kubectl get pvc -n commoncal"
echo "  Expected: PVCs still bound, no new claims created"
echo "  Run: kubectl describe pvc -n commoncal | grep 'Volume:'"
echo ""

# Step 8: Verify ingress and endpoints
echo "Step 8: Verify ingress and endpoints"
echo "  Run: kubectl get ingress -n commoncal"
echo "  Run: curl -sk https://cal.hajnal.space/health/ready"
echo "  Run: curl -sk https://mcal.hajnal.space/health/live"
echo "  Expected: 200 OK responses"
echo ""

# Step 9: Verify Flux reports no issues
echo "Step 9: Verify Flux health"
echo "  Run: flux get sources git -n flux-system"
echo "  Run: flux get kustomizations -n flux-system"
echo "  Run: flux get helmreleases -n flux-system"
echo "  Expected: All Ready, no stalled/failed resources"
echo ""

# Step 10: Test rollback
echo "Step 10: Test rollback"
echo "  Find automation commit:"
echo "    git log --grep='chore(deploy): update.*image' --oneline"
echo "  Revert:"
echo "    git revert <commit-hash>"
echo "    git push"
echo "  Verify workloads return to previous version:"
echo "    kubectl get pods -n commoncal -o jsonpath='{.items[*].spec.containers[0].image}'"
echo ""

echo "=== Verification Complete ==="
echo "If all steps pass, the flux-image-automation workflow is working correctly."
