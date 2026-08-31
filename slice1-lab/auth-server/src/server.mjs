import { createHash, timingSafeEqual } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import Provider, { errors } from 'oidc-provider';
import pg from 'pg';

import PostgresAdapter, { configureAdapter } from './postgres-adapter.mjs';
import { HandoffStore } from './handoff-store.mjs';

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
    ?? 'postgres://oidc:oidc-lab-only@127.0.0.1:55432/oidc',
  max: 8,
});
configureAdapter(pool);

const migration = await readFile(resolve(here, '../migrations/0001_lab.sql'), 'utf8');
await pool.query(migration);
const jwksDocument = JSON.parse(await readFile(resolve(here, '../test-jwks.json'), 'utf8'));
const jwks = { keys: jwksDocument.keys };
const handoffs = new HandoffStore(pool);

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

function validateRegisteredMetadata(_ctx, key, value, metadata) {
  if (key === 'redirect_uris') {
    if (!Array.isArray(value) || value.length !== 1 || value[0] !== REDIRECT) {
      metadata.invalidate(`redirect_uris must contain only the Slice 1 loopback callback ${REDIRECT}`);
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
    keys: ['slice1-cookie-key-a-not-production', 'slice1-cookie-key-b-not-production'],
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
          jwt: { sign: { alg: 'RS256', kid: 'slice1-test-rs256' } },
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
  new Promise((resolveListen) => publicServer.listen(4000, '127.0.0.1', resolveListen)),
  new Promise((resolveListen) => privateServer.listen(4001, '127.0.0.1', resolveListen)),
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
