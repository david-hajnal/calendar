import { createHash, timingSafeEqual } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Readable } from 'node:stream';

import Provider, { errors } from 'oidc-provider';
import pg from 'pg';

import PostgresAdapter, { configureAdapter } from './postgres-adapter.mjs';
import { HandoffStore } from './handoff-store.mjs';
import { validateRedirect, defaultLabCatalog } from './dcr-policy.mjs';

const { Pool } = pg;
const here = dirname(fileURLToPath(import.meta.url));

const ISSUER = process.env.LAB_ISSUER ?? 'http://127.0.0.1:4000';
const RESOURCE = process.env.LAB_RESOURCE_URL ?? 'http://127.0.0.1:3001/mcp';
// Use 127.0.0.1 (not localhost) so the CommonCal session cookie — set for the
// 127.0.0.1 host by the lab login — is presented on the consent page. A
// localhost/127.0.0.1 split would drop the cookie and loop on login.
const COMMONCAL = process.env.LAB_COMMONCAL_URL ?? 'http://127.0.0.1:4002';
const REDIRECT = process.env.LAB_LOOPBACK_REDIRECT ?? 'http://127.0.0.1:8321/callback';
const BRIDGE_KEY = process.env.LAB_BRIDGE_KEY ?? 'slice1-loopback-bridge-key';
const FIXED_SUBJECT = '1';

// Bind addresses are configurable so the same image serves both the loopback
// lab (default 127.0.0.1) and the deployment (0.0.0.0, with the private port
// restricted by NetworkPolicy to CommonCal only). Ports are fixed by contract.
const PUBLIC_BIND = process.env.AUTH_PUBLIC_BIND ?? '127.0.0.1';
const PRIVATE_BIND = process.env.AUTH_PRIVATE_BIND ?? '127.0.0.1';
const PUBLIC_PORT = Number(process.env.AUTH_PUBLIC_PORT ?? 4000);
const PRIVATE_PORT = Number(process.env.AUTH_PRIVATE_PORT ?? 4001);

const SCOPE_CATALOG = [
  'commoncal.calendar.metadata.read',
  'commoncal.availability.read',
  'commoncal.event.read.basic',
  'commoncal.event.read.details',
  'commoncal.event.create',
  'commoncal.event.update',
  'commoncal.event.delete',
  'commoncal.reminder.read',
  'commoncal.reminder.write',
];
// OIDC scopes (ID-token / refresh). The CommonCal catalog scopes are RESOURCE
// scopes, declared on the resource server below — putting them here would make
// the provider treat them as OIDC scopes and loop on consent.
const OIDC_SCOPES = ['openid', 'offline_access'];

const pool = new Pool({
  connectionString: process.env.DATABASE_URL
    ?? 'postgres://oidc:oidc-lab-only@127.0.0.1:5432/oidc',
  max: 8,
});
configureAdapter(pool);

const migration = await readFile(resolve(here, '../migrations/0001_lab.sql'), 'utf8');
await pool.query(migration);
// The JWKS document path is configurable so the deployment can mount a
// persistent, rotatable key set from a Secret. The lab default keeps the
// disposable test keys. The document must contain at least one `sig` key whose
// `kid` matches AUTH_SIGNING_KID (used to sign resource tokens).
const JWKS_FILE = process.env.AUTH_JWKS_FILE ?? resolve(here, '../test-jwks.json');
const jwksDocument = JSON.parse(await readFile(JWKS_FILE, 'utf8'));
const jwks = { keys: jwksDocument.keys };
const SIGNING_KID = process.env.AUTH_SIGNING_KID ?? 'slice1-test-rs256';
const COOKIE_KEYS = (process.env.AUTH_COOKIE_KEYS ?? 'slice1-cookie-key-a-not-production,slice1-cookie-key-b-not-production')
  .split(',')
  .map((k) => k.trim())
  .filter(Boolean);
const handoffs = new HandoffStore(pool);

// ---------------------------------------------------------------------------
// Slice 4: DCR ingress controls — rate limiting, audit log, redaction
// ---------------------------------------------------------------------------

