# Happening plan migration — 2026-10-09

Runtime: `/Users/kaszperek/repos/plan-orchestrator`. Target project: `/Users/kaszperek/repos/happening` (already registered as `happening`). Canonical plans remain in the target repository so fresh tasks can read them.

## Migrated tasks

| Task | Initial state | Source / outcome |
| --- | --- | --- |
| `ics-import-t1` | ready | Atomically create a native event batch — docs/plans/ics-import/04-task-slices.md#t1 |
| `ics-import-t2` | blocked | Convert an ICS file into a native event batch — docs/plans/ics-import/04-task-slices.md#t2 |
| `ics-import-t3` | blocked | Expose the authenticated, limited import endpoint — docs/plans/ics-import/04-task-slices.md#t3 |
| `ics-import-t4` | ready | Add the typed browser import API — docs/plans/ics-import/04-task-slices.md#t4 |
| `ics-import-t5` | blocked | Deliver the accessible calendar import flow — docs/plans/ics-import/04-task-slices.md#t5 |
| `ics-import-t6` | blocked | Prove the integrated user journey and document it — docs/plans/ics-import/04-task-slices.md#t6 |
| `mcp-connector-slice-9` | blocked | MCP and OAuth resource-server hardening — docs/plans/cross-client-mcp-connector/04-slices.md |
| `mcp-connector-slice-10` | blocked | Production resilience and controlled issuer cutover — docs/plans/cross-client-mcp-connector/04-slices.md |
| `mcp-connector-slice-11` | blocked | Codex compatibility — docs/plans/cross-client-mcp-connector/04-slices.md |
| `mcp-connector-slice-12` | blocked | Claude Code compatibility — docs/plans/cross-client-mcp-connector/04-slices.md |
| `mcp-connector-slice-13` | blocked | OpenCode compatibility — docs/plans/cross-client-mcp-connector/04-slices.md |
| `mcp-connector-slice-14` | blocked | README delivery — docs/plans/cross-client-mcp-connector/04-slices.md |
| `mcp-connector-slice-15` | blocked | Cross-client release proof — docs/plans/cross-client-mcp-connector/04-slices.md |
| `code-quality-hardening-reconcile` | blocked | Reconcile historical findings — docs/plans/code-quality-hardening/plan.md |
| `code-review-improvements-reconcile` | blocked | Reconcile historical findings — docs/plans/code-review-improvements.md |
| `mcp-production-remediation-reconcile` | blocked | Reconcile historical findings — docs/plans/mcp-production-remediation |
| `flux-version-production-proof` | blocked | Verify version-tagged Flux production rollout — docs/plans/flux-version-tag-deployments.md |
| `mcp-production-rollout` | blocked | Complete SQLite authorization production rollout and deployment validation — docs/MCP-PRODUCTION-FIX-PLAN.md |

## Inventory decisions

- ICS import: all six approved packets migrated with exact scope, acceptance, verification, handoff and dependency graph.
- MCP connector: unfinished slices 9–15 retained under manual holds; newer production fix plan and completed task manifests must be reconciled first.
- Both historical code review plans and all remediation proposals are captured by reconciliation entries, rather than assumed to still require implementation.
- Flux version deployment: local implementation complete, live rollout proof outstanding.
- Production MCP fix: phases 0–4 have completed manifests; phase 5 has local SQLite proof, live phases 5–7 still require deployment/client gates. Three pre-existing deleted mcp deployment manifests were not recreated.
- Dark/light theme: duplicate unchecked slices 3–5 are stale; checked versions, frontend/src/theme/themeContext.tsx and themeContext.test.tsx establish existing implementation.
- cert.md: key-length change already exists in backend/src/backup.rs (domain-separated SHA-256 with legacy 32-byte raw path).
- Account management, ad-hoc SQLite console, all-day events, Apple Calendar, calendar click/drag, Flux image automation, hardening observations, MCP server/storage, notifications and rate limiting have completed ledgers and are excluded.
- ad-hoc-production-sqlite-console.md is covered by its completed folder ledger.
- Screenshot is a planning asset, not a separate executable plan.

Manual holds do not automatically unblock. Dependency-only ICS waits opt in to automatic unblocking. No implementation, model call, deployment or publication was performed.

## Import evidence

2026-10-09: validated all 45 Happening manifests with plan-orchestrator TaskFileStore; no schema or dependency-graph errors. Imported 18 new tasks into the configured live database without running agents. Existing task statuses preserved.
