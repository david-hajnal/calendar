#!/usr/bin/env node
"use strict";

/**
 * MCP acceptance harness — Phase 0.3 OAuth lifecycle baseline.
 *
 * A standalone black-box harness that targets a configurable production-component
 * MCP endpoint and asserts the full OAuth + MCP lifecycle:
 *
 *   L1  DCR (RFC 7591) registration succeeds with the supported OpenCode
 *       loopback redirect shape (201, client_id present, no client_secret);
 *   L2  Authorization Code flow with state and S256 PKCE completes (code
 *       present, state round-trips);
 *   L3  Token exchange completes and yields a resource-bound access token
 *       (3-part JWT) without manual JWT construction;
 *   L4  With that token, `initialize` succeeds (200, protocolVersion,
 *       capabilities, serverInfo);
 *   L5  `notifications/initialized` is accepted (202);
 *   L6  `tools/list` returns the expected nine tools, each with a description
 *       and a complete input schema;
 *   L7  `tools/call calendar_list` succeeds (200, content block).
 *
 * Baseline expectation (Phase 0.3): against the current production component,
 * L1-L7 FAIL because the authorization service is not deployed (the advertised
 * issuer host falls through to the CommonCal SPA) and the MCP gateway does not
 * implement the MCP lifecycle (initialize returns HTTP 400 with JSON-RPC -32601).
 * The harness asserts the target contract and therefore exits non-zero against
 * the current production component. That failure is the intentional, asserted
 * exit condition for this phase: it documents the current broken state that
 * later phases must fix. The harness is a regression gate: it stays red until
 * the production component implements the full OAuth + MCP lifecycle.
 *
 * This harness does NOT launch or depend on the slice1-lab binaries, and it
 * does NOT treat the lab binary as a release artifact. It reuses the
 * slice1-lab fixture shapes (DCR request shape, standard access-token claim
 * contract, scope catalog) and the MCP/OAuth standards-compliance
 * expectations. It is a pure HTTP client: it starts no servers and leaves no
 * processes behind.
 *
 * Configuration (command-line flags, then environment variables, then defaults):
 *   --url / MCP_URL             MCP endpoint URL (default: https://mcal.hajnal.space/mcp)
 *   --issuer / MCP_OAUTH_ISSUER configured authorization-server issuer
 *                               (default: https://cal.hajnal.space)
 *   --redirect / MCP_REDIRECT   admitted loopback redirect URI
 *                               (default: http://127.0.0.1:8765/callback)
 *   --timeout-ms / MCP_TIMEOUT_MS              request timeout ms (default: 15000)
 *   --record-dir / MCP_RECORD_DIR              record directory (default: OS temp dir)
 *
 * Exit codes:
 *   0  all lifecycle assertions pass (PASS)
 *   1  one or more lifecycle assertions fail (FAIL) — the intentional Phase 0.3
 *      baseline exit condition against the current production component
 *   2  harness error (invalid configuration, network failure, unexpected error)
 */

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const {
  SCOPE_CATALOG,
  isPlainObject,
  decodeJwtPayload,
  runOAuthFlow,
  mcpCall,
  mcpNotify,
  deterministicStringify,
} = require("./lib");

const HARNESS_NAME = "commoncal-mcp-acceptance";
const HARNESS_VERSION = "0.3.0";

const DEFAULTS = {
  url: "https://mcal.hajnal.space/mcp",
  issuer: "https://cal.hajnal.space",
  redirect: "http://127.0.0.1:8765/callback",
  timeoutMs: 15000,
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

class HarnessError extends Error {}

function env(name, fallback) {
  const v = process.env[name];
  return v === undefined || v === "" ? fallback : v;
}

function parseArgs(argv) {
  const flags = {};
  const unknown = [];
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--help" || arg === "-h") {
      flags.help = true;
      continue;
    }
    if (arg.startsWith("--")) {
      const eq = arg.indexOf("=");
      let name;
      let value;
      if (eq !== -1) {
        name = arg.slice(2, eq);
        value = arg.slice(eq + 1);
      } else {
        name = arg.slice(2);
        const next = argv[i + 1];
        if (next !== undefined && !next.startsWith("--")) {
          value = next;
          i++;
        } else {
          value = "true";
        }
      }
      flags[name] = value;
    } else {
      unknown.push(arg);
    }
  }
  flags._unknown = unknown;
  return flags;
}

