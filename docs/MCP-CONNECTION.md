# CommonCal MCP connection verification

The canonical setup instructions are in [Connect an AI assistant](../README.md#connect-an-ai-assistant).
Use the public MCP URL, `https://mcal.hajnal.space/mcp`, and browser OAuth.
The authorization issuer is `https://auth.hajnal.space`. The September 17
initialization/discovery failures previously described here are historical;
see [production operations](AUTH-PRODUCTION.md) for the current SQLite deployment.
Successful discovery or DCR alone does not prove authenticated calendar access.

## Local client checks

From the repository root, run each installed client's README snippets:

```sh
node mcp-acceptance/client-snippets.js opencode
node mcp-acceptance/client-snippets.js codex
node mcp-acceptance/client-snippets.js claude
```

The harness extracts the marked shell blocks directly from README, substitutes
only the endpoint with a temporary loopback fixture, and runs the actual CLI.
It creates temporary config/data/cache/state/runtime directories and removes
them afterward. It never uses production credentials. OpenCode's background
service is disabled in the temporary profile to avoid attaching to the user's
service. Missing binaries and failed checks exit nonzero; they are not passes.
Requires Node 22+ and permission to listen on loopback. This Unix harness uses
`/bin/sh` and process groups; Windows needs a separate runner.

Checks cover add, status, logout/removal, and the OpenCode JSON example's
agreement with the CLI-generated configuration. The OpenCode check also attempts
DCR against a fixture that intentionally rejects registration after observing
the callback. It never opens a browser or issues credentials. A callback outside
the current production shape fails the check rather than silently widening the
policy. These checks do not establish production OAuth, minimum supported
versions, a calendar read, or refresh/revocation behavior.

See [the slice 13–14 evidence](plans/cross-client-mcp-connector/08-client-delivery-evidence.md)
for the actual results, including failed checks.

## Live compatibility matrix

Use a clean client profile and a test CommonCal account with at least one
calendar. Do not import another client's tokens. Record the exact client version,
OS, date, endpoint, commands/configuration, observed redirect host/path/port,
and the outcome of each step. Keep authorization URLs, codes, tokens, cookies,
and secrets out of the evidence.

| Step | Required observation |
| --- | --- |
| Add and list | The documented registration/configuration is accepted in a clean profile |
| Authorize | DCR public client, browser login and consent complete; no pasted tokens or secrets |
| Discover | Initialize succeeds and all nine named tools have descriptions and input schemas |
| Read | The client's `calendar_list` returns the test account's real eligible calendars |
| Refresh | After access-token expiry, the client reads again using its stored refresh flow |
| Deny | A fresh denied authorization creates no grant and cannot read calendars |
| Revoke | Revoke the owning grant, then immediately retry a read; it must fail |
| Remove | Local logout/removal leaves the server absent or disabled and clears its credentials |
| Recover | Re-add and authorize after denial/revocation; restart the client and read again |
| Callback recovery | A cancelled/expired callback fails safely; a fresh login succeeds |

For OpenCode v2, deterministic config uses `mcp.servers`, not the v1 flat
`mcp` map; temporary disabling uses `disabled: true`. Prefer the documented CLI
and JSON path. The optional `--wizard` path is not part of the verified
instructions until separately exercised. OpenCode owns MCP; a model provider
such as LM Studio does not replace it.

Only after observing a released client's actual callback may an operator
narrowly adjust the callback policy and rerun redirect-policy negatives.
An ephemeral port alone does not prove the path matches. The updated source retains the configured `/mcp/oauth/callback` and adds
only the observed v2 HTTP IPv4 loopback `/callback` shape. This requires an
auth-service image rollout before production can accept it; no deployment was
performed here. Do not infer callback compatibility from older records
mentioning only `127.0.0.1:7777`. Do not admit arbitrary HTTPS callbacks to make a client pass.

## Server-side revocation

There is currently no grant-management settings screen. While signed into
CommonCal, open browser developer tools on the CommonCal application origin.
Use the Network panel or a same-origin browser request to:

1. Fetch `GET /api/v1/mcp-grants` with the existing browser session. It returns
   only your active grants. Identify the connection by `oauth_client_id` and
   `grant_id`; do not guess a grant ID or revoke every grant.
2. Fetch `GET /api/v1/auth/session` in the same origin and keep its `csrf_token`
   in browser memory. Do not print, copy, or share that response.
3. Send `DELETE /api/v1/mcp-grants/{grant_id}` with the existing session and
   `X-CSRF-Token` header. A successful response is HTTP 200. A 404 means the
   grant is absent, already revoked, or not owned by this account.
4. Retry `calendar_list` from the connected client. It must fail immediately,
   even if the access token has not expired. Then perform local logout/removal.

A concrete browser-console procedure is below. It displays only grant IDs and
client IDs, asks which grant to revoke, and requires confirmation. Review the
code before running it on your CommonCal tab. The session/CSRF value stays in
the function's memory.

```js
(async () => {
  const grantsResponse = await fetch('/api/v1/mcp-grants', { credentials: 'same-origin' });
  if (!grantsResponse.ok) throw new Error(`List grants: HTTP ${grantsResponse.status}`);
  const grants = await grantsResponse.json();
  console.table(grants.map(({ grant_id, oauth_client_id }) => ({ grant_id, oauth_client_id })));
  const id = prompt('Enter the grant_id of the connection to revoke (Cancel to stop)');
  const grant = grants.find((entry) => entry.grant_id === id);
  if (!grant || !confirm(`Revoke access for client ${grant.oauth_client_id}?`)) return;
  const sessionResponse = await fetch('/api/v1/auth/session', { credentials: 'same-origin' });
  if (!sessionResponse.ok) throw new Error(`Session: HTTP ${sessionResponse.status}`);
  const session = await sessionResponse.json();
  if (!session.csrf_token) throw new Error('Missing session CSRF token');
  const result = await fetch(`/api/v1/mcp-grants/${encodeURIComponent(id)}`, {
    method: 'DELETE', credentials: 'same-origin', headers: { 'X-CSRF-Token': session.csrf_token },
  });
  if (result.status !== 200) throw new Error(`Revoke: HTTP ${result.status}`);
  console.info('Grant revoked. Retry calendar_list in the client; it must fail.');
})().catch((error) => console.error(error.message));
```

Local removal does not revoke the CommonCal grant. Conversely, revocation
blocks calendar access but does not delete the client's local configuration.

## Official references

- [OpenCode v2 MCP setup, configuration and OAuth](https://opencode.ai/v2/docs/mcp-servers)
- [OpenCode v2 service and isolation behavior](https://opencode.ai/v2/docs/cli)
- [Codex MCP configuration and OAuth](https://developers.openai.com/codex/mcp/)
- [Claude Code HTTP MCP, user scope and authentication](https://code.claude.com/docs/en/mcp)
