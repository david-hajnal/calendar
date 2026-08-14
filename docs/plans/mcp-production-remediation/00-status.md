# Status: MCP Production Remediation

- Gate 1 — Product: APPROVED 2026-08-14
- Gate 2 — Architecture: APPROVED 2026-08-14
- Gate 3 — Program Design: APPROVED 2026-08-14
- Gate 4 — Slice plan: APPROVED 2026-08-14

## Slices
- [x] Slice 1 — externalize chart defaults + Flux hostname correction
- [x] Slice 2 — APP_ENV enum + config validation (fail-closed)
- [ ] Slice 3 — fix OAuth resource URL
- [ ] Slice 4 — align internal API config
- [x] Slice 5 — fix bind address + deploy scripts
- [x] Slice 6 — OAuth discovery ingress + K8s secret hygiene
- [x] Slice 7 — verification + cleanup

## Notes for a fresh session
- Read all docs in docs/plans/mcp-production-remediation/ before continuing.
- 7 sub-plans exist (01-07); this feature covers all of them.
- Slice 1 done: chart default → mcp.example.com, Flux MCP → mcal.hajnal.space, deploy script MCP_DOMAIN var.
