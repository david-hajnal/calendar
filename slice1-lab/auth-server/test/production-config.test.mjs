import { test } from 'node:test';
import assert from 'node:assert/strict';
import { generateKeyPairSync } from 'node:crypto';
import { productionConfig } from '../src/production-config.mjs';
import { validateRedirect } from '../src/dcr-policy.mjs';
const key = generateKeyPairSync('rsa', { modulusLength: 2048 }).privateKey.export({ format: 'jwk' });
const jwks = { keys: [{ ...key, kid: 'current', use: 'sig', alg: 'RS256' }] };
const env = { AUTH_ISSUER: 'https://auth.commoncal.test', AUTH_RESOURCE_URL: 'https://mcp.commoncal.test/mcp', AUTH_COMMONCAL_URL: 'https://commoncal.test', AUTH_SQLITE_PATH: '/data/auth.sqlite', AUTH_BRIDGE_KEY: 'a'.repeat(40), AUTH_COOKIE_KEYS: `${'b'.repeat(40)},${'c'.repeat(40)}`, AUTH_JWKS_FILE: '/secret/jwks', AUTH_SIGNING_KID: 'current', AUTH_TRUST_PROXY: 'true', AUTH_DCR_LOOPBACK_HOSTS: '127.0.0.1,[::1]', AUTH_DCR_CALLBACK_PATH: '/mcp/oauth/callback' };
test('production rejects every missing required setting', () => {
  for (const name of Object.keys(env)) assert.throws(() => productionConfig({ ...env, [name]: undefined }, jwks), undefined, name);
});
test('production validates HTTPS origins, dedicated issuer and strong credentials', () => {
  for (const override of [{ AUTH_SQLITE_PATH: 'relative.sqlite' }, { AUTH_CLEANUP_INTERVAL_MS: '0' }, { AUTH_ISSUER: 'http://auth.test' }, { AUTH_ISSUER: `${env.AUTH_COMMONCAL_URL}` }, { AUTH_COMMONCAL_URL: 'https://commoncal.test/path' }, { AUTH_BRIDGE_KEY: 'slice1-loopback-bridge-key' }, { AUTH_COOKIE_KEYS: `${'b'.repeat(40)},${'b'.repeat(40)}` }, { AUTH_TRUST_PROXY: 'false' }, { AUTH_DCR_LOOPBACK_HOSTS: 'evil.test' }, { AUTH_DCR_CALLBACK_PATH: '/callback?x=1' }, { AUTH_DCR_RATE_LIMIT: '0' }]) assert.throws(() => productionConfig({ ...env, ...override }, jwks));
});
test('signing key must exist, be private and not a test key; overlap public keys accepted', () => {
  assert.throws(() => productionConfig({ ...env, AUTH_SIGNING_KID: 'missing' }, jwks));
  const publicKey = { ...jwks.keys[0] }; delete publicKey.d; delete publicKey.p; delete publicKey.q; delete publicKey.dp; delete publicKey.dq; delete publicKey.qi;
  assert.throws(() => productionConfig(env, { keys: [publicKey] }));
  assert.throws(() => productionConfig({ ...env, AUTH_SIGNING_KID: 'slice1-test' }, { keys: [{ ...jwks.keys[0], kid: 'slice1-test' }] }));
  assert.equal(productionConfig(env, { keys: [...jwks.keys, { ...publicKey, kid: 'previous' }] }).kid, 'current');
});
test('OpenCode exact loopback callback permits ephemeral ports and excludes other callback forms', () => {
  const { catalog } = productionConfig(env, jwks);
  for (const uri of ['http://127.0.0.1:19876/mcp/oauth/callback', 'http://[::1]:23456/mcp/oauth/callback']) assert.ok(validateRedirect(uri, catalog));
  for (const uri of ['http://localhost:19876/mcp/oauth/callback', 'http://127.0.0.1/mcp/oauth/callback', 'http://127.0.0.1:80/mcp/oauth/callback', 'http://127.0.0.1:19876/callback', 'http://127.0.0.1:19876/mcp/oauth/callback?token=secret', 'https://evil.test/mcp/oauth/callback']) assert.equal(validateRedirect(uri, catalog), false, uri);
});
