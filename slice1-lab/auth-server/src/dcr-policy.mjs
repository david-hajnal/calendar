//! DCR callback-shape policy framework.
//!
//! A `RedirectShape` is a configuration-backed description of an allowed
//! redirect URI. The policy validates a candidate redirect URI against the
//! admitted shapes and rejects anything that does not match exactly.
//!
//! Slice 4 admits ONLY the lab loopback callback. Client slices (11-13) may
//! add narrowly-proven shapes (custom-scheme, exact-https) after observing the
//! released client. Arbitrary HTTPS callbacks remain forbidden.

/**
 * @typedef {Object} LoopbackShape
 * @property {'loopback'} kind
 * @property {'127.0.0.1' | '[::1]'} host
 * @property {'any' | number} port
 * @property {string} path
 */

/**
 * @typedef {Object} CustomSchemeShape
 * @property {'custom-scheme'} kind
 * @property {string} scheme
 * @property {string} path
 */

/**
 * @typedef {Object} ExactHttpsShape
 * @property {'exact-https'} kind
 * @property {string} uri
 */

/**
 * @typedef {LoopbackShape | CustomSchemeShape | ExactHttpsShape} RedirectShape
 */

/**
 * Parse a redirect URI string into its components, or null if malformed.
 * @param {string} uri
 * @returns {{scheme: string, host: string, port: number|null, path: string, query: string, fragment: string} | null}
 */
function parseRedirect(uri) {
  if (typeof uri !== 'string' || uri.length === 0) return null;
  // Reject fragments and credentials outright (never valid in a redirect).
  if (uri.includes('#') || uri.includes('@')) return null;
  let parsed;
  try {
    parsed = new URL(uri);
  } catch {
    return null;
  }
  if (!parsed.protocol || !parsed.hostname) return null;
  return {
    scheme: parsed.protocol.replace(':', '').toLowerCase(),
    host: parsed.hostname.toLowerCase(),
    port: parsed.port ? Number(parsed.port) : null,
    path: parsed.pathname,
    query: parsed.search,
    fragment: parsed.hash,
  };
}

/**
 * Validate a candidate redirect URI against a single admitted shape.
 * @param {string} uri
 * @param {RedirectShape} shape
 * @returns {boolean}
 */
function matchesShape(uri, shape) {
  const parts = parseRedirect(uri);
  if (!parts) return false;

  switch (shape.kind) {
    case 'loopback': {
      if (parts.scheme !== 'http' && parts.scheme !== 'https') return false;
      // Normalize [::1] vs ::1.
      const host = parts.host === '[::1]' ? '[::1]' : parts.host;
      if (host !== shape.host) return false;
      if (shape.port !== 'any' && parts.port !== shape.port) return false;
      if (parts.path !== shape.path) return false;
      // Loopback redirects must not carry a query or fragment.
      if (parts.query || parts.fragment) return false;
      return true;
    }
    case 'custom-scheme': {
      if (parts.scheme !== shape.scheme) return false;
      // Custom schemes have no host/port; the path is the meaningful part.
      // Accept the path with or without a leading slash.
      const expectedPath = shape.path.startsWith('/') ? shape.path : `/${shape.path}`;
      const actualPath = parts.path.startsWith('/') ? parts.path : `/${parts.path}`;
      if (actualPath !== expectedPath) return false;
      if (parts.fragment) return false;
      return true;
    }
    case 'exact-https': {
      // Exact string match on the full URI (scheme + host + port + path).
      const expected = new URL(shape.uri);
      if (parts.scheme !== expected.protocol.replace(':', '').toLowerCase()) return false;
      if (parts.host !== expected.hostname.toLowerCase()) return false;
      const expectedPort = expected.port ? Number(expected.port) : null;
      if (parts.port !== expectedPort) return false;
      if (parts.path !== expected.pathname) return false;
      if (parts.query !== expected.search) return false;
      if (parts.fragment) return false;
      return true;
    }
    default:
      return false;
  }
}

/**
 * Validate a candidate redirect URI against a catalog of admitted shapes.
 * @param {string} uri
 * @param {ReadonlyArray<RedirectShape>} catalog
 * @returns {boolean} true if the URI matches at least one admitted shape.
 */
export function validateRedirect(uri, catalog) {
  if (!Array.isArray(catalog) || catalog.length === 0) return false;
  return catalog.some((shape) => matchesShape(uri, shape));
}

/**
 * Build the default Slice 4 catalog: only the lab loopback callback.
 * @param {string} loopbackUri
 * @returns {ReadonlyArray<RedirectShape>}
 */
export function defaultLabCatalog(loopbackUri) {
  const parts = parseRedirect(loopbackUri);
  if (!parts) throw new Error(`invalid loopback redirect: ${loopbackUri}`);
  return Object.freeze([
    Object.freeze({
      kind: 'loopback',
      host: parts.host === '[::1]' ? '[::1]' : '127.0.0.1',
      port: parts.port ?? 'any',
      path: parts.path,
    }),
  ]);
}
