// Bounded batches cap each maintenance pass. Database expiry remains authoritative
// for reads even when more than one batch awaits deletion.
export async function cleanupExpired(pool, batchSize = 1000) {
  if (typeof pool.cleanup === 'function') return pool.cleanup(batchSize);
  const queries = {
    provider: `DELETE FROM provider_entity WHERE ctid IN (SELECT ctid FROM provider_entity WHERE expires_at <= NOW() ORDER BY expires_at LIMIT $1)`,
    handoffs: `DELETE FROM interaction_handoff WHERE ctid IN (SELECT ctid FROM interaction_handoff WHERE expires_at <= NOW() ORDER BY expires_at LIMIT $1)`,
    rateBuckets: `DELETE FROM dcr_rate_bucket WHERE ctid IN (SELECT ctid FROM dcr_rate_bucket WHERE window_start < NOW() - INTERVAL '1 day' ORDER BY window_start LIMIT $1)`,
    audit: `DELETE FROM authorization_audit WHERE ctid IN (SELECT ctid FROM authorization_audit WHERE created_at < NOW() - INTERVAL '90 days' ORDER BY created_at LIMIT $1)`,
  };
  const removed = {};
  for (const [name, sql] of Object.entries(queries)) removed[name] = (await pool.query(sql, [batchSize])).rowCount;
  return removed;
}

export function startRetention(pool, intervalMs = 60_000) {
  let running;
  const timer = setInterval(() => {
    if (running) return;
    running = cleanupExpired(pool)
      .catch(() => console.error('authorization retention cleanup failed'))
      .finally(() => { running = undefined; });
  }, intervalMs);
  timer.unref();
  return async () => { clearInterval(timer); await running; };
}
