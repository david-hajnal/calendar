"use strict";

/**
 * Verification-only mock OAuth authorization server for the Phase 0.3
 * (oauth-lifecycle) and Phase 0.4 (security-isolation) acceptance harnesses.
 *
 * This is NOT a release artifact and NOT the production authorization server.
 * It exists solely to exercise the harnesses' PASS and FAIL paths
 * deterministically. It is a self-contained, dependency-free OAuth provider
 * (DCR + Authorization Code + S256 PKCE + token exchange + JWKS) built on
 * Node built-ins only.
 *
 * It reuses the slice1-lab fixture shapes:
 *   - the DCR request shape (slice1-lab/negative_tests.py fresh_dcr,
 *     slice1-lab/auth-server/src/server.mjs validateRegisteredMetadata);
 *   - the standard access-token claim contract (slice1-lab/src/jwt.rs
 *     AccessClaims: iss, numeric sub, aud, exp, iat, client_id, scope, jti,
 *     amr);
 *   - the scope catalog (slice1-lab/src/common.rs SCOPE_CATALOG);
 *   - the negative-token / fail-closed cases (slice1-lab/negative_tests.py
 *     cases 3-9, slice1-lab/PROOF.md P7).
 *
 * It does NOT import lab code and does NOT launch the lab binaries. It owns
 * its own signing key (generated at startup), its own client/code/token
 * stores, and its own grant store.
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
// RSA key + JWK + RS256 JWT
// ---------------------------------------------------------------------------

/**
 * Generate an RSA key pair and expose the public key as a JWK (n, e) plus a
 * signing function. The private key never leaves this module.
 */
function createSigningKey(kid) {
  const { publicKey, privateKey } = crypto.generateKeyPairSync("rsa", {
    modulusLength: 2048,
    publicKeyEncoding: { type: "spki", format: "pem" },
    privateKeyEncoding: { type: "pkcs8", format: "pem" },
  });

  // Extract n and e from the SPKI public key.
  const keyObj = crypto.createPublicKey(publicKey);
  const jwk = keyObj.export({ format: "jwk" });

  return {
    kid,
    jwk: {
      kty: "RSA",
      n: jwk.n,
      e: jwk.e,
      use: "sig",
      alg: "RS256",
      kid,
    },
    sign(payload) {
      const signer = crypto.createSign("RSA-SHA256");
      signer.update(payload);
      signer.end();
      // Output encoding is base64url; the signature format is DER (PKCS#1 v1.5),
      // which is what RSA-SHA256 produces by default.
      return signer.sign(privateKey, "base64url");
    },
  };
}

/**
 * Sign a JWT (RS256) with the given claims and signing key.
 */
function signJwt(claims, signingKey) {
  const header = { alg: "RS256", typ: "JWT", kid: signingKey.kid };
  const signingInput = `${b64urlJson(header)}.${b64urlJson(claims)}`;
  const signature = signingKey.sign(signingInput);
  return `${signingInput}.${signature}`;
}

/**
 * Verify an RS256 JWT signature against a JWK. Returns true if valid.
 */
function verifyJwtSignature(jwt, jwk) {
  const parts = jwt.split(".");
  if (parts.length !== 3) return false;
  const [headerB64, payloadB64, signatureB64] = parts;
  let header;
  try {
    header = JSON.parse(b64urlDecode(headerB64).toString("utf8"));
  } catch {
    return false;
  }
  if (header.alg !== "RS256") return false;
  if (jwk.kty !== "RSA" || !jwk.n || !jwk.e) return false;

  // Reconstruct the RSA public key from the JWK (n, e). Node handles the
  // modulus padding internally for the JWK->key conversion.
  let publicKey;
  try {
    publicKey = crypto.createPublicKey({
      key: { kty: "RSA", n: jwk.n, e: jwk.e },
      format: "jwk",
    });
  } catch {
    return false;
  }

  const verifier = crypto.createVerify("RSA-SHA256");
  verifier.update(`${headerB64}.${payloadB64}`);
  verifier.end();
  try {
    // Pass the signature as a Buffer (default 'buffer' format).
    return verifier.verify(publicKey, b64urlDecode(signatureB64));
  } catch {
    return false;
  }
}

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

// ---------------------------------------------------------------------------
// PKCE S256
// ---------------------------------------------------------------------------

