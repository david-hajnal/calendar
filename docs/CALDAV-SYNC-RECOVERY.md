# Apple Calendar sync recovery

## Production observation — 2026-10-15

The operator reported that Apple Calendar had previously synced, then stopped
showing event updates after a bug. After deployment of the fixes, account
requests completed but events still did not sync.

The operator supplied production output confirming:

- Core image `sha-a552805ea67370e544335bf1b7b479efb097be15`, digest
  `sha256:fcb944ea748f38a559b1d3992447a4819c2fe83aaea9d345ca0e736fe9877dd3`;
  pod ready with zero restarts.
- Migration 31 applied successfully, including the DAV collection columns and
  unique index.
- The account's four existing calendars still used legacy numeric collection
  URLs, with no persisted UUID collection mappings.

**Recovery reported by the operator:** incrementing the CalDAV visibility epoch
for all affected calendars caused Apple Calendar to start syncing again.
The operator ran the production database change; Codex did not execute it.

This supports stale or inconsistent sync state as a recovery target. It does
not establish which earlier bug caused that state, prove that every event was
downloaded, or establish full bidirectional CRUD acceptance. No client protocol
capture was supplied to confirm the exact reset/retry sequence. The earlier
MKCALENDAR 207 responses were property failures, but were not established as the
cause of missing events in existing calendars.

## Why an epoch increment helps

Happening uses `DAV:sync-token`, rather than `getctag`, for collection sync.
The token context includes the calendar ID, user ID, ACL role, calendar version,
ACL creation time, and the corresponding `caldav_visibility_epochs.epoch`.
Incrementing that epoch invalidates previously issued tokens for that
user/calendar pair without modifying event data, URLs, or permissions.

The server rejects the old token with `403 DAV:valid-sync-token`. The protocol
requires recovery through a full synchronization using an empty token. The
fresh listing returns event URLs and requested properties; event content can
then be fetched separately. A full listing does not necessarily redownload
unchanged resources already cached by the client.

References:

- [RFC 6578 §3.1–3.2: synchronization and token invalidation](https://www.rfc-editor.org/rfc/rfc6578.html#section-3.1)
- [Apple's CTag extension: deprecated in favor of WebDAV Sync](https://github.com/apple/ccs-calendarserver/blob/master/doc/Extensions/caldav-ctag.txt)
- Implementation: `backend/src/caldav/http.rs` (`sync_context`,
  `handle_sync_collection`), `backend/src/caldav/auth.rs`
  (`collection_sync_state`), and migration 0028.

## Targeted recovery procedure

Use as a one-time recovery for affected accounts after the underlying server
bug has been addressed, rather than invalidating tokens on every refresh or
deployment. Start with one affected calendar where practical. Follow the
normal production database backup procedure before a manual write.

In a production SQLite session, substitute the affected account's actual
principal ID. This example invalidates tokens for all of that account's
currently accessible calendars, including shared calendars, without affecting
other users' tokens:

```sql
BEGIN IMMEDIATE;

UPDATE caldav_visibility_epochs
SET epoch = epoch + 1
WHERE user_id = (
    SELECT user_id FROM caldav_accounts
    WHERE principal_id = '<principal-id>'
)
AND calendar_id IN (
    SELECT calendar_id FROM calendar_acl
    WHERE user_id = caldav_visibility_epochs.user_id
)
RETURNING calendar_id, user_id, epoch;

COMMIT;
```

For a single-calendar trial, add `AND calendar_id = <calendar-id>` before
`RETURNING`. Check that the returned rows match the intended scope. Do not
decrement epochs afterward: this could make previously invalidated tokens
valid again. No application restart or deployment is needed.

Refresh the affected Apple Calendar account. Verify an existing event appears,
then check an update from Happening and an edit from Apple, followed by another
refresh. Successful HTTP statuses alone do not prove event synchronization.
If events remain missing, inspect the full listing and subsequent event-content
retrieval; repeated epoch increments will not repair missing server results or
invalid iCalendar data.
