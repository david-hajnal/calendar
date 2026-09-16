-- Rollback for migration 0023: caldav_accounts
-- Reverses: CREATE TABLE caldav_accounts, caldav_credentials
-- Safe to run even if forward migration was not applied.

DROP INDEX IF EXISTS idx_caldav_credentials_user_id;
DROP INDEX IF EXISTS idx_caldav_credentials_token_prefix;
DROP TABLE IF EXISTS caldav_credentials;
DROP TABLE IF EXISTS caldav_accounts;
