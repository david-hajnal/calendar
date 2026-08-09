-- Rollback for migration 0012: public_view_links
-- Reverses: CREATE TABLE public_view_links
-- Safe to run even if forward migration was not applied.

DROP TABLE IF EXISTS public_view_links;
