#!/usr/bin/env node
"use strict";

/**
 * MCP acceptance harness — Phase 0.2 discovery baseline.
 *
 * A standalone black-box harness that targets a configurable production-component
 * MCP endpoint and asserts the public OAuth discovery contract:
 *
 *   A1  an unauthenticated JSON-RPC `initialize` request receives HTTP 401
 *       (an OAuth challenge), not a protocol-dispatch error (e.g. HTTP 400
 *       with JSON-RPC -32601 "Method not found");
 *   A2  the 401 challenge carries a Bearer `WWW-Authenticate` header whose
 *       `resource_metadata` parameter is the configured public
 *       protected-resource metadata URL (the MCP endpoint origin plus
 *       `/.well-known/oauth-protected-resource`) and never a loopback or lab
 *       URL (e.g. `http://127.0.0.1:3001/...`);
 *   A3  the protected-resource metadata document parses as JSON and names the
 *       configured authorization-server issuer in `authorization_servers`;
 *   A4  the authorization-server metadata document parses as JSON and supplies
 *       usable `authorization_endpoint`, `token_endpoint`,
 *       `registration_endpoint`, and `jwks_uri` values (absolute http(s)
 *       URLs; https when the issuer is public).
 *
 * Baseline expectation (Phase 0.2): against the current production component,
 * A1, A2, and A4 FAIL and A3 passes. Verified current state (2026-09-17,
 * docs/MCP-PRODUCTION-FIX-PLAN.md):
 *   - `initialize` returns HTTP 400 with JSON-RPC -32601 (mcp-server/src/gateway.rs
 *     only recognizes tools/list and tools/call), so no 401 challenge is ever
 *     issued for the initial request;
 *   - the 401 builders that do exist hard-code the lab loopback URL
 *     `http://127.0.0.1:3001` (mcp-server/src/gateway.rs:456-541);
 *   - `https://mcal.hajnal.space/.well-known/oauth-protected-resource` is valid
 *     JSON advertising `https://cal.hajnal.space` (A3 passes);
 *   - `https://cal.hajnal.space/.well-known/openid-configuration` returns the
 *     CommonCal SPA HTML, not authorization-server metadata (A4 fails).
 *
 * The harness asserts the target contract and therefore exits non-zero against
 * the current production component. That failure is the intentional, asserted
 * exit condition for this phase: it documents the current broken state that
 * later phases must fix. The harness is a regression gate: it stays red until
 * the production component implements the public challenge and discovery.
 *
 * This harness does NOT launch or depend on the slice1-lab binaries, and it
 * does not treat the lab binary as a release artifact. It reuses the Phase 0.1
 * `initialize` request fixture (mcp-acceptance/fixtures/initialize-request.json)
 * and the MCP/OAuth standards-compliance expectations. It is a pure HTTP
 * client: it starts no servers and leaves no processes behind.
 *
 * Configuration (command-line flags, then environment variables, then defaults):
 *   --url / MCP_URL             MCP endpoint URL (default: https://mcal.hajnal.space/mcp)
 *   --issuer / MCP_OAUTH_ISSUER configured authorization-server issuer
 *                               (default: https://cal.hajnal.space)
 *   --timeout-ms / MCP_TIMEOUT_MS              request timeout ms (default: 15000)
 *   --record-dir / MCP_RECORD_DIR              record directory (default: OS temp dir)
 *
 * Exit codes:
 *   0  all discovery assertions pass (PASS)
 *   1  one or more discovery assertions fail (FAIL) — the intentional Phase 0.2
 *      baseline exit condition against the current production component
 *   2  harness error (invalid configuration, network failure, unexpected error)
 */

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const HARNESS_NAME = "commoncal-mcp-acceptance";
const HARNESS_VERSION = "0.2.0";

const DEFAULTS = {
  url: "https://mcal.hajnal.space/mcp",
  issuer: "https://cal.hajnal.space",
  timeoutMs: 15000,
};

class HarnessError extends Error {}

function env(name, fallback) {
  const v = process.env[name];
  return v === undefined || v === "" ? fallback : v;
}

/**
 * Parse command-line flags. Returns a map of flag -> value. Unknown flags are
 * collected under the `_unknown` key so they can be reported as a config error.
 */
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
  console.log(`Usage: node mcp-acceptance/discovery.js [options]

Options:
  --url <url>              MCP endpoint URL (default: ${DEFAULTS.url})
  --issuer <url>           configured authorization-server issuer (default: ${DEFAULTS.issuer})
  --timeout-ms <n>         request timeout in ms (default: ${DEFAULTS.timeoutMs})
  --record-dir <dir>       directory for the deterministic record (default: OS temp dir)
  -h, --help               show this help

Environment variables MCP_URL, MCP_OAUTH_ISSUER, MCP_TIMEOUT_MS, and
MCP_RECORD_DIR are also honored; command-line flags take precedence.
`);
}

