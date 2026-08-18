DROP VIEW ledger_account_balances;
DROP VIEW ledger_account_movements;

CREATE TABLE payees (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    display_name TEXT NOT NULL CHECK (length(trim(display_name)) > 0),
    normalized_name TEXT NOT NULL CHECK (length(trim(normalized_name)) > 0),
    kind TEXT NOT NULL DEFAULT 'unknown'
        CHECK (kind IN ('merchant', 'person', 'platform', 'unknown')),
    counterparty_id TEXT REFERENCES counterparties(id) ON DELETE SET NULL,
    note TEXT NOT NULL DEFAULT '',
    archived_at TEXT,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(user_id, normalized_name)
);

CREATE TABLE payee_aliases (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    payee_id TEXT NOT NULL REFERENCES payees(id) ON DELETE CASCADE,
    source_channel TEXT NOT NULL DEFAULT '',
    alias TEXT NOT NULL CHECK (length(trim(alias)) > 0),
    normalized_alias TEXT NOT NULL CHECK (length(trim(normalized_alias)) > 0),
    created_at TEXT NOT NULL,
    UNIQUE(user_id, source_channel, normalized_alias)
);

CREATE TABLE categories (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    parent_id TEXT REFERENCES categories(id) ON DELETE SET NULL,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0 AND length(name) <= 60),
    normalized_name TEXT NOT NULL CHECK (length(trim(normalized_name)) > 0),
    kind TEXT NOT NULL CHECK (kind IN ('income', 'expense')),
    sort_order INTEGER NOT NULL DEFAULT 0,
    archived_at TEXT,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE UNIQUE INDEX idx_categories_unique_name
    ON categories(user_id, COALESCE(parent_id, ''), normalized_name);

CREATE TABLE ledger_transactions_new (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('income', 'expense', 'transfer')),
    amount_cents INTEGER NOT NULL CHECK (amount_cents > 0 AND amount_cents <= 9007199254740991),
    currency TEXT NOT NULL DEFAULT 'CNY' CHECK (length(currency) = 3),
    occurred_on TEXT NOT NULL,
    occurred_at TEXT,
    occurred_at_precision TEXT NOT NULL DEFAULT 'day'
        CHECK (occurred_at_precision IN ('second', 'day')),
    category TEXT NOT NULL DEFAULT '' CHECK (length(category) <= 60),
    category_id TEXT REFERENCES categories(id) ON DELETE SET NULL,
    category_source TEXT NOT NULL DEFAULT 'none'
        CHECK (category_source IN ('none', 'user', 'rule', 'import')),
    payee_id TEXT REFERENCES payees(id) ON DELETE SET NULL,
    payee_name TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    account_id TEXT REFERENCES ledger_accounts(id) ON DELETE SET NULL,
    transfer_from_account_id TEXT REFERENCES ledger_accounts(id) ON DELETE SET NULL,
    transfer_to_account_id TEXT REFERENCES ledger_accounts(id) ON DELETE SET NULL,
    note TEXT NOT NULL DEFAULT '',
    archived_at TEXT,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (kind <> 'transfer' OR account_id IS NULL),
    CHECK (kind = 'transfer' OR (transfer_from_account_id IS NULL AND transfer_to_account_id IS NULL)),
    CHECK (kind <> 'transfer' OR transfer_from_account_id IS NOT NULL OR transfer_to_account_id IS NOT NULL),
    CHECK (transfer_from_account_id IS NULL OR transfer_to_account_id IS NULL
           OR transfer_from_account_id <> transfer_to_account_id)
);

INSERT INTO ledger_transactions_new (
    id,
    user_id,
    kind,
    amount_cents,
    occurred_on,
    category,
    account_id,
    note,
    archived_at,
    version,
    created_at,
    updated_at
)
SELECT
    id,
    user_id,
    kind,
    amount_cents,
    occurred_on,
    category,
    account_id,
    note,
    archived_at,
    version,
    created_at,
    updated_at
FROM ledger_transactions;

DROP TABLE ledger_transactions;
ALTER TABLE ledger_transactions_new RENAME TO ledger_transactions;

CREATE INDEX idx_ledger_transactions_user_date ON ledger_transactions(user_id, occurred_on, id);
CREATE INDEX idx_ledger_transactions_user_category ON ledger_transactions(user_id, category);
CREATE INDEX idx_ledger_transactions_account ON ledger_transactions(user_id, account_id);
CREATE INDEX idx_ledger_transactions_payee ON ledger_transactions(user_id, payee_id);
CREATE INDEX idx_ledger_transactions_category_id ON ledger_transactions(user_id, category_id);
CREATE INDEX idx_ledger_transactions_transfer_from ON ledger_transactions(user_id, transfer_from_account_id);
CREATE INDEX idx_ledger_transactions_transfer_to ON ledger_transactions(user_id, transfer_to_account_id);

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