function printHelp() {
  console.log(`Usage: node mcp-acceptance/oauth-lifecycle.js [options]

Options:
  --url <url>              MCP endpoint URL (default: ${DEFAULTS.url})
  --issuer <url>           configured authorization-server issuer (default: ${DEFAULTS.issuer})
  --redirect <url>         admitted loopback redirect URI (default: ${DEFAULTS.redirect})
  --timeout-ms <n>         request timeout in ms (default: ${DEFAULTS.timeoutMs})
  --record-dir <dir>       directory for the deterministic record (default: OS temp dir)
  -h, --help               show this help

Environment variables MCP_URL, MCP_OAUTH_ISSUER, MCP_REDIRECT, MCP_TIMEOUT_MS,
and MCP_RECORD_DIR are also honored; command-line flags take precedence.
`);
}

function describeError(err) {
  if (err === undefined || err === null) return "unknown error";
  if (typeof err === "string") return err;
  return err.message || String(err);
}

function resolve(flags, flagName, envName, fallback) {
  if (flags[flagName] !== undefined) return flags[flagName];
  return env(envName, fallback);
}

function loadConfig(flags) {
  const url = resolve(flags, "url", "MCP_URL", DEFAULTS.url);
  let parsed;
  try {
    parsed = new URL(url);
  } catch {
    throw new HarnessError(`MCP_URL is not a valid URL: ${url}`);
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    throw new HarnessError(`MCP_URL must use http or https: ${url}`);
  }

  const issuer = resolve(flags, "issuer", "MCP_OAUTH_ISSUER", DEFAULTS.issuer);
  let issuerParsed;
  try {
    issuerParsed = new URL(issuer);
  } catch {
    throw new HarnessError(`MCP_OAUTH_ISSUER is not a valid URL: ${issuer}`);
  }
  if (issuerParsed.protocol !== "http:" && issuerParsed.protocol !== "https:") {
    throw new HarnessError(`MCP_OAUTH_ISSUER must use http or https: ${issuer}`);
  }

  const redirect = resolve(flags, "redirect", "MCP_REDIRECT", DEFAULTS.redirect);
  let redirectParsed;
  try {
    redirectParsed = new URL(redirect);
  } catch {
    throw new HarnessError(`MCP_REDIRECT is not a valid URL: ${redirect}`);
  }

  const timeoutRaw = resolve(flags, "timeout-ms", "MCP_TIMEOUT_MS", String(DEFAULTS.timeoutMs));
  const timeoutMs = Number(timeoutRaw);
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) {
    throw new HarnessError(`MCP_TIMEOUT_MS must be a positive number: ${timeoutRaw}`);
  }

  return {
    url,
    issuer,
    redirect,
    timeoutMs,
    recordDir: resolve(flags, "record-dir", "MCP_RECORD_DIR", os.tmpdir()),
  };
}

// ---------------------------------------------------------------------------
// Assertions
// ---------------------------------------------------------------------------

/**
 * L1: DCR registration succeeds with the supported OpenCode loopback redirect
 * shape (201, client_id present, no client_secret).
 */
function assertL1(dcrResp) {
  const violations = [];
  if (dcrResp.status !== 201) {
    violations.push(
      `L1: DCR registration returned HTTP ${dcrResp.status}; expected 201`
    );
    return violations;
  }
  if (dcrResp.bodyJsonParseError !== null) {
    violations.push(
      `L1: DCR response is not valid JSON: ${dcrResp.bodyJsonParseError}`
    );
    return violations;
  }
  if (!isPlainObject(dcrResp.bodyJson)) {
    violations.push("L1: DCR response is not a JSON object");
    return violations;
  }
  const body = dcrResp.bodyJson;
  if (typeof body.client_id !== "string" || body.client_id.length === 0) {
    violations.push("L1: DCR response has no client_id");
  }
  if (body.client_secret !== undefined) {
    violations.push(
      "L1: DCR response contains a client_secret; a public client must not have one"
    );
  }
  return violations;
}

