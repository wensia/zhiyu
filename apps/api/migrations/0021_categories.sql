CREATE TABLE category_rules (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    priority INTEGER NOT NULL DEFAULT 100,
    enabled INTEGER NOT NULL DEFAULT 1,
    source_channel TEXT NOT NULL DEFAULT ''
        CHECK (source_channel IN ('', 'alipay', 'wechat', 'cmb', 'cmbc')),
    category_id TEXT NOT NULL REFERENCES categories(id) ON DELETE CASCADE,
    note TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE category_rule_conditions (
    id TEXT PRIMARY KEY,
    rule_id TEXT NOT NULL REFERENCES category_rules(id) ON DELETE CASCADE,
    match_field TEXT NOT NULL CHECK (match_field IN (
        'payee_key', 'payee_name', 'description', 'note',
        'channel_category', 'pay_method', 'merchant_order_id',
        'amount_cents', 'kind'
    )),
    match_kind TEXT NOT NULL CHECK (match_kind IN (
        'exact', 'contains', 'prefix', 'gte', 'lte'
    )),
    match_value TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX idx_category_rule_conditions_rule ON category_rule_conditions(rule_id);
