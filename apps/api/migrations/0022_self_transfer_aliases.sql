CREATE TABLE self_transfer_aliases (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    alias TEXT NOT NULL CHECK (length(trim(alias)) > 0 AND length(alias) <= 60),
    normalized_alias TEXT NOT NULL CHECK (length(trim(normalized_alias)) > 0),
    note TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(user_id, normalized_alias)
);