/**
 * L2: Authorization Code flow with state and S256 PKCE completes (code
 * present, state round-trips).
 */
function assertL2(authResult) {
  const violations = [];
  if (!authResult) {
    violations.push(
      "L2: authorization flow was not attempted (DCR failed or issuer unreachable)"
    );
    return violations;
  }
  if (!authResult.code) {
    violations.push(
      "L2: authorization flow did not return a code"
    );
    return violations;
  }
  if (authResult.state === null) {
    violations.push(
      "L2: authorization flow did not round-trip the state parameter"
    );
  }
  return violations;
}

/**
 * L3: Token exchange completes and yields a resource-bound access token
 * (3-part JWT) without manual JWT construction.
 */
function assertL3(tokenResp) {
  const violations = [];
  if (!tokenResp) {
    violations.push(
      "L3: token exchange was not attempted (authorization flow failed)"
    );
    return violations;
  }
  if (tokenResp.status !== 200) {
    violations.push(
      `L3: token exchange returned HTTP ${tokenResp.status}; expected 200`
    );
    return violations;
  }
  if (tokenResp.bodyJsonParseError !== null) {
    violations.push(
      `L3: token exchange response is not valid JSON: ${tokenResp.bodyJsonParseError}`
    );
    return violations;
  }
  if (!isPlainObject(tokenResp.bodyJson)) {
    violations.push("L3: token exchange response is not a JSON object");
    return violations;
  }
  const body = tokenResp.bodyJson;
  if (typeof body.access_token !== "string" || body.access_token.length === 0) {
    violations.push("L3: token exchange response has no access_token");
    return violations;
  }
  const parts = body.access_token.split(".");
  if (parts.length !== 3) {
    violations.push(
      `L3: access_token is not a 3-part JWT (has ${parts.length} parts)`
    );
  }
  return violations;
}

/**
 * L4: With the access token, `initialize` succeeds (200, protocolVersion,
 * capabilities, serverInfo).
 */
function assertL4(resp) {
  const violations = [];
  if (resp.status !== 200) {
    violations.push(
      `L4: initialize returned HTTP ${resp.status}; expected 200`
    );
    return violations;
  }
  if (resp.bodyJsonParseError !== null) {
    violations.push(
      `L4: initialize response is not valid JSON: ${resp.bodyJsonParseError}`
    );
    return violations;
  }
  if (!isPlainObject(resp.bodyJson)) {
    violations.push("L4: initialize response is not a JSON object");
    return violations;
  }
  const body = resp.bodyJson;
  if (body.error !== undefined) {
    const e = body.error;
    violations.push(
      `L4: initialize returned JSON-RPC error ${e.code !== undefined ? e.code : "?"}: ${e.message || "(no message)"}`
    );
    return violations;
  }
  if (body.result === undefined) {
    violations.push("L4: initialize response has no 'result' member");
    return violations;
  }
  const result = body.result;
  if (!isPlainObject(result)) {
    violations.push("L4: initialize 'result' is not an object");
    return violations;
  }
  if (typeof result.protocolVersion !== "string") {
    violations.push("L4: result.protocolVersion is missing or not a string");
  }
  if (!isPlainObject(result.capabilities)) {
    violations.push("L4: result.capabilities is missing or not an object");
  }
  const serverInfo = result.serverInfo;
  if (!isPlainObject(serverInfo)) {
    violations.push("L4: result.serverInfo is missing or not an object");
  } else {
    if (typeof serverInfo.name !== "string") {
      violations.push("L4: result.serverInfo.name is missing or not a string");
    }
    if (typeof serverInfo.version !== "string") {
      violations.push("L4: result.serverInfo.version is missing or not a string");
    }
  }
  return violations;
}

/**
 * L5: `notifications/initialized` is accepted (202).
 */
function assertL5(resp) {
  const violations = [];
  if (resp.status !== 202) {
    violations.push(
      `L5: notifications/initialized returned HTTP ${resp.status}; expected 202`
    );
  }
  return violations;
}

