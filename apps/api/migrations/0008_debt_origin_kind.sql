ALTER TABLE debts ADD COLUMN origin_kind TEXT NOT NULL DEFAULT 'cash_movement' CHECK (origin_kind IN ('cash_movement', 'no_cash_movement', 'legacy_unknown'));
UPDATE debts SET origin_kind = 'legacy_unknown' WHERE account_id IS NULL;
