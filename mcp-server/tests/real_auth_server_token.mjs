// Test helper: launch the repository's real authorization-server implementation
// (slice1-lab/auth-server/src/server.mjs), drive the full OAuth flow
// (DCR + Authorization Code + S256 PKCE + CommonCal consent via the private
// bridge + token exchange), and print the issued access token to stdout as JSON.
//
// This is NOT a hand-built fixture: the token is signed by the real auth server
// using its configured JWKS key. The Rust integration test spawns this helper,
// reads the token, and validates it with the mcp-server's TokenValidator.
//
// Output (stdout, single JSON line):
//   { ok, issuer, resource, client_id, access_token, claims }
//
// Exit codes: 0 success; non-zero on any failure (details on stderr).
import { spawn } from 'node:child_process';
import { setTimeout as sleep } from 'node:timers/promises';
import { createHash, randomBytes } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(here, '..', '..');

const ISSUER = process.env.REAL_AUTH_ISSUER ?? 'http://127.0.0.1:4000';
const RESOURCE = process.env.REAL_AUTH_RESOURCE ?? 'http://127.0.0.1:3001/mcp';
const REDIRECT = process.env.REAL_AUTH_REDIRECT ?? 'http://127.0.0.1:8321/callback';
const BRIDGE = process.env.REAL_AUTH_BRIDGE ?? 'http://127.0.0.1:4001';
const BRIDGE_KEY = process.env.REAL_AUTH_BRIDGE_KEY ?? 'slice1-loopback-bridge-key';
const COMMONCAL = process.env.REAL_AUTH_COMMONCAL ?? 'http://127.0.0.1:4002';
const DATABASE_URL =
  process.env.REAL_AUTH_DATABASE_URL ?? 'postgres://oidc:oidc-lab-only@127.0.0.1:5432/oidc';
const OIDC_SCOPE = 'openid offline_access';
const RESOURCE_SCOPE = 'commoncal.calendar.metadata.read commoncal.event.read.basic';
const AUTH_SCOPE = `${RESOURCE_SCOPE} ${OIDC_SCOPE}`;

const env = {
  ...process.env,
  LAB_ISSUER: ISSUER,
  LAB_RESOURCE_URL: RESOURCE,
  LAB_COMMONCAL_URL: COMMONCAL,
  LAB_LOOPBACK_REDIRECT: REDIRECT,
  LAB_BRIDGE_KEY: BRIDGE_KEY,
  DATABASE_URL,
  AUTH_PUBLIC_BIND: '127.0.0.1',
  AUTH_PRIVATE_BIND: '127.0.0.1',
  AUTH_PUBLIC_PORT: new URL(ISSUER).port || '80',
  AUTH_PRIVATE_PORT: new URL(BRIDGE).port || '80',
  AUTH_JWKS_FILE: `${REPO}/slice1-lab/auth-server/test-jwks.json`,
  AUTH_SIGNING_KID: 'slice1-test-rs256',
};

// Use inherited stdio so the auth server does not exit when its stdout/stderr
// pipes close (a Node.js behavior with piped stdio). The helper writes the
// token to a file (REAL_AUTH_TOKEN_FILE) instead of stdout.
const server = spawn('node', [`${REPO}/slice1-lab/auth-server/src/server.mjs`], {
  env,
  stdio: 'inherit',
});

function fail(code, msg) {
  process.stderr.write(`${msg}\n`);
  server.kill('SIGTERM');
  process.exit(code);
}

async function waitForReady() {
  for (let i = 0; i < 150; i++) {
    try {
      const r = await fetch(`${ISSUER}/health`);
      if (r.ok) return true;
    } catch {}
    await sleep(100);
  }
  return false;
}

function b64url(buf) {
  return Buffer.from(buf).toString('base64url');
}
const verifier = b64url(randomBytes(48));
const challenge = b64url(createHash('sha256').update(verifier).digest());
const state = randomBytes(16).toString('hex');

// Minimal cookie jar for the issuer host (the provider's interaction cookie).
const cookies = new Map();
function cookieHeader() {
  return [...cookies.entries()].map(([k, v]) => `${k}=${v}`).join('; ');
}
function storeCookies(res) {
  const setCookies = res.headers.getSetCookie?.() ?? [];
  for (const sc of setCookies) {
    const [pair] = sc.split(';');
    const idx = pair.indexOf('=');
    if (idx === -1) continue;
    const name = pair.slice(0, idx).trim();
    const value = pair.slice(idx + 1).trim();
    if (value === '' || /expires=Thu, 01 Jan 1970/i.test(sc)) cookies.delete(name);
    else cookies.set(name, value);
  }
}

