# Apple Calendar sync REPORT Depth compatibility

## Observed failure

The October 13, 2026 production capture showed successful principal and
calendar discovery followed by `REPORT` requests returning HTTP 400 on four
calendars. Each rejected request was a valid `DAV:sync-collection` body with
`DAV:sync-level` set to `1`, requesting `getetag` and `getcontenttype`.
The response body was `Depth: only 0 and 1 are supported`.

The packet output did not include the exact Depth header value. The server
had already parsed the REPORT successfully and rejected its Depth header:
the sync branch accepted only an omitted header or `0`. Regression tests
reproduce the same failure with both `1` and `infinity`.

## Compatibility choice

[RFC 6578 section 3.2](https://www.rfc-editor.org/rfc/rfc6578.html#section-3.2)
specifies Depth `0` for sync-collection. This repair deliberately tolerates
the valid nonzero Depth values `1` and `infinity` for Apple compatibility.
It does not implement recursive synchronization: `sync-level` must still
be `1`, and the handler still selects only the authenticated calendar's
immediate resources. Malformed Depth values remain HTTP 400 errors.
Token verification, permissions, pagination, and other REPORT behavior
retain their existing handling.

The new HTTP regression tests use the captured request's namespace and
property selection, with locally minted tokens rather than production
tokens. For omitted Depth, `0`, `1`, and `infinity`, they verify initial
snapshot, collection isolation, incremental ETag/token advancement, and
an empty subsequent sync. Negative cases cover malformed Depth and
unsupported sync-level `2` with nonzero Depth.

## Verification commands

```sh
cargo test --manifest-path backend/Cargo.toml --locked --offline --test caldav_event_resource apple_sync_collection
cargo test --manifest-path backend/Cargo.toml --locked --offline
cargo fmt --manifest-path backend/Cargo.toml --check
cargo clippy --manifest-path backend/Cargo.toml --all-targets --locked --offline -- -D warnings
```

## Production acceptance

Local tests do not establish successful production sync. After deploying
the repaired core image, refresh the existing Apple Calendar account and
confirm sync REPORT requests return `207`. An expired or invalid sync token
can instead return `403 DAV:valid-sync-token`, requiring a fresh snapshot.
The separately observed `MKCALENDAR` HTTP 405 concerns calendar creation
and is outside this synchronization repair.

Remove a temporary access-log override after verification:

```sh
kubectl -n commoncal set env statefulset/commoncal ACCESS_LOG_LEVEL-
```

## Hunt outcome

Scope: REPORT Depth handling and adjacent sync behavior. Severity floor:
Critical + High. Constraints: no breaking changes, deployments, pushes,
or dependency installations. No refactor finisher.

Local finding `LBH-CALDAV-1` (High) was reproduced and fixed. Two subsequent
independent focused hunts found no additional Critical/High defects; this
is scoped convergence, not a proof of the entire CalDAV implementation.
All six new regressions pass. Test quality review found no required fixes.
One Low recommendation is deferred: also assert the Depth-specific error
text in the malformed-Depth negative test, which currently checks HTTP 400.

Before merging, scrutinize the intentional RFC Depth compatibility
exception and verify a real Apple refresh after deploying. The exact
nonzero Depth value remains absent from the supplied capture.