/**
 * Compute the S256 code challenge for a verifier (RFC 7636).
 */
function s256Challenge(verifier) {
  return b64urlEncode(crypto.createHash("sha256").update(verifier).digest());
}

// ---------------------------------------------------------------------------
// The mock OAuth provider
// ---------------------------------------------------------------------------

/**
 * A self-contained mock OAuth authorization server.
 *
 * @param {object} opts
 * @param {string} opts.issuer      The issuer base URL (e.g. http://127.0.0.1:PORT).
 * @param {string} opts.resource    The exact MCP resource / audience.
 * @param {string} opts.redirect    The admitted loopback redirect URI.
 * @param {string} [opts.kid]       The signing key id.
 */
class MockOAuthProvider {
  constructor({ issuer, resource, redirect, kid = "mcp-acceptance-rs256" }) {
    this.issuer = issuer.replace(/\/$/, "");
    this.resource = resource;
    this.redirect = redirect;
    this.signingKey = createSigningKey(kid);

    // Stores.
    this.clients = new Map(); // client_id -> client metadata
    this.codes = new Map(); // code -> { client_id, redirect_uri, challenge, scope, sub, used }
    this.tokens = new Map(); // jti -> { client_id, sub, scope, aud, exp, iat }
    this.grants = new Map(); // `${sub}:${client_id}` -> grant

    this.clientSeq = 0;
  }

  // -- DCR ------------------------------------------------------------------

  /**
   * Handle a DCR (RFC 7591) registration request. Mirrors the lab's DCR
   * policy (slice1-lab/auth-server/src/server.mjs validateRegisteredMetadata):
   * exactly one loopback redirect, grant_types [authorization_code,
   * refresh_token], response_types [code], token_endpoint_auth_method none.
   *
   * @returns {{status: number, body: object}}
   */
  handleRegister(body) {
    const redirectUris = body.redirect_uris;
    if (!Array.isArray(redirectUris) || redirectUris.length !== 1) {
      return { status: 400, body: { error: "invalid_redirect_uris" } };
    }
    if (redirectUris[0] !== this.redirect) {
      return {
        status: 400,
        body: {
          error: "invalid_redirect_uri",
          error_description: `only the admitted loopback ${this.redirect} is allowed`,
        },
      };
    }
    const grantTypes = Array.isArray(body.grant_types) ? [...body.grant_types].sort() : [];
    if (
      JSON.stringify(grantTypes) !==
      JSON.stringify(["authorization_code", "refresh_token"])
    ) {
      return { status: 400, body: { error: "invalid_grant_types" } };
    }
    const responseTypes = Array.isArray(body.response_types) ? body.response_types : [];
    if (responseTypes.length !== 1 || responseTypes[0] !== "code") {
      return { status: 400, body: { error: "invalid_response_types" } };
    }
    if (body.token_endpoint_auth_method !== "none") {
      return { status: 400, body: { error: "invalid_token_endpoint_auth_method" } };
    }

    this.clientSeq += 1;
    const client_id = `mcp-acceptance-client-${this.clientSeq}`;
    const client = {
      client_id,
      client_name: body.client_name ?? "mcp-acceptance",
      redirect_uris: [this.redirect],
      grant_types: ["authorization_code", "refresh_token"],
      response_types: ["code"],
      token_endpoint_auth_method: "none",
      scope: body.scope ?? SCOPE_CATALOG.join(" "),
    };
    this.clients.set(client_id, client);
    return { status: 201, body: client };
  }

  // -- Authorization --------------------------------------------------------

