#!/usr/bin/env node
"use strict";

/**
 * Verification-only mock MCP + OAuth server for the Phase 0.3 (oauth-lifecycle)
 * and Phase 0.4 (security-isolation) acceptance harnesses.
 *
 * This is NOT a release artifact and NOT the production component. It exists
 * solely to exercise the harnesses' PASS and FAIL paths deterministically.
 *
 * The mock mirrors production's two-host topology and is driven by a single
 * world selector so its behavior is order-independent (a real OAuth client
 * discovers before it ever calls the MCP endpoint, so the world cannot be
 * inferred from the first MCP probe):
 *
 *   MOCK_WORLD=compliant (default) — the TARGET state:
 *     MCP host (MOCK_PORT, default 4001):
 *       GET  /.well-known/oauth-protected-resource
 *            -> RFC 9728 protected-resource metadata (valid JSON), naming the
 *               issuer host
 *       POST /mcp
 *            -> full authenticated MCP lifecycle (initialize,
 *               notifications/initialized, tools/list, tools/call
 *               calendar_list) gated by Bearer JWT validation + live grant
 *     Issuer host (MOCK_ISSUER_PORT, default MOCK_PORT + 1):
 *       GET  /.well-known/oauth-authorization-server
 *            -> RFC 8414 authorization-server metadata (valid JSON with
 *               authorization, token, registration, and JWKS endpoints)
 *       GET  /.well-known/openid-configuration
 *            -> CommonCal SPA HTML (mirrors the verified current state of
 *               cal.hajnal.space, which falls through to the SPA ingress)
 *       POST /register  -> DCR (RFC 7591) client registration
 *       GET  /authorize -> Authorization Code flow with S256 PKCE (auto-approves
 *                          login and consent, redirects back with the code)
 *       POST /token     -> Token exchange (Authorization Code + S256 PKCE)
 *       GET  /jwks      -> JWKS document (public key only)
 *
 *   MOCK_WORLD=current — the CURRENT production state:
 *     MCP host:
 *       GET  /.well-known/oauth-protected-resource
 *            -> valid JSON (production serves this document correctly)
 *       POST /mcp
 *            -> the CURRENT production gateway: only tools/list and tools/call
 *               are recognized; initialize (and everything else) is HTTP 400
 *               with JSON-RPC -32601 and no WWW-Authenticate header
 *               (mcp-server/src/gateway.rs)
 *     Issuer host:
 *       every route -> CommonCal SPA HTML (the authorization service is not
 *          deployed; the advertised issuer host is handled by core's "/"
 *          ingress and falls through to the SPA — docs/MCP-PRODUCTION-FIX-PLAN.md)
 *
 * The world is fixed at startup (MOCK_WORLD), so discovery, DCR, authorization,
 * token exchange, and the MCP lifecycle all agree regardless of request order.
 *
 * Run:  node mcp-acceptance/mock/lifecycle-server.js
 * Env:  MOCK_PORT (default 4001), MOCK_ISSUER_PORT (default MOCK_PORT + 1),
 *       MOCK_WORLD (default "compliant"; "current" mirrors production today)
 */

const http = require("node:http");
const {
  MockOAuthProvider,
  SCOPE_CATALOG,
  decodeJwtPayload,
} = require("./oauth-provider");

const PORT = Number(process.env.MOCK_PORT || 4001);
const ISSUER_PORT = Number(process.env.MOCK_ISSUER_PORT || PORT + 1);
const MCP_BASE = `http://127.0.0.1:${PORT}`;
const ISSUER_BASE = `http://127.0.0.1:${ISSUER_PORT}`;
const RESOURCE_URL = `${MCP_BASE}/mcp`;
const LOOPBACK_REDIRECT = "http://127.0.0.1:8765/callback";
const WORLD = (process.env.MOCK_WORLD || "compliant").toLowerCase();
const COMPLIANT = WORLD !== "current";

const SPA_HTML =
  '<!doctype html><html><head><meta charset="utf-8"><title>CommonCal</title></head><body><div id="root"></div><script src="/static/js/main.js"></script></body></html>';

// The mock OAuth provider (DCR + Authorization Code + S256 PKCE + token
// exchange + JWKS + token validation + grants).
const provider = new MockOAuthProvider({
  issuer: ISSUER_BASE,
  resource: RESOURCE_URL,
  redirect: LOOPBACK_REDIRECT,
});

