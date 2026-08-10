CREATE TABLE handoff_tickets (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    expires_at TEXT NOT NULL,
    consumed_at TEXT,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_handoff_tickets_expiry ON handoff_tickets(expires_at);
CREATE INDEX idx_handoff_tickets_user ON handoff_tickets(user_id);
