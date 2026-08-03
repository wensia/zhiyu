CREATE TABLE debt_addition_events (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    debt_id TEXT NOT NULL REFERENCES debts(id) ON DELETE CASCADE,
    amount_cents INTEGER NOT NULL CHECK (amount_cents > 0 AND amount_cents <= 9007199254740991),
    effective_on TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL
);
CREATE INDEX idx_debt_addition_events_debt ON debt_addition_events(debt_id, user_id, effective_on DESC, created_at DESC);

DROP VIEW debt_balances;
CREATE VIEW debt_balances AS
SELECT
    d.id AS debt_id,
    COALESCE(SUM(CASE e.kind WHEN 'payment' THEN e.amount_cents WHEN 'reversal' THEN -e.amount_cents ELSE 0 END), 0) AS paid_cents,
    d.principal_cents - COALESCE(SUM(CASE e.kind WHEN 'payment' THEN e.amount_cents WHEN 'reversal' THEN -e.amount_cents ELSE 0 END), 0) AS remaining_cents,
    COUNT(e.id) + (SELECT COUNT(*) FROM debt_addition_events a WHERE a.debt_id = d.id) AS event_count
FROM debts d
LEFT JOIN repayment_events e ON e.debt_id = d.id
GROUP BY d.id;