function describeError(err) {
  if (err === undefined || err === null) return "unknown error";
  if (typeof err === "string") return err;
  return err.message || String(err);
}

/**
 * Resolve a configuration value from (in order of precedence): command-line
 * flag, environment variable, default.
 */
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

  const timeoutRaw = resolve(flags, "timeout-ms", "MCP_TIMEOUT_MS", String(DEFAULTS.timeoutMs));
  const timeoutMs = Number(timeoutRaw);
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) {
    throw new HarnessError(`MCP_TIMEOUT_MS must be a positive number: ${timeoutRaw}`);
  }

  return {
    url,
    issuer,
    timeoutMs,
    recordDir: resolve(flags, "record-dir", "MCP_RECORD_DIR", os.tmpdir()),
  };
}

/**
 * The configured public protected-resource metadata URL: the MCP endpoint
 * origin plus the RFC 9728 well-known path. This mirrors the production
 * configuration logic (mcp-server/src/config.rs) and the fix-plan requirement
 * to derive resource_metadata from MCP_PUBLIC_RESOURCE_URL.
 */
function expectedResourceMetadataUrl(mcpUrl) {
  const u = new URL(mcpUrl);
  return `${u.origin}/.well-known/oauth-protected-resource`;
}

/**
 * True for loopback hosts: the 127.0.0.0/8 range, ::1, localhost, and 0.0.0.0.
 * The current production 401 builders hard-code the lab loopback URL
 * http://127.0.0.1:3001 (mcp-server/src/gateway.rs:456-541); this is the
 * "loopback or lab URL" the A2 assertion must reject.
 */
function isLoopbackHost(host) {
  if (!host) return false;
  const h = host.toLowerCase();
  if (h === "localhost" || h === "::1" || h === "0.0.0.0") return true;
  if (h === "127.0.0.1" || h.startsWith("127.")) return true;
  return false;
}

function isLoopbackUrl(rawUrl) {
  try {
    return isLoopbackHost(new URL(rawUrl).hostname);
  } catch {
    return false;
  }
}

/**
 * Load the `initialize` request fixture shared with the Phase 0.1 harness.
 */
function loadInitializeFixture() {
  const fixturePath = path.join(__dirname, "fixtures", "initialize-request.json");
  let raw;
  try {
    raw = fs.readFileSync(fixturePath, "utf8");
  } catch (err) {
    throw new HarnessError(
      `cannot read initialize fixture ${fixturePath}: ${describeError(err)}`
    );
  }
  let fixture;
  try {
    fixture = JSON.parse(raw);
  } catch (err) {
    throw new HarnessError(
      `initialize fixture ${fixturePath} is not valid JSON: ${describeError(err)}`
    );
  }
  if (
    !isPlainObject(fixture) ||
    fixture.method !== "initialize" ||
    !isPlainObject(fixture.params)
  ) {
    throw new HarnessError(
      `initialize fixture ${fixturePath} must be a JSON-RPC initialize request with params`
    );
  }
  return fixture;
}

function buildInitializeRequest() {
  const fixture = loadInitializeFixture();
  return {
    ...fixture,
    id: 1,
    params: {
      ...fixture.params,
      clientInfo: { name: HARNESS_NAME, version: HARNESS_VERSION },
    },
  };
}

/**
 * Perform one HTTP request and capture the full response deterministically.
 */
async function httpProbe(cfg, method, url, body) {
  const headers = {};
  let bodyString = null;
  if (body !== null && body !== undefined) {
    bodyString = typeof body === "string" ? body : JSON.stringify(body);
    headers["Content-Type"] = "application/json";
  }
  if (method === "POST") {
    headers.Accept = "application/json, text/event-stream";
  }

  let status;
  let respHeaders;
  let rawBody;
  try {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), cfg.timeoutMs);
    let res;
    try {
      res = await fetch(url, {
        method,
        headers,
        body: bodyString,
        signal: controller.signal,
      });
    } finally {
      clearTimeout(timer);
    }
    status = res.status;
    respHeaders = normalizeHeaders(res.headers);
    rawBody = await res.text();
  } catch (err) {
    throw new HarnessError(`network failure contacting ${url}: ${describeError(err)}`);
  }

  let bodyJson = null;
  let bodyJsonParseError = null;
  try {
    bodyJson = JSON.parse(rawBody);
  } catch (err) {
    bodyJsonParseError = describeError(err);
  }

  return {
    status,
    headers: respHeaders,
    bodyRaw: rawBody,
    bodyJson,
    bodyJsonParseError,
  };
}

