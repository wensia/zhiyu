CREATE TABLE ledger_transactions (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('income', 'expense')),
    amount_cents INTEGER NOT NULL CHECK (amount_cents > 0 AND amount_cents <= 9007199254740991),
    occurred_on TEXT NOT NULL,
    category TEXT NOT NULL DEFAULT '' CHECK (length(category) <= 60),
    account_id TEXT REFERENCES ledger_accounts(id) ON DELETE SET NULL,
    note TEXT NOT NULL DEFAULT '',
    archived_at TEXT,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_ledger_transactions_user_date ON ledger_transactions(user_id, occurred_on, id);
CREATE INDEX idx_ledger_transactions_user_category ON ledger_transactions(user_id, category);
CREATE INDEX idx_ledger_transactions_account ON ledger_transactions(user_id, account_id);

ALTER TABLE ledger_accounts ADD COLUMN opening_balance_cents INTEGER NOT NULL DEFAULT 0;

CREATE VIEW ledger_account_movements AS
SELECT t.account_id AS account_id, t.user_id AS user_id,
       CASE t.kind WHEN 'income' THEN t.amount_cents ELSE -t.amount_cents END AS delta_cents
FROM ledger_transactions t
WHERE t.account_id IS NOT NULL AND t.archived_at IS NULL
UNION ALL
SELECT d.account_id, d.user_id,
       CASE d.direction WHEN 'borrow_in' THEN 1 ELSE -1 END
         * (d.principal_cents - COALESCE((SELECT SUM(a.amount_cents) FROM debt_addition_events a WHERE a.debt_id = d.id), 0))
FROM debts d
WHERE d.account_id IS NOT NULL AND d.origin_kind = 'cash_movement'
UNION ALL
SELECT e.account_id, e.user_id,
       CASE d.direction WHEN 'borrow_in' THEN 1 ELSE -1 END * e.amount_cents
FROM debt_addition_events e JOIN debts d ON d.id = e.debt_id
WHERE e.account_id IS NOT NULL AND d.origin_kind = 'cash_movement'
UNION ALL
SELECT e.account_id, e.user_id,
       CASE WHEN (d.direction = 'lend_out' AND e.kind = 'payment')
              OR (d.direction = 'borrow_in' AND e.kind = 'reversal')
            THEN 1 ELSE -1 END * e.amount_cents
FROM repayment_events e JOIN debts d ON d.id = e.debt_id
WHERE e.account_id IS NOT NULL AND d.origin_kind = 'cash_movement';

CREATE VIEW ledger_account_balances AS
SELECT a.id AS account_id,
       a.opening_balance_cents + COALESCE((
           SELECT SUM(m.delta_cents) FROM ledger_account_movements m
           WHERE m.account_id = a.id AND m.user_id = a.user_id
       ), 0) AS balance_cents
FROM ledger_accounts a;
