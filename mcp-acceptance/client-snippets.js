#!/usr/bin/env node
"use strict";

// Run the README's non-interactive client snippets against a loopback fixture.
// This verifies real CLI/config syntax, not production OAuth compatibility.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const http = require("node:http");
const { spawn } = require("node:child_process");

function run(command, args, options) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { ...options, detached: true, stdio: ["ignore", "pipe", "pipe"] });
    let output = "";
    child.stdout.on("data", (data) => { output += data; });
    child.stderr.on("data", (data) => { output += data; });
    const timer = setTimeout(() => {
      try { process.kill(-child.pid, "SIGKILL"); } catch { /* Already exited. */ }
    }, 20000);
    child.on("error", (error) => { clearTimeout(timer); reject(error); });
    child.on("close", (code) => {
      clearTimeout(timer);
      resolve({ code, output });
    });
  });
}

async function main() {
  const client = process.argv[2];
  assert.ok(["opencode", "codex", "claude"].includes(client),
    "Usage: node mcp-acceptance/client-snippets.js opencode|codex|claude");
  const root = await fs.realpath(await fs.mkdtemp(path.join(os.tmpdir(), "commoncal-client-")));
  const server = http.createServer();
  let base;
  let challenge = false;
  let registration;
  const failures = [];
  server.on("request", async (req, res) => {
    const url = new URL(req.url, base);
    const json = (status, data, headers = {}) => {
      res.writeHead(status, { "content-type": "application/json", ...headers });
      res.end(JSON.stringify(data));
    };
    if (url.pathname.startsWith("/.well-known/oauth-protected-resource")) {
      if (!challenge) return json(404, {});
      return json(200, { resource: `${base}/mcp`, authorization_servers: [base] });
    }
    if (url.pathname === "/.well-known/oauth-authorization-server" || url.pathname === "/.well-known/openid-configuration") {
      if (!challenge) return json(404, {});
      return json(200, { issuer: base, authorization_endpoint: `${base}/authorize`,
        token_endpoint: `${base}/token`, registration_endpoint: `${base}/register`,
        response_types_supported: ["code"], grant_types_supported: ["authorization_code", "refresh_token"],
        code_challenge_methods_supported: ["S256"], token_endpoint_auth_methods_supported: ["none"] });
    }
    let body = "";
    for await (const chunk of req) body += chunk;
    if (url.pathname === "/register") {
      registration = JSON.parse(body);
      // Observe DCR without opening a browser or issuing any credentials.
      return json(400, { error: "invalid_client_metadata", error_description: "callback observation complete" });
    }
    if (url.pathname !== "/mcp") return json(404, {});
    if (challenge) return json(401, {}, {
      "www-authenticate": `Bearer resource_metadata="${base}/.well-known/oauth-protected-resource"`,
    });
    if (req.method !== "POST") { res.writeHead(405); return res.end(); }
    const rpc = JSON.parse(body);
    if (rpc.id === undefined) { res.writeHead(202); return res.end(); }
    let result;
    switch (rpc.method) {
      case "initialize": result = { protocolVersion: rpc.params.protocolVersion,
        capabilities: { tools: {} }, serverInfo: { name: "commoncal-snippet-fixture", version: "1" } }; break;
      case "tools/list": result = { tools: [{ name: "calendar_list", description: "Fixture calendars",
        inputSchema: { type: "object", properties: {} } }] }; break;
      case "resources/list": result = { resources: [] }; break;
      case "prompts/list": result = { prompts: [] }; break;
      case "ping": result = {}; break;
      default: return json(200, { jsonrpc: "2.0", id: rpc.id, error: { code: -32601, message: "Method not found" } });
    }
    json(200, { jsonrpc: "2.0", id: rpc.id, result });
  });
  try {
    await new Promise((resolve, reject) => {
      server.once("error", reject);
      server.listen(0, "127.0.0.1", resolve);
    });
    base = `http://127.0.0.1:${server.address().port}`;
    const env = { ...process.env, CODEX_HOME: path.join(root, "codex"),
      CLAUDE_CONFIG_DIR: path.join(root, "claude"),
      XDG_CONFIG_HOME: path.join(root, "config"), XDG_DATA_HOME: path.join(root, "data"),
      XDG_CACHE_HOME: path.join(root, "cache"), XDG_STATE_HOME: path.join(root, "state"),
      OPENCODE_TEST_HOME: root, OPENCODE_CONFIG: path.join(root, "config", "opencode", "opencode.json") };
    delete env.OPENCODE_CONFIG_CONTENT;
    delete env.OPENCODE_CLI_CONFIG_CONTENT;
    for (const key of ["CODEX_HOME", "CLAUDE_CONFIG_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_STATE_HOME"]) {
      await fs.mkdir(env[key], { recursive: true });
    }
    // Avoid inheriting user API credentials; these checks need none.
    for (const key of Object.keys(env)) {
      if (/TOKEN|SECRET|API_KEY/.test(key)) delete env[key];
    }
    env.XDG_RUNTIME_DIR = path.join(root, "runtime");
    await fs.mkdir(env.XDG_RUNTIME_DIR, { mode: 0o700 });
    const options = { cwd: root, env };
    const version = await run(client, ["--version"], options);
    assert.equal(version.code, 0, `${client} must be installed`);
    console.log(version.output.trim());
    if (client === "opencode") {
      const disabled = await run(client, ["service", "set", "disabled", "true"], options);
      assert.equal(disabled.code, 0, disabled.output);
    }
    const readme = await fs.readFile(path.join(__dirname, "..", "README.md"), "utf8");
    const snippets = [...readme.matchAll(/<!-- mcp-snippet: (\w+) (\w+) -->\s*```sh\n([\s\S]*?)```/g)]
      .filter((match) => match[1] === client);
    assert.deepEqual(snippets.map((match) => match[2]), ["add", "list", "remove"]);
    for (const [, , phase, snippet] of snippets) {
      const command = snippet.trim().replaceAll("https://mcal.hajnal.space/mcp", `${base}/mcp`);
      const executed = await run("/bin/sh", ["-eu", "-c", command], options);
      if (executed.code !== 0 || (phase === "list" && !executed.output.includes("commoncal"))) {
        failures.push(`${client} ${phase}: ${executed.output.trim() || "command timed out"}`);
        console.log(`FAIL README ${client} ${phase}`);
        if (phase === "add") break;
      } else {
        console.log(`PASS README ${client} ${phase}`);
      }
      if (phase === "add" && client === "opencode") {
        const config = JSON.parse(await fs.readFile(env.OPENCODE_CONFIG, "utf8"));
        assert.ok(config.mcp?.servers?.commoncal, "Expected mcp.servers.commoncal");
        assert.equal(config.mcp.servers.commoncal.type, "remote");
        assert.equal(config.mcp.servers.commoncal.url, `${base}/mcp`);
        const match = readme.match(/<!-- mcp-config: opencode -->\s*```json\n([\s\S]*?)```/);
        assert.ok(match, "README must contain deterministic OpenCode config");
        const documented = JSON.parse(match[1].replaceAll("https://mcal.hajnal.space/mcp", `${base}/mcp`));
        assert.deepEqual(documented.mcp, config.mcp);
        await fs.writeFile(env.OPENCODE_CONFIG, JSON.stringify(documented));
        console.log("PASS README OpenCode JSON matches CLI-generated config");
      }
      if (phase === "list" && client === "opencode") {
        challenge = true;
        await run(client, ["mcp", "auth", "commoncal"], options);
        if (!registration) {
          failures.push("OpenCode auth did not reach fixture DCR; callback remains unobserved");
          challenge = false;
          continue;
        }
        assert.equal(registration.token_endpoint_auth_method, "none");
        const { validateRedirect, OPENCODE_V2_CALLBACK } = await import("../slice1-lab/auth-server/src/dcr-policy.mjs");
        assert.ok(registration.redirect_uris.length > 0);
        for (const uri of registration.redirect_uris) {
          assert.ok(validateRedirect(uri, [OPENCODE_V2_CALLBACK]),
            `Observed callback is outside the production shape: ${uri}`);
          console.log(`PASS observed OpenCode callback ${uri}`);
        }
        challenge = false;
      }
    }
    if (client === "opencode") {
      const config = JSON.parse(await fs.readFile(env.OPENCODE_CONFIG, "utf8"));
      delete config.mcp.servers.commoncal;
      await fs.writeFile(env.OPENCODE_CONFIG, JSON.stringify(config));
    }
    const after = await run(client, ["mcp", "list"], options);
    assert.equal(after.code, 0);
    assert.doesNotMatch(after.output, /commoncal/, "removal must remove the server");
    console.log("PASS removal; temporary profiles only. Live OAuth/calendar proof is separate.");
    assert.equal(failures.length, 0, failures.join("\n"));
  } finally {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
    await fs.rm(root, { recursive: true, force: true });
  }
}

main().catch((error) => { console.error(error.message); process.exitCode = 1; });
