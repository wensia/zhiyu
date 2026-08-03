use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use libsql::{Builder, Database};

use crate::config::Config;

const INITIAL_MIGRATION: &str = include_str!("../migrations/0001_initial.sql");
const DEBT_ADDITIONS_MIGRATION: &str = include_str!("../migrations/0002_debt_additions.sql");
const LEDGER_ACCOUNTS_MIGRATION: &str = include_str!("../migrations/0003_ledger_accounts.sql");
const LEDGER_ACCOUNT_TYPES_MIGRATION: &str =
    include_str!("../migrations/0004_ledger_account_types.sql");
const LEDGER_ACCOUNT_DETAILS_MIGRATION: &str =
    include_str!("../migrations/0005_ledger_account_details.sql");
const LEDGER_ACCOUNT_NAME_SOURCE_MIGRATION: &str =
    include_str!("../migrations/0006_ledger_account_name_source.sql");
const LEDGER_ACCOUNT_CARD_NUMBER_MIGRATION: &str =
    include_str!("../migrations/0007_ledger_account_card_number.sql");
const MIGRATIONS: &[(i64, &str)] = &[
    (1, INITIAL_MIGRATION),
    (2, DEBT_ADDITIONS_MIGRATION),
    (3, LEDGER_ACCOUNTS_MIGRATION),
    (4, LEDGER_ACCOUNT_TYPES_MIGRATION),
    (5, LEDGER_ACCOUNT_DETAILS_MIGRATION),
    (6, LEDGER_ACCOUNT_NAME_SOURCE_MIGRATION),
    (7, LEDGER_ACCOUNT_CARD_NUMBER_MIGRATION),
];

pub async fn connect(config: &Config) -> Result<Database> {
    let db = if config.database_url.starts_with("libsql://")
        || config.database_url.starts_with("https://")
    {
        let token = config
            .turso_auth_token
            .clone()
            .context("TURSO_AUTH_TOKEN is required for a remote DATABASE_URL")?;
        Builder::new_remote(config.database_url.clone(), token)
            .build()
            .await?
    } else {
        let path = config
            .database_url
            .strip_prefix("file:")
            .unwrap_or(&config.database_url);
        if path != ":memory:"
            && let Some(parent) = Path::new(path).parent()
        {
            tokio::fs::create_dir_all(parent).await?;
        }
        Builder::new_local(path).build().await?
    };

    migrate(&db).await?;
    Ok(db)
}

pub async fn migrate(db: &Database) -> Result<()> {
    let conn = db.connect()?;
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")
        .await?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
        (),
    )
    .await?;
    for (version, sql) in MIGRATIONS {
        let mut rows = conn
            .query(
                "SELECT version FROM schema_migrations WHERE version = ?1",
                [*version],
            )
            .await?;
        let applied = rows.next().await?.is_some();
        drop(rows);
        if !applied {
            apply_migration(&conn, *version, sql).await?;
        }
    }
    Ok(())
}

