-- Rollback for migration 0019: default_admin_user
-- Removes the default admin user if it exists.

DELETE FROM users
WHERE normalized_email = 'admin@localhost'
  AND is_superadmin = 1;
