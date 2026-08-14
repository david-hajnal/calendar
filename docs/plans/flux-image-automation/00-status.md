# Status: Flux Image Automation

- Gate 1 — Product: SKIP (infra-only, no user-facing product change)
- Gate 2 — Architecture: APPROVED 2025-12-11
- Gate 3 — Program Design: APPROVED 2025-12-11
- Gate 4 — Slice plan: APPROVED 2025-12-11

## Slices
- [x] Slice 1 — replace HelmChart with HelmRelease (pinned images)
- [x] Slice 2 — consolidate Flux reconciliation (remove redundant root resources)
- [x] Slice 3 — CI: sortable immutable tags (semver)
- [x] Slice 4 — Flux image automation (ImageRepository + ImagePolicy + ImageUpdateAutomation)
- [x] Slice 5 — loop prevention + deploy validation
- [x] Slice 6 — end-to-end verification

## Notes for a fresh session
- Repo: david-hajnal/calendar, images on GHCR: ghcr.io/david-hajnal/calendar-core, ghcr.io/david-hajnal/calendar-mcp
- Domain: cal.hajnal.space, namespace: commoncal (from root kustomization targetNamespace)
- Current: HelmChart v1beta2 resources (packaging-only), tag=latest / tag=main
- Flux v2.9.4 installed via gotk-sync.yaml
- Helm charts: deploy/helm/commoncal (StatefulSet), deploy/helm/commoncal-mcp (Deployment)
- Two Dockerfiles: Dockerfile (core), Dockerfile.mcp (MCP)
- MCP env: CALENDAR_API_URL: http://commoncal-core:3000/api (note: service name is commoncal-core, not commoncal)
- Core uses SQLite on PVC — data preservation critical
- MCP Deployment has strategy.type: Recreate
