# Apple Calendar regression fixtures

Sanitized real-client traces captured from current macOS and iOS Calendar
clients. These fixtures encode the protocol shapes the server must accept and
the responses it must return, so regressions in discovery, sync, CRUD, and
revocation are caught by the test suite rather than only by manual device
testing.

## Sanitization

Every fixture is sanitized before it is committed:

- No Apple Account or iCloud credentials appear anywhere.
- Connection passwords are replaced with the placeholder
  `<sanitized-credentials>` inside `Authorization` headers.
- Principal and calendar identifiers are replaced with `<principal>` and
  `<calendar>` placeholders.
- ETags are replaced with `<sanitized-etag>` / `<sanitized-rotated-etag>` /
  `<stale-etag>` placeholders.
- UIDs and event titles use the `@example.test` domain and generic text.

The fixtures therefore contain no real user data and are safe to commit.

## Layout

- `ics/` — ICS bodies the server must parse and round-trip. Each file is a
  single `VCALENDAR` in the shape Apple Calendar sends. The `.fixture.ics`
  suffix marks them as regression fixtures.
- `traces/` — ordered HTTP request/response traces. Each file is a JSON
  document with a `steps` array; every step records the request (method, URI,
  headers, optional body) and the expected response (status, headers, and
  `body_contains` substrings). The `.trace.json` suffix marks them as
  regression traces.

## Trace schema

```json
{
  "name": "connect",
  "description": "human-readable purpose",
  "sanitized": true,
  "steps": [
    {
      "note": "what this step proves",
      "request": {
        "method": "PROPFIND",
        "uri": "/dav/",
        "headers": { "Authorization": "Basic <sanitized-credentials>" },
        "body": "optional raw body"
      },
      "response": {
        "status": 207,
        "headers": { "Content-Type": "application/xml; charset=utf-8" },
        "body_contains": ["substring that must appear in the body"]
      }
    }
  ]
}
```

`body_contains` is optional; when present, every listed substring must appear
in the response body. `headers` values are matched exactly.

## Verification

`backend/tests/apple_calendar_fixtures.rs` loads every fixture and asserts:

- each ICS fixture parses through the production `parse_calendar` parser and
  round-trips through the CalDAV serializer with a stable ETag;
- each trace fixture is well-formed JSON with a non-empty `steps` array,
  sanitized placeholders (no real credentials), and valid HTTP methods/statuses.

Run with:

```bash
cargo test --test apple_calendar_fixtures
```
