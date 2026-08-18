ALTER TABLE ledger_transactions
ADD COLUMN category_rule_id TEXT REFERENCES category_rules(id) ON DELETE SET NULL;
