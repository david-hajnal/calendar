# CalDAV compatibility repair prompt

Repair Happening's complete CalDAV request/response implementation so Apple Calendar can discover an account and synchronize reliably. Work from the current code, not from previous claims that the compatibility fixes are complete. Preserve unrelated working-tree changes. Implement and verify the fixes below; do not stop after fixing the first discovery response. Do not deploy automatically.

## Scope and evidence

Inspect `backend/src/caldav/{http,query,auth,repository,ical,types}.rs`, the domain event change-log hooks in `backend/src/event.rs`, the iCalendar input parser, routing/middleware and deployment discovery configuration, and `e2e/tests/caldav-account.spec.ts`. Review all success and error branches of every exposed DAV method.

Production authentication and root discovery currently succeed. Authenticated PROPFIND on the principal returns its correct principal resource type and CalDAV calendar-home-set. Calendar-home Depth 1 returns two calendar collections with correct CalDAV namespaces and report wrappers. However, unsupported properties appear as bare text, for example:

```xml
<D:prop>D:current-user-principal</D:prop>
```

and:

```xml
<D:prop>D:supported-report-set
C:supported-calendar-component-set</D:prop>
```

Calendar responses also contain an empty 404 propstat even when every requested property is supported. These are confirmed production defects. Apple still rejects account setup, but there is no captured Apple request trace yet; do not claim any one defect conclusively explains the entire failure.

Use primary specifications: RFC 4918 (WebDAV), RFC 3744 (privileges/principals), RFC 4791 (CalDAV), RFC 5397 (current-user-principal), RFC 6764 (discovery), RFC 6578 (sync), RFC 5545 (iCalendar), and RFC 9110 (HTTP). Distinguish confirmed defects from follow-up checks and preserve already-correct behavior.

## 1. XML serialization and property selection — first priority

In `http.rs`, `prop_element()` currently returns `D:name` or `C:name` as text. Fix all callers, including missing-property propstats and propname responses, to emit actual empty XML elements. Preserve each property's exact namespace URI and local name, including arbitrary client extension namespaces; the fallback `X` prefix currently has no declaration. Escape namespace attribute values and text correctly. Prefer an XML writer and a structured QName representation over string splitting with `:` or `/`.

Generate only nonempty status groups. Return requested known properties in 200 groups and requested unsupported properties as empty elements in 404 groups. Do not emit properties the client did not request. Handle an explicitly empty property selection distinctly from absent selection. Audit escaping for every generated href, ETag, sync token, principal URL and metadata value.

Provide DAV:current-user-principal consistently on DAV resources covered by RFC 5397. Its value must identify the authenticated user's principal, not necessarily the calendar owner. Keep DAV:owner separate. Add appropriate root/principal/home/collection/object resource metadata without inventing properties.

## 2. Replace fragile XML parsing

`query.rs` uses one global namespace map that is updated but never restored on element close; declarations on empty elements also leak into siblings. Use a namespace-aware reader with proper scope or a correct scope stack. Preserve namespace URI and local name separately. Reject unbound prefixes and malformed XML rather than substituting empty namespaces or silently ignoring attribute/unescape errors.

`parse_propfind()` handles direct self-closing property children, but uses the wrong depth for explicit start/end property elements: `<D:getetag></D:getetag>` can disappear from the request. Fix this and test equivalent empty-element syntax. Validate one document root, selector placement/cardinality, balanced structure, and trailing content. Implement valid allprop/include behavior. Ignore allowed extension elements according to the applicable RFC instead of accepting arbitrary invalid structure or rejecting all extensions.

`parse_prop_set()` currently swallows errors and conflates absent D:prop with `<D:prop/>`. Make selection explicit and error-returning. Validate report grammar in one coherent parse rather than separately reparsing malformed documents with weaker rules.

## 3. Discovery and PROPFIND across every resource

Verify `/.well-known/caldav`, the DAV root, principal, calendar-home, calendar collection and individual .ics URLs. Complete the chain with the same credentials and real router/middleware. Root currently ignores Depth while other routes validate it; make Depth handling consistent with resource semantics. The finite-depth helper currently returns 400 despite its intended 403 precondition: correct it. Distinguish absent/default infinity, explicit infinity, 0, 1 and malformed headers.

Individual .ics routes currently lack PROPFIND, although calendar Depth 1 can render their properties. Implement direct object PROPFIND and advertise it in Allow. Keep object resourcetype empty and collection resourcetype DAV:collection plus CalDAV:calendar. Verify supported-calendar-component-set VEVENT, owner, privileges, description and Apple calendar-color. Add RFC-required calendar metadata, including supported-calendar-data, as applicable. Unsupported optional Apple properties should get valid 404 property elements, not fabricated success values.

Check public hostname, HTTPS, redirects, account setup paths, and per-route authentication challenges. Do not assume HTTP 207 means every requested property succeeded.

## 4. All REPORT response types

CalendarQuery, CalendarMultiget and SyncCollection now parse `props`, but their HTTP handlers ignore these fields and always emit getetag/getcontenttype/calendar-data. Honor the requested property selection using the same reliable property machinery. Preserve unknown property namespaces and correctly group statuses. Support or explicitly reject requested calendar-data projection/expansion with the appropriate CalDAV response; do not silently ignore it.

Keep supported-report-set wrappers and report QNames correct. Return the specified DAV/CalDAV error preconditions for unsupported reports and unsupported filters instead of mapping every failure to generic 400. Validate REPORT Depth per report; sync must follow RFC 6578's Depth 0/default semantics.

