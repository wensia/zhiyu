ALTER TABLE ledger_transactions
ADD COLUMN pnl_scope TEXT NOT NULL DEFAULT 'counted'
CHECK (pnl_scope IN ('counted', 'excluded'));

UPDATE ledger_transactions AS t
SET pnl_scope = 'excluded'
WHERE EXISTS (
        SELECT 1
        FROM debts d
        WHERE d.transaction_id = t.id AND d.user_id = t.user_id
    )
   OR EXISTS (
        SELECT 1
        FROM debt_addition_events e
        WHERE e.transaction_id = t.id AND e.user_id = t.user_id
    )
   OR EXISTS (
        SELECT 1
        FROM repayment_events r
        WHERE r.transaction_id = t.id AND r.user_id = t.user_id
    );

CREATE INDEX idx_ledger_transactions_user_pnl_scope
ON ledger_transactions(user_id, pnl_scope);
