// Separate immutable production entrypoint; the lab entrypoint retains its defaults.
process.env.AUTH_RUNTIME = 'production';
try {
  await import('./server.mjs');
} catch {
  // Provider initialization errors can embed private JWKS material or database
  // credentials as their cause. Never print the exception or stack in production.
  console.error('production authorization startup failed');
  process.exit(1);
}
