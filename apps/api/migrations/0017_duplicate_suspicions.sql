CREATE TABLE duplicate_suspicions (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    transaction_id_a TEXT NOT NULL REFERENCES ledger_transactions(id) ON DELETE CASCADE,
    transaction_id_b TEXT NOT NULL REFERENCES ledger_transactions(id) ON DELETE CASCADE,
    score REAL NOT NULL CHECK (score >= 0 AND score <= 1),
    match_rule TEXT NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'open'
        CHECK (status IN ('open', 'confirmed', 'dismissed')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (transaction_id_a < transaction_id_b),
    UNIQUE(user_id, transaction_id_a, transaction_id_b)
);

CREATE INDEX idx_duplicate_suspicions_user_status
    ON duplicate_suspicions(user_id, status);
