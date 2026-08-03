ALTER TABLE ledger_accounts ADD COLUMN account_type TEXT NOT NULL DEFAULT 'other'
    CHECK (account_type IN ('wechat_balance', 'alipay_balance', 'bank_card', 'cash', 'digital_cny', 'other'));

UPDATE ledger_accounts
SET account_type = CASE
    WHEN name LIKE '微信%' THEN 'wechat_balance'
    WHEN name LIKE '支付宝%' THEN 'alipay_balance'
    WHEN name = '现金' THEN 'cash'
    WHEN name LIKE '数字人民币%' THEN 'digital_cny'
    ELSE 'other'
END;
