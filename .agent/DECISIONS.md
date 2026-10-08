# Decisions

## Apple Calendar account sync

- Implement a real two-way CalDAV account connection; public WebCal and per-event ICS export do not satisfy the product statement.
- Happening is the source of truth. Users authenticate with revocable, per-device Happening connection passwords; Happening never receives Apple credentials.
- Keep DAV routing/authentication separate from ordinary browser/API routes while reusing domain services for mutations and authorization.
- Use stable principal, calendar, resource, and UID mappings; canonical representations produce conflict-checking ETags.
- Record resource changes and tombstones transactionally with domain mutations; expose them using opaque sync tokens.
- Treat recurrence as the highest-risk semantic slice and defer release until current macOS and iOS clients pass the full lifecycle.
- Store orchestrator task manifests in this target project (`.agent/tasks/`), not in the plan-orchestrator source repository.