  /**
   * Handle an authorization request. In the mock, login + consent are
   * auto-approved (the lab's stub adapter / CommonCal consent is the identity
   * authority; the mock stands in for it). It issues an authorization code
   * bound to the client, redirect URI, PKCE challenge, scope, and subject.
   *
   * @param {object} params  URLSearchParams of the authorization request.
   * @returns {{status: number, body: object, location?: string}}
   */
  handleAuthorize(params) {
    const client_id = params.get("client_id");
    const client = this.clients.get(client_id);
    if (!client) {
      return { status: 400, body: { error: "unknown_client" } };
    }
    const redirect_uri = params.get("redirect_uri");
    if (redirect_uri !== this.redirect) {
      return { status: 400, body: { error: "invalid_redirect_uri" } };
    }
    const response_type = params.get("response_type");
    if (response_type !== "code") {
      return { status: 400, body: { error: "unsupported_response_type" } };
    }
    const code_challenge = params.get("code_challenge");
    const code_challenge_method = params.get("code_challenge_method");
    if (!code_challenge || code_challenge_method !== "S256") {
      return { status: 400, body: { error: "invalid_request", error_description: "S256 PKCE required" } };
    }
    const scope = params.get("scope") ?? "";
    const state = params.get("state") ?? "";

    // The subject is a CommonCal user id (numeric). The mock stands in for
    // CommonCal's identity authority and binds the fixed lab subject "1".
    const sub = "1";

    const code = `mcp-acceptance-code-${crypto.randomBytes(12).toString("hex")}`;
    this.codes.set(code, {
      client_id,
      redirect_uri,
      challenge: code_challenge,
      scope,
      sub,
      used: false,
    });

    const location = `${this.redirect}?code=${encodeURIComponent(code)}&state=${encodeURIComponent(state)}`;
    return { status: 302, body: {}, location };
  }

  // -- Token ----------------------------------------------------------------

  /**
   * Handle a token exchange (Authorization Code + S256 PKCE). Verifies the
   * code, the PKCE verifier, and the redirect URI, then issues a JWT access
   * token with the standard claim contract.
   *
   * @param {object} params  URLSearchParams of the token request.
   * @returns {{status: number, body: object}}
   */
  handleToken(params) {
    const grant_type = params.get("grant_type");
    if (grant_type !== "authorization_code") {
      return { status: 400, body: { error: "unsupported_grant_type" } };
    }
    const code = params.get("code");
    const entry = this.codes.get(code);
    if (!entry) {
      return { status: 400, body: { error: "invalid_grant", error_description: "unknown or expired code" } };
    }
    if (entry.used) {
      return { status: 400, body: { error: "invalid_grant", error_description: "code already redeemed" } };
    }
    const client_id = params.get("client_id");
    if (client_id !== entry.client_id) {
      return { status: 400, body: { error: "invalid_grant", error_description: "client_id mismatch" } };
    }
    const redirect_uri = params.get("redirect_uri");
    if (redirect_uri !== entry.redirect_uri) {
      return { status: 400, body: { error: "invalid_grant", error_description: "redirect_uri mismatch" } };
    }
    const code_verifier = params.get("code_verifier");
    if (!code_verifier) {
      return { status: 400, body: { error: "invalid_request", error_description: "code_verifier required" } };
    }
    if (s256Challenge(code_verifier) !== entry.challenge) {
      return { status: 400, body: { error: "invalid_grant", error_description: "PKCE verifier mismatch" } };
    }

    entry.used = true;

    // The granted scopes are the intersection of requested and catalog
    // (slice1-lab/auth-server/src/server.mjs approvedResourceScopes).
    const requested = new Set(String(entry.scope).split(" ").filter(Boolean));
    const granted = SCOPE_CATALOG.filter((s) => requested.has(s));

    const now = Math.floor(Date.now() / 1000);
    const jti = `mcp-acceptance-jti-${crypto.randomBytes(12).toString("hex")}`;
    const claims = {
      iss: this.issuer,
      sub: entry.sub,
      aud: this.resource,
      exp: now + 300,
      iat: now,
      jti,
      client_id: entry.client_id,
      scope: granted.join(" "),
      amr: ["pwd"],
    };
    const access_token = signJwt(claims, this.signingKey);
    this.tokens.set(jti, {
      client_id: entry.client_id,
      sub: entry.sub,
      scope: granted.join(" "),
      aud: this.resource,
      exp: claims.exp,
      iat: now,
    });

    return {
      status: 200,
      body: {
        access_token,
        token_type: "Bearer",
        expires_in: 300,
        refresh_token: `mcp-acceptance-refresh-${crypto.randomBytes(12).toString("hex")}`,
        scope: granted.join(" "),
      },
    };
  }

  // -- JWKS -----------------------------------------------------------------

  /**
   * The JWKS document (public key only).
   */
  jwks() {
    return { keys: [this.signingKey.jwk] };
  }

  // -- Token validation (resource-server side) ------------------------------