/**
 * Structured redaction: mask known secret values so they never appear in
 * audit/log output. Applied to every string field in an audit record.
 * @param {unknown} value
 * @param {ReadonlySet<string>} secrets
 * @returns {unknown}
 */
function redact(value, secrets) {
  if (typeof value === 'string') {
    for (const secret of secrets) {
      if (secret && value.includes(secret)) {
        return value.split(secret).join('[REDACTED]');
      }
    }
    return value;
  }
  if (Array.isArray(value)) return value.map((v) => redact(v, secrets));
  if (value && typeof value === 'object') {
    const out = {};
    for (const [k, v] of Object.entries(value)) out[k] = redact(v, secrets);
    return out;
  }
  return value;
}

// Secrets that must never appear in audit/log output.
const REDACT_SECRETS = new Set([BRIDGE_KEY]);

/**
 * Append a structured audit record to an in-memory ring buffer (lab-only).
 * The harness reads it via the private `/internal/audit` endpoint.
 * @type {Array<{ts: number, event: string, detail: unknown}>}
 */
const AUDIT_LOG = [];
const AUDIT_MAX = 1000;
function audit(event, detail) {
  AUDIT_LOG.push({ ts: Date.now(), event, detail: redact(detail, REDACT_SECRETS) });
  if (AUDIT_LOG.length > AUDIT_MAX) AUDIT_LOG.shift();
}

/**
 * Fixed-window rate limiter keyed by client IP. Lab default: 20 DCR
 * registrations per 60 s window. The limit is mutable at runtime via the
 * `/internal/test/dcr-rate-limit` test hook (used by the S4-2 proof).
 */
let DCR_RATE_LIMIT = Number(process.env.LAB_DCR_RATE_LIMIT ?? 100);
const DCR_RATE_WINDOW_MS = 60_000;
const dcrRateBuckets = new Map();
function dcrRateLimit(ip) {
  const now = Date.now();
  let bucket = dcrRateBuckets.get(ip);
  if (!bucket || now - bucket.windowStart >= DCR_RATE_WINDOW_MS) {
    bucket = { windowStart: now, count: 0 };
    dcrRateBuckets.set(ip, bucket);
  }
  bucket.count += 1;
  return bucket.count <= DCR_RATE_LIMIT;
}
// Periodically drop stale buckets so the map does not grow unbounded.
setInterval(() => {
  const now = Date.now();
  for (const [ip, b] of dcrRateBuckets) {
    if (now - b.windowStart >= DCR_RATE_WINDOW_MS) dcrRateBuckets.delete(ip);
  }
}, DCR_RATE_WINDOW_MS).unref?.();

function exactEqual(left, right) {
  const a = Buffer.from(left);
  const b = Buffer.from(right);
  return a.length === b.length && timingSafeEqual(a, b);
}

function splitScopes(scope) {
  return new Set(String(scope ?? '').split(' ').filter(Boolean));
}

function approvedResourceScopes(scope) {
  const requested = splitScopes(scope);
  return SCOPE_CATALOG.filter((candidate) => requested.has(candidate));
}

// Slice 4: callback-shape policy framework. Only the lab loopback callback is
// admitted. Client slices (11-13) may extend this catalog with narrowly-proven
// shapes after observing the released client. Arbitrary HTTPS remains forbidden.
const REDIRECT_CATALOG = defaultLabCatalog(REDIRECT);

function validateRegisteredMetadata(_ctx, key, value, metadata) {
  if (key === 'redirect_uris') {
    if (!Array.isArray(value) || value.length !== 1) {
      metadata.invalidate('redirect_uris must contain exactly one URI');
      return;
    }
    if (!validateRedirect(value[0], REDIRECT_CATALOG)) {
      metadata.invalidate(
        `redirect_uris[0] does not match an admitted callback shape (only the lab loopback ${REDIRECT} is admitted)`,
      );
    }
  }
  if (key === 'grant_types') {
    const values = Array.isArray(value) ? [...value].sort() : [];
    if (JSON.stringify(values) !== JSON.stringify(['authorization_code', 'refresh_token'])) {
      metadata.invalidate('grant_types must be authorization_code and refresh_token');
    }
  }
  if (key === 'response_types' && (!Array.isArray(value) || value.length !== 1 || value[0] !== 'code')) {
    metadata.invalidate('response_types must contain only code');
  }
  if (key === 'token_endpoint_auth_method' && value !== 'none') {
    metadata.invalidate('token_endpoint_auth_method must be none');
  }
}

