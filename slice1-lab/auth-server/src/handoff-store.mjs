import { createHash, randomBytes } from 'node:crypto';

function digest(token) {
  return createHash('sha256').update(token).digest('hex');
}

export class HandoffStore {
  constructor(pool) {
    this.pool = pool;
  }

  async create(uid, view, ttlSeconds = 120) {
    const token = randomBytes(32).toString('base64url');
    await this.pool.query(
      `INSERT INTO interaction_handoff
         (token_hash, interaction_uid, view, expires_at)
       VALUES ($1, $2, $3::jsonb, NOW() + ($4 * INTERVAL '1 second'))`,
      [digest(token), uid, JSON.stringify(view), ttlSeconds],
    );
    return token;
  }

  async lookup(token) {
    const result = await this.pool.query(
      `SELECT interaction_uid, view, decision
         FROM interaction_handoff
        WHERE token_hash = $1 AND expires_at > NOW() AND consumed_at IS NULL`,
      [digest(token)],
    );
    return result.rows[0];
  }

  async decide(token, decision) {
    const result = await this.pool.query(
      `UPDATE interaction_handoff
          SET decision = $2::jsonb
        WHERE token_hash = $1 AND expires_at > NOW()
          AND consumed_at IS NULL AND decision IS NULL
      RETURNING interaction_uid`,
      [digest(token), JSON.stringify(decision)],
    );
    return result.rows[0]?.interaction_uid;
  }

  /// Lab test hook: force a handoff to be expired (for the expiry proof).
  async expire(token) {
    const result = await this.pool.query(
      `UPDATE interaction_handoff
          SET expires_at = NOW() - INTERVAL '1 second'
        WHERE token_hash = $1
        RETURNING token_hash`,
      [digest(token)],
    );
    return result.rows.length > 0;
  }

  async consume(token, expectedUid) {
    const result = await this.pool.query(
      `UPDATE interaction_handoff
          SET consumed_at = NOW()
        WHERE token_hash = $1 AND interaction_uid = $2
          AND expires_at > NOW() AND consumed_at IS NULL
          AND decision IS NOT NULL
      RETURNING decision`,
      [digest(token), expectedUid],
    );
    return result.rows[0]?.decision;
  }
}
