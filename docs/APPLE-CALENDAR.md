# Apple Calendar Guide

Connect Apple Calendar (macOS and iOS) to Happening and use the same calendars
from both apps. Changes made in either place follow to the other.

## What this is

- **Two-way sync.** Create, edit, move, or cancel an event in Apple Calendar or
  in Happening and the change appears in the other.
- **Happening is the source of truth.** Your Happening account owns the data.
- **No Apple password.** Happening never asks for your Apple Account or iCloud
  password. You connect with a Happening email plus a one-time connection
  password you generate.
- **Account-level.** Every Happening calendar you can access shows up in Apple
  Calendar, subject to your existing Happening permissions.

## What syncs

| Happening calendar type | In Apple Calendar |
|---|---|
| Writable calendar | Two-way (create, edit, delete) |
| Read-only calendar | Read-only |
| Free/busy-only calendar | Busy placeholders, no private details |
| Imported external-feed events | Read-only |

All-day events and recurring events (including single-occurrence and
this-and-following edits) round-trip both directions.

## What you need

- A Happening account and a signed-in browser session.
- macOS or iOS with the built-in Calendar app.
- The **Server** and **Username** values shown on your Settings screen (see
  below). The current production server is `cal.hajnal.space`.

## Connect a device

### 1. Generate a connection password

In the Happening web app:

1. Click the **Settings** icon in the top navigation.
2. On the **Apple Calendar** screen, note the **Server** and **Username**
   values. Username is your Happening account email.
3. Enter a **Device label** (e.g. `My iPhone`) so you can tell devices apart
   later.
4. Click **Generate password**.
5. **Copy the password now.** It is shown exactly once and cannot be retrieved
   again.

### 2. Add the CalDAV account on the device

**macOS**

1. System Settings → Internet Accounts → Add Account → **Other CalDAV Account**.
2. Server: the **Server** value from your Settings screen (e.g.
   `cal.hajnal.space`).
3. Username: your Happening email.
4. Password: the one-time connection password you just copied.
5. Click **Next**. macOS discovers the account, lists your calendars, and
   starts syncing.

**iOS**

1. Settings → Mail → Accounts → Add Account → **Other** → **Other CalDAV
   Account**.
2. Server: the **Server** value from your Settings screen.
3. Username: your Happening email.
4. Password: the one-time connection password.
5. Tap **Next**. iOS discovers the account, lists your calendars, and starts
   syncing.

After setup, sync is automatic. Apple Calendar decides its own refresh timing;
the first sync is a full snapshot and later syncs are incremental.

## Manage connections

On the **Apple Calendar** settings screen:

- **Connected devices** lists each device by label, with its last-used time.
- **Revoke** a single device to cut it off immediately. Its next sync fails and
  it stops seeing changes. Other devices are unaffected.
- **Disconnect Apple Calendar** revokes every connection password at once.
  Your event data is untouched, so reconnecting later does not duplicate events.

Revoking a password does not delete any events. It only stops that device from
syncing.

## Troubleshooting

- **Password not working / sync fails with 401.** The password was revoked or
  mistyped. Generate a new one and re-add the account on the device.
- **Calendars not appearing.** Confirm you are signed in to the right Happening
  account, and that the calendar is not archived. Read-only and free/busy
  calendars appear but are not editable.
- **A change isn't showing up yet.** Sync is driven by Apple Calendar's refresh
  cycle. Pull to refresh in Calendar, or wait a moment and check again.
- **Editing a recurring event.** You can change a single occurrence or
  this-and-following. Unsupported operations (e.g. calendar invitations,
  to-dos) are rejected with a clear error rather than silently dropped.

## What is not supported

- Calendar creation or deletion from Apple Calendar.
- To-dos (`VTODO`) and journals (`VJOURNAL`).
- Calendar invitations / scheduling (organizer and attendee properties).
- Importing unrelated iCloud calendars into Happening.

## API reference

For scripting or non-Apple CalDAV clients, the connection endpoints are:

| Action | Endpoint |
|---|---|
| Connection status | `GET /api/v1/calendar-connections/apple` |
| Issue a password | `POST /api/v1/calendar-connections/apple/passwords` |
| Revoke one password | `DELETE /api/v1/calendar-connections/apple/passwords/:id` |
| Revoke all | `DELETE /api/v1/calendar-connections/apple` |

The CalDAV surface is at `https://<server>/dav/`, with discovery at
`/.well-known/caldav`. See `DEPLOYMENT.md` for the full protocol and
deployment details.
