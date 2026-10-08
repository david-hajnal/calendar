#!/usr/bin/env node
"use strict";

/**
 * MCP acceptance harness — Phase 0.1 baseline.
 *
 * A standalone black-box harness that targets a configurable production-component
 * MCP endpoint and asserts that the JSON-RPC `initialize` handshake is
 * standards-compatible (MCP Streamable HTTP).
 *
 * This harness does NOT launch or depend on the slice1-lab binaries, and it does
 * not treat the lab binary as a release artifact. It reuses only the lab's
 * `initialize` request fixture shape (slice1-lab/negative_tests.py, case 10,
 * the unauthenticated MCP challenge case), captured in
 * mcp-acceptance/fixtures/initialize-request.json, and the MCP
 * standards-compliance expectations. It owns its own production-facing
 * configuration, lifecycle, reporting, and cleanup: it is a pure HTTP client,
 * starts no servers, and leaves no processes behind.
 *
 * Baseline expectation (Phase 0.1): against the current production component,
 * `initialize` is NOT standards-compatible — the custom gateway dispatcher
 * (mcp-server/src/gateway.rs) only recognizes `tools/list` and `tools/call`, so
 * `initialize` returns HTTP 400 with JSON-RPC error -32601 "Method not found".
 * The harness asserts standards-compliance and therefore exits non-zero. That
 * failure is the intentional, asserted exit condition for this phase: it
 * documents the current broken state that later phases must fix.
 *
 * Configuration (command-line flags, then environment variables, then defaults):
 *   --url / MCP_URL             MCP endpoint URL (default: https://mcal.hajnal.space/mcp)
 *   --protocol-version / MCP_PROTOCOL_VERSION  protocol version (default: 2025-03-26)
 *   --client-name / MCP_CLIENT_NAME            clientInfo.name (default: commoncal-mcp-acceptance)
 *   --client-version / MCP_CLIENT_VERSION      clientInfo.version (default: 0.1.0)
 *   --timeout-ms / MCP_TIMEOUT_MS              request timeout ms (default: 15000)
 *   --record-dir / MCP_RECORD_DIR              record directory (default: OS temp dir)
 *
 * Exit codes:
 *   0  initialize is standards-compatible (PASS)
 *   1  initialize is NOT standards-compatible (FAIL) — the intentional Phase 0.1
 *      exit condition against the current production component
 *   2  harness error (invalid configuration, network failure, unexpected error)
 */

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const HARNESS_NAME = "commoncal-mcp-acceptance";
const HARNESS_VERSION = "0.1.0";

