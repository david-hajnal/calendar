#!/bin/bash
# Verify main-branch image publication and immutable GitOps promotion.
set -euo pipefail

echo "=== E2E Verification: main promotion ==="
echo "1. Confirm the promote-main workflow succeeded for the source commit:"
echo "   gh run list --workflow=promote-main.yml --limit 5"
echo "2. Confirm the workflow committed all three immutable tags:"
echo "   git log --grep='chore(deploy): promote' --oneline -5"
echo "   grep -h 'tag:' deploy/flux/overlays/production/charts/*-helmrelease.yaml"
echo "3. Reconcile and verify Flux:"
echo "   flux reconcile kustomization flux-system --namespace=flux-system --with-source"
echo "   flux get helmreleases --namespace=flux-system"
echo "4. Verify every workload uses the promoted sha-* tag:"
echo "   kubectl get pods -n commoncal -o jsonpath='{.items[*].spec.containers[0].image}'"
echo "5. Verify endpoints and persistent volumes:"
echo "   curl -sk https://cal.hajnal.space/health/ready"
echo "   curl -sk https://mcal.hajnal.space/health/live"
echo "   kubectl get pvc -n commoncal"
echo "6. Roll back by reverting the promotion commit, pushing, and reconciling."