/**
 * L6: `tools/list` returns the expected nine tools, each with a description
 * and a complete input schema.
 */
function assertL6(resp) {
  const violations = [];
  if (resp.status !== 200) {
    violations.push(
      `L6: tools/list returned HTTP ${resp.status}; expected 200`
    );
    return violations;
  }
  if (resp.bodyJsonParseError !== null) {
    violations.push(
      `L6: tools/list response is not valid JSON: ${resp.bodyJsonParseError}`
    );
    return violations;
  }
  if (!isPlainObject(resp.bodyJson)) {
    violations.push("L6: tools/list response is not a JSON object");
    return violations;
  }
  const body = resp.bodyJson;
  if (body.error !== undefined) {
    const e = body.error;
    violations.push(
      `L6: tools/list returned JSON-RPC error ${e.code !== undefined ? e.code : "?"}: ${e.message || "(no message)"}`
    );
    return violations;
  }
  if (body.result === undefined) {
    violations.push("L6: tools/list response has no 'result' member");
    return violations;
  }
  const result = body.result;
  if (!isPlainObject(result)) {
    violations.push("L6: tools/list 'result' is not an object");
    return violations;
  }
  const tools = result.tools;
  if (!Array.isArray(tools)) {
    violations.push("L6: tools/list result has no 'tools' array");
    return violations;
  }
  // Assert the expected nine tools are present.
  const toolNames = tools.map((t) => (isPlainObject(t) ? t.name : null));
  for (const expected of NINE_TOOLS) {
    if (!toolNames.includes(expected)) {
      violations.push(`L6: tools/list is missing the expected tool '${expected}'`);
    }
  }
  // Assert each tool has a description and a complete input schema.
  for (const tool of tools) {
    if (!isPlainObject(tool)) continue;
    const name = tool.name || "(unnamed)";
    if (typeof tool.description !== "string" || tool.description.length === 0) {
      violations.push(`L6: tool '${name}' has no description`);
    }
    if (!isPlainObject(tool.inputSchema)) {
      violations.push(`L6: tool '${name}' has no inputSchema`);
    } else {
      if (tool.inputSchema.type !== "object") {
        violations.push(`L6: tool '${name}' inputSchema.type is not 'object'`);
      }
      if (!isPlainObject(tool.inputSchema.properties)) {
        violations.push(`L6: tool '${name}' inputSchema has no 'properties' object`);
      }
    }
  }
  return violations;
}

/**
 * L7: `tools/call calendar_list` succeeds (200, content block).
 */
function assertL7(resp) {
  const violations = [];
  if (resp.status !== 200) {
    violations.push(
      `L7: tools/call calendar_list returned HTTP ${resp.status}; expected 200`
    );
    return violations;
  }
  if (resp.bodyJsonParseError !== null) {
    violations.push(
      `L7: tools/call response is not valid JSON: ${resp.bodyJsonParseError}`
    );
    return violations;
  }
  if (!isPlainObject(resp.bodyJson)) {
    violations.push("L7: tools/call response is not a JSON object");
    return violations;
  }
  const body = resp.bodyJson;
  if (body.error !== undefined) {
    const e = body.error;
    violations.push(
      `L7: tools/call calendar_list returned JSON-RPC error ${e.code !== undefined ? e.code : "?"}: ${e.message || "(no message)"}`
    );
    return violations;
  }
  if (body.result === undefined) {
    violations.push("L7: tools/call response has no 'result' member");
    return violations;
  }
  const result = body.result;
  if (!isPlainObject(result)) {
    violations.push("L7: tools/call 'result' is not an object");
    return violations;
  }
  if (!Array.isArray(result.content) || result.content.length === 0) {
    violations.push("L7: tools/call result has no non-empty 'content' array");
  }
  return violations;
}

// ---------------------------------------------------------------------------
// Record
// ---------------------------------------------------------------------------

