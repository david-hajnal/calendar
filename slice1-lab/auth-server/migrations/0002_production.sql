CREATE TABLE IF NOT EXISTS authorization_audit (
    id BIGSERIAL PRIMARY KEY,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    event TEXT NOT NULL,
    detail JSONB NOT NULL
);
CREATE INDEX IF NOT EXISTS authorization_audit_created_idx ON authorization_audit (created_at);
CREATE TABLE IF NOT EXISTS dcr_rate_bucket (
    bucket_key TEXT PRIMARY KEY,
    window_start TIMESTAMPTZ NOT NULL,
    count INTEGER NOT NULL
);
