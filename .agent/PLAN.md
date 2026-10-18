# Current plans

Migration inventory: `.agent/PLAN-MIGRATION.md` (2026-10-09).

## ICS import

All four gates approved 2026-09-27. Six packets in `docs/plans/ics-import/04-task-slices.md`; manifests `ics-import-t1` through `ics-import-t6`. T1 and T4 ready; other packets wait on explicit dependencies. Update the canonical status after verification.

## Held plans

MCP connector slices 9–15, historical review reconciliation, Flux live rollout and SQLite auth production delivery are migrated as manually blocked entries. Read the migration inventory and original plans before releasing any hold. SQLite supersedes PostgreSQL hosting in the production auth plan.

## Completed plans

Apple Calendar T01–T19 and existing mcp-* task manifests are complete and preserved. No completed task is recreated. Preserve pre-existing deleted deployment manifests.

`mcp-production-remediation-reconcile` completed 2026-10-18: all 7 findings classified implemented; reconciliation artifact at `docs/plans/mcp-production-remediation/00-status.md`.

`code-quality-hardening-reconcile` completed 2026-10-18: 17 findings inventoried — 7 implemented (C1–C3, H1–H3, L1), 10 outstanding (H4, H5, H6, M1–M5, L2, L3) as bounded packets in `docs/plans/code-quality-hardening/04-task-slices.md`; reconciliation artifact at `docs/plans/code-quality-hardening/00-status.md`. Manual hold preserved: no fixes applied.