/**
 * Normalize response headers into a plain object, dropping volatile /
 * hop-by-hop headers so the recorded response is deterministic.
 */
function normalizeHeaders(headers) {
  const skip = new Set([
    "date",
    "x-request-id",
    "set-cookie",
    "transfer-encoding",
    "connection",
    "keep-alive",
  ]);
  const out = {};
  for (const [key, value] of headers.entries()) {
    const k = key.toLowerCase();
    if (skip.has(k)) continue;
    if (k in out) {
      out[k] = [].concat(out[k], value);
    } else {
      out[k] = value;
    }
  }
  return out;
}

function isPlainObject(v) {
  return v !== null && typeof v === "object" && !Array.isArray(v);
}

/**
 * Extract the resource_metadata parameter from a WWW-Authenticate header
 * value. Returns the raw parameter value or null when absent.
 */
function extractResourceMetadata(wwwAuthenticate) {
  if (typeof wwwAuthenticate !== "string") return null;
  const m = wwwAuthenticate.match(/resource_metadata\s*=\s*"([^"]*)"/i);
  if (m) return m[1];
  const bare = wwwAuthenticate.match(/resource_metadata\s*=\s*([^\s,]+)/i);
  if (bare) return bare[1];
  return null;
}

/**
 * A1: an unauthenticated JSON-RPC initialize request must receive HTTP 401
 * (an OAuth challenge), not a protocol-dispatch error.
 */
function assertA1(resp) {
  const violations = [];
  if (resp.status !== 401) {
    const detail = [];
    if (resp.bodyJsonParseError !== null) {
      detail.push(`body is not valid JSON (${resp.bodyJsonParseError})`);
    } else if (isPlainObject(resp.bodyJson) && resp.bodyJson.error) {
      const e = resp.bodyJson.error;
      detail.push(
        `JSON-RPC error ${e.code !== undefined ? e.code : "?"}: ${e.message || "(no message)"}`
      );
    }
    violations.push(
      `A1: unauthenticated initialize returned HTTP ${resp.status}${
        detail.length > 0 ? ` (${detail.join("; ")})` : ""
      }; expected HTTP 401 (OAuth challenge), not a protocol-dispatch error`
    );
  }
  return violations;
}

/**
 * A2: the 401 challenge must carry a Bearer WWW-Authenticate header whose
 * resource_metadata parameter is the configured public protected-resource
 * metadata URL, and never a loopback or lab URL.
 */
function assertA2(resp, expectedUrl, expectedIsLoopback) {
  const violations = [];
  const www = resp.headers["www-authenticate"];
  if (www === undefined) {
    violations.push(
      "A2: 401 response has no WWW-Authenticate header; expected a Bearer challenge with a resource_metadata parameter"
    );
    return violations;
  }
  if (!/^Bearer\s/i.test(www)) {
    violations.push(
      `A2: WWW-Authenticate is not a Bearer challenge: ${JSON.stringify(www)}`
    );
    return violations;
  }
  const resourceMetadata = extractResourceMetadata(www);
  if (resourceMetadata === null) {
    violations.push(
      `A2: Bearer challenge has no resource_metadata parameter: ${JSON.stringify(www)}`
    );
    return violations;
  }
  if (resourceMetadata !== expectedUrl) {
    violations.push(
      `A2: challenge resource_metadata is ${JSON.stringify(resourceMetadata)}; expected the configured public URL ${JSON.stringify(expectedUrl)}`
    );
  }
  if (!expectedIsLoopback) {
    if (isLoopbackUrl(resourceMetadata)) {
      violations.push(
        `A2: challenge resource_metadata points at a loopback/lab URL (${JSON.stringify(resourceMetadata)}); it must be the public URL`
      );
    }
    try {
      if (new URL(resourceMetadata).protocol !== "https:") {
        violations.push(
          `A2: challenge resource_metadata is not HTTPS: ${JSON.stringify(resourceMetadata)}`
        );
      }
    } catch {
      violations.push(
        `A2: challenge resource_metadata is not an absolute URL: ${JSON.stringify(resourceMetadata)}`
      );
    }
  }
  return violations;
}

/**
 * A3: the protected-resource metadata document must parse as JSON and name
 * the configured authorization-server issuer.
 */
