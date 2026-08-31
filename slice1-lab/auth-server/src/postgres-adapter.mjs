let adapterPool;

export function configureAdapter(pool) {
  adapterPool = pool;
}

function pool() {
  if (!adapterPool) throw new Error('PostgresAdapter used before configureAdapter');
  return adapterPool;
}

function epochSeconds() {
  return Math.floor(Date.now() / 1000);
}

export default class PostgresAdapter {
  constructor(modelName) {
    this.modelName = modelName;
  }

  async upsert(id, payload, expiresIn) {
    const expiresAt = typeof expiresIn === 'number'
      ? new Date(Date.now() + expiresIn * 1000)
      : null;
    await pool().query(
      `INSERT INTO provider_entity
         (model, id, payload, grant_id, uid, user_code, expires_at, consumed_at)
       VALUES ($1, $2, $3::jsonb, $4, $5, $6, $7, NULL)
       ON CONFLICT (model, id) DO UPDATE SET
         payload = EXCLUDED.payload,
         grant_id = EXCLUDED.grant_id,
         uid = EXCLUDED.uid,
         user_code = EXCLUDED.user_code,
         expires_at = EXCLUDED.expires_at,
         consumed_at = NULL`,
      [
        this.modelName,
        id,
        JSON.stringify(payload),
        payload.grantId ?? null,
        payload.uid ?? null,
        payload.userCode ?? null,
        expiresAt,
      ],
    );
  }

  async find(id) {
    const result = await pool().query(
      `SELECT payload
         FROM provider_entity
        WHERE model = $1 AND id = $2
          AND (expires_at IS NULL OR expires_at > NOW())`,
      [this.modelName, id],
    );
    return result.rows[0]?.payload;
  }

  async findByUid(uid) {
    const result = await pool().query(
      `SELECT payload
         FROM provider_entity
        WHERE model = $1 AND uid = $2
          AND (expires_at IS NULL OR expires_at > NOW())
        LIMIT 1`,
      [this.modelName, uid],
    );
    return result.rows[0]?.payload;
  }

  async findByUserCode(userCode) {
    const result = await pool().query(
      `SELECT payload
         FROM provider_entity
        WHERE model = $1 AND user_code = $2
          AND (expires_at IS NULL OR expires_at > NOW())
        LIMIT 1`,
      [this.modelName, userCode],
    );
    return result.rows[0]?.payload;
  }

  async destroy(id) {
    await pool().query(
      'DELETE FROM provider_entity WHERE model = $1 AND id = $2',
      [this.modelName, id],
    );
  }

  async revokeByGrantId(grantId) {
    await pool().query(
      'DELETE FROM provider_entity WHERE grant_id = $1',
      [grantId],
    );
  }

  async consume(id) {
    const consumed = epochSeconds();
    await pool().query(
      `UPDATE provider_entity
          SET payload = payload || jsonb_build_object('consumed', $3::bigint),
              consumed_at = NOW()
        WHERE model = $1 AND id = $2
          AND consumed_at IS NULL
          AND (expires_at IS NULL OR expires_at > NOW())`,
      [this.modelName, id, consumed],
    );
  }
}