const DEFAULTS = {
  url: "https://mcal.hajnal.space/mcp",
  protocolVersion: "2025-03-26",
  clientName: "commoncal-mcp-acceptance",
  clientVersion: "0.1.0",
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
  console.log(`Usage: node mcp-acceptance/harness.js [options]

Options:
  --url <url>              MCP endpoint URL (default: ${DEFAULTS.url})
  --protocol-version <v>   protocolVersion to negotiate (default: ${DEFAULTS.protocolVersion})
  --client-name <name>     clientInfo.name (default: ${DEFAULTS.clientName})
  --client-version <v>     clientInfo.version (default: ${DEFAULTS.clientVersion})
  --timeout-ms <n>         request timeout in ms (default: ${DEFAULTS.timeoutMs})
  --record-dir <dir>       directory for the deterministic record (default: OS temp dir)
  -h, --help               show this help

Environment variables MCP_URL, MCP_PROTOCOL_VERSION, MCP_CLIENT_NAME,
MCP_CLIENT_VERSION, MCP_TIMEOUT_MS, and MCP_RECORD_DIR are also honored;
command-line flags take precedence.
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

  const timeoutRaw = resolve(flags, "timeout-ms", "MCP_TIMEOUT_MS", String(DEFAULTS.timeoutMs));
  const timeoutMs = Number(timeoutRaw);
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) {
    throw new HarnessError(`MCP_TIMEOUT_MS must be a positive number: ${timeoutRaw}`);
  }

  return {
    url,
    protocolVersion: resolve(flags, "protocol-version", "MCP_PROTOCOL_VERSION", DEFAULTS.protocolVersion),
    clientName: resolve(flags, "client-name", "MCP_CLIENT_NAME", DEFAULTS.clientName),
    clientVersion: resolve(flags, "client-version", "MCP_CLIENT_VERSION", DEFAULTS.clientVersion),
    timeoutMs,
    recordDir: resolve(flags, "record-dir", "MCP_RECORD_DIR", os.tmpdir()),
  };
}

/**
 * Load the `initialize` request fixture.
 *
 * The fixture (mcp-acceptance/fixtures/initialize-request.json) reuses the
 * slice1-lab request shape (slice1-lab/negative_tests.py, case 10): a
 * JSON-RPC 2.0 request with `protocolVersion`, empty `capabilities`, and
 * `clientInfo`. The harness owns this copy; it does not import lab code.
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

/**
 * Build the standards-compliant MCP `initialize` request from the fixture,
 * applying the harness configuration (protocol version, client info, id).
 */
function buildInitializeRequest(cfg, requestId) {
  const fixture = loadInitializeFixture();
  return {
    ...fixture,
    id: requestId,
    params: {
      ...fixture.params,
      protocolVersion: cfg.protocolVersion,
      clientInfo: { name: cfg.clientName, version: cfg.clientVersion },
    },
  };
}

/**
 * Send the initialize request and capture the full response.
 * Returns a deterministic record of the request and response.
 */
async function probeInitialize(cfg) {
  const requestId = 1;
  const request = buildInitializeRequest(cfg, requestId);
  const body = JSON.stringify(request);

  let status;
  let headers;
  let rawBody;
  try {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), cfg.timeoutMs);
    let res;
    try {
      res = await fetch(cfg.url, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Accept: "application/json, text/event-stream",
        },
        body,
        signal: controller.signal,
      });
    } finally {
      clearTimeout(timer);
    }
    status = res.status;
    headers = normalizeHeaders(res.headers);
    rawBody = await res.text();
  } catch (err) {
    throw new HarnessError(
      `network failure contacting ${cfg.url}: ${describeError(err)}`
    );
  }

  let bodyJson = null;
  let bodyJsonParseError = null;
  try {
    bodyJson = JSON.parse(rawBody);
  } catch (err) {
    bodyJsonParseError = describeError(err);
  }

  return {
    request,
    response: {
      status,
      headers,
      bodyRaw: rawBody,
      bodyJson,
      bodyJsonParseError,
    },
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
 * Validate the initialize response against the MCP Streamable HTTP
 * standards-compliance expectations. Returns a list of violations; an empty
 * list means the response is standards-compatible.
 */
function validateInitialize(record) {
  const violations = [];
  const resp = record.response;
  const body = resp.bodyJson;

  if (resp.status !== 200) {
    violations.push(
      `HTTP status is ${resp.status}; a standards-compatible initialize must return 200`
    );
  }

  if (resp.bodyJsonParseError !== null) {
    violations.push(
      `response body is not valid JSON: ${resp.bodyJsonParseError}`
    );
    return violations;
  }

  if (!isPlainObject(body)) {
    violations.push("response body is not a JSON object");
    return violations;
  }

  if (body.jsonrpc !== "2.0") {
    violations.push(
      `jsonrpc is ${JSON.stringify(body.jsonrpc)}; expected "2.0"`
    );
  }

  if (body.id !== record.request.id) {
    violations.push(
      `response id is ${JSON.stringify(body.id)}; expected ${JSON.stringify(record.request.id)}`
    );
  }

  if (body.error !== undefined) {
    const e = body.error;
    const code = isPlainObject(e) && e.code !== undefined ? e.code : "?";
    const message = isPlainObject(e) && e.message ? e.message : "(no message)";
    violations.push(`response contains JSON-RPC error ${code}: ${message}`);
  }

  if (body.result === undefined) {
    violations.push("response has no 'result' member");
    return violations;
  }

  const result = body.result;
  if (!isPlainObject(result)) {
    violations.push("'result' is not an object");
    return violations;
  }

  if (typeof result.protocolVersion !== "string") {
    violations.push("result.protocolVersion is missing or not a string");
  }

  if (!isPlainObject(result.capabilities)) {
    violations.push("result.capabilities is missing or not an object");
  }

  const serverInfo = result.serverInfo;
  if (!isPlainObject(serverInfo)) {
    violations.push("result.serverInfo is missing or not an object");
  } else {
    if (typeof serverInfo.name !== "string") {
      violations.push("result.serverInfo.name is missing or not a string");
    }
    if (typeof serverInfo.version !== "string") {
      violations.push("result.serverInfo.version is missing or not a string");
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
  const recordPath = path.join(cfg.recordDir, "mcp-acceptance-initialize.json");
  const envelope = {
    harness: HARNESS_NAME,
    harnessVersion: HARNESS_VERSION,
    target: cfg.url,
    recordedBy: "baseline-initialize-probe",
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

  console.log(`[mcp-acceptance] target: ${cfg.url}`);
  console.log(`[mcp-acceptance] protocolVersion: ${cfg.protocolVersion}`);

  let record;
  try {
    record = await probeInitialize(cfg);
  } catch (err) {
    if (err instanceof HarnessError) {
      console.error(`[mcp-acceptance] PROBE ERROR: ${err.message}`);
      return 2;
    }
    throw err;
  }

  const violations = validateInitialize(record);
  let recordPath = null;
  try {
    recordPath = writeRecord(cfg, record);
  } catch (err) {
    // Recording is best-effort; a failure to write the record must not mask the
    // actual standards-compliance result.
    console.error(
      `[mcp-acceptance] WARNING: could not write record: ${describeError(err)}`
    );
  }

  const resp = record.response;
  console.log(`[mcp-acceptance] response status: ${resp.status}`);
  if (resp.bodyJsonParseError !== null) {
    console.log(`[mcp-acceptance] response body (non-JSON): ${resp.bodyRaw}`);
  } else if (isPlainObject(resp.bodyJson)) {
    console.log(
      `[mcp-acceptance] response body: ${deterministicStringify(resp.bodyJson)}`
    );
  } else {
    console.log(`[mcp-acceptance] response body: ${resp.bodyRaw}`);
  }

  if (violations.length > 0) {
    console.log("");
    console.log(
      "[mcp-acceptance] FAIL: initialize is NOT standards-compatible"
    );
    for (const v of violations) {
      console.log(`  - ${v}`);
    }
    if (recordPath) {
      console.log(`[mcp-acceptance] deterministic record: ${recordPath}`);
    }
    console.log("");
    console.log(
      "[mcp-acceptance] This failure is the intentional Phase 0.1 baseline exit"
    );
    console.log(
      "[mcp-acceptance] condition: the current production component does not"
    );
    console.log(
      "[mcp-acceptance] implement a standards-compatible initialize handshake."
    );
    return 1;
  }

  console.log("");
  console.log("[mcp-acceptance] PASS: initialize is standards-compatible");
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
