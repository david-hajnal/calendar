# calendar-local MCP Tools (CRUD)

## Create
| Tool | Description |
|---|---|
| `event_create` | Create calendar events and optional reminders |
| `reminder_set` | Set a reminder for an event |

## Read
| Tool | Description |
|---|---|
| `calendar_list` | List the calendars available to the authenticated user |
| `event_get` | Get the details of a specific event by calendar and event ID |
| `event_search` | Search for events |
| `availability_find` | Find available time slots |

## Update
| Tool | Description |
|---|---|
| `event_update` | Update an existing event |

## Delete
| Tool | Description |
|---|---|
| `event_delete_prepare` | Begin two-phase deletion (returns a confirmation intent) |
| `event_delete_commit` | Commit the pending deletion after user confirmation |
