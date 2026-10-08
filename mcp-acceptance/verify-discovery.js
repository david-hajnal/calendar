#!/usr/bin/env node
"use strict";

/**
 * Self-contained verification for the Phase 0.2 MCP discovery harness.
 *
 * This script proves discovery.js behaves correctly on BOTH paths:
 *
 *   1. FAIL path — against a server that exhibits the CURRENT production
 *      behavior (initialize -> HTTP 400, JSON-RPC -32601, no
 *      WWW-Authenticate), the harness must exit 1 with focused violations on
 *      A1 (401 challenge), A2 (public resource_metadata), and A4
 *      (authorization-server metadata), while A3 (protected-resource metadata
 *      naming the configured issuer) passes — matching the verified current
 *      production state in docs/MCP-PRODUCTION-FIX-PLAN.md.
 *   2. PASS path — against a server that issues the public 401 challenge and
 *      serves valid protected-resource and authorization-server metadata, the
 *      harness must exit 0.
 *
 * It starts the verification-only mock server (mock/server.js) as a child
 * process, runs the harness (discovery.js) as a child process against each
 * endpoint, asserts the expected exit codes and focused violations, and
 * cleans up the mock server.
 *
 * The mock mirrors production's two-host topology: the MCP host (MOCK_PORT,
 * default 3998) serves the MCP endpoint and the protected-resource metadata;
 * the issuer host (MOCK_PORT + 1) serves the authorization-server metadata
 * (SPA HTML in the current mode, valid JSON in the compliant mode).
 *
 * This is a test harness for the harness; it is not a release artifact and
 * does not touch production.
 *
 * Run:  node mcp-acceptance/verify-discovery.js
 * Exit: 0 if both paths behave as expected, 1 otherwise.
 */

const { spawn, execFile } = require("node:child_process");
const path = require("node:path");

const HERE = __dirname;
const MOCK = path.join(HERE, "mock", "server.js");
const HARNESS = path.join(HERE, "discovery.js");

function flagValue(argv, name, fallback) {
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === `--${name}`) return argv[i + 1];
    if (arg.startsWith(`--${name}=`)) return arg.slice(name.length + 3);
  }
  const envVal = process.env[name.toUpperCase()];
  return envVal !== undefined && envVal !== "" ? envVal : fallback;
}

const PORT = Number(flagValue(process.argv.slice(2), "port", process.env.MOCK_PORT || 3998));
const MCP_BASE = `http://127.0.0.1:${PORT}`;
const ISSUER_BASE = `http://127.0.0.1:${PORT + 1}`;

function log(msg) {
  console.log(`[verify-discovery] ${msg}`);
}

function runHarness(url, issuer) {
  return new Promise((resolve) => {
    execFile(
      process.execPath,
      [HARNESS, "--url", url, "--issuer", issuer, "--timeout-ms", "5000"],
      { timeout: 20000 },
      (error, stdout, stderr) => {
        // execFile sets error when the exit code is non-zero; error.code holds
        // the numeric exit status in that case.
        const exitCode =
          error === null ? 0 : typeof error.code === "number" ? error.code : -1;
        resolve({ exitCode, stdout, stderr });
      }
    );
  });
}

async function waitForServer(url, timeoutMs = 10000) {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    try {
      const res = await fetch(url);
      void res.status;
      await res.arrayBuffer();
      return true;
    } catch {
      // Not up yet; wait and retry.
      await new Promise((r) => setTimeout(r, 100));
    }
  }
  return false;
}

async function main() {
  log(`starting mock server (MCP host ${MCP_BASE}, issuer host ${ISSUER_BASE})`);
  const mock = spawn(process.execPath, [MOCK], {
    env: { ...process.env, MOCK_PORT: String(PORT) },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let mockOutput = "";
  mock.stdout.on("data", (d) => {
    mockOutput += d.toString();
  });
  mock.stderr.on("data", (d) => {
    mockOutput += d.toString();
  });

  const cleanup = () => {
    if (mock.exitCode === null && mock.signalCode === null) {
      mock.kill("SIGTERM");
    }
  };
  process.on("exit", cleanup);
  process.on("SIGINT", () => {
    cleanup();
    process.exit(130);
  });

  const up =
    (await waitForServer(`${MCP_BASE}/.well-known/oauth-protected-resource`)) &&
    (await waitForServer(`${ISSUER_BASE}/.well-known/openid-configuration`));
  if (!up) {
    log("ERROR: mock server did not become ready");
    log(mockOutput);
    cleanup();
    return 1;
  }
  log("mock server is ready");

  let allPassed = true;
  const check = (label, ok) => {
    log(`  ${label} -> ${ok ? "OK" : "MISMATCH"}`);
    if (!ok) allPassed = false;
  };

  // 1. FAIL path: current-production behavior -> harness must exit 1 with
  //    focused A1/A2/A4 violations and a passing A3.
  log("FAIL path: harness vs current-production behavior (expect exit 1)");
  const fail = await runHarness(`${MCP_BASE}/mcp-discovery-current`, ISSUER_BASE);
  check(`harness exit code: ${fail.exitCode} (expected 1)`, fail.exitCode === 1);
  check(
    "violation A1 (401 challenge) present",
    fail.stdout.includes("A1:") && fail.stdout.includes("expected HTTP 401")
  );
  check(
    "violation A2 (public resource_metadata) present",
    fail.stdout.includes("A2:") && fail.stdout.includes("resource_metadata")
  );
  check(
    "violation A4 (authorization-server metadata) present",
    fail.stdout.includes("A4:")
  );
  check("A3 (protected-resource metadata issuer) passes", fail.stdout.includes("A3: PASS"));

  // 2. PASS path: compliant challenge + discovery -> harness must exit 0.
  log("PASS path: harness vs compliant discovery server (expect exit 0)");
  const pass = await runHarness(`${MCP_BASE}/mcp-discovery-compliant`, ISSUER_BASE);
  check(`harness exit code: ${pass.exitCode} (expected 0)`, pass.exitCode === 0);
  check("A1 passes", pass.stdout.includes("A1: PASS"));
  check("A2 passes", pass.stdout.includes("A2: PASS"));
  check("A3 passes", pass.stdout.includes("A3: PASS"));
  check("A4 passes", pass.stdout.includes("A4: PASS"));

  cleanup();

  log("");
  if (allPassed) {
    log("VERIFY PASS: discovery harness exits 1 on current-production behavior (A1/A2/A4 red, A3 green) and 0 on compliant behavior.");
    return 0;
  }
  log("VERIFY FAIL: discovery harness behavior did not match expectations.");
  return 1;
}

main()
  .then((code) => {
    process.exitCode = code;
  })
  .catch((err) => {
    console.error(`[verify-discovery] UNEXPECTED ERROR: ${err && err.message ? err.message : String(err)}`);
    if (err && err.stack) console.error(err.stack);
    process.exitCode = 1;
  });
