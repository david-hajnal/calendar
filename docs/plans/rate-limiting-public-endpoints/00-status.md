# Status: rate-limiting-public-endpoints

- Gate 1 — Product: APPROVED
- Gate 2 — Architecture: APPROVED (already implemented)
- Gate 3 — Program Design: APPROVED (already implemented)
- Gate 4 — Slice plan: APPROVED (already implemented)

## Slices
- [x] Slice 1 — tracer bullet: wired rate limiter on public router with mock
- [x] Slice 2 — real limiter, happy path through public endpoint
- [x] Slice 3 — IP extraction (x-forwarded-for, ConnectInfo, unknown fallback)
- [x] Slice 4 — tests (unit + integration)
- [x] Slice 5 — development env skip, retry-after header polish

## Notes for a fresh session
- Plan source: docs/plan/03-rate-limiting-public-endpoints-impl.md
- 100 req/60s per IP, all public endpoints share one limit
- Only active when APP_ENV != "development"
