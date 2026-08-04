ALTER TABLE backup_metadata ADD COLUMN encryption_algorithm TEXT;
ALTER TABLE backup_metadata ADD COLUMN encrypted_sha256 TEXT;
ALTER TABLE backup_metadata ADD COLUMN encrypted_bytes INTEGER;
ALTER TABLE backup_metadata ADD COLUMN upload_status TEXT NOT NULL DEFAULT 'not_requested'
    CHECK (upload_status IN ('not_requested', 'uploaded', 'failed'));
