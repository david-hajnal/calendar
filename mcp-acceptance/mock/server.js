#!/usr/bin/env node
"use strict";

/**
 * Verification-only mock MCP server for the Phase 0.1 and Phase 0.2
 * acceptance harnesses.
 *
 * This is NOT a release artifact and NOT the production component. It exists
 * solely to exercise the harnesses' PASS and FAIL paths deterministically.
 *
 * It mirrors production's two-host topology:
 *
 *   MCP host (MOCK_PORT, default 3999):
 *     POST /mcp-compliant  -> Phase 0.1 PASS: standards-compatible initialize
 *                             response (200)
 *     POST /mcp-current    -> Phase 0.1 FAIL: the CURRENT production behavior:
 *                             HTTP 400 with JSON-RPC error -32601 "Method not
 *                             found" (mirrors mcp-server/src/gateway.rs, which
 *                             only recognizes tools/list and tools/call)
 *     POST /mcp-discovery-compliant
 *                           -> Phase 0.2 PASS: initialize returns HTTP 401
 *                              with a Bearer WWW-Authenticate challenge whose
 *                              resource_metadata points at the MCP host's
 *                              well-known URL
 *     POST /mcp-discovery-current
 *                           -> Phase 0.2 FAIL: mirrors the CURRENT production
 *                              behavior: initialize returns HTTP 400 with
 *                              JSON-RPC -32601 and no WWW-Authenticate header
 *     GET /.well-known/oauth-protected-resource
 *                           -> RFC 9728 protected-resource metadata (valid
 *                              JSON in both modes, mirroring production where
 *                              the MCP host serves this document correctly),
 *                              naming the issuer host
 *
 *   Issuer host (MOCK_ISSUER_PORT, default MOCK_PORT + 1):
 *     GET /.well-known/oauth-authorization-server
 *                           -> RFC 8414 authorization-server metadata (valid
 *                              JSON with authorization, token, registration,
 *                              and JWKS endpoints) in the compliant mode;
 *                              CommonCal SPA HTML in the current mode
 *     GET /.well-known/openid-configuration
 *                           -> CommonCal SPA HTML (mirrors the verified
 *                              current state of cal.hajnal.space, which falls
 *                              through to the SPA ingress)
 *
 * The mode (current vs compliant) is set by the last discovery endpoint the
 * harness probes on the MCP host; the harness always probes initialize first,
 * so the mode is fixed before any metadata document is requested.
 *
 * Run:  node mcp-acceptance/mock/server.js
 * Env:  MOCK_PORT (default 3999), MOCK_ISSUER_PORT (default MOCK_PORT + 1)
 */

const http = require("node:http");

const PORT = Number(process.env.MOCK_PORT || 3999);
const ISSUER_PORT = Number(process.env.MOCK_ISSUER_PORT || PORT + 1);
const MCP_BASE = `http://127.0.0.1:${PORT}`;
const ISSUER_BASE = `http://127.0.0.1:${ISSUER_PORT}`;

// "current" mirrors the verified current production state; "compliant"
// mirrors the target state. Set by the discovery endpoints on the MCP host.
let mode = null;

const SPA_HTML =
  '<!doctype html><html><head><meta charset="utf-8"><title>CommonCal</title></head><body><div id="root"></div><script src="/static/js/main.js"></script></body></html>';

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
  // Mirrors the production shape (mcp-server/src/config.rs
  // OauthProtectedResourceMetadata) with the mock MCP host as the resource
  // and the mock issuer host as the authorization server.
  return {
    resource: `${MCP_BASE}/mcp`,
    authorization_servers: [ISSUER_BASE],
    scopes_supported: [
      "commoncal.calendar.metadata.read",
      "commoncal.availability.read",
      "commoncal.event.read.basic",
      "commoncal.event.read.details",
      "commoncal.event.create",
      "commoncal.event.update",
      "commoncal.event.delete",
      "commoncal.reminder.read",
      "commoncal.reminder.write",
    ],
    dpop_bound_access_tokens: false,
  };
}

