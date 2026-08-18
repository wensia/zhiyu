ALTER TABLE repayment_events ADD COLUMN transaction_id TEXT REFERENCES ledger_transactions(id) ON DELETE SET NULL;
ALTER TABLE debt_addition_events ADD COLUMN transaction_id TEXT REFERENCES ledger_transactions(id) ON DELETE SET NULL;
ALTER TABLE debts ADD COLUMN transaction_id TEXT REFERENCES ledger_transactions(id) ON DELETE SET NULL;

CREATE UNIQUE INDEX idx_repayment_events_transaction ON repayment_events(transaction_id) WHERE transaction_id IS NOT NULL;
CREATE UNIQUE INDEX idx_debt_addition_events_transaction ON debt_addition_events(transaction_id) WHERE transaction_id IS NOT NULL;
CREATE UNIQUE INDEX idx_debts_transaction ON debts(transaction_id) WHERE transaction_id IS NOT NULL;

DROP VIEW ledger_account_movements;
CREATE VIEW ledger_account_movements AS
SELECT t.account_id AS account_id, t.user_id AS user_id,
       t.amount_cents AS delta_cents
FROM ledger_transactions t
WHERE t.kind = 'income' AND t.account_id IS NOT NULL AND t.archived_at IS NULL
UNION ALL
SELECT t.account_id, t.user_id, -t.amount_cents
FROM ledger_transactions t
WHERE t.kind = 'expense' AND t.account_id IS NOT NULL AND t.archived_at IS NULL
UNION ALL
SELECT t.transfer_from_account_id, t.user_id, -t.amount_cents
FROM ledger_transactions t
WHERE t.kind = 'transfer' AND t.transfer_from_account_id IS NOT NULL AND t.archived_at IS NULL
UNION ALL
SELECT t.transfer_to_account_id, t.user_id, t.amount_cents
FROM ledger_transactions t
WHERE t.kind = 'transfer' AND t.transfer_to_account_id IS NOT NULL AND t.archived_at IS NULL
UNION ALL
SELECT d.account_id, d.user_id,
       CASE d.direction WHEN 'borrow_in' THEN 1 ELSE -1 END
         * (d.principal_cents - COALESCE((SELECT SUM(a.amount_cents) FROM debt_addition_events a WHERE a.debt_id = d.id), 0))
FROM debts d
WHERE d.account_id IS NOT NULL AND d.origin_kind = 'cash_movement' AND d.transaction_id IS NULL
UNION ALL
SELECT e.account_id, e.user_id,
       CASE d.direction WHEN 'borrow_in' THEN 1 ELSE -1 END * e.amount_cents
FROM debt_addition_events e JOIN debts d ON d.id = e.debt_id
WHERE e.account_id IS NOT NULL AND d.origin_kind = 'cash_movement' AND e.transaction_id IS NULL
UNION ALL
SELECT e.account_id, e.user_id,
       CASE WHEN (d.direction = 'lend_out' AND e.kind = 'payment')
              OR (d.direction = 'borrow_in' AND e.kind = 'reversal')
            THEN 1 ELSE -1 END * e.amount_cents
FROM repayment_events e JOIN debts d ON d.id = e.debt_id
WHERE e.account_id IS NOT NULL AND d.origin_kind = 'cash_movement' AND e.transaction_id IS NULL;
