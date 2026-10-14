# CalDAV calendar creation

Happening accepts authenticated `MKCALENDAR` requests at
`/dav/calendars/<principal>/<client-key>/`. Creation is restricted to the
authenticated account's own calendar home. This implements the collection
creation contract in [RFC 4791 §5.3.1](https://www.rfc-editor.org/rfc/rfc4791#section-5.3.1).

## URL identity and persistence

Migration `0031_caldav_calendar_collections.sql` adds nullable collection key,
original principal user ID, and timezone metadata columns to `calendars`, plus
a unique index on `(dav_principal_user_id, dav_collection_key)`. Existing rows
need no backfill. Numeric IDs remain the internal identity and existing numeric
DAV URLs continue to work.

A new collection's client key is preserved, including its case, in discovery,
event Locations, collection and event PROPFIND, calendar-query, multiget, and
sync responses. Resolution always applies the existing calendar ACL and read/write
authorization after resolving the key. Names are scoped to the original principal,
independently of later ownership transfers. A recipient's home lists a shared
calendar using its numeric ID, avoiding collisions with that recipient's own
client keys. Ownership transfer leaves the original URL usable while its
principal retains calendar access; the new owner sees a numeric URL in their own
home. Revocation of access still hides the collection.

Client keys are 1–255 ASCII letters, digits, hyphens, or underscores. Entirely
numeric keys and signed numeric IDs are reserved for legacy addressing.
Percent-encoded spellings, query strings, and other characters are rejected for
creation. This deliberately narrow collection namespace supports Apple's UUID
names without ambiguous alternate spellings. Archived collections retain their
reserved key. The unique index also prevents concurrent requests from claiming
the same URL.

## Supported properties

All properties supplied in MKCALENDAR are required: an unsupported or invalid
property causes the entire request to fail. An empty body is supported.

| Expanded property | Behavior |
| --- | --- |
| `DAV:displayname` | Nonblank name; defaults to the collection key when omitted. XML entities are decoded and escaped when returned. |
| `urn:ietf:params:xml:ns:caldav:calendar-description` | Optional text, including empty text. |
| `http://apple.com/ns/ical/calendar-color` | `#RRGGBB` or `#RRGGBBAA`; preserved, including alpha and case. Default `#3367d6`. |
| `urn:ietf:params:xml:ns:caldav:calendar-timezone` | iCalendar metadata with VERSION 2.0, PRODID, exactly one VTIMEZONE with TZID, and at least one STANDARD/DAYLIGHT observance with valid local DTSTART and UTC offsets. Parsed using existing ICS line unfolding and property syntax; returned via PROPFIND. |
| `urn:ietf:params:xml:ns:caldav:supported-calendar-component-set` | Exactly one `comp name="VEVENT"`; other component sets fail. |

The timezone metadata is preserved as supplied after XML decoding. Its TZID is
used as the calendar's default timezone when recognized by the existing IANA
implementation; otherwise the default stays UTC. Embedded timezone transition
rules are metadata, not a replacement for the existing IANA event-time engine.
The validator checks component structure and mandatory observance fields; it
does not evaluate custom timezone RRULEs. Custom TZIDs do not enable custom
zone interpretation for event PUTs.

Calendar creation reuses CalendarService validation, owner ACL creation, audit
logging, and authorization projections. Calendar, ACL, visibility epoch, DAV
identity, timezone, and audit entry are committed in one transaction. The
response projection is prepared before commit, eliminating a fallible read
after creation. XML/property validation runs before persistence. A database
error or identity collision rolls back the whole transaction.

## Protocol responses and limits

- Complete creation: `201`, empty body, `Cache-Control: no-cache`.
- Existing target: `405`, endpoint `Allow`, `DAV:resource-must-be-null` error.
- Missing/invalid credentials: `401` with the existing Basic challenge.
- Another principal: `404`, following existing resource-hiding conventions.
- Invalid collection key/location: `409` with `calendar-collection-location-ok`.
- Malformed XML or MKCALENDAR grammar: `400`. DTDs are rejected.
- Non-XML body media type: `415`; accepted types are `application/xml` and `text/xml`.
- Body over the existing 64 KiB DAV bound: `413`.
- Required property failure: `207` with a `CALDAV:mkcalendar-response`, property
  failures (`403` for unsupported properties, `409` for invalid supported values)
  and `424` for otherwise valid properties. Invalid timezone metadata carries
  the `valid-calendar-data` condition. No collection is created.

Existing collection OPTIONS/Allow retains `PROPFIND, OPTIONS, REPORT`.
Authenticated OPTIONS on a valid unmapped child target advertises
`MKCALENDAR, OPTIONS`; invalid targets do not advertise creation. The owner's
calendar home advertises `DAV:bind`. Existing event privileges remain governed
by their existing ACL projection. REPORT Depth compatibility remains unchanged.

This adds creation only. Collection PROPPATCH, collection DELETE, scheduling,
VTODO, arbitrary dead properties, and nested calendar creation remain unsupported.
Apple may use additional properties in versions/configurations not represented
by these tests; unsupported required properties will fail explicitly.

## TDD and validation evidence

The new authenticated HTTP integration test file is
`backend/tests/caldav_mkcalendar.rs`. Its initial four scenarios were run before
implementation: all four failed because MKCALENDAR returned 405 (expected 201,
401, or an appropriate invalid-path response). Property failure assertions were
corrected to RFC 4791's outer 207 response before the GREEN run.

Additional RED/GREEN loops reproduced and fixed:

- POST's Allow advertised existing-collection methods on an unmapped target.
- Signed numeric keys were created but subsequently resolved as numeric IDs.
- Percent-encoded names were rediscovered with a different spelling.
- Ownership transfer collided with an identically named collection belonging
  to the new owner.
- Multiple calendar roots were accepted in timezone metadata.
- Unnamespaced unknown properties produced invalid namespace declarations in
  error responses.

The tests cover creation/property discovery, query and multiget REPORTs,
conditional event PUT/GET/DELETE, initial and incremental sync, legacy numeric
collections, duplicate/concurrent targets, principal and event isolation,
ownership changes, invalid XML/paths/properties, protocol advertisement, and a
forced database failure after calendar/ACL insertion followed by successful
retry. Test bodies are synthetic protocol examples, **not Apple captures**.

Final validation:

- `cargo test --offline --manifest-path backend/Cargo.toml --test caldav_mkcalendar`:
  all 10 new tests passed.
- `cargo test --offline --manifest-path backend/Cargo.toml`: 717 passed, 0 failed,
  4 existing ignored Apple audit tests across 48 test binaries/doc-test runs.
  The initial sandbox run failed 11 existing tests because local socket binding
  was denied; the final full run used approved loopback access and passed.
- `cargo fmt --manifest-path backend/Cargo.toml --check`: passed.
- `cargo clippy --offline --manifest-path backend/Cargo.toml --all-targets -- -D warnings`:
  passed.
- `git diff --check`: passed.

The existing REPORT Depth compatibility logic is unchanged. Pre-existing
`.gitignore`, `backend/tests/caldav_event_resource.rs`, and Apple audit/deployment
document changes were preserved. No dependencies were installed; no push,
merge, deployment, or actual Apple device acceptance run was performed.

## Production verification and Apple Calendar acceptance

Perform this procedure only after the usual authorized release and deployment.
No deployment or production mutation was performed as part of implementation.

1. Back up the production database through the established procedure. Confirm
   migration 0031 succeeds and its unique index exists. Verify existing numeric
   calendars still appear and sync using an existing test account. Ensure the
   reverse proxy forwards MKCALENDAR, REPORT, PROPFIND, and conditional headers.
2. Use a dedicated registered test account. Issue a CalDAV app password in
   Happening, and configure Apple Calendar with the account's advertised server
   URL, username, and app password. Never record the password or Authorization
   header in logs or acceptance evidence.
3. In Apple Calendar, select **File → New Calendar → the Happening account**.
   Name it `MKCALENDAR acceptance`, choose a recognizable color, and refresh.
   Observe a genuine MKCALENDAR to a client-generated collection URL and a 201
   response. Record OS/Calendar versions, timestamp, sanitized path, status,
   and requested property names from the real request. If it fails, retain the
   sanitized real error rather than inventing a fixture.
4. Refresh account discovery. Verify PROPFIND returns the exact same collection
   URL and the selected display name/color. If Apple sends calendar-timezone,
   request it explicitly via PROPFIND and compare the decoded metadata. Verify
   the calendar appears in Happening with the expected owner and defaults.
5. Create a timed event in that calendar. Refresh both applications. Confirm
   PUT succeeds at the UUID collection URL; GET and REPORT hydration use that
   URL and return the event. Edit the title and timing in Apple, refresh, and
   confirm Happening sees the changes. Edit it in Happening, refresh Apple, and
   confirm incremental sync returns the updated event at the same URL.
6. Delete the event in Apple. Confirm Happening removes it and subsequent sync
   returns its tombstone under the same collection URL. Quit/reopen Calendar,
   refresh, and confirm the calendar remains discoverable at its original URL.
7. Verify an existing numeric calendar still syncs. With a second test account,
   verify the new calendar/event is inaccessible without an ACL grant. In an
   authorized staging protocol check, retry the same MKCALENDAR URL and confirm
   405 with no replacement or duplicate; submit an unsupported required property
   and confirm 207 plus absence from discovery. Remove test data through existing
   supported application controls.

Acceptance requires real Apple creation, stable discovery URLs, bidirectional
event edits, deletion sync, and successful reopen/refresh. Passing synthetic
integration tests does not itself establish acceptance on an Apple device.
