"use strict";

/**
 * Shared helpers for the Phase 0.3 (oauth-lifecycle) and Phase 0.4
 * (security-isolation) acceptance harnesses.
 *
 * This module is a pure HTTP client. It does NOT launch or depend on the
 * slice1-lab binaries, and it does NOT treat the lab binary as a release
 * artifact. It reuses the slice1-lab fixture shapes:
 *   - the DCR request shape (slice1-lab/negative_tests.py fresh_dcr,
 *     slice1-lab/auth-server/src/server.mjs validateRegisteredMetadata);
 *   - the standard access-token claim contract (slice1-lab/src/jwt.rs
 *     AccessClaims: iss, numeric sub, aud, exp, iat, client_id, scope, jti,
 *     amr);
 *   - the scope catalog (slice1-lab/src/common.rs SCOPE_CATALOG);
 *   - the negative-token / fail-closed cases (slice1-lab/negative_tests.py
 *     cases 3-9, slice1-lab/PROOF.md P7).
 *
 * It owns its own production-facing configuration, lifecycle, reporting, and
 * cleanup: it is a pure HTTP client, starts no servers, and leaves no
 * processes behind.
 */

const crypto = require("node:crypto");

// ---------------------------------------------------------------------------
// Scope catalog (reused from slice1-lab/src/common.rs SCOPE_CATALOG)
// ---------------------------------------------------------------------------

const SCOPE_CATALOG = [
  "commoncal.calendar.metadata.read",
  "commoncal.availability.read",
  "commoncal.event.read.basic",
  "commoncal.event.read.details",
  "commoncal.event.create",
  "commoncal.event.update",
  "commoncal.event.delete",
  "commoncal.reminder.read",
  "commoncal.reminder.write",
];

// A scope deliberately outside the catalog (slice1-lab/src/common.rs EVIL_SCOPE).
const EVIL_SCOPE = "evil.unknown.scope";

// ---------------------------------------------------------------------------
// base64url helpers
// ---------------------------------------------------------------------------

function b64urlEncode(buf) {
  return Buffer.from(buf).toString("base64url");
}

function b64urlDecode(str) {
  return Buffer.from(str, "base64url");
}

function b64urlJson(obj) {
  return b64urlEncode(JSON.stringify(obj));
}

// ---------------------------------------------------------------------------
// PKCE S256
// ---------------------------------------------------------------------------

/**
 * Compute the S256 code challenge for a verifier (RFC 7636).
 */
function s256Challenge(verifier) {
  return b64urlEncode(crypto.createHash("sha256").update(verifier).digest());
}

/**
 * Generate a random PKCE code verifier (43-128 chars, RFC 7636).
 */
function generateVerifier() {
  return b64urlEncode(crypto.randomBytes(48));
}

// ---------------------------------------------------------------------------
// JWT helpers (decode only; no signature verification)
// ---------------------------------------------------------------------------

/**
 * Decode a JWT payload (no signature verification). Returns the claims object
 * or null.
 */
function decodeJwtPayload(jwt) {
  const parts = jwt.split(".");
  if (parts.length !== 3) return null;
  try {
    return JSON.parse(b64urlDecode(parts[1]).toString("utf8"));
  } catch {
    return null;
  }
}

/**
 * Decode a JWT header (no signature verification). Returns the header object
 * or null.
 */
function decodeJwtHeader(jwt) {
  const parts = jwt.split(".");
  if (parts.length !== 3) return null;
  try {
    return JSON.parse(b64urlDecode(parts[0]).toString("utf8"));
  } catch {
    return null;
  }
}

/**
 * Tamper a JWT's payload (re-encode with modified claims) without re-signing.
 * The signature becomes invalid, but the point is that validation fails closed.
 */
function tamperJwtPayload(jwt, claimOverrides) {
  const parts = jwt.split(".");
  if (parts.length !== 3) throw new Error("not a 3-part JWT");
  const claims = decodeJwtPayload(jwt);
  if (!claims) throw new Error("cannot decode payload");
  Object.assign(claims, claimOverrides);
  return `${parts[0]}.${b64urlJson(claims)}.${parts[2]}`;
}

/**
 * Tamper a JWT's signature (replace with a random signature of the same length).
 */
function tamperJwtSignature(jwt) {
  const parts = jwt.split(".");
  if (parts.length !== 3) throw new Error("not a 3-part JWT");
  const sigBytes = b64urlDecode(parts[2]);
  const randomSig = b64urlEncode(crypto.randomBytes(sigBytes.length));
  return `${parts[0]}.${parts[1]}.${randomSig}`;
}

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

