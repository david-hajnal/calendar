-- Migration 0021: caldav_token
-- Adds CalDAV publishing support to public_view_links.

ALTER TABLE public_view_links ADD COLUMN caldav_token_hash BLOB;
ALTER TABLE public_view_links ADD COLUMN caldav_enabled INTEGER DEFAULT 0 CHECK (caldav_enabled IN (0, 1));

CREATE INDEX public_view_links_caldav_lookup_idx
    ON public_view_links(caldav_token_hash);