const configuration = {
  adapter: PostgresAdapter,
  clients: [],
  claims: { amr: null },
  scopes: OIDC_SCOPES,
  responseTypes: ['code'],
  grantTypes: ['authorization_code', 'refresh_token'],
  subjectTypes: ['public'],
  clientAuthMethods: ['none'],
  jwks,
  cookies: {
    keys: COOKIE_KEYS,
    short: { sameSite: 'lax' },
    long: { sameSite: 'lax' },
  },
  pkce: { required: () => true },
  interactions: {
    // Single per-uid path. Bootstrap and resume both live at this exact path
    // (resume adds a `handoff` query param) so the provider's interaction
    // cookie — scoped to this path — is presented to both.
    url: (_ctx, interaction) => `/interaction/${interaction.uid}`,
  },
  findAccount: async (_ctx, sub) => {
    // The subject is a CommonCal user id (numeric). CommonCal is the identity
    // authority: it approves the subject through the trusted, bearer-keyed
    // bridge decision, and the auth server resolves it to minimal claims
    // WITHOUT reading CommonCal's store or trusting a browser-supplied subject.
    // Lab: accept any positive-integer subject (a valid CommonCal user id).
    if (!/^\d+$/.test(String(sub))) return undefined;
    return {
      accountId: String(sub),
      claims: async () => ({ sub: String(sub) }),
    };
  },
  extraClientMetadata: {
    properties: ['redirect_uris', 'grant_types', 'response_types', 'token_endpoint_auth_method'],
    validator: validateRegisteredMetadata,
  },
  extraTokenClaims: async (ctx) => {
    const source = ctx.oidc.entities.AuthorizationCode ?? ctx.oidc.entities.RefreshToken;
    return { amr: source?.amr ?? ['pwd'] };
  },
  features: {
    devInteractions: { enabled: false },
    registration: { enabled: true, issueRegistrationAccessToken: false },
    registrationManagement: { enabled: false },
    revocation: { enabled: true },
    introspection: { enabled: false },
    userinfo: { enabled: false },
    claimsParameter: { enabled: false },
    clientCredentials: { enabled: false },
    deviceFlow: { enabled: false },
    resourceIndicators: {
      enabled: true,
      defaultResource: async () => undefined,
      useGrantedResource: async () => false,
      getResourceServerInfo: async (_ctx, indicator) => {
        if (indicator !== RESOURCE) throw new errors.InvalidTarget('unknown resource');
        return {
          audience: RESOURCE,
          scope: SCOPE_CATALOG.join(' '),
          accessTokenFormat: 'jwt',
          accessTokenTTL: 300,
          jwt: { sign: { alg: 'RS256', kid: SIGNING_KID } },
        };
      },
    },
  },
  ttl: {
    AccessToken: 300,
    AuthorizationCode: 60,
    Interaction: 180,
    RefreshToken: 3600,
    Session: 3600,
    Grant: 3600,
  },
};

const provider = new Provider(ISSUER, configuration);
provider.proxy = false;

function json(res, status, value) {
  const body = JSON.stringify(value);
  res.writeHead(status, {
    'content-type': 'application/json',
    'content-length': Buffer.byteLength(body),
    'cache-control': 'no-store',
  });
  res.end(body);
}

function redirect(res, location) {
  res.writeHead(303, { location, 'content-length': '0', 'cache-control': 'no-store' });
  res.end();
}

async function readJson(req, limit = 16_384) {
  const chunks = [];
  let size = 0;
  for await (const chunk of req) {
    size += chunk.length;
    if (size > limit) throw new Error('request too large');
    chunks.push(chunk);
  }
  return JSON.parse(Buffer.concat(chunks).toString('utf8') || '{}');
}

function bearerAuthorized(req) {
  const presented = req.headers.authorization?.replace(/^Bearer /, '') ?? '';
  return exactEqual(presented, BRIDGE_KEY);
}