/**
 * Perform one HTTP request and capture the full response deterministically.
 */
async function httpProbe(cfg, method, url, body, headers = {}) {
  const reqHeaders = { ...headers };
  let bodyString = null;
  if (body !== null && body !== undefined) {
    bodyString = typeof body === "string" ? body : JSON.stringify(body);
    if (!reqHeaders["Content-Type"] && !reqHeaders["content-type"]) {
      reqHeaders["Content-Type"] = "application/json";
    }
  }
  if (method === "POST" && !reqHeaders.Accept && !reqHeaders.accept) {
    reqHeaders.Accept = "application/json, text/event-stream";
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
        headers: reqHeaders,
        body: bodyString,
        signal: controller.signal,
        redirect: "manual",
      });
    } finally {
      clearTimeout(timer);
    }
    status = res.status;
    respHeaders = normalizeHeaders(res.headers);
    rawBody = await res.text();
  } catch (err) {
    throw new Error(`network failure contacting ${url}: ${err.message}`);
  }

  let bodyJson = null;
  let bodyJsonParseError = null;
  try {
    bodyJson = JSON.parse(rawBody);
  } catch (err) {
    bodyJsonParseError = err.message;
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

// ---------------------------------------------------------------------------
// OAuth flow (DCR + Authorization Code + S256 PKCE + token exchange)
// ---------------------------------------------------------------------------

/**
 * Register a client through DCR (RFC 7591) using the supported OpenCode
 * loopback redirect shape. Mirrors the lab's DCR policy
 * (slice1-lab/auth-server/src/server.mjs validateRegisteredMetadata).
 *
 * @param {object} cfg  Harness configuration.
 * @param {string} issuer  The issuer base URL.
 * @param {string} redirect  The admitted loopback redirect URI.
 * @returns {Promise<{status: number, body: object}>}
 */
async function dcrRegister(cfg, issuer, redirect) {
  const body = {
    client_name: "commoncal-mcp-acceptance",
    redirect_uris: [redirect],
    grant_types: ["authorization_code", "refresh_token"],
    response_types: ["code"],
    token_endpoint_auth_method: "none",
    scope: SCOPE_CATALOG.join(" "),
  };
  const url = `${issuer.replace(/\/$/, "")}/register`;
  const resp = await httpProbe(cfg, "POST", url, body);
  return resp;
}

/**
 * Drive the Authorization Code flow with S256 PKCE. Returns the authorization
 * code and state from the redirect.
 *
 * @param {object} cfg  Harness configuration.
 * @param {string} issuer  The issuer base URL.
 * @param {string} clientId  The DCR client id.
 * @param {string} redirect  The admitted loopback redirect URI.
 * @param {string} scope  The requested scope.
 * @param {string} verifier  The PKCE code verifier.
 * @returns {Promise<{code: string|null, state: string|null, resp: object}>}
 */
async function authorize(cfg, issuer, clientId, redirect, scope, verifier) {
  const challenge = s256Challenge(verifier);
  const state = crypto.randomBytes(16).toString("hex");
  const params = new URLSearchParams({
    response_type: "code",
    client_id: clientId,
    redirect_uri: redirect,
    scope,
    state,
    code_challenge: challenge,
    code_challenge_method: "S256",
  });
  const url = `${issuer.replace(/\/$/, "")}/authorize?${params.toString()}`;
  const resp = await httpProbe(cfg, "GET", url, null);

  // Extract the code and state from the redirect Location header.
  let code = null;
  let returnedState = null;
  const location = resp.headers["location"];
  if (location) {
    try {
      const locUrl = new URL(location);
      code = locUrl.searchParams.get("code");
      returnedState = locUrl.searchParams.get("state");
    } catch {
      // Location is not a valid URL; leave code/state null.
    }
  }
  return { code, state: returnedState, resp };
}

/**
 * Exchange an authorization code for a token (Authorization Code + S256 PKCE).
 *
 * @param {object} cfg  Harness configuration.
 * @param {string} issuer  The issuer base URL.
 * @param {string} code  The authorization code.
 * @param {string} clientId  The DCR client id.
 * @param {string} redirect  The admitted loopback redirect URI.
 * @param {string} verifier  The PKCE code verifier.
 * @returns {Promise<{status: number, body: object}>}
 */
async function tokenExchange(cfg, issuer, code, clientId, redirect, verifier) {
  const params = new URLSearchParams({
    grant_type: "authorization_code",
    code,
    redirect_uri: redirect,
    client_id: clientId,
    code_verifier: verifier,
  });
  const url = `${issuer.replace(/\/$/, "")}/token`;
  const resp = await httpProbe(cfg, "POST", url, params.toString(), {
    "Content-Type": "application/x-www-form-urlencoded",
  });
  return resp;
}

/**
 * Run the full OAuth flow: DCR + Authorization Code + S256 PKCE + token
 * exchange. Returns the access token and the full flow record.
 *
 * @param {object} cfg  Harness configuration.
 * @param {string} issuer  The issuer base URL.
 * @param {string} redirect  The admitted loopback redirect URI.
 * @returns {Promise<{accessToken: string|null, clientId: string|null, record: object}>}
 */
async function runOAuthFlow(cfg, issuer, redirect) {
  const record = {
    dcr: null,
    authorize: null,
    token: null,
  };

  // 1. DCR.
  const dcrResp = await dcrRegister(cfg, issuer, redirect);
  record.dcr = dcrResp;
  const clientId = dcrResp.bodyJson && dcrResp.bodyJson.client_id;
  if (!clientId) {
    return { accessToken: null, clientId: null, record };
  }

  // 2. Authorization Code + S256 PKCE.
  const verifier = generateVerifier();
  const scope = SCOPE_CATALOG.join(" ");
  const authResult = await authorize(cfg, issuer, clientId, redirect, scope, verifier);
  record.authorize = authResult;
  if (!authResult.code) {
    return { accessToken: null, clientId, record };
  }

  // 3. Token exchange.
  const tokenResp = await tokenExchange(cfg, issuer, authResult.code, clientId, redirect, verifier);
  record.token = tokenResp;
  const accessToken = tokenResp.bodyJson && tokenResp.bodyJson.access_token;

  return { accessToken, clientId, record };
}

// ---------------------------------------------------------------------------
// MCP calls
// ---------------------------------------------------------------------------

/**
 * Send a JSON-RPC request to the MCP endpoint.
 *
 * @param {object} cfg  Harness configuration.
 * @param {string} mcpUrl  The MCP endpoint URL.
 * @param {string} method  The JSON-RPC method.
 * @param {object} params  The JSON-RPC params.
 * @param {number} id  The JSON-RPC id.
 * @param {string|null} accessToken  The Bearer access token (or null).
 * @returns {Promise<object>} The response record.
 */
async function mcpCall(cfg, mcpUrl, method, params, id, accessToken) {
  const body = {
    jsonrpc: "2.0",
    id,
    method,
    params,
  };
  const headers = {};
  if (accessToken) {
    headers.Authorization = `Bearer ${accessToken}`;
  }
  return httpProbe(cfg, "POST", mcpUrl, body, headers);
}

/**
 * Send a JSON-RPC notification to the MCP endpoint (no id expected).
 *
 * @param {object} cfg  Harness configuration.
 * @param {string} mcpUrl  The MCP endpoint URL.
 * @param {string} method  The JSON-RPC method.
 * @param {object} params  The JSON-RPC params.
 * @param {string|null} accessToken  The Bearer access token (or null).
 * @returns {Promise<object>} The response record.
 */
async function mcpNotify(cfg, mcpUrl, method, params, accessToken) {
  const body = {
    jsonrpc: "2.0",
    method,
    params,
  };
  const headers = {};
  if (accessToken) {
    headers.Authorization = `Bearer ${accessToken}`;
  }
  return httpProbe(cfg, "POST", mcpUrl, body, headers);
}

// ---------------------------------------------------------------------------
// Deterministic JSON
// ---------------------------------------------------------------------------

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

module.exports = {
  SCOPE_CATALOG,
  EVIL_SCOPE,
  b64urlEncode,
  b64urlDecode,
  b64urlJson,
  s256Challenge,
  generateVerifier,
  decodeJwtPayload,
  decodeJwtHeader,
  tamperJwtPayload,
  tamperJwtSignature,
  httpProbe,
  normalizeHeaders,
  isPlainObject,
  dcrRegister,
  authorize,
  tokenExchange,
  runOAuthFlow,
  mcpCall,
  mcpNotify,
  deterministicStringify,
};
