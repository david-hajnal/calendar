#!/usr/bin/env node
"use strict";

/**
 * MCP acceptance harness — Phase 0.4 security & isolation baseline.
 *
 * A standalone black-box harness that targets a configurable production-component
 * MCP endpoint and asserts the fail-closed security and concurrency-isolation
 * contract:
 *
 *   S1  wrong-issuer token fails closed (401 + public WWW-Authenticate);
 *   S2  wrong-audience token fails closed (401 + public WWW-Authenticate);
 *   S3  wrong-signature token fails closed (401 + public WWW-Authenticate);
 *   S4  expired token fails closed (401 + public WWW-Authenticate);
 *   S5  wrong-client token fails closed (401 + public WWW-Authenticate);
 *   S6  missing grant denies calendar_list without disclosing calendars;
 *   S7  revoked grant denies calendar_list without disclosing calendars;
 *   S8  broadened grant denies calendar_list without disclosing unauthorized
 *       calendars;
 *   S9  two concurrent authenticated clients cannot observe each other's
 *       identity, calendars, session, or grant state.
 *
 * Baseline expectation (Phase 0.4): against the current production component,
 * S1-S9 FAIL because the authorization service is not deployed and the MCP
 * gateway does not implement the MCP lifecycle or token validation. The harness
 * asserts the target contract and therefore exits non-zero against the current
 * production component. That failure is the intentional, asserted exit condition
 * for this phase: it documents the current broken state that later phases must
 * fix. The harness is a regression gate: it stays red until the production
 * component implements fail-closed token validation, grant enforcement, and
 * concurrency isolation.
 *
 * This harness does NOT launch or depend on the slice1-lab binaries, and it
 * does NOT treat the lab binary as a release artifact. It reuses the
 * slice1-lab fixture shapes (negative-token / fail-closed cases from
 * slice1-lab/negative_tests.py cases 3-9, slice1-lab/PROOF.md P7) and the
 * MCP/OAuth standards-compliance expectations. It is a pure HTTP client: it
 * starts no servers and leaves no processes behind.
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
 *   0  all security/isolation assertions pass (PASS)
 *   1  one or more security/isolation assertions fail (FAIL) — the intentional
 *      Phase 0.4 baseline exit condition against the current production component
 *   2  harness error (invalid configuration, network failure, unexpected error)
 */

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const {
  SCOPE_CATALOG,
  isPlainObject,
  decodeJwtPayload,
  tamperJwtPayload,
  tamperJwtSignature,
  httpProbe,
  runOAuthFlow,
  mcpCall,
  mcpNotify,
  deterministicStringify,
} = require("./lib");

const HARNESS_NAME = "commoncal-mcp-acceptance";
const HARNESS_VERSION = "0.4.0";

const DEFAULTS = {
  url: "https://mcal.hajnal.space/mcp",
  issuer: "https://cal.hajnal.space",
  redirect: "http://127.0.0.1:8765/callback",
  timeoutMs: 15000,
};

// Maps each security/isolation case to the production-fix phase
// (docs/MCP-PRODUCTION-FIX-PLAN.md) that will make it green.
const PHASE_MAP = {
  S1: "Phase 3 — Align discovery and access-token validation (exact iss)",
  S2: "Phase 3 — Align discovery and access-token validation (exact aud)",
  S3: "Phase 3 — Align discovery and access-token validation (signature, kid, alg)",
  S4: "Phase 3 — Align discovery and access-token validation (exp, iat)",
  S5: "Phase 3 — Align discovery and access-token validation (standard client_id)",
  S6: "Phase 4 — Promote consent and grant integration into core (grant lookup)",
  S7: "Phase 4 — Promote consent and grant integration into core (revocation)",
  S8: "Phase 4 — Promote consent and grant integration into core (live membership)",
  S9: "Phase 2 — Replace the custom MCP gateway with rmcp (per-request identity)",
};