function interactionView(details) {
  const resources = Array.isArray(details.params.resource)
    ? details.params.resource
    : [details.params.resource].filter(Boolean);
  return {
    clientId: details.params.client_id,
    clientName: details.params.client_id,
    redirectUri: details.params.redirect_uri,
    resource: resources[0],
    requestedScopes: [...splitScopes(details.params.scope)],
    // The granted scopes are the intersection of requested and catalog. CommonCal
    // records these on the grant (not the raw requested set) so the grant and the
    // JWT agree on which scopes were actually approved.
    grantedScopes: approvedResourceScopes(details.params.scope),
    prompt: details.prompt.name,
    expiresAt: details.exp,
  };
}

async function bootstrapInteraction(req, res) {
  const details = await provider.interactionDetails(req, res);
  const handoff = await handoffs.create(details.uid, interactionView(details));
  // Slice 2: redirect to the real CommonCal consent page (session-gated).
  // CommonCal handles login (if needed) and consent, then resumes here.
  redirect(res, `${COMMONCAL}/consent?handoff=${encodeURIComponent(handoff)}`);
}

async function resumeInteraction(req, res, url) {
  const handoff = url.searchParams.get('handoff');
  if (!handoff) return json(res, 400, { error: 'missing handoff' });
  const details = await provider.interactionDetails(req, res);
  const decision = await handoffs.consume(handoff, details.uid);
  if (!decision) return json(res, 400, { error: 'expired, mismatched, or replayed handoff' });

  if (decision.kind === 'deny') {
    return provider.interactionFinished(
      req,
      res,
      { error: 'access_denied', error_description: 'fixed lab denial' },
      { mergeWithLastSubmission: false },
    );
  }

  if (details.prompt.name === 'login' && decision.kind === 'login') {
    // Bind the login to the CommonCal-approved subject (trusted bridge
    // decision). Falls back to the fixed lab subject only for decisions that
    // predate the subject field.
    const accountId = String(decision.subject ?? FIXED_SUBJECT);
    return provider.interactionFinished(
      req,
      res,
      { login: { accountId, amr: ['pwd'], remember: true } },
      { mergeWithLastSubmission: false },
    );
  }

  if (details.prompt.name === 'consent' && decision.kind === 'consent') {
    const resources = Array.isArray(details.params.resource)
      ? details.params.resource
      : [details.params.resource].filter(Boolean);
    if (resources.length !== 1 || resources[0] !== RESOURCE) {
      return provider.interactionFinished(
        req,
        res,
        { error: 'access_denied', error_description: 'resource mismatch' },
        { mergeWithLastSubmission: false },
      );
    }

    const grant = details.grantId
      ? await provider.Grant.find(details.grantId)
      : new provider.Grant({
        accountId: details.session.accountId,
        clientId: details.params.client_id,
      });
    if (!grant) return json(res, 400, { error: 'grant not found' });

    const scopes = splitScopes(details.params.scope);
    const approved = approvedResourceScopes(details.params.scope);
    if (approved.length === 0) {
      return provider.interactionFinished(
        req,
        res,
        { error: 'access_denied', error_description: 'no approved CommonCal scope' },
        { mergeWithLastSubmission: false },
      );
    }
    grant.addResourceScope(RESOURCE, approved);
    if (scopes.has('openid')) grant.addOIDCScope('openid');
    if (scopes.has('offline_access')) grant.addOIDCScope('offline_access');
    const grantId = await grant.save();
    return provider.interactionFinished(
      req,
      res,
      { consent: { grantId } },
      { mergeWithLastSubmission: false },
    );
  }

  return json(res, 400, { error: 'decision does not match current prompt' });
}

