ALTER TABLE ledger_accounts ADD COLUMN bank_name TEXT
    CHECK (
        bank_name IS NULL OR (
            account_type = 'bank_card'
            AND length(trim(bank_name)) BETWEEN 1 AND 80
        )
    );

ALTER TABLE ledger_accounts ADD COLUMN branch_name TEXT
    CHECK (
        branch_name IS NULL OR (
            account_type = 'bank_card'
            AND length(trim(branch_name)) BETWEEN 1 AND 120
        )
    );

ALTER TABLE ledger_accounts ADD COLUMN nickname TEXT
    CHECK (
        nickname IS NULL OR (
            account_type IN ('wechat_balance', 'alipay_balance')
            AND length(trim(nickname)) BETWEEN 1 AND 80
        )
    );

ALTER TABLE ledger_accounts ADD COLUMN phone TEXT
    CHECK (
        phone IS NULL OR (
            account_type IN ('wechat_balance', 'alipay_balance')
            AND length(trim(phone)) BETWEEN 7 AND 64
        )
    );

ALTER TABLE ledger_accounts ADD COLUMN email TEXT
    CHECK (
        email IS NULL OR (
            account_type = 'alipay_balance'
            AND length(trim(email)) BETWEEN 3 AND 254
        )
    );
