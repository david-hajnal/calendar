CREATE TABLE public_view_links (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    view_id INTEGER NOT NULL UNIQUE REFERENCES shared_views(id) ON DELETE CASCADE,
    token_prefix TEXT NOT NULL UNIQUE CHECK (length(token_prefix) = 8),
    token_hash BLOB NOT NULL CHECK (length(token_hash) = 32),
    projection TEXT NOT NULL
        CHECK (projection IN ('full_details', 'title_and_time', 'free_busy')),
    display_timezone TEXT NOT NULL CHECK (length(trim(display_timezone)) > 0),
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER,
    version INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX public_view_links_active_lookup_idx
    ON public_view_links(token_prefix, expires_at, revoked_at);
