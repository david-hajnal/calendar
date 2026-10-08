import { createPrivateKey, createPublicKey, sign, verify } from 'node:crypto';

export function productionConfig(env, jwks) {
  const required = (name) => {
    if (!env[name]) throw new Error(`${name} is required`);
    return env[name];
  };
  const https = (name, originOnly = false) => {
    const raw = required(name);
    const url = new URL(raw);
    if (url.protocol !== 'https:' || url.username || url.password || url.search || url.hash || (originOnly && raw !== url.origin)) throw new Error(`${name} must be a canonical HTTPS ${originOnly ? 'origin' : 'URL'}`);
    return raw;
  };
  const issuer = https('AUTH_ISSUER', true);
  const resource = https('AUTH_RESOURCE_URL');
  const commoncal = https('AUTH_COMMONCAL_URL', true);
  if ([resource, commoncal].some((url) => new URL(url).origin === issuer)) throw new Error('AUTH_ISSUER must use a dedicated origin');
  const database = new URL(required('DATABASE_URL'));
  if (!['postgres:', 'postgresql:'].includes(database.protocol)) throw new Error('DATABASE_URL must use postgres or postgresql');
  const strong = (value) => typeof value === 'string' && value.length >= 32 && !/slice1|lab-only|not-production|example|changeme/i.test(value);
  const bridge = required('AUTH_BRIDGE_KEY');
  const cookies = required('AUTH_COOKIE_KEYS').split(',').map((key) => key.trim());
  if (!strong(bridge) || cookies.length < 2 || cookies.some((key) => !strong(key)) || new Set(cookies).size !== cookies.length) throw new Error('production bridge and distinct cookie keys must be strong (at least 32 characters)');
  required('AUTH_JWKS_FILE');
  const kid = required('AUTH_SIGNING_KID');
  if (!jwks || !Array.isArray(jwks.keys) || !jwks.keys.length) throw new Error('production JWKS is empty');
  const kids = new Set();
  for (const key of jwks.keys) {
    if (!key.kid || /test|slice1/i.test(key.kid) || kids.has(key.kid) || key.kty !== 'RSA' || key.use !== 'sig' || key.alg !== 'RS256') throw new Error('production JWKS requires unique non-test RSA RS256 signing keys');
    kids.add(key.kid);
    const publicKey = createPublicKey({ key, format: 'jwk' });
    if (publicKey.asymmetricKeyDetails.modulusLength < 2048) throw new Error('RSA keys must be at least 2048 bits');
    if (key.kid === kid) {
      const privateKey = createPrivateKey({ key, format: 'jwk' });
      const probe = Buffer.from('commoncal signing key consistency');
      if (!verify('RSA-SHA256', probe, publicKey, sign('RSA-SHA256', probe, privateKey))) throw new Error('signing key public and private parts do not match');
    }
  }
  if (!kids.has(kid)) throw new Error('AUTH_SIGNING_KID is not present in JWKS');
  if (env.AUTH_TRUST_PROXY !== 'true') throw new Error('AUTH_TRUST_PROXY=true is required behind the restricted TLS ingress');
  const hosts = required('AUTH_DCR_LOOPBACK_HOSTS').split(',').map((host) => host.trim());
  if (!hosts.length || hosts.some((host) => !['127.0.0.1', '[::1]', 'localhost'].includes(host))) throw new Error('invalid loopback host allowlist');
  const path = required('AUTH_DCR_CALLBACK_PATH');
  if (!/^\/[A-Za-z0-9/_-]+$/.test(path)) throw new Error('invalid exact callback path');
  const rate = Number(env.AUTH_DCR_RATE_LIMIT ?? 20);
  if (!Number.isInteger(rate) || rate < 1 || rate > 1000) throw new Error('invalid DCR rate limit');
  const cleanupInterval = Number(env.AUTH_CLEANUP_INTERVAL_MS ?? 60_000);
  if (!Number.isInteger(cleanupInterval) || cleanupInterval < 1000 || cleanupInterval > 3_600_000) throw new Error('invalid retention cleanup interval');
  return { cleanupInterval, issuer, resource, commoncal, bridge, cookies, kid, rate, catalog: hosts.map((host) => ({ kind: 'loopback', host, port: 'any', path })) };
}