const publicServer = createServer(async (req, res) => {
  try {
    const url = new URL(req.url, ISSUER);
    if (req.method === 'GET' && /^\/interaction\/[^/]+$/.test(url.pathname)) {
      // Same path for bootstrap and resume so the provider's interaction
      // cookie (scoped to this path) is presented to both. The `handoff`
      // query param distinguishes a return-from-CommonCal resume from a fresh
      // bootstrap.
      const handoff = url.searchParams.get('handoff');
      if (handoff) return await resumeInteraction(req, res, url);
      return await bootstrapInteraction(req, res);
    }
    if (req.method === 'GET' && url.pathname === '/health') return json(res, 200, { ok: true });

    // Slice 4: DCR ingress controls. Intercept POST /reg to apply rate
    // limiting and audit logging before the provider handles registration.
    if (req.method === 'POST' && url.pathname === '/reg') {
      const ip = req.socket?.remoteAddress ?? 'unknown';
      if (!dcrRateLimit(ip)) {
        audit('dcr_rate_limited', { ip });
        return json(res, 429, { error: 'too_many_requests', error_description: 'DCR rate limit exceeded' });
      }
      let body;
      try {
        body = await readJson(req, 16_384);
      } catch (e) {
        audit('dcr_rejected', { ip, reason: e.message });
        return json(res, 413, { error: 'payload_too_large' });
      }
      // Audit the attempt (redacted). The provider will validate the shape.
      audit('dcr_attempt', {
        ip,
        client_name: body.client_name ?? null,
        redirect_uris: body.redirect_uris ?? null,
        grant_types: body.grant_types ?? null,
      });
      // Replay the buffered body to the provider via a fresh Readable stream.
      // The provider's body-parser reads `req` as a readable stream, so we
      // build a minimal request-like object carrying the original headers,
      // method, and url plus a fresh body stream.
      const bodyBuffer = Buffer.from(JSON.stringify(body));
      const bodyStream = Readable.from(bodyBuffer);
      const replayed = Object.assign(bodyStream, {
        headers: req.headers,
        method: req.method,
        url: req.url,
        socket: req.socket,
        httpVersion: req.httpVersion,
      });
      return provider.callback()(replayed, res);
    }

    return provider.callback()(req, res);
  } catch (error) {
    console.error('public request failed', error?.message ?? 'unknown error');
    if (!res.headersSent) json(res, 500, { error: 'internal_error' });
    else res.end();
  }
});