// Per-subject calendar data for the mock. Each subject has a distinct set of
// calendars so the concurrency isolation test can verify that two clients
// cannot observe each other's calendars.
const SUBJECT_CALENDARS = {
  "1": [
    { id: 101, name: "Alice Work", color: "#ff0000", access: "full" },
    { id: 102, name: "Alice Personal", color: "#00ff00", access: "read" },
  ],
  "2": [
    { id: 201, name: "Bob Work", color: "#0000ff", access: "full" },
    { id: 202, name: "Bob Personal", color: "#ff00ff", access: "read" },
  ],
};

// The nine MCP tools (mcp-server/src/tools/mod.rs list_tools()).
const NINE_TOOLS = [
  "availability_find",
  "calendar_list",
  "event_get",
  "event_search",
  "event_create",
  "event_update",
  "reminder_set",
  "event_delete_prepare",
  "event_delete_commit",
];

// Tool descriptions and input schemas (mirrors the rmcp SDK-generated shapes
// from slice1-lab/src/bin/mcp_echo.rs).
const TOOL_SCHEMAS = {
  availability_find: {
    description: "Find availability slots for the specified calendars within a time range.",
    inputSchema: {
      type: "object",
      properties: {
        calendar_ids: { type: "array", items: { type: "integer" } },
        from: { type: "string" },
        to: { type: "string" },
      },
      required: ["calendar_ids", "from", "to"],
    },
  },
  calendar_list: {
    description: "List the calendars available to the authenticated user.",
    inputSchema: {
      type: "object",
      properties: {
        include_access: { type: "boolean" },
      },
    },
  },
  event_get: {
    description: "Get the details of a specific event by calendar and event ID.",
    inputSchema: {
      type: "object",
      properties: {
        calendar_id: { type: "integer" },
        event_id: { type: "integer" },
      },
      required: ["calendar_id", "event_id"],
    },
  },
  event_search: {
    description: "Search events in a calendar within a time range, optionally filtered by query.",
    inputSchema: {
      type: "object",
      properties: {
        calendar_id: { type: "integer" },
        from: { type: "string" },
        to: { type: "string" },
        query: { type: "string" },
      },
      required: ["calendar_id", "from", "to"],
    },
  },
  event_create: {
    description: "Create a new event in the specified calendar.",
    inputSchema: {
      type: "object",
      properties: {
        calendar_id: { type: "integer" },
        title: { type: "string" },
        description: { type: "string" },
        location: { type: "string" },
        start_utc: { type: "string" },
        end_utc: { type: "string" },
        idempotency_key: { type: "string" },
      },
      required: ["calendar_id", "title", "start_utc", "end_utc"],
    },
  },
  event_update: {
    description: "Update an existing event. Requires the expected_version for optimistic concurrency.",
    inputSchema: {
      type: "object",
      properties: {
        calendar_id: { type: "integer" },
        event_id: { type: "integer" },
        expected_version: { type: "integer" },
        title: { type: "string" },
        description: { type: "string" },
        location: { type: "string" },
        start_utc: { type: "string" },
        end_utc: { type: "string" },
      },
      required: ["calendar_id", "event_id", "expected_version"],
    },
  },
  reminder_set: {
    description: "Set a reminder on an event.",
    inputSchema: {
      type: "object",
      properties: {
        calendar_id: { type: "integer" },
        event_id: { type: "integer" },
        offset_minutes: { type: "integer" },
      },
      required: ["calendar_id", "event_id", "offset_minutes"],
    },
  },
  event_delete_prepare: {
    description: "Prepare to delete an event (two-phase deletion, step 1).",
    inputSchema: {
      type: "object",
      properties: {
        calendar_id: { type: "integer" },
        event_id: { type: "integer" },
      },
      required: ["calendar_id", "event_id"],
    },
  },
  event_delete_commit: {
    description: "Commit the deletion of an event (two-phase deletion, step 2).",
    inputSchema: {
      type: "object",
      properties: {
        calendar_id: { type: "integer" },
        event_id: { type: "integer" },
        confirmation_token: { type: "string" },
      },
      required: ["calendar_id", "event_id", "confirmation_token"],
    },
  },
};

function sendJson(res, status, body, headers = {}) {
  const payload = JSON.stringify(body);
  res.writeHead(status, {
    "Content-Type": "application/json",
    "Content-Length": Buffer.byteLength(payload),
    ...headers,
  });
  res.end(payload);
}

function sendHtml(res, status, html) {
  res.writeHead(status, {
    "Content-Type": "text/html; charset=utf-8",
    "Content-Length": Buffer.byteLength(html),
  });
  res.end(html);
}