function authorizationServerMetadata() {
  // RFC 8414 authorization-server metadata with the four endpoints the
  // Phase 0.2 A4 assertion requires.
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
    const method = message && message.method;
    const id = message ? message.id : null;

    if (req.url === "/mcp-discovery-compliant") {
      // Phase 0.2 PASS path: an unauthenticated initialize receives the
      // standards-compliant 401 challenge with a resource_metadata parameter
      // pointing at the MCP host's well-known URL (mirrors the lab's
      // challenge shape, slice1-lab/src/bin/mcp_echo.rs unauthorized()).
      mode = "compliant";
      if (method === "initialize") {
        sendJson(
          res,
          401,
          { error: { code: -2000, message: "authorization token is required" } },
          {
            "WWW-Authenticate":
              `Bearer realm="mcp", resource_metadata="${MCP_BASE}/.well-known/oauth-protected-resource", error="missing_token"`,
          }
        );
      } else {
        sendJson(res, 401, {
          jsonrpc: "2.0",
          id,
          error: { code: -2000, message: "authorization token is required" },
        });
      }
      return;
    }

    if (req.url === "/mcp-discovery-current") {
      // Phase 0.2 FAIL path: mirror the CURRENT production gateway — only
      // tools/list and tools/call are recognized; initialize (and everything
      // else) is HTTP 400 with JSON-RPC -32601 and no WWW-Authenticate header.
      // See mcp-server/src/gateway.rs.
      mode = "current";
      if (method === "tools/list" || method === "tools/call") {
        sendJson(res, 200, {
          jsonrpc: "2.0",
          id,
          result: { tools: [{ name: "calendar_list" }] },
        });
      } else {
        sendJson(res, 400, {
          jsonrpc: "2.0",
          id,
          error: { code: -32601, message: `Method not found: ${method}` },
        });
      }
      return;
    }

    if (req.url === "/mcp-compliant") {
      // Standards-compatible initialize response.
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
      } else {
        sendJson(res, 200, {
          jsonrpc: "2.0",
          id,
          error: { code: -32601, message: `Method not found: ${method}` },
        });
      }
      return;
    }

    if (req.url === "/mcp-current") {
      // Mirror the CURRENT production gateway: only tools/list and tools/call
      // are recognized; everything else (including initialize) is -32601 with
      // HTTP 400. See mcp-server/src/gateway.rs.
      if (method === "tools/list" || method === "tools/call") {
        sendJson(res, 200, {
          jsonrpc: "2.0",
          id,
          result: { tools: [{ name: "calendar_list" }] },
        });
      } else {
        sendJson(res, 400, {
          jsonrpc: "2.0",
          id,
          error: { code: -32601, message: `Method not found: ${method}` },
        });
      }
      return;
    }

    sendJson(res, 404, { error: "not found" });
  });
});

const issuerServer = http.createServer((req, res) => {
  if (req.method !== "GET") {
    sendJson(res, 405, { error: "method not allowed" });
    return;
  }
  if (req.url === "/.well-known/oauth-authorization-server") {
    if (mode === "compliant") {
      sendJson(res, 200, authorizationServerMetadata());
    } else {
      // Current production: the issuer host is handled by core's "/" ingress
      // and falls through to the SPA (docs/MCP-PRODUCTION-FIX-PLAN.md).
      sendHtml(res, 200, SPA_HTML);
    }
    return;
  }
  if (req.url === "/.well-known/openid-configuration") {
    // Verified current state: cal.hajnal.space returns the CommonCal SPA
    // HTML, not authorization-server metadata.
    sendHtml(res, 200, SPA_HTML);
    return;
  }
  sendHtml(res, 200, SPA_HTML);
});

mcpServer.listen(PORT, "127.0.0.1", () => {
  console.log(`[mock-mcp] MCP host on http://127.0.0.1:${PORT}`);
  console.log(`[mock-mcp] compliant endpoint: http://127.0.0.1:${PORT}/mcp-compliant`);
  console.log(`[mock-mcp] current-production endpoint: http://127.0.0.1:${PORT}/mcp-current`);
  console.log(`[mock-mcp] discovery-compliant endpoint: http://127.0.0.1:${PORT}/mcp-discovery-compliant`);
  console.log(`[mock-mcp] discovery-current endpoint: http://127.0.0.1:${PORT}/mcp-discovery-current`);
});

issuerServer.listen(ISSUER_PORT, "127.0.0.1", () => {
  console.log(`[mock-mcp] issuer host on http://127.0.0.1:${ISSUER_PORT}`);
});
