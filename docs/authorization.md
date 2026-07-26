# Authorization

Calendar authorization is a pure, centralized decision. The caller supplies an
active actor's recognized platform role, the actor's calendar role, and the
requested action. Missing or unrecognized roles deny access.

Platform superadmins do not bypass calendar ACLs. They receive calendar access
only through an explicit calendar role. Suspended, invited, and deleted users
are denied every calendar action.

## Calendar role/action matrix

| Calendar role | Read details | Read free/busy | Create event | Edit any event | Manage settings | Manage ACL | Transfer ownership | Delete calendar |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Owner | Yes | Yes | Yes | Yes | Yes | Yes | Yes | Yes |
| Manager | Yes | Yes | Yes | Yes | Yes | Yes | No | No |
| Editor | Yes | Yes | Yes | Yes | No | No | No | No |
| Viewer | Yes | Yes | No | No | No | No | No | No |
| Free/busy viewer | No | Yes | No | No | No | No | No | No |

Any role/action pair not explicitly allowed by this matrix is denied.

## Platform user administration

User-administration routes use the same centralized deny-by-default decision
boundary. Only an active user with the `superadmin` platform role may list
users, manage invitations, change account status or platform roles, or revoke
another user's sessions. A resource identifier in the route never grants
authority by itself.

The service serializes demotion and suspension changes and rejects either
operation when it would remove the final active superadmin. Every successful
administrative mutation is appended to the immutable audit log.

## Calendar API projection and absence

`GET /api/v1/calendars` returns only calendars for which the authenticated user
has a current ACL entry. Owner, manager, editor, and viewer responses include
calendar metadata. A free/busy viewer receives only the calendar identifier,
their role, and `access: "free_busy"`; names, descriptions, ownership, settings,
archive state, versions, and timestamps are omitted.

`GET`, `PATCH`, archive, restore, and delete requests return the same
`404 not_found` envelope for a nonexistent calendar and for a calendar on which the
requested action is not authorized. This prevents identifier substitution from
revealing private resource existence.

Calendar settings use `PATCH /api/v1/calendars/{id}` with a required `version`.
Archive and restore use `POST /api/v1/calendars/{id}/archive` and
`POST /api/v1/calendars/{id}/restore`. Only an owner may delete a calendar.

## Calendar sharing API

Owners and managers may list ACL entries with
`GET /api/v1/calendars/{id}/acl` and grant or update the `manager`, `editor`,
`viewer`, and `free_busy_viewer` roles with
`PUT /api/v1/calendars/{id}/acl/{userId}`. They may revoke non-owner entries
with `DELETE /api/v1/calendars/{id}/acl/{userId}`. Unauthorized and inaccessible
calendar requests use the same `404 not_found` response as other calendar
operations.

Only the current owner may transfer ownership, using
`POST /api/v1/calendars/{id}/transfer` with `new_owner_user_id` and the current
calendar `version`. The target must be active. Transfer updates the calendar,
promotes the new owner, demotes the prior owner to manager, and writes the audit
record in one transaction.

Each grant, role update, revocation, and transfer is audited. Revocation invokes
the pending-notification cancellation contract inside the ACL transaction.
Notification persistence is introduced by the notification feature; until then,
the application supplies the no-pending-notifications implementation.
