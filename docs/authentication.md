# Authentication API

## Invitation consumption

`POST /api/v1/auth/invitations/consume` accepts:

```json
{"token":"<invitation token>"}
```

On success, the response sets the opaque `__Host-commoncal_session` cookie with
`Secure`, `HttpOnly`, `SameSite=Lax`, and `Path=/`. Any session token supplied in
that cookie is revoked and replaced; callers must use the newly returned cookie.

The JSON response contains the active user summary and a `csrf_token`. The
`csrf_token` is the CSRF bootstrap mechanism: browser clients keep it in memory
and send it with later authenticated unsafe requests when CSRF enforcement is
introduced. It is tied to the newly issued session and is not a session
credential.

Invalid, expired, revoked, and previously consumed invitation tokens receive the
same `invalid_invitation` response. Token and session hashes are never returned.

## Authenticated sessions

`GET /api/v1/auth/session` resolves the opaque cookie and returns the active user
and current session timestamps. Sessions are rejected when revoked, when their
absolute deadline is reached, or after seven days of inactivity. Last-seen
writes are throttled to once every five minutes.

`DELETE /api/v1/auth/session` revokes the current session.
`DELETE /api/v1/auth/sessions` revokes every session for the current user. Both
clear the session cookie.

Authenticated `POST`, `PUT`, `PATCH`, and `DELETE` requests must send the
session-bound token in `X-CSRF-Token`, an `Origin` matching `APP_ORIGIN`, and
same-origin or same-site Fetch Metadata. Public health, login-link request, and
authentication token-consumption routes are outside this authenticated
middleware.
