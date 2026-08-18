CREATE TABLE transaction_events (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('consume', 'transfer', 'refund')),
    -- consume: one purchase, potentially evidenced by multiple channel records.
    -- transfer: an account transfer, potentially split into transfer and fee transactions.
    -- refund: a refund or reversal.
    note TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX idx_transaction_events_user
    ON transaction_events(user_id, created_at);

ALTER TABLE ledger_transactions ADD COLUMN event_id TEXT
    REFERENCES transaction_events(id) ON DELETE SET NULL;

ALTER TABLE ledger_transactions ADD COLUMN payee_key TEXT NOT NULL DEFAULT '';

CREATE INDEX idx_ledger_transactions_event
    ON ledger_transactions(user_id, event_id);

CREATE INDEX idx_ledger_transactions_payee_key
    ON ledger_transactions(user_id, payee_key);

ALTER TABLE import_records
    ADD COLUMN counterparty_normalized TEXT NOT NULL DEFAULT '';

ALTER TABLE import_records
    ADD COLUMN normalization_version INTEGER NOT NULL DEFAULT 0;