function protectedResourceMetadata() {
  return {
    resource: RESOURCE_URL,
    authorization_servers: [ISSUER_BASE],
    scopes_supported: SCOPE_CATALOG,
    dpop_bound_access_tokens: false,
  };
}

function authorizationServerMetadata() {
  return {
    issuer: ISSUER_BASE,
    authorization_endpoint: `${ISSUER_BASE}/authorize`,
    token_endpoint: `${ISSUER_BASE}/token`,
    registration_endpoint: `${ISSUER_BASE}/register`,
    jwks_uri: `${ISSUER_BASE}/jwks`,
    response_types_supported: ["code"],
    grant_types_supported: ["authorization_code"],
    code_challenge_methods_supported: ["S256"],
  };
}

/**
 * Build the 401 challenge response referencing protected-resource metadata.
 * Mirrors the lab's challenge shape (slice1-lab/src/bin/mcp_echo.rs unauthorized()).
 */
function unauthorizedChallenge(reason) {
  const challenge =
    `Bearer realm="mcp", resource_metadata="${MCP_BASE}/.well-known/oauth-protected-resource"` +
    (reason ? `, error="${reason}"` : "");
  return {
    status: 401,
    headers: { "WWW-Authenticate": challenge },
    body: { error: { code: -2000, message: "authorization token is required" } },
  };
}

/**
 * Handle a compliant MCP request (full authenticated lifecycle).
 * Validates the Bearer token, checks the grant, and dispatches to the tool.
 * Uses per-request identity (no global CURRENT_CLAIMS slot) so concurrent
 * clients cannot observe each other's identity.
 */
function handleCompliantMcp(req, res, message) {
  const method = message && message.method;
  const id = message ? message.id : null;

  // Extract the Bearer token.
  const authHeader = req.headers["authorization"];
  if (!authHeader || !authHeader.startsWith("Bearer ")) {
    const c = unauthorizedChallenge("missing_token");
    sendJson(res, c.status, c.body, c.headers);
    return;
  }
  const token = authHeader.slice(7);

  // Validate the token (signature, iss, aud, exp, sub, client_id).
  const validation = provider.validateAccessToken(token);
  if (!validation.ok) {
    const c = unauthorizedChallenge(validation.error);
    sendJson(res, c.status, c.body, c.headers);
    return;
  }
  const claims = validation.claims;
  const sub = claims.sub;
  const client_id = claims.client_id;

  // Dispatch by method.
  if (method === "initialize") {
    sendJson(res, 200, {
      jsonrpc: "2.0",
      id,
      result: {
        protocolVersion: "2025-03-26",
        capabilities: { tools: {} },
        serverInfo: { name: "mock-standards-compliant", version: "1.0.0" },
      },
    });
    return;
  }

  if (method === "notifications/initialized") {
    // This is a notification (no id expected). Return 202 Accepted.
    res.writeHead(202, { "Content-Length": 0 });
    res.end();
    return;
  }

  if (method === "tools/list") {
    const tools = NINE_TOOLS.map((name) => ({
      name,
      ...TOOL_SCHEMAS[name],
    }));
    sendJson(res, 200, {
      jsonrpc: "2.0",
      id,
      result: { tools },
    });
    return;
  }

  if (method === "tools/call") {
    const toolName = message.params && message.params.name;
    if (toolName !== "calendar_list") {
      sendJson(res, 200, {
        jsonrpc: "2.0",
        id,
        error: { code: -32601, message: `Method not found: ${toolName}` },
      });
      return;
    }

    // Check the active grant for (sub, client_id).
    const grant = provider.getActiveGrant(sub, client_id);
    if (!grant) {
      sendJson(res, 200, {
        jsonrpc: "2.0",
        id,
        error: { code: -2003, message: "no MCP grant found" },
      });
      return;
    }

    // Filter calendars by the grant's allowed calendar IDs.
    const allCalendars = SUBJECT_CALENDARS[sub] || [];
    const filtered = allCalendars.filter((c) =>
      grant.allowed_calendar_ids.includes(c.id)
    );

    const output = { calendars: filtered };
    sendJson(res, 200, {
      jsonrpc: "2.0",
      id,
      result: {
        content: [
          {
            type: "text",
            text: JSON.stringify(output),
          },
        ],
      },
    });
    return;
  }

  // Unknown method.
  sendJson(res, 200, {
    jsonrpc: "2.0",
    id,
    error: { code: -32601, message: `Method not found: ${method}` },
  });
}

