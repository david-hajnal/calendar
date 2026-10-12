---
name: mcp-add-events
description: Create calendar events and optional reminders through a chosen MCP server, collecting missing server, environment, calendar, title, and timing details.
---

# Add events through MCP

Create one or more events using the user's selected MCP server. Carry forward
explicit choices from the prompt and current conversation; ask only for missing
or conflicting details. Bundle related questions so the user can answer once.
An event-creation request authorizes creation once the required details are known.

## Select the destination

1. Discover available calendar MCP servers and their actual tool schemas.
   If the server is unspecified, ask which MCP server to use, offering the
   available names. If it is specified, use that exact server.
2. If local versus production is unspecified, ask which environment to use.
   An explicit server/environment mapping in the prompt or current conversation
   counts as a supplied answer. Check the selected server's configured endpoint
   when available. Resolve any mismatch before writing.
3. Call the selected server's calendar-list tool. Show calendar names and IDs,
   then ask which calendar to use unless the user already supplied one. Resolve
   supplied names against the returned list; ask if a name is ambiguous.
   For multiple events, support different calendars when explicitly requested.

Destination selection is complete only when the server, environment, and each
calendar ID are resolved. Use this server for all event and reminder operations.
If discovery, authentication, or calendar access fails, report the actual error
and the necessary next step. Do not substitute a different server, AppleScript,
or the operating system's Calendar app. An empty calendar list requires the user
to choose whether to create a calendar or authorize access to an existing one.

## Collect event details

For each event, collect these details from the request or ask for them:

- **Title:** ask for the event title when absent; preserve supplied wording.
- **Schedule:** ask for the start date, end date, and whether it is all day.
  For a timed event, ask for missing start/end times. A supplied duration can
  determine the end; explicit clock times imply a timed event. A one-day all-day
  request determines both dates. Do not invent a date, time, or duration.
- **Timezone:** default to the user's local IANA timezone from trusted client
  context or their stated preference. State the timezone being used. Ask only
  if it cannot be determined; the server or execution host timezone is not a
  substitute. Resolve relative dates using the user's local current date.
- **Reminders:** ask whether they want no reminder, calendar defaults, or an
  explicit reminder, unless already specified. For explicit reminders, collect
  the lead time and any channel supported by the server. A numeric lead time
  needs a unit. Check whether calendar defaults can be preserved by omission.

For batches, ask whether missing calendar, timezone, or reminder choices apply
to all events. Keep per-event overrides. Proceed when every event has a title,
a resolved calendar, a complete schedule, and an explicit reminder choice.

## Translate and create

Read the selected server's current schemas rather than guessing tool names or
parameters. CommonCal exposes `calendar_list`, `event_create`, `event_get` or
`event_search`, and `reminder_set`; these names are hints for discovery.

Convert local timed dates to the representation required by the tool, using the
IANA timezone and its offset on the event date. Check that end follows start.
Ask about ambiguous or nonexistent clock times during daylight-saving changes.
For all-day events, use the tool's date/all-day representation and its documented
end-date convention; convert an inclusive last day to an exclusive end when
required. If the tool lacks all-day support, explain the limitation and ask
whether a timed substitute is acceptable before creating it. CommonCal's current
`event_create` has `start_utc` and `end_utc` but no all-day field.

Create each event, then apply requested reminders to its returned event ID.
Validate reminders against the discovered tool's supported lead times and
channels. If a requested reminder cannot be represented, resolve that choice
before creating the event. If reminder creation fails after event creation,
report the event as created and the reminder as failed; retain its ID for retry.

Use a stable idempotency/operation ID per event when the server supports it.
After a timeout or ambiguous response, inspect the selected calendar for the
result before retrying. Keep successful events when part of a batch fails and
retry only unresolved operations. Never report success solely from an attempted
call.

## Verify and report

Read back each created event and verify its calendar, title, start/end, timezone
interpretation, and all-day status. Verify reminders using a read tool when
available; otherwise identify the successful reminder response as the evidence.
Report the server and environment, calendar, event IDs, local schedule with
timezone, and reminder outcome. Clearly identify any unresolved event or reminder.