  /**
   * Validate an access token the way the production MCP server must:
   * signature (via JWKS), exact iss, exact aud, exp, and the standard claims.
   * Returns { ok, claims?, error? }.
   */
  validateAccessToken(token) {
    if (typeof token !== "string" || token.split(".").length !== 3) {
      return { ok: false, error: "malformed_token" };
    }
    const header = decodeJwtHeader(token);
    if (!header || header.alg !== "RS256") {
      return { ok: false, error: "unsupported_alg" };
    }
    const kid = header.kid;
    if (kid !== this.signingKey.kid) {
      return { ok: false, error: "unknown_kid" };
    }
    if (!verifyJwtSignature(token, this.signingKey.jwk)) {
      return { ok: false, error: "invalid_signature" };
    }
    const claims = decodeJwtPayload(token);
    if (!claims) {
      return { ok: false, error: "malformed_payload" };
    }
    if (claims.iss !== this.issuer) {
      return { ok: false, error: "invalid_issuer" };
    }
    const aud = Array.isArray(claims.aud) ? claims.aud : [claims.aud];
    if (!aud.includes(this.resource)) {
      return { ok: false, error: "invalid_audience" };
    }
    const now = Math.floor(Date.now() / 1000);
    if (typeof claims.exp !== "number" || claims.exp <= now) {
      return { ok: false, error: "token_expired" };
    }
    if (typeof claims.sub !== "string" || !/^\d+$/.test(claims.sub)) {
      return { ok: false, error: "invalid_sub" };
    }
    if (typeof claims.client_id !== "string" || !this.clients.has(claims.client_id)) {
      return { ok: false, error: "unknown_client" };
    }
    return { ok: true, claims };
  }

  // -- Token minting (verification-only) ------------------------------------

  /**
   * Mint a token with the given claims, signed with the provider's key.
   *
   * This is a verification-only helper used by the Phase 0.4 security harness
   * to drive the negative token cases (wrong issuer / audience / expiry /
   * client) with a VALID signature, so each claim check is exercised in
   * isolation (not masked by a signature failure). It is NOT part of the
   * production contract and is only reachable through the mock's `/_test/*`
   * hooks.
   *
   * @param {object} claims  The claims to embed in the token.
   * @returns {string} A signed JWT.
   */
  mintToken(claims) {
    const now = Math.floor(Date.now() / 1000);
    const fullClaims = {
      iss: this.issuer,
      sub: "1",
      aud: this.resource,
      exp: now + 300,
      iat: now,
      jti: `mcp-acceptance-mint-${crypto.randomBytes(12).toString("hex")}`,
      client_id: "mcp-acceptance-client-1",
      scope: SCOPE_CATALOG.join(" "),
      amr: ["pwd"],
      ...claims,
    };
    return signJwt(fullClaims, this.signingKey);
  }

  // -- Grants ---------------------------------------------------------------

  /**
   * Create an active grant for (sub, client_id) with the given allowed
   * calendar ids and scopes. Mirrors slice1-lab/src/bin/commoncal.rs
   * upsert_grant (replace semantics).
   */
  upsertGrant(sub, client_id, allowedCalendarIds, scopes) {
    const key = `${sub}:${client_id}`;
    this.grants.set(key, {
      sub,
      client_id,
      allowed_calendar_ids: allowedCalendarIds,
      scopes,
      revoked: false,
    });
  }

  /**
   * Revoke the active grant for (sub, client_id).
   */
  revokeGrant(sub, client_id) {
    const key = `${sub}:${client_id}`;
    const grant = this.grants.get(key);
    if (grant) grant.revoked = true;
  }

  /**
   * Delete the grant for (sub, client_id) entirely (simulates a missing grant).
   */
  deleteGrant(sub, client_id) {
    this.grants.delete(`${sub}:${client_id}`);
  }

  /**
   * Get the active grant for (sub, client_id), or null.
   */
  getActiveGrant(sub, client_id) {
    const grant = this.grants.get(`${sub}:${client_id}`);
    if (!grant || grant.revoked) return null;
    return grant;
  }
}

module.exports = {
  MockOAuthProvider,
  SCOPE_CATALOG,
  EVIL_SCOPE,
  b64urlEncode,
  b64urlDecode,
  b64urlJson,
  createSigningKey,
  signJwt,
  verifyJwtSignature,
  decodeJwtPayload,
  decodeJwtHeader,
  s256Challenge,
};
