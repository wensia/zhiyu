ALTER TABLE debts ADD COLUMN transaction_auto_created INTEGER NOT NULL DEFAULT 0;
ALTER TABLE debt_addition_events ADD COLUMN transaction_auto_created INTEGER NOT NULL DEFAULT 0;
ALTER TABLE repayment_events ADD COLUMN transaction_auto_created INTEGER NOT NULL DEFAULT 0;

CREATE TEMP TABLE debt_cash_migration_0024 (
    source_kind TEXT NOT NULL,
    source_id TEXT NOT NULL,
    transaction_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    amount_cents INTEGER NOT NULL CHECK (amount_cents > 0),
    currency TEXT NOT NULL,
    occurred_on TEXT NOT NULL,
    account_id TEXT NOT NULL,
    description TEXT NOT NULL,
    PRIMARY KEY (source_kind, source_id),
    UNIQUE (transaction_id)
);

INSERT INTO debt_cash_migration_0024 (
    source_kind, source_id, transaction_id, user_id, kind, amount_cents,
    currency, occurred_on, account_id, description
)
SELECT
    'principal', d.id, lower(hex(randomblob(16))), d.user_id,
    CASE d.direction WHEN 'borrow_in' THEN 'income' ELSE 'expense' END,
    d.principal_cents - COALESCE((
        SELECT SUM(a.amount_cents)
        FROM debt_addition_events a
        WHERE a.debt_id = d.id
    ), 0),
    d.currency, d.occurred_on, d.account_id,
    CASE d.direction WHEN 'borrow_in' THEN '债务本金（借入）' ELSE '债务本金（借出）' END
FROM debts d
WHERE d.origin_kind = 'cash_movement'
  AND d.account_id IS NOT NULL
  AND d.transaction_id IS NULL
  -- 本金减去追加之和 ≤ 0 的记录对余额贡献 ≤ 0，且流水金额必须为正；跳过它才是等价，
  -- 否则 CHECK 会让整个迁移中止、服务起不来。这类记录继续留在视图的债务分支里。
  AND d.principal_cents - COALESCE((
        SELECT SUM(a.amount_cents)
        FROM debt_addition_events a
        WHERE a.debt_id = d.id
      ), 0) > 0;

INSERT INTO debt_cash_migration_0024 (
    source_kind, source_id, transaction_id, user_id, kind, amount_cents,
    currency, occurred_on, account_id, description
)
SELECT
    'addition', e.id, lower(hex(randomblob(16))), e.user_id,
    CASE d.direction WHEN 'borrow_in' THEN 'income' ELSE 'expense' END,
    e.amount_cents, d.currency, e.effective_on, e.account_id,
    CASE d.direction WHEN 'borrow_in' THEN '追加借款（借入）' ELSE '追加借款（借出）' END
FROM debt_addition_events e
JOIN debts d ON d.id = e.debt_id AND d.user_id = e.user_id
WHERE d.origin_kind = 'cash_movement'
  AND e.account_id IS NOT NULL
  AND e.transaction_id IS NULL;

INSERT INTO debt_cash_migration_0024 (
    source_kind, source_id, transaction_id, user_id, kind, amount_cents,
    currency, occurred_on, account_id, description
)
SELECT
    'repayment', e.id, lower(hex(randomblob(16))), e.user_id,
    CASE
        WHEN (d.direction = 'lend_out' AND e.kind = 'payment')
          OR (d.direction = 'borrow_in' AND e.kind = 'reversal')
        THEN 'income'
        ELSE 'expense'
    END,
    e.amount_cents, d.currency, e.effective_on, e.account_id,
    CASE
        WHEN e.kind = 'reversal' AND d.direction = 'borrow_in' THEN '撤销还款（借入）'
        WHEN e.kind = 'reversal' THEN '撤销还款（借出）'
        WHEN d.direction = 'borrow_in' THEN '还款（借入）'
        ELSE '还款（借出）'
    END
FROM repayment_events e
JOIN debts d ON d.id = e.debt_id AND d.user_id = e.user_id
WHERE d.origin_kind = 'cash_movement'
  AND e.account_id IS NOT NULL
  AND e.transaction_id IS NULL;

INSERT INTO ledger_transactions (
    id, user_id, kind, amount_cents, currency, occurred_on,
    occurred_at_precision, description, account_id, note,
    version, created_at, updated_at
)
SELECT
    transaction_id, user_id, kind, amount_cents, currency, occurred_on,
    'day', description, account_id, description,
    1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM debt_cash_migration_0024;

UPDATE debts
SET transaction_id = (
        SELECT m.transaction_id
        FROM debt_cash_migration_0024 m
        WHERE m.source_kind = 'principal' AND m.source_id = debts.id
    ),
    transaction_auto_created = 1
WHERE EXISTS (
    SELECT 1 FROM debt_cash_migration_0024 m
    WHERE m.source_kind = 'principal' AND m.source_id = debts.id
);

UPDATE debt_addition_events
SET transaction_id = (
        SELECT m.transaction_id
        FROM debt_cash_migration_0024 m
        WHERE m.source_kind = 'addition' AND m.source_id = debt_addition_events.id
    ),
    transaction_auto_created = 1
WHERE EXISTS (
    SELECT 1 FROM debt_cash_migration_0024 m
    WHERE m.source_kind = 'addition' AND m.source_id = debt_addition_events.id
);

UPDATE repayment_events
SET transaction_id = (
        SELECT m.transaction_id
        FROM debt_cash_migration_0024 m
        WHERE m.source_kind = 'repayment' AND m.source_id = repayment_events.id
    ),
    transaction_auto_created = 1
WHERE EXISTS (
    SELECT 1 FROM debt_cash_migration_0024 m
    WHERE m.source_kind = 'repayment' AND m.source_id = repayment_events.id
);

DROP TABLE debt_cash_migration_0024;
