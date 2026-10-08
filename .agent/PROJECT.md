# Happening

Happening is a calendar application with a Rust backend, a web frontend, MCP integration, and a separate CalDAV HTTP surface. Repository-specific commands and conventions in `AGENTS.md` remain authoritative.

## Working rules

- Read the linked plan packet before editing.
- Preserve unrelated worktree changes.
- Route event mutations through existing domain services rather than writing tables directly.
- Add a failing test for the packet behavior, make the smallest vertical change, then run the packet verification plus affected suites.
- Do not store or request Apple Account/iCloud credentials.
- Do not present public ICS/WebCal export as account-level CalDAV synchronization.

## Apple Calendar plan

The approved product, architecture, and program design live in `docs/plans/apple-calendar-support/`. Gate 4 packets are in `04-task-slices.md`.
