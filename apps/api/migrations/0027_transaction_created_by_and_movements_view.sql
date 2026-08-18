ALTER TABLE ledger_transactions
ADD COLUMN created_by TEXT NOT NULL DEFAULT 'user'
CHECK (created_by IN ('user', 'plugin:debts', 'plugin:bill-imports'));

UPDATE ledger_transactions
SET created_by = 'plugin:debts'
WHERE EXISTS (
        SELECT 1
        FROM debts d
        WHERE d.transaction_id = ledger_transactions.id
          AND d.user_id = ledger_transactions.user_id
          AND d.transaction_auto_created = 1
    )
   OR EXISTS (
        SELECT 1
        FROM debt_addition_events e
        WHERE e.transaction_id = ledger_transactions.id
          AND e.user_id = ledger_transactions.user_id
          AND e.transaction_auto_created = 1
    )
   OR EXISTS (
        SELECT 1
        FROM repayment_events e
        WHERE e.transaction_id = ledger_transactions.id
          AND e.user_id = ledger_transactions.user_id
          AND e.transaction_auto_created = 1
    );

UPDATE ledger_transactions
SET created_by = 'plugin:bill-imports'
WHERE created_by = 'user'
  AND (
      import_batch_id IS NOT NULL
      OR source_channel <> ''
      OR EXISTS (
          SELECT 1
          FROM import_records r
          WHERE r.transaction_id = ledger_transactions.id
      )
  );

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
WHERE t.kind = 'transfer' AND t.transfer_to_account_id IS NOT NULL AND t.archived_at IS NULL;
