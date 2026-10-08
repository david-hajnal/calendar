# Connecting an agent to CommonCal MCP

## Production status

As verified on 2026-09-17, the production endpoint is reachable but is **not
currently connectable by a standard MCP client**:

- `POST https://mcal.hajnal.space/mcp` with `tools/list` returns the tool catalog.
- An MCP `initialize` request returns `-32601 Method not found`.
- Protected-resource discovery advertises `https://cal.hajnal.space` as the
  authorization server, but its OIDC discovery URL returns the CommonCal HTML
  application instead of OAuth metadata.

Both MCP initialization and OAuth discovery must be fixed before an agent can
make authenticated tool calls.

## Connect from OpenCode

OpenCode is the MCP client. LM Studio is the model provider and Qwen is the
model, so neither one connects to MCP directly. No additional MCP client needs
to be supplied. Once production is fixed, add the remote Streamable HTTP server:

```sh
opencode mcp add commoncal-production --url https://mcal.hajnal.space/mcp
```

Run `opencode mcp list`. If authentication is required, start it with:

```sh
opencode mcp auth commoncal-production
```

OpenCode can discover OAuth configuration, register a public client when the
authorization server supports dynamic registration, and manage its tokens. Do
not configure `MCP_INTERNAL_API_KEY` in the agent; that secret is only for
communication from the MCP server to CommonCal core.

OpenCode's browser OAuth flow requires a callback reachable from the user's
browser. See the [OpenCode MCP documentation](https://opencode.ai/v2/docs/mcp-servers).
After authentication, verify `initialize`, `tools/list`, and `calendar_list`.

An additional MCP client is useful only for independent smoke testing, for
example MCP Inspector. It is not part of the production connection architecture.