function writeRecord(cfg, record) {
  fs.mkdirSync(cfg.recordDir, { recursive: true });
  const recordPath = path.join(cfg.recordDir, "mcp-acceptance-oauth-lifecycle.json");
  const envelope = {
    harness: HARNESS_NAME,
    harnessVersion: HARNESS_VERSION,
    phase: "0.3-oauth-lifecycle",
    target: cfg.url,
    issuer: cfg.issuer,
    redirect: cfg.redirect,
    recordedBy: "oauth-lifecycle-probe",
    record,
  };
  fs.writeFileSync(recordPath, deterministicStringify(envelope) + "\n", "utf8");
  return recordPath;
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

async function main() {
  const flags = parseArgs(process.argv.slice(2));

  if (flags.help) {
    printHelp();
    return 0;
  }

  if (flags._unknown.length > 0) {
    console.error(
      `[mcp-acceptance] CONFIG ERROR: unknown argument(s): ${flags._unknown.join(", ")}`
    );
    console.error("[mcp-acceptance] Run with --help for usage.");
    return 2;
  }

  let cfg;
  try {
    cfg = loadConfig(flags);
  } catch (err) {
    if (err instanceof HarnessError) {
      console.error(`[mcp-acceptance] CONFIG ERROR: ${err.message}`);
      return 2;
    }
    throw err;
  }

  console.log(`[mcp-acceptance] target: ${cfg.url}`);
  console.log(`[mcp-acceptance] issuer: ${cfg.issuer}`);
  console.log(`[mcp-acceptance] redirect: ${cfg.redirect}`);

  const record = {
    assertions: {},
  };

  // L1-L3: OAuth flow (DCR + Authorization Code + S256 PKCE + token exchange).
  let flow;
  try {
    flow = await runOAuthFlow(cfg, cfg.issuer, cfg.redirect);
  } catch (err) {
    if (err instanceof HarnessError) {
      console.error(`[mcp-acceptance] PROBE ERROR: ${err.message}`);
      return 2;
    }
    throw err;
  }
  record.oauthFlow = flow.record;

  const l1 = assertL1(flow.record.dcr);
  record.assertions.L1 = { pass: l1.length === 0, violations: l1 };

  const l2 = assertL2(flow.record.authorize);
  record.assertions.L2 = { pass: l2.length === 0, violations: l2 };

  const l3 = assertL3(flow.record.token);
  record.assertions.L3 = { pass: l3.length === 0, violations: l3 };

  // If the OAuth flow failed, we cannot proceed to the MCP lifecycle.
  if (!flow.accessToken) {
    const allViolations = [...l1, ...l2, ...l3];
    let recordPath = null;
    try {
      recordPath = writeRecord(cfg, record);
    } catch (err) {
      console.error(
        `[mcp-acceptance] WARNING: could not write record: ${describeError(err)}`
      );
    }
    for (const [id, list] of [["L1", l1], ["L2", l2], ["L3", l3]]) {
      if (list.length === 0) {
        console.log(`[mcp-acceptance] ${id}: PASS`);
      }
    }
    if (allViolations.length > 0) {
      console.log("");
      console.log("[mcp-acceptance] FAIL: OAuth flow did not complete");
      for (const v of allViolations) {
        console.log(`  - ${v}`);
      }
      if (recordPath) {
        console.log(`[mcp-acceptance] deterministic record: ${recordPath}`);
      }
      console.log("");
      console.log(
        "[mcp-acceptance] This failure is the intentional Phase 0.3 baseline exit"
      );
      console.log(
        "[mcp-acceptance] condition: the current production component does not"
      );
      console.log(
        "[mcp-acceptance] deploy the authorization service or implement the MCP"
      );
      console.log(
        "[mcp-acceptance] lifecycle."
      );
      return 1;
    }
    console.log("");
    console.log("[mcp-acceptance] PASS: OAuth flow completed");
    if (recordPath) {
      console.log(`[mcp-acceptance] deterministic record: ${recordPath}`);
    }
    return 0;
  }

  // L4-L7: Authenticated MCP lifecycle.
  let initializeResp;
  try {
    initializeResp = await mcpCall(
      cfg,
      cfg.url,
      "initialize",
      {
        protocolVersion: "2025-03-26",
        capabilities: {},
        clientInfo: { name: HARNESS_NAME, version: HARNESS_VERSION },
      },
      1,
      flow.accessToken
    );
  } catch (err) {
    if (err instanceof HarnessError) {
      console.error(`[mcp-acceptance] PROBE ERROR: ${err.message}`);
      return 2;
    }
    throw err;
  }
  record.initialize = initializeResp;
  const l4 = assertL4(initializeResp);
  record.assertions.L4 = { pass: l4.length === 0, violations: l4 };

  let initializedResp;
  try {
    initializedResp = await mcpNotify(
      cfg,
      cfg.url,
      "notifications/initialized",
      {},
      flow.accessToken
    );
  } catch (err) {
    if (err instanceof HarnessError) {
      console.error(`[mcp-acceptance] PROBE ERROR: ${err.message}`);
      return 2;
    }
    throw err;
  }
  record.initialized = initializedResp;
  const l5 = assertL5(initializedResp);
  record.assertions.L5 = { pass: l5.length === 0, violations: l5 };

  let toolsListResp;
  try {
    toolsListResp = await mcpCall(
      cfg,
      cfg.url,
      "tools/list",
      {},
      2,
      flow.accessToken
    );
  } catch (err) {
    if (err instanceof HarnessError) {
      console.error(`[mcp-acceptance] PROBE ERROR: ${err.message}`);
      return 2;
    }
    throw err;
  }
  record.toolsList = toolsListResp;
  const l6 = assertL6(toolsListResp);
  record.assertions.L6 = { pass: l6.length === 0, violations: l6 };

  let calendarListResp;
  try {
    calendarListResp = await mcpCall(
      cfg,
      cfg.url,
      "tools/call",
      { name: "calendar_list", arguments: {} },
      3,
      flow.accessToken
    );
  } catch (err) {
    if (err instanceof HarnessError) {
      console.error(`[mcp-acceptance] PROBE ERROR: ${err.message}`);
      return 2;
    }
    throw err;
  }
  record.calendarList = calendarListResp;
  const l7 = assertL7(calendarListResp);
  record.assertions.L7 = { pass: l7.length === 0, violations: l7 };

  let recordPath = null;
  try {
    recordPath = writeRecord(cfg, record);
  } catch (err) {
    console.error(
      `[mcp-acceptance] WARNING: could not write record: ${describeError(err)}`
    );
  }

  const allViolations = [...l1, ...l2, ...l3, ...l4, ...l5, ...l6, ...l7];
  for (const [id, list] of [
    ["L1", l1],
    ["L2", l2],
    ["L3", l3],
    ["L4", l4],
    ["L5", l5],
    ["L6", l6],
    ["L7", l7],
  ]) {
    if (list.length === 0) {
      console.log(`[mcp-acceptance] ${id}: PASS`);
    }
  }

  if (allViolations.length > 0) {
    console.log("");
    console.log("[mcp-acceptance] FAIL: OAuth + MCP lifecycle is NOT compliant");
    for (const v of allViolations) {
      console.log(`  - ${v}`);
    }
    if (recordPath) {
      console.log(`[mcp-acceptance] deterministic record: ${recordPath}`);
    }
    console.log("");
    console.log(
      "[mcp-acceptance] This failure is the intentional Phase 0.3 baseline exit"
    );
    console.log(
      "[mcp-acceptance] condition: the current production component does not"
    );
    console.log(
      "[mcp-acceptance] deploy the authorization service or implement the MCP"
    );
    console.log(
      "[mcp-acceptance] lifecycle."
    );
    return 1;
  }

  console.log("");
  console.log("[mcp-acceptance] PASS: OAuth + MCP lifecycle are compliant");
  if (recordPath) {
    console.log(`[mcp-acceptance] deterministic record: ${recordPath}`);
  }
  return 0;
}

main()
  .then((code) => {
    process.exitCode = code;
  })
  .catch((err) => {
    console.error(`[mcp-acceptance] UNEXPECTED ERROR: ${describeError(err)}`);
    if (err && err.stack) {
      console.error(err.stack);
    }
    process.exitCode = 2;
  });
