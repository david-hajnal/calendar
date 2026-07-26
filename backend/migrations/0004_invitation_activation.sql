ALTER TABLE users
ADD COLUMN is_superadmin INTEGER NOT NULL DEFAULT 0
CHECK (is_superadmin IN (0, 1));
