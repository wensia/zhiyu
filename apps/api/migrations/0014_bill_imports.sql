CREATE TABLE import_batches (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    source_channel TEXT NOT NULL
        CHECK (source_channel IN ('alipay', 'wechat', 'cmb', 'cmbc')),
    parser_version INTEGER NOT NULL DEFAULT 1
        CHECK (parser_version > 0),
    file_name TEXT NOT NULL DEFAULT ''
        CHECK (length(file_name) <= 255),
    file_sha256 TEXT NOT NULL
        CHECK (length(file_sha256) = 64),
    period_start TEXT NOT NULL,
    period_end TEXT NOT NULL,
    total_count INTEGER NOT NULL CHECK (total_count > 0),
    status TEXT NOT NULL DEFAULT 'preview'
        CHECK (status IN ('preview', 'blocked', 'committed', 'discarded')),
    committed_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (period_start <= period_end)
);

CREATE INDEX idx_import_batches_user
    ON import_batches(user_id, created_at DESC, id DESC);

CREATE TABLE import_records (
    id TEXT PRIMARY KEY,
    batch_id TEXT NOT NULL
        REFERENCES import_batches(id) ON DELETE CASCADE,
    row_index INTEGER NOT NULL CHECK (row_index > 0),
    external_id TEXT NOT NULL
        CHECK (
            length(trim(external_id)) > 0
            AND length(external_id) <= 256
        ),
    merchant_order_id TEXT NOT NULL DEFAULT ''
        CHECK (length(merchant_order_id) <= 256),
    occurred_at TEXT NOT NULL,
    occurred_on TEXT NOT NULL,
    direction TEXT NOT NULL
        CHECK (direction IN ('income', 'expense', 'neutral')),
    amount_cents INTEGER NOT NULL
        CHECK (
            amount_cents >= 0
            AND amount_cents <= 9007199254740991
        ),
    channel_category TEXT NOT NULL DEFAULT ''
        CHECK (length(channel_category) <= 4096),
    counterparty TEXT NOT NULL DEFAULT ''
        CHECK (length(counterparty) <= 4096),
    product TEXT NOT NULL DEFAULT ''
        CHECK (length(product) <= 4096),
    pay_method TEXT NOT NULL DEFAULT ''
        CHECK (length(pay_method) <= 4096),
    channel_status TEXT NOT NULL DEFAULT ''
        CHECK (length(channel_status) <= 128),
    source_note TEXT NOT NULL DEFAULT ''
        CHECK (length(source_note) <= 4096),
    -- Date-only bank rows use 'day'; timestamped rows use 'second'.
    occurred_at_precision TEXT NOT NULL DEFAULT 'second'
        CHECK (occurred_at_precision IN ('second', 'day')),
    -- Counterparty bank/channel name supplied by the source statement.
    counter_channel_raw TEXT NOT NULL DEFAULT ''
        CHECK (length(counter_channel_raw) <= 4096),
    -- Counterparty account identifier supplied by the source statement.
    counterparty_account_raw TEXT NOT NULL DEFAULT ''
        CHECK (length(counterparty_account_raw) <= 4096),
    -- Post-transaction balance supplied by bank statements, in cents.
    balance_after_cents INTEGER,
    -- ISO 4217 currency code for the imported amount.
    currency TEXT NOT NULL DEFAULT 'CNY' CHECK (length(currency) = 3),
    -- Whether external_id came from the statement or a generated fingerprint.
    external_id_source TEXT NOT NULL DEFAULT 'native'
        CHECK (external_id_source IN ('native', 'fingerprint')),
    -- Allowlisted channel-specific fields only; never serialize a complete source row.
    raw_json TEXT NOT NULL DEFAULT '{}',
    disposition TEXT NOT NULL
        CHECK (
            disposition IN (
                'import',
                'pending',
                'neutral',
                'closed',
                'zero_amount',
                'unknown',
                'duplicate'
            )
        ),
    transaction_id TEXT
        REFERENCES ledger_transactions(id) ON DELETE SET NULL,
    created_at TEXT NOT NULL,

    CHECK (disposition <> 'neutral' OR direction = 'neutral'),
    CHECK (
        disposition NOT IN ('import', 'duplicate')
        OR (
            direction IN ('income', 'expense', 'neutral')
            AND amount_cents > 0
        )
    ),
    CHECK (
        disposition <> 'zero_amount'
        OR (
            direction IN ('income', 'expense', 'neutral')
            AND amount_cents = 0
        )
    ),
    CHECK (transaction_id IS NULL OR disposition = 'import'),
    UNIQUE(batch_id, row_index)
);

CREATE INDEX idx_import_records_batch
    ON import_records(batch_id, row_index);

CREATE UNIQUE INDEX idx_import_records_transaction
    ON import_records(transaction_id)
    WHERE transaction_id IS NOT NULL;

ALTER TABLE ledger_transactions
ADD COLUMN source_channel TEXT NOT NULL DEFAULT ''
CHECK (source_channel IN ('', 'alipay', 'wechat', 'cmb', 'cmbc'));

ALTER TABLE ledger_transactions
ADD COLUMN external_id TEXT NOT NULL DEFAULT ''
CHECK (
    (source_channel = '' AND external_id = '')
    OR
    (
        source_channel IN ('alipay', 'wechat', 'cmb', 'cmbc')
        AND length(trim(external_id)) > 0
    )
);

ALTER TABLE ledger_transactions
ADD COLUMN import_batch_id TEXT
REFERENCES import_batches(id) ON DELETE SET NULL
CHECK (
    import_batch_id IS NULL
    OR source_channel IN ('alipay', 'wechat', 'cmb', 'cmbc')
);

CREATE UNIQUE INDEX idx_ledger_transactions_external
    ON ledger_transactions(user_id, source_channel, external_id)
    WHERE external_id != '';

CREATE INDEX idx_ledger_transactions_import_batch
    ON ledger_transactions(user_id, import_batch_id)
    WHERE import_batch_id IS NOT NULL;
