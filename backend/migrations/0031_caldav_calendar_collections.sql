-- Collection names belong to their original principal, independently of ownership.
-- Existing numeric URLs need no backfill and remain reserved.
ALTER TABLE calendars ADD COLUMN dav_collection_key TEXT;
ALTER TABLE calendars ADD COLUMN dav_principal_user_id INTEGER REFERENCES caldav_accounts(user_id);
ALTER TABLE calendars ADD COLUMN dav_timezone TEXT;
CREATE UNIQUE INDEX calendars_dav_collection_key ON calendars(dav_principal_user_id, dav_collection_key)
WHERE dav_collection_key IS NOT NULL;