## 5. Calendar query semantics and completeness

The current parser merely permits comp-filter names without checking their `name` attributes or tree shape. It extracts the first time range anywhere inside a filter and requires both bounds. Implement correct VCALENDAR/VEVENT component filtering, legal queries without a time range, and permitted one-sided bounds. Reject unsupported components/filters explicitly rather than returning an unrelated VEVENT result set.

Repository range selection admits all recurring masters starting before the upper bound without verifying an occurrence overlaps the requested range. Apply correct recurrence-aware overlap semantics, including exceptions, deleted occurrences, all-day events, timezone/DST transitions and modified instances moved into or out of the range. Ensure each logical resource occurs once.

The query SQL silently limits results to MAX_RESULTS. Detect overflow and return the applicable CalDAV limit error; do not deliver an apparently complete truncated result. Preserve resource and expansion safeguards without rejecting routine Apple requests unnecessarily.

## 6. Sync correctness and data loss prevention

Current sync output puts the new token in a synthetic collection response/propstat. RFC 6578 requires DAV:sync-token directly beneath DAV:multistatus. Correct all initial/incremental/empty responses and retain direct response-level 404 for deletions. Add the live DAV:sync-token collection property.

Invalid tokens currently return plain 400. Return 403 with DAV:error/DAV:valid-sync-token. Tokens currently contain only a signed revision, are not absolute URIs, and are not bound to a collection. Make tokens opaque URI values bound to collection identity and relevant visibility/state; reject foreign, future, expired, tampered or otherwise invalid tokens safely. Validate required sync input elements and limits according to RFC 6578.

Initial snapshot reads resources and then reads the latest revision, which can skip concurrent changes. Establish a consistent snapshot/high-water mark. Incremental pages currently emit one response per change-log row, so repeated updates/deletion/recreation can duplicate or contradict the same href. Coalesce changes to the correct final state and implement RFC truncation/continuation semantics. Prove no changes are skipped at page boundaries or under concurrent writes. Bound initial snapshots without silently dropping resources.

Audit all event writes, recurrence changes, imports and deletions for durable change-log entries and usable deleted hrefs. The HTTP delete path currently ignores errors while stamping a deleted resource name; handle failures so a successful deletion cannot leave unusable sync tombstones. Check permission and free/busy visibility changes and prevent stale private data remaining in client caches.

## 7. HTTP, permissions and iCalendar

Oversized request bodies currently return 400; use 413 where appropriate. Successful event DELETE currently returns 200 with no body; use 204. Unsupported methods must return an accurate resource-specific Allow header. Audit DAV capability claims: class 2 implies locking, and access-control implies protocol obligations; advertise only capabilities actually implemented, or implement required behavior within the agreed scope.

PUT returns strong ETags after parsing and canonicalizing submitted iCalendar. Follow RFC 9110 section 9.3.4: return validators only when the submitted representation is stored without transformation; otherwise omit the PUT validator and let GET return canonical content/ETag. Verify create/update status, Location, If-Match/If-None-Match, missing-precondition policy, and atomic stale-write rejection. Missing conditions are not equivalent to supplied stale conditions. Confirm permission checks match advertised privileges; do not advertise write-properties/write-acl operations that the server rejects.

Review iCalendar input/output for valid CRLF, folding by octets, text/parameter escaping, UID, DTSTAMP, all-day end exclusivity, recurrence identifiers/exceptions and round-trip preservation. Check TZID references: either provide valid VTIMEZONE components or serialize in a compliant alternative. Compare ETags and bytes consistently across GET/HEAD/PROPFIND/REPORT and free/busy views. Verify failures in serialization/hydration do not silently hide valid events as absent resources.

## 8. Verification and handoff

Create regression tests before each confirmed fix. Parse emitted XML and assert namespace URI/local name, hierarchy, status grouping and property values; substring-only tests are insufficient. Include these production principal/home requests as fixtures, unsupported properties in DAV/CalDAV/Apple/arbitrary namespaces, all-supported/all-unsupported/mixed selections, propname, empty selection, explicit closing tags, prefix rebinding, default namespaces, malformed documents and escaping.

Exercise every route/method with valid and invalid authentication, Depth variants, permission levels and unknown resources. Cover query/multiget/sync, absolute same-origin multiget hrefs as well as paths, foreign/traversal hrefs, requested subsets, empty calendars, pagination, concurrent writes, deletion/recreation and stable ETags. Absolute href normalization must preserve collection authorization and avoid fetching external URLs.

Build an integration test that performs the full discovery and first-sync sequence through the real router, then create/read/update/delete and incremental sync. Retain imported-event and free/busy restrictions and recurrence safeguards. Run formatting, focused regression tests, the relevant backend suite and executable E2E checks. Report actual commands/results and any unavailable verification.

Provide a credential-safe production smoke script using environment variables. It must validate XML semantics and per-property statuses, follow advertised URLs, and fail on malformed output. Never print Authorization or passwords. Document how to capture Apple setup method/path/status/user-agent without secrets. The final handoff must distinguish code-level protocol verification from an actual successful macOS account setup; do not claim Apple compatibility solely from green unit tests or a root 207.

Deliver a concise list of fixed defects, residual risks, test results and exact post-deployment checks. Update Apple Calendar documentation only where verified behavior changes.