/**
 * Handle a current-production MCP request (mirrors mcp-server/src/gateway.rs).
 * Only tools/list and tools/call are recognized; everything else (including
 * initialize) is HTTP 400 with JSON-RPC -32601 and no WWW-Authenticate header.
 */
function handleCurrentMcp(req, res, message) {
  const method = message && message.method;
  const id = message ? message.id : null;

  if (method === "tools/list" || method === "tools/call") {
    // Current production: tools/list returns only {name} for each tool,
    // no description or inputSchema (mcp-server/src/gateway.rs:187-212).
    if (method === "tools/list") {
      const tools = NINE_TOOLS.map((name) => ({ name }));
      sendJson(res, 200, {
        jsonrpc: "2.0",
        id,
        result: { tools },
      });
    } else {
      // tools/call requires auth; the current production 401 builder
      // hard-codes the lab loopback URL (mcp-server/src/gateway.rs:456-541).
      const authHeader = req.headers["authorization"];
      if (!authHeader || !authHeader.startsWith("Bearer ")) {
        sendJson(
          res,
          401,
          { error: { code: -2000, message: "authorization token is required" } },
          {
            "WWW-Authenticate":
              'Bearer realm="mcp", resource_metadata="http://127.0.0.1:3001/.well-known/oauth-protected-resource", error="missing_token"',
          }
        );
      } else {
        // With a token, the current production would validate it and dispatch.
        // For the mock, we return a generic success.
        sendJson(res, 200, {
          jsonrpc: "2.0",
          id,
          result: { content: [{ type: "text", text: '{"calendars":[]}' }] },
        });
      }
    }
    return;
  }

  // Everything else (including initialize) is HTTP 400 with -32601.
  sendJson(res, 400, {
    jsonrpc: "2.0",
    id,
    error: { code: -32601, message: `Method not found: ${method}` },
  });
}

const mcpServer = http.createServer((req, res) => {
  if (req.method === "GET") {
    if (req.url === "/.well-known/oauth-protected-resource") {
      sendJson(res, 200, protectedResourceMetadata());
      return;
    }
    sendJson(res, 404, { error: "not found" });
    return;
  }

  if (req.method !== "POST") {
    sendJson(res, 405, { error: "method not allowed" });
    return;
  }

  let raw = "";
  req.on("data", (chunk) => {
    raw += chunk;
  });
  req.on("end", () => {
    let message = null;
    try {
      message = JSON.parse(raw);
    } catch {
      message = null;
    }

    if (COMPLIANT) {
      handleCompliantMcp(req, res, message);
      return;
    }
    handleCurrentMcp(req, res, message);
  });
});