const privateServer = createServer(async (req, res) => {
  try {
    if (!bearerAuthorized(req)) return json(res, 401, { error: 'unauthorized' });
    const url = new URL(req.url, 'http://127.0.0.1:4001');
    // Slice 4: audit log endpoint (lab-only, bridge-keyed). The harness reads
    // DCR attempts and verifies redaction.
    if (req.method === 'GET' && url.pathname === '/internal/audit') {
      return json(res, 200, { entries: AUDIT_LOG });
    }
    // Slice 4: cleanup endpoint — purge expired provider entities + handoffs.
    // Returns the number of rows removed from each store.
    if (req.method === 'POST' && url.pathname === '/internal/cleanup') {
      // Purge expired rows for every model the provider uses.
      const models = ['Client', 'Grant', 'AccessToken', 'AuthorizationCode', 'RefreshToken', 'Interaction', 'Session'];
      const removed = {};
      for (const m of models) {
        removed[m] = await new PostgresAdapter(m).cleanup();
      }
      removed.InteractionHandoff = await handoffs.cleanup();
      audit('cleanup', { removed });
      return json(res, 200, { removed });
    }
    // Slice 4: test hook — set the DCR rate limit and reset the counter.
    // Used by the S4-2 proof to lower the limit, then restore it.
    if (req.method === 'POST' && url.pathname === '/internal/test/dcr-rate-limit') {
      const body = await readJson(req, 4096);
      const limit = Number(body.limit);
      if (!Number.isInteger(limit) || limit < 1) {
        return json(res, 400, { error: 'invalid_limit' });
      }
      DCR_RATE_LIMIT = limit;
      dcrRateBuckets.clear();
      audit('dcr_rate_limit_set', { limit });
      return json(res, 200, { limit });
    }
    // Slice 4: test hook — insert an expired provider_entity row so the
    // cleanup proof can verify expired rows are purged.
    if (req.method === 'POST' && url.pathname === '/internal/test/insert-expired-entity') {
      const body = await readJson(req, 4096);
      const model = body.model ?? 'AuthorizationCode';
      const id = body.id ?? `expired-test-${Date.now()}`;
      await pool.query(
        `INSERT INTO provider_entity (model, id, payload, expires_at)
         VALUES ($1, $2, $3::jsonb, NOW() - INTERVAL '1 second')
         ON CONFLICT (model, id) DO UPDATE SET expires_at = NOW() - INTERVAL '1 second'`,
        [model, id, JSON.stringify({ test: 'expired' })],
      );
      audit('insert_expired_entity', { model, id });
      return json(res, 200, { model, id });
    }
    // Slice 4: test hook — check whether a provider_entity row exists.
    if (req.method === 'GET' && url.pathname === '/internal/test/entity-exists') {
      const model = url.searchParams.get('model') ?? '';
      const id = url.searchParams.get('id') ?? '';
      const result = await pool.query(
        'SELECT 1 FROM provider_entity WHERE model = $1 AND id = $2',
        [model, id],
      );
      return json(res, 200, { exists: result.rows.length > 0 });
    }
    // Lab test hook: force-expire a handoff so the harness can prove expiry
    // handling (an expired handoff must be rejected on decide and resume).
    const expireMatch = url.pathname.match(/^\/internal\/test\/expire-handoff\/([^/]+)$/);
    if (req.method === 'POST' && expireMatch) {
      const expired = await handoffs.expire(decodeURIComponent(expireMatch[1]));
      return json(res, 200, { expired });
    }
    const match = url.pathname.match(/^\/internal\/interactions\/([^/]+)$/);
    if (!match) return json(res, 404, { error: 'not_found' });
    const token = decodeURIComponent(match[1]);
    if (req.method === 'GET') {
      const row = await handoffs.lookup(token);
      return row ? json(res, 200, row.view) : json(res, 404, { error: 'not_found' });
    }
    if (req.method === 'PUT') {
      const decision = await readJson(req);
      if (!['login', 'consent', 'deny'].includes(decision.kind)) {
        return json(res, 400, { error: 'invalid_decision' });
      }
      // Carry the CommonCal-approved subject (identity authority) so the
      // provider grant binds to the real user, not a hardcoded value.
      const stored = { kind: decision.kind };
      if (decision.subject !== undefined) stored.subject = decision.subject;
      const uid = await handoffs.decide(token, stored);
      if (!uid) return json(res, 409, { error: 'expired_or_already_decided' });
      // Resume at the SAME path the provider scoped the interaction cookie to
      // (`/interaction/{uid}`), adding the handoff as a query param. A different
      // path would not receive the cookie and interactionDetails would fail.
      return json(res, 200, {
        resumeUrl: `${ISSUER}/interaction/${encodeURIComponent(uid)}?handoff=${encodeURIComponent(token)}`,
      });
    }
    return json(res, 405, { error: 'method_not_allowed' });
  } catch (error) {
    console.error('private request failed', error?.message ?? 'unknown error');
    return json(res, error?.message === 'request too large' ? 413 : 500, { error: 'internal_error' });
  }
});

const callbackServer = createServer((req, res) => {
  const url = new URL(req.url, 'http://127.0.0.1:8321');
  if (url.pathname !== '/callback') return json(res, 404, { error: 'not_found' });
  return json(res, 200, { received: true });
});

await Promise.all([
  new Promise((resolveListen) => publicServer.listen(PUBLIC_PORT, PUBLIC_BIND, resolveListen)),
  new Promise((resolveListen) => privateServer.listen(PRIVATE_PORT, PRIVATE_BIND, resolveListen)),
  // The callback server is a lab artifact (the production client runs its own
  // loopback callback). It stays bound to loopback only and is never exposed.
  new Promise((resolveListen) => callbackServer.listen(8321, '127.0.0.1', resolveListen)),
]);

console.log(`slice1 auth issuer listening at ${ISSUER}`);
console.log(`CommonCal interaction host expected at ${COMMONCAL} (separate process)`);

async function shutdown() {
  await Promise.all([
    new Promise((resolveClose) => publicServer.close(resolveClose)),
    new Promise((resolveClose) => privateServer.close(resolveClose)),
    new Promise((resolveClose) => callbackServer.close(resolveClose)),
  ]);
  await pool.end();
}

process.once('SIGTERM', () => shutdown().then(() => process.exit(0)));
process.once('SIGINT', () => shutdown().then(() => process.exit(0)));
