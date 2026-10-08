#!/usr/bin/env node
"use strict";

/**
 * Self-contained verification for the Phase 0.4 MCP security-isolation harness.
 *
 * This script proves security-isolation.js behaves correctly on BOTH paths:
 *
 *   1. FAIL path — against a server that exhibits the CURRENT production
 *      behavior (DCR returns SPA HTML, no OAuth flow), the harness must exit 1
 *      with a focused violation (S0: OAuth flow did not complete).
 *   2. PASS path — against a server that implements fail-closed token
 *      validation, grant enforcement, and concurrency isolation, the harness
 *      must exit 0.
 *
 * It starts the verification-only mock server (mock/lifecycle-server.js) as a
 * child process, runs the harness (security-isolation.js) as a child process
 * against each endpoint, asserts the expected exit codes, and cleans up the
 * mock server.
 *
 * This is a test harness for the harness; it is not a release artifact and
 * does not touch production.
 *
 * Run:  node mcp-acceptance/verify-security-isolation.js
 * Exit: 0 if both paths behave as expected, 1 otherwise.
 */

const { spawn, execFile } = require("node:child_process");
const path = require("node:path");

const HERE = __dirname;
const MOCK = path.join(HERE, "mock", "lifecycle-server.js");
const HARNESS = path.join(HERE, "security-isolation.js");

function flagValue(argv, name, fallback) {
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === `--${name}`) return argv[i + 1];
    if (arg.startsWith(`--${name}=`)) return arg.slice(name.length + 3);
  }
  const envVal = process.env[name.toUpperCase()];
  return envVal !== undefined && envVal !== "" ? envVal : fallback;
}

const PORT = Number(flagValue(process.argv.slice(2), "port", process.env.MOCK_PORT || 4003));
const MCP_BASE = `http://127.0.0.1:${PORT}`;
const ISSUER_BASE = `http://127.0.0.1:${PORT + 1}`;

function log(msg) {
  console.log(`[verify-security-isolation] ${msg}`);
}

function runHarness(url, issuer) {
  return new Promise((resolve) => {
    execFile(
      process.execPath,
      [HARNESS, "--url", url, "--issuer", issuer, "--timeout-ms", "5000"],
      { timeout: 20000 },
      (error, stdout, stderr) => {
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
      await new Promise((r) => setTimeout(r, 100));
    }
  }
  return false;
}

function startMock(world) {
  return new Promise((resolve) => {
    const mock = spawn(process.execPath, [MOCK], {
      env: { ...process.env, MOCK_PORT: String(PORT), MOCK_WORLD: world },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let mockOutput = "";
    mock.stdout.on("data", (d) => {
      mockOutput += d.toString();
    });
    mock.stderr.on("data", (d) => {
      mockOutput += d.toString();
    });
    resolve({ mock, getOutput: () => mockOutput });
  });
}

async function main() {
  let allPassed = true;
  const check = (label, ok) => {
    log(`  ${label} -> ${ok ? "OK" : "MISMATCH"}`);
    if (!ok) allPassed = false;
  };

  // 1. FAIL path: current-production behavior -> harness must exit 1.
  log("FAIL path: harness vs current-production behavior (expect exit 1)");
  const { mock: mockFail, getOutput: getFailOutput } = await startMock("current");
  const upFail = await waitForServer(`${MCP_BASE}/.well-known/oauth-protected-resource`);
  if (!upFail) {
    log("ERROR: mock server (current) did not become ready");
    log(getFailOutput());
    mockFail.kill("SIGTERM");
    return 1;
  }
  const fail = await runHarness(`${MCP_BASE}/mcp`, ISSUER_BASE);
  check(`harness exit code: ${fail.exitCode} (expected 1)`, fail.exitCode === 1);
  check(
    "violation S0 (OAuth flow) present",
    fail.stdout.includes("S0:")
  );
  check(
    "production-fix phase map reported",
    fail.stdout.includes("Production-fix phase that makes each case green")
  );
  check(
    "phase map names S1",
    fail.stdout.includes("S1: Phase")
  );
  check(
    "phase map names S9",
    fail.stdout.includes("S9: Phase")
  );
  mockFail.kill("SIGTERM");
  await new Promise((r) => setTimeout(r, 200));

  // 2. PASS path: compliant behavior -> harness must exit 0.
  log("PASS path: harness vs compliant security/isolation server (expect exit 0)");
  const { mock: mockPass, getOutput: getPassOutput } = await startMock("compliant");
  const upPass = await waitForServer(`${MCP_BASE}/.well-known/oauth-protected-resource`);
  if (!upPass) {
    log("ERROR: mock server (compliant) did not become ready");
    log(getPassOutput());
    mockPass.kill("SIGTERM");
    return 1;
  }
  const pass = await runHarness(`${MCP_BASE}/mcp`, ISSUER_BASE);
  check(`harness exit code: ${pass.exitCode} (expected 0)`, pass.exitCode === 0);
  check("S1 passes", pass.stdout.includes("S1: PASS"));
  check("S2 passes", pass.stdout.includes("S2: PASS"));
  check("S3 passes", pass.stdout.includes("S3: PASS"));
  check("S4 passes", pass.stdout.includes("S4: PASS"));
  check("S5 passes", pass.stdout.includes("S5: PASS"));
  check("S6 passes", pass.stdout.includes("S6: PASS"));
  check("S7 passes", pass.stdout.includes("S7: PASS"));
  check("S8 passes", pass.stdout.includes("S8: PASS"));
  check("S9 passes", pass.stdout.includes("S9: PASS"));
  mockPass.kill("SIGTERM");
  await new Promise((r) => setTimeout(r, 200));

  log("");
  if (allPassed) {
    log("VERIFY PASS: security-isolation harness exits 1 on current-production behavior and 0 on compliant behavior.");
    return 0;
  }
  log("VERIFY FAIL: security-isolation harness behavior did not match expectations.");
  return 1;
}

main()
  .then((code) => {
    process.exitCode = code;
  })
  .catch((err) => {
    console.error(`[verify-security-isolation] UNEXPECTED ERROR: ${err && err.message ? err.message : String(err)}`);
    if (err && err.stack) console.error(err.stack);
    process.exitCode = 1;
  });
