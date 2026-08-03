CREATE TABLE ledger_accounts (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    normalized_name TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    archived_at TEXT,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE UNIQUE INDEX idx_ledger_accounts_name ON ledger_accounts(user_id, normalized_name);
CREATE INDEX idx_ledger_accounts_user_name ON ledger_accounts(user_id, normalized_name, id);

ALTER TABLE debts ADD COLUMN account_id TEXT REFERENCES ledger_accounts(id) ON DELETE SET NULL;
ALTER TABLE debt_addition_events ADD COLUMN account_id TEXT REFERENCES ledger_accounts(id) ON DELETE SET NULL;
ALTER TABLE repayment_events ADD COLUMN account_id TEXT REFERENCES ledger_accounts(id) ON DELETE SET NULL;

CREATE INDEX idx_debts_account ON debts(user_id, account_id);
CREATE INDEX idx_debt_addition_events_account ON debt_addition_events(user_id, account_id);
CREATE INDEX idx_repayment_events_account ON repayment_events(user_id, account_id);

CREATE TABLE ledger_account_backfill_labels (
    source_kind TEXT NOT NULL,
    source_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    account_name TEXT NOT NULL,
    PRIMARY KEY(source_kind, source_id)
);

INSERT OR IGNORE INTO ledger_account_backfill_labels(source_kind, source_id, user_id, account_name)
SELECT
    'debt',
    id,
    user_id,
    trim(substr(
        substr(note, instr(note, '付款账户：') + length('付款账户：')),
        1,
        CASE instr(substr(note, instr(note, '付款账户：') + length('付款账户：')), '；')
            WHEN 0 THEN length(substr(note, instr(note, '付款账户：') + length('付款账户：')))
            ELSE instr(substr(note, instr(note, '付款账户：') + length('付款账户：')), '；') - 1
        END
    ))
FROM debts
WHERE instr(note, '付款账户：') > 0;

INSERT OR IGNORE INTO ledger_account_backfill_labels(source_kind, source_id, user_id, account_name)
SELECT
    'debt',
    id,
    user_id,
    trim(substr(
        substr(note, instr(note, '收款账户：') + length('收款账户：')),
        1,
        CASE instr(substr(note, instr(note, '收款账户：') + length('收款账户：')), '；')
            WHEN 0 THEN length(substr(note, instr(note, '收款账户：') + length('收款账户：')))
            ELSE instr(substr(note, instr(note, '收款账户：') + length('收款账户：')), '；') - 1
        END
    ))
FROM debts
WHERE instr(note, '收款账户：') > 0;

INSERT OR IGNORE INTO ledger_account_backfill_labels(source_kind, source_id, user_id, account_name)
SELECT
    'addition',
    id,
    user_id,
    trim(substr(
        substr(note, instr(note, '付款账户：') + length('付款账户：')),
        1,
        CASE instr(substr(note, instr(note, '付款账户：') + length('付款账户：')), '；')
            WHEN 0 THEN length(substr(note, instr(note, '付款账户：') + length('付款账户：')))
            ELSE instr(substr(note, instr(note, '付款账户：') + length('付款账户：')), '；') - 1
        END
    ))
FROM debt_addition_events
WHERE instr(note, '付款账户：') > 0;

INSERT OR IGNORE INTO ledger_account_backfill_labels(source_kind, source_id, user_id, account_name)
SELECT
    'addition',
    id,
    user_id,
    trim(substr(
        substr(note, instr(note, '收款账户：') + length('收款账户：')),
        1,
        CASE instr(substr(note, instr(note, '收款账户：') + length('收款账户：')), '；')
            WHEN 0 THEN length(substr(note, instr(note, '收款账户：') + length('收款账户：')))
            ELSE instr(substr(note, instr(note, '收款账户：') + length('收款账户：')), '；') - 1
        END
    ))
FROM debt_addition_events
WHERE instr(note, '收款账户：') > 0;

INSERT OR IGNORE INTO ledger_account_backfill_labels(source_kind, source_id, user_id, account_name)
SELECT
    'repayment',
    id,
    user_id,
    trim(substr(
        substr(note, instr(note, '付款账户：') + length('付款账户：')),
        1,
        CASE instr(substr(note, instr(note, '付款账户：') + length('付款账户：')), '；')
            WHEN 0 THEN length(substr(note, instr(note, '付款账户：') + length('付款账户：')))
            ELSE instr(substr(note, instr(note, '付款账户：') + length('付款账户：')), '；') - 1
        END
    ))
FROM repayment_events
WHERE instr(note, '付款账户：') > 0;

INSERT OR IGNORE INTO ledger_account_backfill_labels(source_kind, source_id, user_id, account_name)
SELECT
    'repayment',
    id,
    user_id,
    trim(substr(
        substr(note, instr(note, '收款账户：') + length('收款账户：')),
        1,
        CASE instr(substr(note, instr(note, '收款账户：') + length('收款账户：')), '；')
            WHEN 0 THEN length(substr(note, instr(note, '收款账户：') + length('收款账户：')))
            ELSE instr(substr(note, instr(note, '收款账户：') + length('收款账户：')), '；') - 1
        END
    ))
FROM repayment_events
WHERE instr(note, '收款账户：') > 0;

DELETE FROM ledger_account_backfill_labels WHERE account_name = '' OR account_name = '无';

INSERT INTO ledger_accounts(id, user_id, name, normalized_name, note, created_at, updated_at)
SELECT
    'legacy-' || lower(hex(randomblob(16))),
    user_id,
    min(account_name),
    lower(account_name),
    '从历史往来记录自动识别',
    strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
    strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM ledger_account_backfill_labels
GROUP BY user_id, lower(account_name);

UPDATE debts
SET account_id = (
    SELECT account.id
    FROM ledger_account_backfill_labels AS label
    JOIN ledger_accounts AS account
      ON account.user_id = label.user_id
     AND account.normalized_name = lower(label.account_name)
    WHERE label.source_kind = 'debt' AND label.source_id = debts.id
)
WHERE EXISTS (
    SELECT 1 FROM ledger_account_backfill_labels AS label
    WHERE label.source_kind = 'debt' AND label.source_id = debts.id
);

UPDATE debt_addition_events
SET account_id = (
    SELECT account.id
    FROM ledger_account_backfill_labels AS label
    JOIN ledger_accounts AS account
      ON account.user_id = label.user_id
     AND account.normalized_name = lower(label.account_name)
    WHERE label.source_kind = 'addition' AND label.source_id = debt_addition_events.id
)
WHERE EXISTS (
    SELECT 1 FROM ledger_account_backfill_labels AS label
    WHERE label.source_kind = 'addition' AND label.source_id = debt_addition_events.id
);

UPDATE repayment_events
SET account_id = (
    SELECT account.id
    FROM ledger_account_backfill_labels AS label
    JOIN ledger_accounts AS account
      ON account.user_id = label.user_id
     AND account.normalized_name = lower(label.account_name)
    WHERE label.source_kind = 'repayment' AND label.source_id = repayment_events.id
)
WHERE kind = 'payment' AND EXISTS (
    SELECT 1 FROM ledger_account_backfill_labels AS label
    WHERE label.source_kind = 'repayment' AND label.source_id = repayment_events.id
);

UPDATE repayment_events
SET account_id = (
    SELECT original.account_id
    FROM repayment_events AS original
    WHERE original.id = repayment_events.reverses_event_id
)
WHERE kind = 'reversal';

DROP TABLE ledger_account_backfill_labels;
