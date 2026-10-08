#!/usr/bin/env node
"use strict";

/**
 * Self-contained verification for the Phase 0.1 MCP acceptance harness.
 *
 * This script proves the harness behaves correctly on BOTH paths:
 *
 *   1. FAIL path — against a server that exhibits the CURRENT production
 *      behavior (initialize -> HTTP 400, JSON-RPC -32601), the harness must
 *      exit 1. This is the intentional Phase 0.1 baseline exit condition.
 *   2. PASS path — against a standards-compatible server, the harness must
 *      exit 0.
 *
 * It starts the verification-only mock server (mock/server.js) as a child
 * process, runs the harness (harness.js) as a child process against each
 * endpoint, asserts the expected exit codes, and cleans up the mock server.
 *
 * This is a test harness for the harness; it is not a release artifact and does
 * not touch production.
 *
 * Run:  node mcp-acceptance/verify.js
 * Exit: 0 if both paths behave as expected, 1 otherwise.
 */

const { spawn, execFile } = require("node:child_process");
const path = require("node:path");

const HERE = __dirname;
const MOCK = path.join(HERE, "mock", "server.js");
const HARNESS = path.join(HERE, "harness.js");

function flagValue(argv, name, fallback) {
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === `--${name}`) return argv[i + 1];
    if (arg.startsWith(`--${name}=`)) return arg.slice(name.length + 3);
  }
  const envVal = process.env[name.toUpperCase()];
  return envVal !== undefined && envVal !== "" ? envVal : fallback;
}

const PORT = Number(flagValue(process.argv.slice(2), "port", process.env.MOCK_PORT || 3999));
const BASE = `http://127.0.0.1:${PORT}`;

function log(msg) {
  console.log(`[verify] ${msg}`);
}

function runHarness(url) {
  return new Promise((resolve) => {
    execFile(
      process.execPath,
      [HARNESS, "--url", url, "--timeout-ms", "5000"],
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
      const res = await fetch(url, { method: "POST", body: "{}" });
      // Any HTTP response (even 400/405) means the server is up.
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
  log(`starting mock server on ${BASE}`);
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

  const up = await waitForServer(`${BASE}/mcp-compliant`);
  if (!up) {
    log("ERROR: mock server did not become ready");
    log(mockOutput);
    cleanup();
    return 1;
  }
  log("mock server is ready");

  let allPassed = true;

  // 1. FAIL path: current-production behavior -> harness must exit 1.
  log("FAIL path: harness vs current-production behavior (expect exit 1)");
  const fail = await runHarness(`${BASE}/mcp-current`);
  const failOk = fail.exitCode === 1;
  log(`  harness exit code: ${fail.exitCode} (expected 1) -> ${failOk ? "OK" : "MISMATCH"}`);
  if (!failOk) allPassed = false;

  // 2. PASS path: standards-compatible behavior -> harness must exit 0.
  log("PASS path: harness vs standards-compatible server (expect exit 0)");
  const pass = await runHarness(`${BASE}/mcp-compliant`);
  const passOk = pass.exitCode === 0;
  log(`  harness exit code: ${pass.exitCode} (expected 0) -> ${passOk ? "OK" : "MISMATCH"}`);
  if (!passOk) allPassed = false;

  cleanup();

  log("");
  if (allPassed) {
    log("VERIFY PASS: harness exits 1 on current-production behavior and 0 on standards-compatible behavior.");
    return 0;
  }
  log("VERIFY FAIL: harness exit codes did not match expectations.");
  return 1;
}

main()
  .then((code) => {
    process.exitCode = code;
  })
  .catch((err) => {
    console.error(`[verify] UNEXPECTED ERROR: ${err && err.message ? err.message : String(err)}`);
    if (err && err.stack) console.error(err.stack);
    process.exitCode = 1;
  });