async fn apply_migration(conn: &libsql::Connection, version: i64, sql: &str) -> Result<()> {
    let tx = conn.transaction().await?;
    for statement in split_sql(sql) {
        tx.execute(&statement, ()).await.with_context(|| {
            format!(
                "migration v{version} failed near: {}",
                statement.chars().take(90).collect::<String>()
            )
        })?;
    }
    tx.execute(
        "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
        libsql::params![version, Utc::now().to_rfc3339()],
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

fn split_sql(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    for line in sql.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("--") {
            continue;
        }
        current.push_str(line);
        current.push('\n');
        if trimmed.ends_with(';') {
            statements.push(current.trim().trim_end_matches(';').to_owned());
            current.clear();
        }
    }
    assert!(
        current.trim().is_empty(),
        "unterminated SQL migration statement"
    );
    statements
}

#[cfg(test)]
mod tests {
    use libsql::Builder;

    use super::{
        DEBT_ADDITIONS_MIGRATION, INITIAL_MIGRATION, LEDGER_ACCOUNT_DETAILS_MIGRATION,
        LEDGER_ACCOUNT_NAME_SOURCE_MIGRATION, LEDGER_ACCOUNT_TYPES_MIGRATION,
        LEDGER_ACCOUNTS_MIGRATION, MIGRATIONS, apply_migration, migrate, split_sql,
    };

    #[test]
    fn migration_is_split_into_complete_statements() {
        let statements = split_sql("-- x\nCREATE TABLE x(id TEXT);\n\nINSERT INTO x VALUES ('a');");
        assert_eq!(statements.len(), 2);
    }

    #[tokio::test]
    async fn failed_migration_rolls_back_every_domain_statement() {
        let db = Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        conn.execute(
            "CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
            (),
        )
        .await
        .unwrap();

        let result = apply_migration(
            &conn,
            99,
            "CREATE TABLE should_rollback(id TEXT);\nTHIS IS NOT VALID SQL;",
        )
        .await;
        assert!(result.is_err());

        let mut rows = conn
            .query(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'should_rollback'",
                (),
            )
            .await
            .unwrap();
        assert!(rows.next().await.unwrap().is_none());
        let mut versions = conn
            .query(
                "SELECT version FROM schema_migrations WHERE version = 99",
                (),
            )
            .await
            .unwrap();
        assert!(versions.next().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn existing_v1_database_is_upgraded_through_all_migrations() {
        let root = tempfile::tempdir().unwrap();
        let db = Builder::new_local(root.path().join("upgrade.db"))
            .build()
            .await
            .unwrap();
        let conn = db.connect().unwrap();
        conn.execute(
            "CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
            (),
        )
        .await
        .unwrap();
        apply_migration(&conn, 1, INITIAL_MIGRATION).await.unwrap();

        migrate(&db).await.unwrap();

        let mut versions = conn
            .query("SELECT version FROM schema_migrations ORDER BY version", ())
            .await
            .unwrap();
        assert_eq!(
            versions
                .next()
                .await
                .unwrap()
                .unwrap()
                .get::<i64>(0)
                .unwrap(),
            1
        );
        assert_eq!(
            versions
                .next()
                .await
                .unwrap()
                .unwrap()
                .get::<i64>(0)
                .unwrap(),
            2
        );
        assert_eq!(
            versions
                .next()
                .await
                .unwrap()
                .unwrap()
                .get::<i64>(0)
                .unwrap(),
            3
        );
        assert_eq!(
            versions
                .next()
                .await
                .unwrap()
                .unwrap()
                .get::<i64>(0)
                .unwrap(),
            4
        );
        assert_eq!(
            versions
                .next()
                .await
                .unwrap()
                .unwrap()
                .get::<i64>(0)
                .unwrap(),
            5
        );
        assert_eq!(
            versions
                .next()
                .await
                .unwrap()
                .unwrap()
                .get::<i64>(0)
                .unwrap(),
            6
        );
        assert_eq!(
            versions
                .next()
                .await
                .unwrap()
                .unwrap()
                .get::<i64>(0)
                .unwrap(),
            7
        );
        assert!(versions.next().await.unwrap().is_none());
        let mut tables = conn
            .query(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'debt_addition_events'",
                (),
            )
            .await
            .unwrap();
        assert!(tables.next().await.unwrap().is_some());
        let mut accounts = conn
            .query(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'ledger_accounts'",
                (),
            )
            .await
            .unwrap();
        assert!(accounts.next().await.unwrap().is_some());
    }

    #[tokio::test]
    async fn existing_v2_database_backfills_exact_account_labels_without_rewriting_notes() {
        let root = tempfile::tempdir().unwrap();
        let db = Builder::new_local(root.path().join("backfill.db"))
            .build()
            .await
            .unwrap();
        let conn = db.connect().unwrap();
        conn.execute(
            "CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
            (),
        )
        .await
        .unwrap();
        apply_migration(&conn, 1, INITIAL_MIGRATION).await.unwrap();
        apply_migration(&conn, 2, DEBT_ADDITIONS_MIGRATION)
            .await
            .unwrap();
        conn.execute(
            "INSERT INTO users(id, email, password_hash, timezone, created_at, updated_at) VALUES ('u1', 'u1@example.com', 'hash', 'Asia/Shanghai', '2026-08-02T00:00:00Z', '2026-08-02T00:00:00Z')",
            (),
        ).await.unwrap();
        conn.execute(
            "INSERT INTO counterparties(id, user_id, display_name, normalized_name, created_at, updated_at) VALUES ('c1', 'u1', '朋友', '朋友', '2026-08-02T00:00:00Z', '2026-08-02T00:00:00Z')",
            (),
        ).await.unwrap();
        conn.execute(
            "INSERT INTO debts(id, user_id, counterparty_id, direction, principal_cents, occurred_on, note, created_at, updated_at) VALUES ('d1', 'u1', 'c1', 'lend_out', 10000, '2026-08-01', '欠款；付款账户：微信零钱；导入自旧账本', '2026-08-02T00:00:00Z', '2026-08-02T00:00:00Z')",
            (),
        ).await.unwrap();
        conn.execute(
            "INSERT INTO debts(id, user_id, counterparty_id, direction, principal_cents, occurred_on, note, created_at, updated_at) VALUES ('d2', 'u1', 'c1', 'lend_out', 5000, '2026-08-01', '付款账户：无；保留为空', '2026-08-02T00:00:00Z', '2026-08-02T00:00:00Z')",
            (),
        ).await.unwrap();
        conn.execute(
            "INSERT INTO repayment_events(id, user_id, debt_id, kind, amount_cents, effective_on, note, created_at) VALUES ('r1', 'u1', 'd1', 'payment', 1000, '2026-08-02', '收款账户：微信零钱', '2026-08-02T00:00:00Z')",
            (),
        ).await.unwrap();
        conn.execute(
            "INSERT INTO debt_addition_events(id, user_id, debt_id, amount_cents, effective_on, note, created_at) VALUES ('a1', 'u1', 'd1', 2000, '2026-08-02', '付款账户：支付宝-测试号；再次借出', '2026-08-02T00:00:00Z')",
            (),
        ).await.unwrap();

        migrate(&db).await.unwrap();

        let mut accounts = conn
            .query(
                "SELECT name, account_type FROM ledger_accounts WHERE user_id = 'u1' ORDER BY name",
                (),
            )
            .await
            .unwrap();
        let wechat = accounts.next().await.unwrap().unwrap();
        assert_eq!(wechat.get::<String>(0).unwrap(), "微信零钱");
        assert_eq!(wechat.get::<String>(1).unwrap(), "wechat_balance");
        let alipay = accounts.next().await.unwrap().unwrap();
        assert_eq!(alipay.get::<String>(0).unwrap(), "支付宝-测试号");
        assert_eq!(alipay.get::<String>(1).unwrap(), "alipay_balance");
        assert!(accounts.next().await.unwrap().is_none());

        let mut rows = conn
            .query(
                "SELECT d.account_id, r.account_id, a.account_id, d.note FROM debts d JOIN repayment_events r ON r.debt_id = d.id JOIN debt_addition_events a ON a.debt_id = d.id WHERE d.id = 'd1'",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        let debt_account: String = row.get(0).unwrap();
        let repayment_account: String = row.get(1).unwrap();
        let addition_account: String = row.get(2).unwrap();
        assert_eq!(debt_account, repayment_account);
        assert_ne!(debt_account, addition_account);
        assert_eq!(
            row.get::<String>(3).unwrap(),
            "欠款；付款账户：微信零钱；导入自旧账本"
        );

        let mut unresolved = conn
            .query("SELECT account_id FROM debts WHERE id = 'd2'", ())
            .await
            .unwrap();
        assert!(
            unresolved
                .next()
                .await
                .unwrap()
                .unwrap()
                .get::<Option<String>>(0)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn existing_v3_accounts_receive_only_the_explicit_account_type_mappings() {
        let root = tempfile::tempdir().unwrap();
        let db = Builder::new_local(root.path().join("account-types.db"))
            .build()
            .await
            .unwrap();
        let conn = db.connect().unwrap();
        conn.execute(
            "CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
            (),
        )
        .await
        .unwrap();
        apply_migration(&conn, 1, INITIAL_MIGRATION).await.unwrap();
        apply_migration(&conn, 2, DEBT_ADDITIONS_MIGRATION)
            .await
            .unwrap();
        apply_migration(&conn, 3, LEDGER_ACCOUNTS_MIGRATION)
            .await
            .unwrap();
        conn.execute(
            "INSERT INTO users(id, email, password_hash, timezone, created_at, updated_at) VALUES ('u1', 'types@example.com', 'hash', 'Asia/Shanghai', '2026-08-02T00:00:00Z', '2026-08-02T00:00:00Z')",
            (),
        ).await.unwrap();
        conn.execute_batch(
            "INSERT INTO ledger_accounts(id, user_id, name, normalized_name, created_at, updated_at) VALUES ('a1', 'u1', '微信支付-测试号', '微信支付-测试号', '2026-08-02T00:00:00Z', '2026-08-02T00:00:00Z');
             INSERT INTO ledger_accounts(id, user_id, name, normalized_name, created_at, updated_at) VALUES ('a2', 'u1', '支付宝余额', '支付宝余额', '2026-08-02T00:00:00Z', '2026-08-02T00:00:00Z');
             INSERT INTO ledger_accounts(id, user_id, name, normalized_name, created_at, updated_at) VALUES ('a3', 'u1', '现金', '现金', '2026-08-02T00:00:00Z', '2026-08-02T00:00:00Z');
             INSERT INTO ledger_accounts(id, user_id, name, normalized_name, created_at, updated_at) VALUES ('a4', 'u1', '数字人民币-钱包', '数字人民币-钱包', '2026-08-02T00:00:00Z', '2026-08-02T00:00:00Z');
             INSERT INTO ledger_accounts(id, user_id, name, normalized_name, created_at, updated_at) VALUES ('a5', 'u1', '招商银行-尾号1234', '招商银行-尾号1234', '2026-08-02T00:00:00Z', '2026-08-02T00:00:00Z');
             INSERT INTO ledger_accounts(id, user_id, name, normalized_name, created_at, updated_at) VALUES ('a6', 'u1', '银行卡-待确认', '银行卡-待确认', '2026-08-02T00:00:00Z', '2026-08-02T00:00:00Z');",
        ).await.unwrap();

        migrate(&db).await.unwrap();

        let mut rows = conn
            .query(
                "SELECT id, account_type FROM ledger_accounts ORDER BY id",
                (),
            )
            .await
            .unwrap();
        let expected = [
            ("a1", "wechat_balance"),
            ("a2", "alipay_balance"),
            ("a3", "cash"),
            ("a4", "digital_cny"),
            ("a5", "other"),
            ("a6", "other"),
        ];
        for (id, account_type) in expected {
            let row = rows.next().await.unwrap().unwrap();
            assert_eq!(row.get::<String>(0).unwrap(), id);
            assert_eq!(row.get::<String>(1).unwrap(), account_type);
        }
        assert!(rows.next().await.unwrap().is_none());

        let invalid = conn
            .execute(
                "UPDATE ledger_accounts SET account_type = 'bank_balance' WHERE id = 'a1'",
                (),
            )
            .await;
        assert!(invalid.is_err());
    }

    #[tokio::test]
    async fn existing_v4_accounts_gain_empty_details_without_changing_existing_data() {
        let root = tempfile::tempdir().unwrap();
        let db = Builder::new_local(root.path().join("account-details.db"))
            .build()
            .await
            .unwrap();
        let conn = db.connect().unwrap();
        conn.execute(
            "CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
            (),
        )
        .await
        .unwrap();
        apply_migration(&conn, 1, INITIAL_MIGRATION).await.unwrap();
        apply_migration(&conn, 2, DEBT_ADDITIONS_MIGRATION)
            .await
            .unwrap();
        apply_migration(&conn, 3, LEDGER_ACCOUNTS_MIGRATION)
            .await
            .unwrap();
        apply_migration(&conn, 4, LEDGER_ACCOUNT_TYPES_MIGRATION)
            .await
            .unwrap();
        conn.execute(
            "INSERT INTO users(id, email, password_hash, timezone, created_at, updated_at) VALUES ('u1', 'details@example.com', 'hash', 'Asia/Shanghai', '2026-08-02T00:00:00Z', '2026-08-02T00:00:00Z')",
            (),
        ).await.unwrap();
        conn.execute(
            "INSERT INTO ledger_accounts(id, user_id, name, normalized_name, account_type, note, version, created_at, updated_at) VALUES ('a1', 'u1', '银行卡-旧账户', '银行卡-旧账户', 'bank_card', '原备注', 3, '2026-08-02T00:00:00Z', '2026-08-02T01:00:00Z')",
            (),
        ).await.unwrap();
        conn.execute(
            "INSERT INTO counterparties(id, user_id, display_name, normalized_name, created_at, updated_at) VALUES ('c1', 'u1', '旧联系人', '旧联系人', '2026-08-02T00:00:00Z', '2026-08-02T00:00:00Z')",
            (),
        ).await.unwrap();
        conn.execute(
            "INSERT INTO debts(id, user_id, counterparty_id, direction, principal_cents, occurred_on, note, account_id, created_at, updated_at) VALUES ('d1', 'u1', 'c1', 'lend_out', 10000, '2026-08-01', '旧借款记录', 'a1', '2026-08-02T00:00:00Z', '2026-08-02T00:00:00Z')",
            (),
        ).await.unwrap();
        conn.execute(
            "INSERT INTO repayment_events(id, user_id, debt_id, kind, amount_cents, effective_on, note, account_id, created_at) VALUES ('r1', 'u1', 'd1', 'payment', 1000, '2026-08-02', '旧还款记录', 'a1', '2026-08-02T00:00:00Z')",
            (),
        ).await.unwrap();

        migrate(&db).await.unwrap();

        let mut rows = conn
            .query(
                "SELECT name, account_type, note, version, created_at, updated_at, bank_name, branch_name, card_number, nickname, phone, email, name_source FROM ledger_accounts WHERE id = 'a1'",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "银行卡-旧账户");
        assert_eq!(row.get::<String>(1).unwrap(), "bank_card");
        assert_eq!(row.get::<String>(2).unwrap(), "原备注");
        assert_eq!(row.get::<i64>(3).unwrap(), 3);
        assert_eq!(row.get::<String>(4).unwrap(), "2026-08-02T00:00:00Z");
        assert_eq!(row.get::<String>(5).unwrap(), "2026-08-02T01:00:00Z");
        for index in 6..=11 {
            assert!(row.get::<Option<String>>(index).unwrap().is_none());
        }
        assert_eq!(row.get::<String>(12).unwrap(), "custom");

        let mut history = conn
            .query(
                "SELECT d.account_id, d.note, r.account_id, r.note FROM debts d JOIN repayment_events r ON r.debt_id = d.id WHERE d.id = 'd1'",
                (),
            )
            .await
            .unwrap();
        let history = history.next().await.unwrap().unwrap();
        assert_eq!(history.get::<String>(0).unwrap(), "a1");
        assert_eq!(history.get::<String>(1).unwrap(), "旧借款记录");
        assert_eq!(history.get::<String>(2).unwrap(), "a1");
        assert_eq!(history.get::<String>(3).unwrap(), "旧还款记录");
        let mut usage = conn
            .query(
                "SELECT (SELECT COUNT(*) FROM debts WHERE account_id = 'a1') + (SELECT COUNT(*) FROM debt_addition_events WHERE account_id = 'a1') + (SELECT COUNT(*) FROM repayment_events WHERE account_id = 'a1')",
                (),
            )
            .await
            .unwrap();
        assert_eq!(
            usage.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            2
        );

        let columns = [
            "bank_name",
            "branch_name",
            "card_number",
            "nickname",
            "phone",
            "email",
        ];
        for column in columns {
            let mut rows = conn
                .query(
                    "SELECT name FROM pragma_table_info('ledger_accounts') WHERE name = ?1",
                    [column],
                )
                .await
                .unwrap();
            assert!(rows.next().await.unwrap().is_some());
        }

        let wrong_type = conn
            .execute(
                "UPDATE ledger_accounts SET nickname = '不应写入' WHERE id = 'a1'",
                (),
            )
            .await;
        assert!(wrong_type.is_err());
    }

    #[tokio::test]
    async fn existing_v5_account_names_are_preserved_as_custom_in_v6() {
        let root = tempfile::tempdir().unwrap();
        let db = Builder::new_local(root.path().join("account-name-source.db"))
            .build()
            .await
            .unwrap();
        let conn = db.connect().unwrap();
        conn.execute(
            "CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
            (),
        )
        .await
        .unwrap();
        apply_migration(&conn, 1, INITIAL_MIGRATION).await.unwrap();
        apply_migration(&conn, 2, DEBT_ADDITIONS_MIGRATION)
            .await
            .unwrap();
        apply_migration(&conn, 3, LEDGER_ACCOUNTS_MIGRATION)
            .await
            .unwrap();
        apply_migration(&conn, 4, LEDGER_ACCOUNT_TYPES_MIGRATION)
            .await
            .unwrap();
        apply_migration(&conn, 5, LEDGER_ACCOUNT_DETAILS_MIGRATION)
            .await
            .unwrap();
        conn.execute(
            "INSERT INTO users(id, email, password_hash, timezone, created_at, updated_at) VALUES ('u1', 'name-source@example.com', 'hash', 'Asia/Shanghai', '2026-08-02T00:00:00Z', '2026-08-02T00:00:00Z')",
            (),
        ).await.unwrap();
        conn.execute(
            "INSERT INTO ledger_accounts(id, user_id, name, normalized_name, account_type, note, nickname, phone, version, created_at, updated_at) VALUES ('a1', 'u1', '原有别名', '原有别名', 'wechat_balance', '原备注', '旧昵称', '13800138000', 4, '2026-08-02T00:00:00Z', '2026-08-02T01:00:00Z')",
            (),
        ).await.unwrap();

        apply_migration(&conn, 6, LEDGER_ACCOUNT_NAME_SOURCE_MIGRATION)
            .await
            .unwrap();

        let mut rows = conn
            .query(
                "SELECT name, normalized_name, account_type, note, nickname, phone, version, created_at, updated_at, name_source FROM ledger_accounts WHERE id = 'a1'",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "原有别名");
        assert_eq!(row.get::<String>(1).unwrap(), "原有别名");
        assert_eq!(row.get::<String>(2).unwrap(), "wechat_balance");
        assert_eq!(row.get::<String>(3).unwrap(), "原备注");
        assert_eq!(row.get::<String>(4).unwrap(), "旧昵称");
        assert_eq!(row.get::<String>(5).unwrap(), "13800138000");
        assert_eq!(row.get::<i64>(6).unwrap(), 4);
        assert_eq!(row.get::<String>(7).unwrap(), "2026-08-02T00:00:00Z");
        assert_eq!(row.get::<String>(8).unwrap(), "2026-08-02T01:00:00Z");
        assert_eq!(row.get::<String>(9).unwrap(), "custom");

        let invalid = conn
            .execute(
                "UPDATE ledger_accounts SET name_source = 'automatic' WHERE id = 'a1'",
                (),
            )
            .await;
        assert!(invalid.is_err());
    }

    #[tokio::test]
    async fn existing_v6_accounts_gain_an_empty_card_number_without_data_loss() {
        let root = tempfile::tempdir().unwrap();
        let db = Builder::new_local(root.path().join("account-card-number.db"))
            .build()
            .await
            .unwrap();
        let conn = db.connect().unwrap();
        conn.execute(
            "CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
            (),
        )
        .await
        .unwrap();
        for (version, sql) in MIGRATIONS.iter().take(6) {
            apply_migration(&conn, *version, sql).await.unwrap();
        }
        conn.execute(
            "INSERT INTO users(id, email, password_hash, timezone, created_at, updated_at) VALUES ('u1', 'card-number@example.com', 'hash', 'Asia/Shanghai', '2026-08-02T00:00:00Z', '2026-08-02T00:00:00Z')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO ledger_accounts(id, user_id, name, normalized_name, name_source, account_type, note, bank_name, branch_name, version, created_at, updated_at) VALUES ('a1', 'u1', '旧工资卡', '旧工资卡', 'custom', 'bank_card', '原备注', '招商银行', '上海支行', 4, '2026-08-02T00:00:00Z', '2026-08-02T01:00:00Z')",
            (),
        )
        .await
        .unwrap();

        migrate(&db).await.unwrap();

        let mut rows = conn
            .query(
                "SELECT name, bank_name, branch_name, card_number, version FROM ledger_accounts WHERE id = 'a1'",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "旧工资卡");
        assert_eq!(row.get::<String>(1).unwrap(), "招商银行");
        assert_eq!(row.get::<String>(2).unwrap(), "上海支行");
        assert!(row.get::<Option<String>>(3).unwrap().is_none());
        assert_eq!(row.get::<i64>(4).unwrap(), 4);
    }
}
