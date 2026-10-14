# Apple Calendar implementation audit — 2026-10-13

Scope: Apple connection management, CalDAV discovery/authentication, REPORT
sync and multiget, and iCalendar event writes/recurrence. No production access,
deployment, dependency changes, or production-code fixes were performed.

This audit identifies local defects and coverage gaps. It does not establish
which defect explains the remaining production symptom.

## Confirmed defects

### APPLE-AUDIT-1 — Valid explicit DATE-TIME values return HTTP 400 (Medium)

`backend/src/ics.rs:600` rejects every explicit VALUE parameter except DATE.
Consequently, a valid timed VEVENT containing
`DTSTART;VALUE=DATE-TIME:20260115T000000Z` and a matching DTEND fails through
the authenticated CalDAV PUT handler with `400 invalid calendar data`.
The equivalent event without the explicit VALUE parameter succeeds in existing
tests. Creates and updates using this valid syntax cannot sync into Happening.

Reproducer: `audit_apple_put_accepts_explicit_datetime_value` in
`backend/tests/caldav_event_resource.rs` expects 201 and currently gets 400.
Accept DATE-TIME explicitly and validate its value as a timed value; retain
DATE handling and rejection of incompatible value types.

Authority: [RFC 5545 section 3.8.2.4](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.2.4).

### APPLE-AUDIT-2 — Valid autumn DST overlap times return HTTP 400 (Medium)

`backend/src/ics.rs:616` accepts only `LocalResult::Single`. A Budapest event
starting at `20261025T023000` with TZID and VTIMEZONE is rejected because that
local time occurs twice. iCalendar defines the first occurrence as the intended
instant, which is 00:30 UTC for this example. This blocks uploads of events
during the clock overlap even when their timezone is a supported IANA zone.

Reproducer: `audit_apple_put_resolves_dst_overlap_to_first_occurrence` expects
201 and the defined UTC instants; currently it gets 400.
Implement the specified overlap resolution and cover nonexistent spring times
separately rather than treating both cases as an undifferentiated parse error.

Authority: [RFC 5545 section 3.3.5](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.3.5).

### APPLE-AUDIT-3 — This-and-following edits silently become single edits (High)

`RECURRENCE-ID;RANGE=THISANDFUTURE` is accepted by PUT, but the RANGE
parameter is discarded during normalization (`backend/src/ics.rs:384`).
The CalDAV handler applies the exception as a single occurrence and returns
200. A subsequent GET has only that one exception; later instances retain
their original times. This silently changes the meaning of an accepted edit.
`docs/APPLE-CALENDAR.md:27` explicitly promises this-and-following support.

Reproducer: `audit_apple_thisandfuture_is_not_silently_narrowed` submits a
master plus a two-hour THISANDFUTURE shift. A correct implementation must
preserve the shift for later instances, or explicitly reject the operation
without changing the resource. Preserve RANGE semantics throughout parsing,
persistence, serialization and query membership; if unsupported, reject it
atomically and correct the support documentation.

Authority: [RFC 5545 section 3.8.4.4](https://www.rfc-editor.org/rfc/rfc5545.html#section-3.8.4.4).

### APPLE-AUDIT-4 — Occurrence metadata overwrites the series (High)

A supported standalone occurrence PUT with its own category and reminder
changes the master's category and attaches the occurrence reminder to the
master. The returned occurrence has neither category nor reminder.
This can change reminders/categories for the series instead of the intended
occurrence, and even an occurrence edit without metadata can clear master
metadata through the same path.

The update handler uses `with_caldav_properties` for occurrence updates
(`backend/src/caldav/http.rs:1123`), and the domain transaction persists those
properties against the master event ID (`backend/src/event.rs:429`).
Exception serialization hardcodes empty properties
(`backend/src/caldav/http.rs:2778`). Full-resource master-plus-exception writes
also lack exception-specific metadata persistence.

Reproducer: `audit_apple_occurrence_metadata_stays_on_occurrence` creates a
categorized series, edits one occurrence with a distinct category and alarm,
then checks GET's parsed master and exception. It fails because the master has
the occurrence category. Persist client metadata per recurrence instance and
serialize it on that instance while preserving untouched master metadata.

## Coverage findings

### Apple trace fixtures are not executed against the server

`backend/tests/apple_calendar_fixtures.rs:261` validates JSON shape and
sanitization. It never dispatches the recorded HTTP requests or compares actual
responses with fixture expectations. Its passing trace tests cannot verify
native-client discovery, synchronization, CRUD, or revocation behavior.

The fixtures already contain contradictions:

- Both sync traces use the calendarserver namespace for `sync-collection`
  instead of DAV. The production parser rejects that root; the existing unit
  test `report_rejects_calendarserver_namespace_root` confirms this.
- The initial-sync fixture expects `sync-level` in the response, whereas the
  implemented response contains a sync token and resource responses.
- The connect fixture expects a 301 redirect, but discovery deliberately
  returns 307 to preserve PROPFIND. Its DAV/Allow headers also differ from the
  implemented capabilities, and its root principal property expectation is
  stale.

Add authenticated fixture replay with placeholder substitution and semantic
XML assertions. Correct stale fixtures from a credential-free real-client
capture; do not loosen the server to accept their invalid namespace.

### ETag listing → multiget hydration → token recovery

Added the passing HTTP test
`audit_apple_sync_hydrates_returned_hrefs_and_recovers_invalid_token`.
It requests the Apple-shaped ETag-only listing with Depth 1, hydrates the
returned href through calendar-multiget, checks matching ETag and ICS content,
then verifies rejected-token 403 and a successful empty-token full resync.

This verifies server recovery when a client performs the reset. It does not
prove that native Apple Calendar automatically performs that reset.

### Additional gaps, not confirmed root causes

- Floating-time PUT/GET semantics and custom VTIMEZONE definitions: the parser
  interprets floating times as UTC and resolves TZID only through chrono-tz.
  Native Apple-shaped HTTP coverage is missing for these cases.
- No current automated test establishes native Apple behavior across an actual
  deployment or exercises its entire discovery-to-refresh request sequence.
- Malformed stored data causing a whole REPORT to fail is a resilience concern;
  this audit did not establish a legitimate application input that triggers it.

## Verification

Before adding audit tests: 115 CalDAV-related library tests, 110 CalDAV HTTP
integration tests, 12 Apple fixture tests, and 8 connection-settings frontend
tests passed.

After adding the audit tests: 111 CalDAV HTTP integration tests and 12 Apple
fixture tests passed, with four known-bug reproductions ignored. Running those
four deliberately produced four failures in 0.13 seconds: two HTTP 400s,
one master-metadata overwrite, and one missing future-occurrence time shift.
`cargo fmt --manifest-path backend/Cargo.toml --check` and `git diff --check`
also passed. Production implementation files remain unchanged.

Known-bug reproductions are explicitly ignored in the normal suite because
this is an advisory audit. Run them deliberately with:

```sh
cargo test --manifest-path backend/Cargo.toml --locked --offline \
  --test caldav_event_resource audit_apple -- --ignored --nocapture
```

The passing new flow test runs with:

```sh
cargo test --manifest-path backend/Cargo.toml --locked --offline \
  --test caldav_event_resource audit_apple_sync
```
