CREATE TABLE IF NOT EXISTS provider_entity (
    model TEXT NOT NULL,
    id TEXT NOT NULL,
    payload JSONB NOT NULL,
    grant_id TEXT,
    uid TEXT,
    user_code TEXT,
    expires_at TIMESTAMPTZ,
    consumed_at TIMESTAMPTZ,
    PRIMARY KEY (model, id)
);

CREATE INDEX IF NOT EXISTS provider_entity_grant_idx
    ON provider_entity (grant_id) WHERE grant_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS provider_entity_uid_idx
    ON provider_entity (model, uid) WHERE uid IS NOT NULL;
CREATE INDEX IF NOT EXISTS provider_entity_user_code_idx
    ON provider_entity (model, user_code) WHERE user_code IS NOT NULL;
CREATE INDEX IF NOT EXISTS provider_entity_expiry_idx
    ON provider_entity (expires_at) WHERE expires_at IS NOT NULL;

CREATE TABLE IF NOT EXISTS interaction_handoff (
    token_hash TEXT PRIMARY KEY,
    interaction_uid TEXT NOT NULL,
    view JSONB NOT NULL,
    decision JSONB,
    expires_at TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS interaction_handoff_expiry_idx
    ON interaction_handoff (expires_at);