const issuerServer = http.createServer((req, res) => {
  // In the current world the authorization service is not deployed; the
  // advertised issuer host is handled by core's "/" ingress and falls through
  // to the SPA (docs/MCP-PRODUCTION-FIX-PLAN.md). Every issuer route returns
  // the SPA HTML.
  if (!COMPLIANT) {
    sendHtml(res, 200, SPA_HTML);
    return;
  }

  if (req.method === "GET") {
    const parsedUrl = new URL(req.url, ISSUER_BASE);
    const pathname = parsedUrl.pathname;
    if (pathname === "/.well-known/oauth-authorization-server") {
      sendJson(res, 200, authorizationServerMetadata());
      return;
    }
    if (pathname === "/.well-known/openid-configuration") {
      // Verified current state: cal.hajnal.space returns the CommonCal SPA
      // HTML, not authorization-server metadata.
      sendHtml(res, 200, SPA_HTML);
      return;
    }
    if (pathname === "/jwks") {
      sendJson(res, 200, provider.jwks());
      return;
    }
    if (pathname === "/authorize") {
      // Authorization Code flow with S256 PKCE. Parse the query params.
      const params = parsedUrl.searchParams;
      const result = provider.handleAuthorize(params);
      if (result.status === 302 && result.location) {
        res.writeHead(302, { Location: result.location, "Content-Length": 0 });
        res.end();
      } else {
        sendJson(res, result.status, result.body);
      }
      return;
    }
    sendHtml(res, 200, SPA_HTML);
    return;
  }

  if (req.method === "POST") {
    let raw = "";
    req.on("data", (chunk) => {
      raw += chunk;
    });
    req.on("end", () => {
      if (req.url === "/register") {
        let body;
        try {
          body = JSON.parse(raw);
        } catch {
          body = {};
        }
        const result = provider.handleRegister(body);
        sendJson(res, result.status, result.body);
        return;
      }
      if (req.url === "/token") {
        // Token exchange: form-encoded body.
        const params = new URLSearchParams(raw);
        const result = provider.handleToken(params);
        if (result.status === 200 && result.body.access_token) {
          // Simulate CommonCal consent having approved a grant for the
          // authenticated subject (Phase 4 in production). The fixed lab
          // subject "1" is granted both of its calendars.
          const claims = decodeJwtPayload(result.body.access_token);
          if (claims && claims.sub && claims.client_id) {
            provider.upsertGrant(
              claims.sub,
              claims.client_id,
              [101, 102],
              SCOPE_CATALOG
            );
          }
        }
        sendJson(res, result.status, result.body);
        return;
      }
      // Verification-only test hooks (not part of the production contract).
      // The Phase 0.4 security harness uses these to drive grant-state cases
      // (missing / revoked / broadened) without depending on the lab binary.
      if (req.url === "/_test/grant/revoke") {
        let body;
        try {
          body = JSON.parse(raw);
        } catch {
          body = {};
        }
        provider.revokeGrant(body.sub, body.client_id);
        sendJson(res, 200, { revoked: true });
        return;
      }
      if (req.url === "/_test/grant/broaden") {
        let body;
        try {
          body = JSON.parse(raw);
        } catch {
          body = {};
        }
        // Broaden the grant to include calendars the subject does not own
        // (ids 999/998 do not exist in SUBJECT_CALENDARS), simulating an
        // attempted grant broadening that must be denied by the resource
        // server's live-membership check.
        provider.upsertGrant(
          body.sub,
          body.client_id,
          [101, 102, 999, 998],
          SCOPE_CATALOG
        );
        sendJson(res, 200, { broadened: true });
        return;
      }
      if (req.url === "/_test/grant/reset") {
        let body;
        try {
          body = JSON.parse(raw);
        } catch {
          body = {};
        }
        provider.upsertGrant(
          body.sub,
          body.client_id,
          [101, 102],
          SCOPE_CATALOG
        );
        sendJson(res, 200, { reset: true });
        return;
      }
      if (req.url === "/_test/grant/delete") {
        let body;
        try {
          body = JSON.parse(raw);
        } catch {
          body = {};
        }
        provider.deleteGrant(body.sub, body.client_id);
        sendJson(res, 200, { deleted: true });
        return;
      }
      // Verification-only test hook: set a grant with explicit allowed
      // calendar IDs. The Phase 0.4 security harness uses this to drive the
      // concurrency-isolation case (S9) with two different subjects, each
      // granted their own calendars.
      if (req.url === "/_test/grant/set") {
        let body;
        try {
          body = JSON.parse(raw);
        } catch {
          body = {};
        }
        const allowed = Array.isArray(body.allowed_calendar_ids)
          ? body.allowed_calendar_ids
          : [101, 102];
        provider.upsertGrant(body.sub, body.client_id, allowed, SCOPE_CATALOG);
        sendJson(res, 200, { set: true });
        return;
      }
      // Verification-only test hook: mint a token with specific claims, signed
      // with the provider's key. The Phase 0.4 security harness uses this to
      // drive the negative token cases (wrong issuer / audience / expiry /
      // client) with a VALID signature, so each claim check is exercised in
      // isolation (not masked by a signature failure). The "wrong signature"
      // case is driven by tampering a valid token's signature instead.
      if (req.url === "/_test/token/mint") {
        let body;
        try {
          body = JSON.parse(raw);
        } catch {
          body = {};
        }
        const claims = body.claims || {};
        const token = provider.mintToken(claims);
        sendJson(res, 200, { access_token: token });
        return;
      }
      sendJson(res, 404, { error: "not found" });
    });
    return;
  }

  sendJson(res, 405, { error: "method not allowed" });
});

mcpServer.listen(PORT, "127.0.0.1", () => {
  console.log(`[mock-lifecycle] world: ${WORLD}`);
  console.log(`[mock-lifecycle] MCP host on http://127.0.0.1:${PORT}`);
  console.log(`[mock-lifecycle] MCP endpoint: http://127.0.0.1:${PORT}/mcp`);
});

issuerServer.listen(ISSUER_PORT, "127.0.0.1", () => {
  console.log(`[mock-lifecycle] issuer host on http://127.0.0.1:${ISSUER_PORT}`);
});
