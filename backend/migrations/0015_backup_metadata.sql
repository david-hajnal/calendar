-- Migration 0015: backup_metadata
-- Creates backup_metadata table.
-- Idempotent: uses CREATE TABLE IF NOT EXISTS.

CREATE TABLE IF NOT EXISTS backup_metadata (
    id TEXT PRIMARY KEY,
    artifact_path TEXT NOT NULL,
    snapshot_sha256 TEXT NOT NULL,
    compressed_sha256 TEXT NOT NULL,
    snapshot_bytes INTEGER NOT NULL CHECK (snapshot_bytes > 0),
    compressed_bytes INTEGER NOT NULL CHECK (compressed_bytes > 0),
    integrity_check TEXT NOT NULL CHECK (integrity_check = 'ok'),
    created_at INTEGER NOT NULL
);