function assertA3(doc, issuer) {
  const violations = [];
  if (doc.status !== 200) {
    violations.push(
      `A3: protected-resource metadata returned HTTP ${doc.status}; expected 200`
    );
    return violations;
  }
  if (doc.bodyJsonParseError !== null) {
    violations.push(
      `A3: protected-resource metadata is not valid JSON: ${doc.bodyJsonParseError}`
    );
    return violations;
  }
  if (!isPlainObject(doc.bodyJson)) {
    violations.push("A3: protected-resource metadata is not a JSON object");
    return violations;
  }
  const servers = doc.bodyJson.authorization_servers;
  if (!Array.isArray(servers) || servers.length === 0) {
    violations.push(
      "A3: protected-resource metadata has no non-empty authorization_servers array"
    );
    return violations;
  }
  if (!servers.every((s) => typeof s === "string")) {
    violations.push(
      "A3: protected-resource metadata authorization_servers contains non-string entries"
    );
  }
  if (!servers.includes(issuer)) {
    violations.push(
      `A3: protected-resource metadata authorization_servers ${JSON.stringify(servers)} does not name the configured issuer ${JSON.stringify(issuer)}`
    );
  }
  return violations;
}

/**
 * A4: the authorization-server metadata document must parse as JSON and
 * supply usable authorization, token, registration, and JWKS endpoints.
 */
function assertA4(doc, issuer, issuerIsLoopback) {
  const violations = [];
  if (doc.status !== 200) {
    violations.push(
      `A4: authorization-server metadata at ${doc.url} returned HTTP ${doc.status}; expected 200`
    );
    return violations;
  }
  if (doc.bodyJsonParseError !== null) {
    const snippet = (doc.bodyRaw || "").replace(/\s+/g, " ").slice(0, 80);
    violations.push(
      `A4: authorization-server metadata at ${doc.url} is not valid JSON (${doc.bodyJsonParseError}); body starts with ${JSON.stringify(snippet)}`
    );
    return violations;
  }
  if (!isPlainObject(doc.bodyJson)) {
    violations.push(
      `A4: authorization-server metadata at ${doc.url} is not a JSON object`
    );
    return violations;
  }
  const meta = doc.bodyJson;
  for (const key of [
    "authorization_endpoint",
    "token_endpoint",
    "registration_endpoint",
    "jwks_uri",
  ]) {
    const value = meta[key];
    if (typeof value !== "string" || value.length === 0) {
      violations.push(
        `A4: authorization-server metadata at ${doc.url} is missing a usable ${key}`
      );
      continue;
    }
    let parsed;
    try {
      parsed = new URL(value);
    } catch {
      violations.push(
        `A4: ${key} is not an absolute URL: ${JSON.stringify(value)}`
      );
      continue;
    }
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
      violations.push(
        `A4: ${key} does not use http or https: ${JSON.stringify(value)}`
      );
      continue;
    }
    if (!issuerIsLoopback && parsed.protocol !== "https:") {
      violations.push(
        `A4: ${key} is not HTTPS for a public issuer: ${JSON.stringify(value)}`
      );
    }
  }
  return violations;
}

/**
 * Serialize a value to deterministic JSON (object keys sorted recursively).
 */
function deterministicStringify(value) {
  function sort(value) {
    if (Array.isArray(value)) return value.map(sort);
    if (isPlainObject(value)) {
      const out = {};
      for (const key of Object.keys(value).sort()) {
        out[key] = sort(value[key]);
      }
      return out;
    }
    return value;
  }
  return JSON.stringify(sort(value), null, 2);
}