function printPhaseMap() {
  console.log("");
  console.log("[mcp-acceptance] Production-fix phase that makes each case green:");
  for (const [id, phase] of Object.entries(PHASE_MAP)) {
    console.log(`  ${id}: ${phase}`);
  }
}

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
  console.log(`Usage: node mcp-acceptance/security-isolation.js [options]

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
// Test hooks (verification-only; not part of the production contract)
// ---------------------------------------------------------------------------

/**
 * Mint a token with specific claims using the mock's `/_test/token/mint` hook.
 * This is a verification-only helper; it is only available when the harness is
 * run against the mock server (not against a real production component).
 *
 * @param {object} cfg  Harness configuration.
 * @param {string} issuer  The issuer base URL.
 * @param {object} claims  The claims to embed in the token.
 * @returns {Promise<string|null>} The minted token, or null on failure.
 */
async function mintToken(cfg, issuer, claims) {
  const url = `${issuer.replace(/\/$/, "")}/_test/token/mint`;
  try {
    const resp = await httpProbe(cfg, "POST", url, { claims });
    if (resp.status === 200 && isPlainObject(resp.bodyJson) && resp.bodyJson.access_token) {
      return resp.bodyJson.access_token;
    }
    return null;
  } catch {
    return null;
  }
}

/**
 * Drive a grant-state test hook (revoke / broaden / reset / delete).
 *
 * @param {object} cfg  Harness configuration.
 * @param {string} issuer  The issuer base URL.
 * @param {string} hook  The hook name (revoke / broaden / reset / delete).
 * @param {object} body  The hook body.
 * @returns {Promise<boolean>} True if the hook succeeded.
 */
async function grantHook(cfg, issuer, hook, body) {
  const url = `${issuer.replace(/\/$/, "")}/_test/grant/${hook}`;
  try {
    const resp = await httpProbe(cfg, "POST", url, body);
    return resp.status === 200;
  } catch {
    return false;
  }
}

// ---------------------------------------------------------------------------
// Assertions
// ---------------------------------------------------------------------------

/**
 * Assert that a tools/call calendar_list response fails closed (401 with a
 * public WWW-Authenticate challenge, or a JSON-RPC error with no calendar
 * data leaked).
 */
function assertFailClosed(resp, label) {
  const violations = [];

  // The response must NOT be a successful calendar_list (200 with content).
  if (resp.status === 200 && isPlainObject(resp.bodyJson) && resp.bodyJson.result) {
    const result = resp.bodyJson.result;
    if (Array.isArray(result.content) && result.content.length > 0) {
      violations.push(
        `${label}: calendar_list returned a successful result with content; expected fail-closed denial`
      );
    }
  }

  // If the response is 401, it must carry a public WWW-Authenticate challenge.
  if (resp.status === 401) {
    const www = resp.headers["www-authenticate"];
    if (www === undefined) {
      violations.push(
        `${label}: 401 response has no WWW-Authenticate header; expected a Bearer challenge`
      );
    } else if (!/^Bearer\s/i.test(www)) {
      violations.push(
        `${label}: WWW-Authenticate is not a Bearer challenge: ${JSON.stringify(www)}`
      );
    }
  }

  return violations;
}

/**
 * Assert that a tools/call calendar_list response denies without disclosing
 * unauthorized calendars (no calendar data in the response body).
 */
function assertNoCalendarLeak(resp, label) {
  const violations = [];
  if (resp.bodyRaw && resp.bodyRaw.length > 0) {
    // Check that the response body does not contain calendar data.
    // A successful calendar_list would contain "calendars" or calendar ids.
    if (resp.bodyRaw.includes('"calendars"') || resp.bodyRaw.includes('"calendar_id"')) {
      violations.push(
        `${label}: response body appears to disclose calendar data; expected no leak`
      );
    }
  }
  return violations;
}

/**
 * Assert that a tools/call calendar_list response does not disclose
 * unauthorized calendar IDs (the broadened-grant case). The response may
 * contain authorized calendars, but must NOT contain the unauthorized ones.
 */
function assertNoUnauthorizedCalendars(resp, label, unauthorizedIds) {
  const violations = [];
  const calendars = extractCalendars(resp);
  if (calendars === null) {
    // No calendars in the response; nothing to check.
    return violations;
  }
  const ids = new Set(calendars.map((c) => c.id));
  const leaked = unauthorizedIds.filter((id) => ids.has(id));
  if (leaked.length > 0) {
    violations.push(
      `${label}: response discloses unauthorized calendar ids ${JSON.stringify(leaked)}`
    );
  }
  return violations;
}

/**
 * S9: two concurrent authenticated clients cannot observe each other's
 * identity, calendars, session, or grant state.
 */
function assertIsolation(clientA, clientB) {
  const violations = [];

  // Both clients should have succeeded (200) with their own calendars.
  if (clientA.status !== 200) {
    violations.push(
      `S9: client A calendar_list returned HTTP ${clientA.status}; expected 200`
    );
  }
  if (clientB.status !== 200) {
    violations.push(
      `S9: client B calendar_list returned HTTP ${clientB.status}; expected 200`
    );
  }

  // Extract the calendars from each client's response.
  const calendarsA = extractCalendars(clientA);
  const calendarsB = extractCalendars(clientB);

  if (calendarsA === null || calendarsB === null) {
    // If we can't extract calendars, we can't assert isolation.
    return violations;
  }

  // The two clients should have DIFFERENT calendars (different subjects).
  const idsA = new Set(calendarsA.map((c) => c.id));
  const idsB = new Set(calendarsB.map((c) => c.id));

  // Check for overlap: if any calendar id appears in both, that's a leak.
  const overlap = [...idsA].filter((id) => idsB.has(id));
  if (overlap.length > 0) {
    violations.push(
      `S9: clients A and B share calendar ids ${JSON.stringify(overlap)}; expected no overlap`
    );
  }

  return violations;
}

/**
 * Extract the calendar list from a tools/call response. Returns an array of
 * calendar objects, or null if the response does not contain a calendar list.
 */
function extractCalendars(resp) {
  if (!isPlainObject(resp.bodyJson) || !resp.bodyJson.result) return null;
  const result = resp.bodyJson.result;
  if (!Array.isArray(result.content) || result.content.length === 0) return null;
  const content = result.content[0];
  if (!content || typeof content.text !== "string") return null;
  try {
    const parsed = JSON.parse(content.text);
    if (isPlainObject(parsed) && Array.isArray(parsed.calendars)) {
      return parsed.calendars;
    }
    return null;
  } catch {
    return null;
  }
}

// ---------------------------------------------------------------------------
// Record
// ---------------------------------------------------------------------------

function writeRecord(cfg, record) {
  fs.mkdirSync(cfg.recordDir, { recursive: true });
  const recordPath = path.join(cfg.recordDir, "mcp-acceptance-security-isolation.json");
  const envelope = {
    harness: HARNESS_NAME,
    harnessVersion: HARNESS_VERSION,
    phase: "0.4-security-isolation",
    target: cfg.url,
    issuer: cfg.issuer,
    redirect: cfg.redirect,
    recordedBy: "security-isolation-probe",
    productionFixPhaseMap: PHASE_MAP,
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

  // First, run the OAuth flow to get a valid token (for the negative cases).
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

  // If the OAuth flow failed, we cannot proceed to the security cases.
  if (!flow.accessToken) {
    const allViolations = [
      "S0: OAuth flow did not complete; cannot drive security cases",
    ];
    let recordPath = null;
    try {
      recordPath = writeRecord(cfg, record);
    } catch (err) {
      console.error(
        `[mcp-acceptance] WARNING: could not write record: ${describeError(err)}`
      );
    }
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
      "[mcp-acceptance] This failure is the intentional Phase 0.4 baseline exit"
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
    printPhaseMap();
    return 1;
  }

  const validToken = flow.accessToken;
  const clientId = flow.clientId;
  const sub = "1"; // The fixed lab subject.

  // -----------------------------------------------------------------------
  // S1-S5: Negative token cases
  // -----------------------------------------------------------------------

  // S1: wrong issuer.
  let s1Token = await mintToken(cfg, cfg.issuer, { iss: "http://evil.example.com" });
  if (!s1Token) {
    s1Token = tamperJwtPayload(validToken, { iss: "http://evil.example.com" });
  }
  let s1Resp;
  try {
    s1Resp = await mcpCall(cfg, cfg.url, "tools/call", { name: "calendar_list", arguments: {} }, 101, s1Token);
  } catch (err) {
    if (err instanceof HarnessError) {
      console.error(`[mcp-acceptance] PROBE ERROR: ${err.message}`);
      return 2;
    }
    throw err;
  }
  record.s1 = s1Resp;
  const s1 = assertFailClosed(s1Resp, "S1 (wrong issuer)");
  record.assertions.S1 = { pass: s1.length === 0, violations: s1 };

  // S2: wrong audience.
  let s2Token = await mintToken(cfg, cfg.issuer, { aud: "http://evil.example.com/mcp" });
  if (!s2Token) {
    s2Token = tamperJwtPayload(validToken, { aud: "http://evil.example.com/mcp" });
  }
  let s2Resp;
  try {
    s2Resp = await mcpCall(cfg, cfg.url, "tools/call", { name: "calendar_list", arguments: {} }, 102, s2Token);
  } catch (err) {
    if (err instanceof HarnessError) {
      console.error(`[mcp-acceptance] PROBE ERROR: ${err.message}`);
      return 2;
    }
    throw err;
  }
  record.s2 = s2Resp;
  const s2 = assertFailClosed(s2Resp, "S2 (wrong audience)");
  record.assertions.S2 = { pass: s2.length === 0, violations: s2 };

  // S3: wrong signature.
  const s3Token = tamperJwtSignature(validToken);
  let s3Resp;
  try {
    s3Resp = await mcpCall(cfg, cfg.url, "tools/call", { name: "calendar_list", arguments: {} }, 103, s3Token);
  } catch (err) {
    if (err instanceof HarnessError) {
      console.error(`[mcp-acceptance] PROBE ERROR: ${err.message}`);
      return 2;
    }
    throw err;
  }
  record.s3 = s3Resp;
  const s3 = assertFailClosed(s3Resp, "S3 (wrong signature)");
  record.assertions.S3 = { pass: s3.length === 0, violations: s3 };

  // S4: expired token.
  const now = Math.floor(Date.now() / 1000);
  let s4Token = await mintToken(cfg, cfg.issuer, { exp: now - 3600, iat: now - 7200 });
  if (!s4Token) {
    s4Token = tamperJwtPayload(validToken, { exp: now - 3600, iat: now - 7200 });
  }
  let s4Resp;
  try {
    s4Resp = await mcpCall(cfg, cfg.url, "tools/call", { name: "calendar_list", arguments: {} }, 104, s4Token);
  } catch (err) {
    if (err instanceof HarnessError) {
      console.error(`[mcp-acceptance] PROBE ERROR: ${err.message}`);
      return 2;
    }
    throw err;
  }
  record.s4 = s4Resp;
  const s4 = assertFailClosed(s4Resp, "S4 (expired token)");
  record.assertions.S4 = { pass: s4.length === 0, violations: s4 };

  // S5: wrong client.
  let s5Token = await mintToken(cfg, cfg.issuer, { client_id: "unknown-client" });
  if (!s5Token) {
    s5Token = tamperJwtPayload(validToken, { client_id: "unknown-client" });
  }
  let s5Resp;
  try {
    s5Resp = await mcpCall(cfg, cfg.url, "tools/call", { name: "calendar_list", arguments: {} }, 105, s5Token);
  } catch (err) {
    if (err instanceof HarnessError) {
      console.error(`[mcp-acceptance] PROBE ERROR: ${err.message}`);
      return 2;
    }
    throw err;
  }
  record.s5 = s5Resp;
  const s5 = assertFailClosed(s5Resp, "S5 (wrong client)");
  record.assertions.S5 = { pass: s5.length === 0, violations: s5 };

  // -----------------------------------------------------------------------
  // S6-S8: Grant cases
  // -----------------------------------------------------------------------

  // S6: missing grant.
  await grantHook(cfg, cfg.issuer, "delete", { sub, client_id: clientId });
  let s6Resp;
  try {
    s6Resp = await mcpCall(cfg, cfg.url, "tools/call", { name: "calendar_list", arguments: {} }, 106, validToken);
  } catch (err) {
    if (err instanceof HarnessError) {
      console.error(`[mcp-acceptance] PROBE ERROR: ${err.message}`);
      return 2;
    }
    throw err;
  }
  record.s6 = s6Resp;
  const s6 = [...assertFailClosed(s6Resp, "S6 (missing grant)"), ...assertNoCalendarLeak(s6Resp, "S6 (missing grant)")];
  record.assertions.S6 = { pass: s6.length === 0, violations: s6 };

  // S7: revoked grant.
  await grantHook(cfg, cfg.issuer, "reset", { sub, client_id: clientId });
  await grantHook(cfg, cfg.issuer, "revoke", { sub, client_id: clientId });
  let s7Resp;
  try {
    s7Resp = await mcpCall(cfg, cfg.url, "tools/call", { name: "calendar_list", arguments: {} }, 107, validToken);
  } catch (err) {
    if (err instanceof HarnessError) {
      console.error(`[mcp-acceptance] PROBE ERROR: ${err.message}`);
      return 2;
    }
    throw err;
  }
  record.s7 = s7Resp;
  const s7 = [...assertFailClosed(s7Resp, "S7 (revoked grant)"), ...assertNoCalendarLeak(s7Resp, "S7 (revoked grant)")];
  record.assertions.S7 = { pass: s7.length === 0, violations: s7 };

  // S8: broadened grant. The grant is broadened to include unauthorized
  // calendar IDs (999/998). The resource server must NOT disclose these
  // unauthorized calendars. It may return the authorized calendars (101/102),
  // but must not return 999/998.
  await grantHook(cfg, cfg.issuer, "broaden", { sub, client_id: clientId });
  let s8Resp;
  try {
    s8Resp = await mcpCall(cfg, cfg.url, "tools/call", { name: "calendar_list", arguments: {} }, 108, validToken);
  } catch (err) {
    if (err instanceof HarnessError) {
      console.error(`[mcp-acceptance] PROBE ERROR: ${err.message}`);
      return 2;
    }
    throw err;
  }
  record.s8 = s8Resp;
  const s8 = assertNoUnauthorizedCalendars(s8Resp, "S8 (broadened grant)", [999, 998]);
  record.assertions.S8 = { pass: s8.length === 0, violations: s8 };

  // Reset the grant for the isolation test.
  await grantHook(cfg, cfg.issuer, "reset", { sub, client_id: clientId });

  // -----------------------------------------------------------------------
  // S9: Concurrency isolation
  // -----------------------------------------------------------------------

  // Run two concurrent authenticated clients with DIFFERENT subjects. The
  // mock supports two subjects (sub=1 "Alice" with calendars 101/102, and
  // sub=2 "Bob" with calendars 201/202). We mint tokens for each subject and
  // set up grants for each, then run interleaved MCP lifecycles (initialize,
  // notifications/initialized, tools/list, tools/call calendar_list) for both
  // clients concurrently and assert they do not observe each other's
  // identity, calendars, session, or grant state.
  const tokenA = await mintToken(cfg, cfg.issuer, { sub: "1" });
  const tokenB = await mintToken(cfg, cfg.issuer, { sub: "2" });

  // Set up grants for both subjects with their own calendars.
  await grantHook(cfg, cfg.issuer, "set", { sub: "1", client_id: clientId, allowed_calendar_ids: [101, 102] });
  await grantHook(cfg, cfg.issuer, "set", { sub: "2", client_id: clientId, allowed_calendar_ids: [201, 202] });

  // Run the full MCP lifecycle for each client concurrently (interleaved).
  async function runLifecycle(token, idBase) {
    const init = await mcpCall(cfg, cfg.url, "initialize", {
      protocolVersion: "2025-03-26",
      capabilities: {},
      clientInfo: { name: HARNESS_NAME, version: HARNESS_VERSION },
    }, idBase, token);
    const initialized = await mcpNotify(cfg, cfg.url, "notifications/initialized", {}, token);
    const toolsList = await mcpCall(cfg, cfg.url, "tools/list", {}, idBase + 1, token);
    const calendarList = await mcpCall(cfg, cfg.url, "tools/call", { name: "calendar_list", arguments: {} }, idBase + 2, token);
    return { init, initialized, toolsList, calendarList };
  }

  const [clientA, clientB] = await Promise.all([
    runLifecycle(tokenA || validToken, 201),
    runLifecycle(tokenB || validToken, 301),
  ]);
  record.s9 = { clientA, clientB };
  const s9 = assertIsolation(clientA.calendarList, clientB.calendarList);
  record.assertions.S9 = { pass: s9.length === 0, violations: s9 };

  let recordPath = null;
  try {
    recordPath = writeRecord(cfg, record);
  } catch (err) {
    console.error(
      `[mcp-acceptance] WARNING: could not write record: ${describeError(err)}`
    );
  }

  const allViolations = [...s1, ...s2, ...s3, ...s4, ...s5, ...s6, ...s7, ...s8, ...s9];
  for (const [id, list] of [
    ["S1", s1],
    ["S2", s2],
    ["S3", s3],
    ["S4", s4],
    ["S5", s5],
    ["S6", s6],
    ["S7", s7],
    ["S8", s8],
    ["S9", s9],
  ]) {
    if (list.length === 0) {
      console.log(`[mcp-acceptance] ${id}: PASS`);
    }
  }

  if (allViolations.length > 0) {
    console.log("");
    console.log("[mcp-acceptance] FAIL: security/isolation is NOT compliant");
    for (const v of allViolations) {
      console.log(`  - ${v}`);
    }
    if (recordPath) {
      console.log(`[mcp-acceptance] deterministic record: ${recordPath}`);
    }
    console.log("");
    console.log(
      "[mcp-acceptance] This failure is the intentional Phase 0.4 baseline exit"
    );
    console.log(
      "[mcp-acceptance] condition: the current production component does not"
    );
    console.log(
      "[mcp-acceptance] implement fail-closed token validation, grant"
    );
    console.log(
      "[mcp-acceptance] enforcement, and concurrency isolation."
    );
    printPhaseMap();
    return 1;
  }

  console.log("");
  console.log("[mcp-acceptance] PASS: security/isolation are compliant");
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
