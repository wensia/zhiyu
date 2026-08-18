CREATE TABLE import_account_mappings (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    source_channel TEXT NOT NULL CHECK (source_channel IN ('alipay','wechat')),
    pay_method TEXT NOT NULL CHECK (length(trim(pay_method)) > 0),
    account_id TEXT REFERENCES ledger_accounts(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(user_id, source_channel, pay_method)
);