function writeRecord(cfg, record) {
  fs.mkdirSync(cfg.recordDir, { recursive: true });
  const recordPath = path.join(cfg.recordDir, "mcp-acceptance-discovery.json");
  const envelope = {
    harness: HARNESS_NAME,
    harnessVersion: HARNESS_VERSION,
    phase: "0.2-discovery",
    target: cfg.url,
    issuer: cfg.issuer,
    recordedBy: "discovery-probe",
    record,
  };
  fs.writeFileSync(recordPath, deterministicStringify(envelope) + "\n", "utf8");
  return recordPath;
}

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

  const expectedResourceUrl = expectedResourceMetadataUrl(cfg.url);
  const expectedIsLoopback = isLoopbackUrl(expectedResourceUrl);
  const issuerIsLoopback = isLoopbackUrl(cfg.issuer);

  console.log(`[mcp-acceptance] target: ${cfg.url}`);
  console.log(`[mcp-acceptance] issuer: ${cfg.issuer}`);
  console.log(`[mcp-acceptance] expected resource_metadata: ${expectedResourceUrl}`);

  const record = {
    assertions: {},
  };

  // A1 + A2: unauthenticated initialize must receive the public 401 challenge.
  let initializeResp;
  try {
    initializeResp = await httpProbe(
      cfg,
      "POST",
      cfg.url,
      buildInitializeRequest()
    );
  } catch (err) {
    if (err instanceof HarnessError) {
      console.error(`[mcp-acceptance] PROBE ERROR: ${err.message}`);
      return 2;
    }
    throw err;
  }
  record.initialize = {
    request: buildInitializeRequest(),
    response: initializeResp,
  };

  const a1 = assertA1(initializeResp);
  const a2 = assertA2(initializeResp, expectedResourceUrl, expectedIsLoopback);
  record.assertions.A1 = { pass: a1.length === 0, violations: a1 };
  record.assertions.A2 = { pass: a2.length === 0, violations: a2 };

  // A3: the configured public protected-resource metadata document.
  let resourceDoc;
  try {
    resourceDoc = await httpProbe(cfg, "GET", expectedResourceUrl, null);
  } catch (err) {
    if (err instanceof HarnessError) {
      console.error(`[mcp-acceptance] PROBE ERROR: ${err.message}`);
      return 2;
    }
    throw err;
  }
  resourceDoc.url = expectedResourceUrl;
  record.protectedResourceMetadata = {
    url: expectedResourceUrl,
    response: resourceDoc,
  };
  const a3 = assertA3(resourceDoc, cfg.issuer);
  record.assertions.A3 = { pass: a3.length === 0, violations: a3 };

  // A4: the authorization-server metadata document. Prefer the RFC 8414
  // location; fall back to the OIDC discovery location where appropriate.
  const asCandidates = [
    `${cfg.issuer.replace(/\/$/, "")}/.well-known/oauth-authorization-server`,
    `${cfg.issuer.replace(/\/$/, "")}/.well-known/openid-configuration`,
  ];
  let asDoc = null;
  const asAttempts = [];
  for (const candidate of asCandidates) {
    let doc;
    try {
      doc = await httpProbe(cfg, "GET", candidate, null);
    } catch (err) {
      if (err instanceof HarnessError) {
        console.error(`[mcp-acceptance] PROBE ERROR: ${err.message}`);
        return 2;
      }
      throw err;
    }
    doc.url = candidate;
    asAttempts.push(doc);
    if (doc.status === 200 && doc.bodyJsonParseError === null && isPlainObject(doc.bodyJson)) {
      asDoc = doc;
      break;
    }
  }
  record.authorizationServerMetadata = {
    attempts: asAttempts,
    used: asDoc ? asDoc.url : null,
  };
  const a4 = asDoc === null
    ? [
        `A4: no authorization-server metadata document found; tried ${asCandidates.join(", ")} (all non-200 or non-JSON)`,
      ]
    : assertA4(asDoc, cfg.issuer, issuerIsLoopback);
  record.assertions.A4 = { pass: a4.length === 0, violations: a4 };

  let recordPath = null;
  try {
    recordPath = writeRecord(cfg, record);
  } catch (err) {
    console.error(
      `[mcp-acceptance] WARNING: could not write record: ${describeError(err)}`
    );
  }

  const allViolations = [...a1, ...a2, ...a3, ...a4];
  for (const [id, list] of [
    ["A1", a1],
    ["A2", a2],
    ["A3", a3],
    ["A4", a4],
  ]) {
    if (list.length === 0) {
      console.log(`[mcp-acceptance] ${id}: PASS`);
    }
  }

  if (allViolations.length > 0) {
    console.log("");
    console.log("[mcp-acceptance] FAIL: public challenge / OAuth discovery is NOT compliant");
    for (const v of allViolations) {
      console.log(`  - ${v}`);
    }
    if (recordPath) {
      console.log(`[mcp-acceptance] deterministic record: ${recordPath}`);
    }
    console.log("");
    console.log(
      "[mcp-acceptance] This failure is the intentional Phase 0.2 baseline exit"
    );
    console.log(
      "[mcp-acceptance] condition: the current production component does not"
    );
    console.log(
      "[mcp-acceptance] issue a public 401 challenge or serve authorization-"
    );
    console.log(
      "[mcp-acceptance] server metadata."
    );
    return 1;
  }

  console.log("");
  console.log("[mcp-acceptance] PASS: public challenge and OAuth discovery are compliant");
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
