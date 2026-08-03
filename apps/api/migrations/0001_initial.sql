CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL
);

CREATE TABLE users (
    id TEXT PRIMARY KEY,
    email TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    timezone TEXT NOT NULL DEFAULT 'Asia/Shanghai',
    email_verified_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);
CREATE INDEX idx_sessions_user ON sessions(user_id);
CREATE INDEX idx_sessions_expiry ON sessions(expires_at);

CREATE TABLE email_tokens (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    purpose TEXT NOT NULL CHECK (purpose IN ('verify_email', 'reset_password')),
    token_hash TEXT NOT NULL UNIQUE,
    expires_at TEXT NOT NULL,
    consumed_at TEXT,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_email_tokens_lookup ON email_tokens(token_hash, purpose, expires_at);

CREATE TABLE counterparties (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    display_name TEXT NOT NULL CHECK (length(trim(display_name)) > 0),
    normalized_name TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    archived_at TEXT,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_counterparties_user_name ON counterparties(user_id, normalized_name);

CREATE TABLE debts (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    counterparty_id TEXT NOT NULL REFERENCES counterparties(id),
    direction TEXT NOT NULL CHECK (direction IN ('borrow_in', 'lend_out')),
    principal_cents INTEGER NOT NULL CHECK (principal_cents > 0 AND principal_cents <= 9007199254740991),
    currency TEXT NOT NULL DEFAULT 'CNY' CHECK (length(currency) = 3),
    occurred_on TEXT NOT NULL,
    due_on TEXT,
    note TEXT NOT NULL DEFAULT '',
    archived_at TEXT,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_debts_user_updated ON debts(user_id, updated_at DESC, id DESC);
CREATE INDEX idx_debts_user_due ON debts(user_id, due_on) WHERE archived_at IS NULL;
CREATE INDEX idx_debts_counterparty ON debts(user_id, counterparty_id);

CREATE TABLE repayment_events (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    debt_id TEXT NOT NULL REFERENCES debts(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('payment', 'reversal')),
    amount_cents INTEGER NOT NULL CHECK (amount_cents > 0 AND amount_cents <= 9007199254740991),
    effective_on TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    reverses_event_id TEXT REFERENCES repayment_events(id),
    created_at TEXT NOT NULL,
    CHECK ((kind = 'payment' AND reverses_event_id IS NULL) OR (kind = 'reversal' AND reverses_event_id IS NOT NULL)),
    UNIQUE(reverses_event_id)
);
CREATE INDEX idx_repayment_events_debt ON repayment_events(user_id, debt_id, created_at);

CREATE TABLE idempotency_records (
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    idempotency_key TEXT NOT NULL,
    operation TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    response_status INTEGER NOT NULL,
    response_body TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY(user_id, idempotency_key, operation)
);

CREATE VIEW debt_balances AS
SELECT
    d.id AS debt_id,
    COALESCE(SUM(CASE e.kind WHEN 'payment' THEN e.amount_cents WHEN 'reversal' THEN -e.amount_cents ELSE 0 END), 0) AS paid_cents,
    d.principal_cents - COALESCE(SUM(CASE e.kind WHEN 'payment' THEN e.amount_cents WHEN 'reversal' THEN -e.amount_cents ELSE 0 END), 0) AS remaining_cents,
    COUNT(e.id) AS event_count
FROM debts d
LEFT JOIN repayment_events e ON e.debt_id = d.id
GROUP BY d.id;