async function main() {
  // Exit 100 signals "environment unavailable" (e.g. PostgreSQL not running)
  // so the Rust test can skip rather than fail.
  if (!(await waitForReady())) fail(100, 'auth server did not become ready (PostgreSQL unavailable?)');

  // 1. DCR (RFC 7591) — public client, loopback redirect, OIDC scopes only.
  const dcr = await fetch(`${ISSUER}/reg`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      client_name: 'mcp-real-auth-integration',
      redirect_uris: [REDIRECT],
      grant_types: ['authorization_code', 'refresh_token'],
      response_types: ['code'],
      token_endpoint_auth_method: 'none',
      scope: OIDC_SCOPE,
    }),
  });
  const dcrBody = await dcr.json().catch(() => ({}));
  if (dcr.status !== 201 || !dcrBody.client_id) {
    fail(3, `DCR failed: ${dcr.status} ${JSON.stringify(dcrBody)}`);
  }
  const clientId = dcrBody.client_id;

  // 2. Authorization Code + S256 PKCE, driving the CommonCal consent bridge.
  const authUrl = `${ISSUER}/auth?${new URLSearchParams({
    response_type: 'code',
    client_id: clientId,
    redirect_uri: REDIRECT,
    scope: AUTH_SCOPE,
    state,
    code_challenge: challenge,
    code_challenge_method: 'S256',
    resource: RESOURCE,
  })}`;

  let code = null;
  let currentUrl = authUrl;
  for (let hop = 0; hop < 16; hop++) {
    const isIssuer = currentUrl.startsWith(ISSUER);
    const res = await fetch(currentUrl, {
      redirect: 'manual',
      headers: isIssuer && cookies.size ? { cookie: cookieHeader() } : {},
    });
    if (isIssuer) storeCookies(res);
    let loc = res.headers.get('location');
    const body = await res.text();
    if (loc && !/^https?:\/\//i.test(loc)) loc = new URL(loc, ISSUER).toString();

    if (loc && loc.includes('/consent?handoff=')) {
      const handoff = new URL(loc).searchParams.get('handoff');
      const lookup = await fetch(
        `${BRIDGE}/internal/interactions/${encodeURIComponent(handoff)}`,
        { headers: { authorization: `Bearer ${BRIDGE_KEY}` } },
      );
      const view = await lookup.json().catch(() => ({}));
      const kind = view.prompt === 'login' ? 'login' : 'consent';
      const decideBody = { kind };
      if (kind === 'login') decideBody.subject = '1';
      const decide = await fetch(
        `${BRIDGE}/internal/interactions/${encodeURIComponent(handoff)}`,
        {
          method: 'PUT',
          headers: {
            authorization: `Bearer ${BRIDGE_KEY}`,
            'content-type': 'application/json',
          },
          body: JSON.stringify(decideBody),
        },
      );
      const decideRes = await decide.json().catch(() => ({}));
      if (!decideRes.resumeUrl) {
        fail(4, `consent decide failed: ${decide.status} ${JSON.stringify(decideRes)}`);
      }
      currentUrl = decideRes.resumeUrl;
      continue;
    }

    if (loc && loc.startsWith(REDIRECT)) {
      code = new URL(loc).searchParams.get('code');
      break;
    }

    if (loc) {
      currentUrl = loc;
      continue;
    }

    fail(5, `no redirect at hop ${hop}: status ${res.status} body ${body.slice(0, 400)}`);
  }

  if (!code) fail(6, 'authorization flow did not yield a code');

  // 3. Token exchange.
  const tokenRes = await fetch(`${ISSUER}/token`, {
    method: 'POST',
    headers: { 'content-type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams({
      grant_type: 'authorization_code',
      code,
      redirect_uri: REDIRECT,
      client_id: clientId,
      code_verifier: verifier,
    }).toString(),
  });
  const tokenBody = await tokenRes.json().catch(() => ({}));

  if (tokenRes.status !== 200 || !tokenBody.access_token) {
    fail(7, `token exchange failed: ${tokenRes.status} ${JSON.stringify(tokenBody)}`);
  }

  const payload = JSON.parse(
    Buffer.from(tokenBody.access_token.split('.')[1], 'base64url').toString('utf8'),
  );
  const tokenFile =
    process.env.REAL_AUTH_TOKEN_FILE ??
    `${REPO}/mcp-server/tests/.real_auth_token.json`;
  const { writeFileSync } = await import('node:fs');
  writeFileSync(
    tokenFile,
    JSON.stringify({
      ok: true,
      issuer: ISSUER,
      resource: RESOURCE,
      client_id: clientId,
      access_token: tokenBody.access_token,
      claims: payload,
    }) + '\n',
  );
  process.stderr.write(`[helper] token written to ${tokenFile}\n`);

  // Keep the auth server alive so the Rust test can validate the token
  // (which fetches discovery + JWKS from the live server). A ref'd interval
  // keeps Node's event loop alive (a never-resolving promise alone does not).
  // Exit when the parent kills us (SIGTERM) or after a safety timeout.
  let shuttingDown = false;
  const shutdown = () => {
    if (shuttingDown) return;
    shuttingDown = true;
    clearInterval(keepAlive);
    try {
      server.kill('SIGTERM');
    } catch {}
    process.exit(0);
  };
  const keepAlive = setInterval(() => {}, 1000);
  process.once('SIGTERM', shutdown);
  process.once('SIGINT', shutdown);
  // Belt-and-suspenders: if the helper exits for any other reason, make sure
  // the auth server child does not become an orphan.
  process.once('exit', () => {
    try {
      server.kill('SIGTERM');
    } catch {}
  });
  setTimeout(shutdown, 120_000).unref?.();
  await new Promise(() => {});
}

main().catch((e) => {
  process.stderr.write(`helper error: ${e?.stack ?? e}\n`);
  server.kill('SIGTERM');
  process.exit(9);
});
