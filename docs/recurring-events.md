# Recurring event API

Recurring timed events use the existing authenticated calendar event routes. All
mutation routes require the normal session, same-origin, and CSRF checks, and
calendar authorization failures use the same non-leaking `404` response as
non-recurring events.

## Supported operations

- Create a timed or all-day series with
  `POST /api/v1/calendars/{calendar_id}/events` by adding `recurrence_rule` to
  the normal event body.
- Expand series in a bounded window with
  `GET /api/v1/calendars/{calendar_id}/events?from={unix}&to={unix}`.
  Occurrences include `series_id`. Timed occurrences identify the source
  instant with numeric UTC `recurrence_id`; all-day occurrences use an ISO
  `recurrence_date`. All-day `end_date` values remain exclusive.
- Replace one occurrence with
  `PATCH /api/v1/calendars/{calendar_id}/events/{series_id}/occurrences/{recurrence_id}`.
  The body contains the current series `version` and normal fields matching the
  series kind. The path identity is the numeric UTC recurrence ID for timed
  events or ISO recurrence date for all-day events.
- Exclude one occurrence with
  `DELETE /api/v1/calendars/{calendar_id}/events/{series_id}/occurrences/{recurrence_id}`.
  The JSON body is `{ "version": current_series_version }`.
- Update the whole series through the normal event `PATCH`. A whole-series
  update clears prior single-occurrence exceptions transactionally so stale
  recurrence identities cannot survive a changed template.

Series and exception writes use optimistic series versions. A stale mutation
returns `409` with `current_version`. Successful recurring mutations invoke the
notification-replanning seam; notification job tables remain deferred to
Prompt 25.

## Not yet supported

`PATCH /api/v1/calendars/{calendar_id}/events/{series_id}/occurrences/{recurrence_id}/following`
authenticates and authorizes the request, verifies the occurrence, and returns:

```json
{
  "error": {
    "code": "not_supported",
    "message": "This and following recurring edits are not yet supported"
  }
}
```

The status is `501 Not Implemented`. Splitting a series is intentionally not
advertised as an MVP capability until exception migration, COUNT/UNTIL
rewriting, and notification ownership have an unambiguous domain contract.
