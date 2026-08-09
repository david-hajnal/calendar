-- Migration 0019: default_admin_user
-- Creates a default superadmin user for initial access.
-- Idempotent: only inserts if the users table is empty.
-- Rollback: see 0019_rollback.sql

-- Insert default admin only when no users exist yet.
-- Uses a pre-computed bcrypt hash (cost 12) for the password.
-- Password: admin-default-password-2026
-- Production: change password after first login or set DEFAULT_ADMIN_PASSWORD env var.
INSERT INTO users (normalized_email, display_name, status, is_superadmin, password_hash, created_at)
SELECT
    'admin@localhost',
    'Admin',
    'active',
    1,
    '$2b$12$fD89zgxmZWyvde6OqeCbhOC0MOIlwDrC6mRkV2Loz7hq439rz/y.6',
    0
WHERE NOT EXISTS (SELECT 1 FROM users LIMIT 1);
