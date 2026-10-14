# Apple Calendar sync deployment verification — October 14, 2026

## Diagnosis

Local `main` is `8e246f971293616e00606c5dac08661f2ad20810`, the merged PR #4
compatibility fix. Its production HelmRelease files still reference
`sha-aa21eb24f3341b5011b93b5f059121f2e66649d9`, which lacks that fix.
The reported 400 is reproduced by that older source revision; no remaining
sync Depth defect was found in the merged implementation.

Remote state was checked before preparing a promotion. Remote `main` is
`234b9ed5bb3c8c2cf30ddf990d4035f17337b9dc`, a promotion commit for
`4fd530e3252ab0b7c6d778d721782b2a49e5a42d`. GitHub's comparison confirms
that source revision descends from `8e246f9`, with only production image
tag changes in the aggregate diff. The remote core tag already references
`sha-4fd530e3252ab0b7c6d778d721782b2a49e5a42d`.

The existing workflow already prepared and published the coordinated
auth/core/MCP promotion. No duplicate promotion patch or code fix is needed.
Do not replace the newer promotion with an older tag merely to synchronize
this stale local checkout.

Evidence:

- [Fix image workflow](https://github.com/david-hajnal/calendar/actions/runs/38387324947): successful builds, production validation, both registry manifest checks, and promotion.
- [Latest image workflow](https://github.com/david-hajnal/calendar/actions/runs/38576799829): the same checks and promotion succeeded.
- [Existing promotion](https://github.com/david-hajnal/calendar/commit/234b9ed5bb3c8c2cf30ddf990d4035f17337b9dc).

Direct unauthenticated Docker manifest inspection returned registry access
denied. Image publication is supported by the authenticated workflow's
successful checks, not a local pull or runtime test.

## Strict TDD evidence

The user-specified seam is authenticated HTTP REPORT through the real Axum
router. Existing regression tests were used without weakening assertions.

RED: an isolated temporary archive of `aa21eb2`, with the six committed
PR #4 HTTP regressions, was built with a separate Cargo target directory.
Four tests passed; Depth `1` and `infinity` failed with HTTP 400 instead of
207 and `Depth: only 0 and 1 are supported`. A first shared-target run was
discarded because Cargo cache reuse produced an unreliable result.

GREEN: replacing only that archive's `backend/src/caldav/http.rs` with the
already-merged version made all six tests pass. The working checkout's
production code was never modified. Its broader `sync_collection` group
also passed all 12 tests, covering invalid/tampered tokens and bad sync-level.

The four accepted Depth cases each exercise initial snapshots, incremental
updates, changed ETags and sync tokens, empty subsequent sync, and isolation
from another owned calendar. Malformed Depth and sync-level `2` are rejected.
No refactoring is necessary.

Verification commands use cached dependencies only:

```sh
cargo test --manifest-path backend/Cargo.toml --locked --offline --test caldav_event_resource apple_sync_collection
cargo test --manifest-path backend/Cargo.toml --locked --offline --test caldav_event_resource sync_collection
cargo test --manifest-path backend/Cargo.toml --locked --offline
cargo fmt --manifest-path backend/Cargo.toml --check
cargo clippy --manifest-path backend/Cargo.toml --all-targets --locked --offline -- -D warnings
```

The full backend suite initially hit sandbox restrictions on local listeners;
it was rerun with local socket access. Existing unrelated working-tree edits,
including ignored Apple audit tests, were preserved.
The final fresh-source run passed 707 tests with zero failures and four
pre-existing ignored audit tests. The CalDAV resource binary passed 111 tests,
including all six Apple Depth regressions and invalid-token recovery.

Formatting and Clippy with warnings denied passed. Additional deployment
validation passed its manifest checks but failed the existing auth TLS tests
under the host OpenSSL (certificate hostname checks and `-days 0` generation).
This is separate from sync REPORT behavior; no TLS changes were made.

## Remaining production acceptance

The Kubernetes read failed because this environment defaults to an unavailable
`localhost:8080` API. Live image identity, Flux reconciliation, and Apple
Calendar refresh remain unverified. A successful GitOps promotion does not
establish a successful rollout.

With authorized production access, confirm the running core image and image
digest match the promoted revision, confirm Flux and workload readiness,
then refresh the existing Apple Calendar account. Its sync REPORTs should
return 207 for both initial and incremental sync. Invalid/expired tokens may
return 403 with `DAV:valid-sync-token` and require a fresh snapshot.
Confirm unchanged sync is empty and an event edit changes its ETag/token.

MKCALENDAR's 405 remains a separate unsupported calendar-creation feature.
No dependencies were installed, and no push, merge, or deployment was performed.
