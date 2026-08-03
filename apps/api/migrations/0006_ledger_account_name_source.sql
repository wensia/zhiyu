ALTER TABLE ledger_accounts ADD COLUMN name_source TEXT NOT NULL DEFAULT 'custom'
    CHECK (name_source IN ('custom', 'derived'));
