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
const DEBT_ORIGIN_KIND_MIGRATION: &str = include_str!("../migrations/0008_debt_origin_kind.sql");
const TRANSACTIONS_MIGRATION: &str = include_str!("../migrations/0009_transactions.sql");
const API_KEYS_MIGRATION: &str = include_str!("../migrations/0010_api_keys.sql");
const HANDOFF_TICKETS_MIGRATION: &str = include_str!("../migrations/0011_handoff_tickets.sql");
const TRANSACTIONS_V2_MIGRATION: &str = include_str!("../migrations/0013_transactions_v2.sql");
const BILL_IMPORTS_MIGRATION: &str = include_str!("../migrations/0014_bill_imports.sql");
const IMPORT_ACCOUNT_MAPPINGS_MIGRATION: &str =
    include_str!("../migrations/0015_import_account_mappings.sql");
const DEBT_TRANSACTION_LINKS_MIGRATION: &str =
    include_str!("../migrations/0016_debt_transaction_links.sql");
const DUPLICATE_SUSPICIONS_MIGRATION: &str =
    include_str!("../migrations/0017_duplicate_suspicions.sql");
const TRANSACTION_EVENTS_MIGRATION: &str =
    include_str!("../migrations/0018_transaction_events.sql");
const DUPLICATE_SUSPICION_CLUSTERS_MIGRATION: &str =
    include_str!("../migrations/0019_duplicate_suspicion_clusters.sql");
const DUPLICATE_CONFIRMATION_ACTIONS_MIGRATION: &str =
    include_str!("../migrations/0020_duplicate_confirmation_actions.sql");
const CATEGORIES_MIGRATION: &str = include_str!("../migrations/0021_categories.sql");
const SELF_TRANSFER_ALIASES_MIGRATION: &str =
    include_str!("../migrations/0022_self_transfer_aliases.sql");
const DROP_BILL_INBOX_MIGRATION: &str = include_str!("../migrations/0023_drop_bill_inbox.sql");
const DEBT_CASH_MOVEMENTS_TO_TRANSACTIONS_MIGRATION: &str =
    include_str!("../migrations/0024_debt_cash_movements_to_transactions.sql");
const TRANSACTION_PNL_SCOPE_MIGRATION: &str =
    include_str!("../migrations/0025_transaction_pnl_scope.sql");
const TRANSACTION_LINKS_MIGRATION: &str = include_str!("../migrations/0026_transaction_links.sql");
const TRANSACTION_CREATED_BY_AND_MOVEMENTS_VIEW_MIGRATION: &str =
    include_str!("../migrations/0027_transaction_created_by_and_movements_view.sql");
const TRANSACTION_CATEGORY_RULE_TRACE_MIGRATION: &str =
    include_str!("../migrations/0028_transaction_category_rule_trace.sql");
const IMPORT_TRANSACTION_LINKS_MIGRATION: &str =
    include_str!("../migrations/0029_import_transaction_links.sql");
const PLUGIN_SETTINGS_MIGRATION: &str = include_str!("../migrations/0030_plugin_settings.sql");
const DASHBOARDS_MIGRATION: &str = include_str!("../migrations/0031_dashboards.sql");
// v12 留给生产 JMAP 暂存迁移；它建的三张表已由 v23 删除。
const MIGRATIONS: &[(i64, &str)] = &[
    (1, INITIAL_MIGRATION),
    (2, DEBT_ADDITIONS_MIGRATION),
    (3, LEDGER_ACCOUNTS_MIGRATION),
    (4, LEDGER_ACCOUNT_TYPES_MIGRATION),
    (5, LEDGER_ACCOUNT_DETAILS_MIGRATION),
    (6, LEDGER_ACCOUNT_NAME_SOURCE_MIGRATION),
    (7, LEDGER_ACCOUNT_CARD_NUMBER_MIGRATION),
    (8, DEBT_ORIGIN_KIND_MIGRATION),
    (9, TRANSACTIONS_MIGRATION),
    (10, API_KEYS_MIGRATION),
    (11, HANDOFF_TICKETS_MIGRATION),
    (13, TRANSACTIONS_V2_MIGRATION),
    (14, BILL_IMPORTS_MIGRATION),
    (15, IMPORT_ACCOUNT_MAPPINGS_MIGRATION),
    (16, DEBT_TRANSACTION_LINKS_MIGRATION),
    (17, DUPLICATE_SUSPICIONS_MIGRATION),
    (18, TRANSACTION_EVENTS_MIGRATION),
    (19, DUPLICATE_SUSPICION_CLUSTERS_MIGRATION),
    (20, DUPLICATE_CONFIRMATION_ACTIONS_MIGRATION),
    (21, CATEGORIES_MIGRATION),
    (22, SELF_TRANSFER_ALIASES_MIGRATION),
    (23, DROP_BILL_INBOX_MIGRATION),
    (24, DEBT_CASH_MOVEMENTS_TO_TRANSACTIONS_MIGRATION),
    (25, TRANSACTION_PNL_SCOPE_MIGRATION),
    (26, TRANSACTION_LINKS_MIGRATION),
    (27, TRANSACTION_CREATED_BY_AND_MOVEMENTS_VIEW_MIGRATION),
    (28, TRANSACTION_CATEGORY_RULE_TRACE_MIGRATION),
    (29, IMPORT_TRANSACTION_LINKS_MIGRATION),
    (30, PLUGIN_SETTINGS_MIGRATION),
    (31, DASHBOARDS_MIGRATION),
];

/// 返回当前程序认识的全部迁移版本，供离线恢复判断备份是否可安全打开。
pub fn known_migration_versions() -> Vec<i64> {
    MIGRATIONS.iter().map(|(version, _)| *version).collect()
}

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
    migrate_up_to(db, i64::MAX).await
}

/// Applies known migrations through `maximum_version` (inclusive).
///
/// This is primarily useful for migration reconciliation tests and offline drills. The production
/// connection path continues to call [`migrate`], which always applies every known migration.
#[doc(hidden)]
pub async fn migrate_up_to(db: &Database, maximum_version: i64) -> Result<()> {
    let conn = db.connect()?;
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")
        .await?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
        (),
    )
    .await?;
    for (version, sql) in MIGRATIONS
        .iter()
        .filter(|(version, _)| *version <= maximum_version)
    {
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
    if version == 27 {
        let mut rows = tx
            .query(
                "SELECT COUNT(*) FROM debts d WHERE d.origin_kind = 'cash_movement' AND d.account_id IS NOT NULL AND d.transaction_id IS NULL AND d.principal_cents - COALESCE((SELECT SUM(a.amount_cents) FROM debt_addition_events a WHERE a.debt_id = d.id), 0) < 0",
                (),
            )
            .await?;
        let invalid_count = rows
            .next()
            .await?
            .context("migration v27 debt guard returned no row")?
            .get::<i64>(0)?;
        drop(rows);
        if invalid_count > 0 {
            anyhow::bail!(
                "migration v27 refused: found {invalid_count} cash-movement debts whose additions exceed principal; 这些债务的追加借款之和超过本金，请先在债务页修正后重启"
            );
        }
    }
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
        LEDGER_ACCOUNTS_MIGRATION, MIGRATIONS, apply_migration, known_migration_versions, migrate,
        split_sql,
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
        let mut applied = Vec::new();
        while let Some(row) = versions.next().await.unwrap() {
            applied.push(row.get::<i64>(0).unwrap());
        }
        assert_eq!(
            applied,
            vec![
                1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
                25, 26, 27, 28, 29, 30, 31
            ]
        );
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

    #[tokio::test]
    async fn existing_v7_debts_gain_origin_kind_from_account_presence() {
        let root = tempfile::tempdir().unwrap();
        let db = Builder::new_local(root.path().join("debt-origin-kind.db"))
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
        for (version, sql) in MIGRATIONS.iter().take(7) {
            apply_migration(&conn, *version, sql).await.unwrap();
        }
        conn.execute(
            "INSERT INTO users(id, email, password_hash, timezone, created_at, updated_at) VALUES ('u1', 'origin-kind@example.com', 'hash', 'Asia/Shanghai', '2026-08-02T00:00:00Z', '2026-08-02T00:00:00Z')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO ledger_accounts(id, user_id, name, normalized_name, name_source, account_type, note, version, created_at, updated_at) VALUES ('a1', 'u1', '微信零钱', '微信零钱', 'derived', 'wechat_balance', '', 1, '2026-08-02T00:00:00Z', '2026-08-02T00:00:00Z')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO counterparties(id, user_id, display_name, normalized_name, created_at, updated_at) VALUES ('c1', 'u1', '旧联系人', '旧联系人', '2026-08-02T00:00:00Z', '2026-08-02T00:00:00Z')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO debts(id, user_id, counterparty_id, direction, principal_cents, occurred_on, note, account_id, created_at, updated_at) VALUES ('d1', 'u1', 'c1', 'lend_out', 10000, '2026-08-01', '有账户的旧借款', 'a1', '2026-08-02T00:00:00Z', '2026-08-02T00:00:00Z')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO debts(id, user_id, counterparty_id, direction, principal_cents, occurred_on, note, account_id, created_at, updated_at) VALUES ('d2', 'u1', 'c1', 'borrow_in', 150000, '2026-08-01', '无账户的历史欠款', NULL, '2026-08-02T00:00:00Z', '2026-08-02T00:00:00Z')",
            (),
        )
        .await
        .unwrap();

        migrate(&db).await.unwrap();

        let mut rows = conn
            .query(
                "SELECT id, origin_kind FROM debts WHERE user_id = 'u1' ORDER BY id",
                (),
            )
            .await
            .unwrap();
        let first = rows.next().await.unwrap().unwrap();
        assert_eq!(first.get::<String>(0).unwrap(), "d1");
        assert_eq!(first.get::<String>(1).unwrap(), "cash_movement");
        let second = rows.next().await.unwrap().unwrap();
        assert_eq!(second.get::<String>(0).unwrap(), "d2");
        assert_eq!(second.get::<String>(1).unwrap(), "legacy_unknown");

        let invalid = conn
            .execute("UPDATE debts SET origin_kind = 'gift' WHERE id = 'd1'", ())
            .await;
        assert!(invalid.is_err());
    }

    #[tokio::test]
    async fn existing_v8_databases_gain_ledger_transactions_and_balance_views() {
        let root = tempfile::tempdir().unwrap();
        let db = Builder::new_local(root.path().join("transactions.db"))
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
        for (version, sql) in MIGRATIONS.iter().take(8) {
            apply_migration(&conn, *version, sql).await.unwrap();
        }
        conn.execute(
            "INSERT INTO users(id, email, password_hash, timezone, created_at, updated_at) VALUES ('u1', 'transactions@example.com', 'hash', 'Asia/Shanghai', '2026-08-02T00:00:00Z', '2026-08-02T00:00:00Z')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO ledger_accounts(id, user_id, name, normalized_name, name_source, account_type, note, version, created_at, updated_at) VALUES ('a1', 'u1', '微信零钱', '微信零钱', 'derived', 'wechat_balance', '', 1, '2026-08-02T00:00:00Z', '2026-08-02T00:00:00Z')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO counterparties(id, user_id, display_name, normalized_name, created_at, updated_at) VALUES ('c1', 'u1', '旧联系人', '旧联系人', '2026-08-02T00:00:00Z', '2026-08-02T00:00:00Z')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO debts(id, user_id, counterparty_id, direction, principal_cents, occurred_on, note, account_id, origin_kind, created_at, updated_at) VALUES ('d1', 'u1', 'c1', 'lend_out', 10000, '2026-08-01', '借出', 'a1', 'cash_movement', '2026-08-02T00:00:00Z', '2026-08-02T00:00:00Z')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO repayment_events(id, user_id, debt_id, kind, amount_cents, effective_on, note, account_id, created_at) VALUES ('r1', 'u1', 'd1', 'payment', 1000, '2026-08-02', '还了一部分', 'a1', '2026-08-02T00:00:00Z')",
            (),
        )
        .await
        .unwrap();

        migrate(&db).await.unwrap();

        let mut versions = conn
            .query("SELECT version FROM schema_migrations ORDER BY version", ())
            .await
            .unwrap();
        let mut applied = Vec::new();
        while let Some(row) = versions.next().await.unwrap() {
            applied.push(row.get::<i64>(0).unwrap());
        }
        assert_eq!(
            applied,
            vec![
                1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
                25, 26, 27, 28, 29, 30, 31
            ]
        );

        for (kind, name) in [
            ("table", "ledger_transactions"),
            ("view", "ledger_account_movements"),
            ("view", "ledger_account_balances"),
        ] {
            let mut rows = conn
                .query(
                    "SELECT name FROM sqlite_master WHERE type = ?1 AND name = ?2",
                    [kind, name],
                )
                .await
                .unwrap();
            assert!(rows.next().await.unwrap().is_some(), "missing {name}");
        }

        conn.execute(
            "INSERT INTO ledger_transactions(id, user_id, kind, amount_cents, occurred_on, category, account_id, created_at, updated_at) VALUES ('t1', 'u1', 'income', 5000, '2026-08-03', '工资', 'a1', '2026-08-03T00:00:00Z', '2026-08-03T00:00:00Z')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO ledger_transactions(id, user_id, kind, amount_cents, occurred_on, category, account_id, created_at, updated_at) VALUES ('t2', 'u1', 'expense', 1200, '2026-08-03', '餐饮', 'a1', '2026-08-03T00:00:00Z', '2026-08-03T00:00:00Z')",
            (),
        )
        .await
        .unwrap();

        let invalid_kind = conn
            .execute(
                "INSERT INTO ledger_transactions(id, user_id, kind, amount_cents, occurred_on, created_at, updated_at) VALUES ('t3', 'u1', 'gift', 100, '2026-08-03', '2026-08-03T00:00:00Z', '2026-08-03T00:00:00Z')",
                (),
            )
            .await;
        assert!(invalid_kind.is_err());
        let zero_amount = conn
            .execute(
                "INSERT INTO ledger_transactions(id, user_id, kind, amount_cents, occurred_on, created_at, updated_at) VALUES ('t4', 'u1', 'income', 0, '2026-08-03', '2026-08-03T00:00:00Z', '2026-08-03T00:00:00Z')",
                (),
            )
            .await;
        assert!(zero_amount.is_err());

        // 余额 = 初始 0 + 记账(5000 - 1200) + 债务现金流水(-10000 + 1000) = -5200
        let mut rows = conn
            .query(
                "SELECT balance_cents FROM ledger_account_balances WHERE account_id = 'a1'",
                (),
            )
            .await
            .unwrap();
        assert_eq!(
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            -5200
        );

        // 归档记账条目即回滚其余额影响：-5200 + 1200 = -4000
        conn.execute(
            "UPDATE ledger_transactions SET archived_at = '2026-08-04T00:00:00Z' WHERE id = 't2'",
            (),
        )
        .await
        .unwrap();
        let mut rows = conn
            .query(
                "SELECT balance_cents FROM ledger_account_balances WHERE account_id = 'a1'",
                (),
            )
            .await
            .unwrap();
        assert_eq!(
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            -4000
        );

        // 初始余额列默认 0，可更新
        let mut rows = conn
            .query(
                "SELECT opening_balance_cents FROM ledger_accounts WHERE id = 'a1'",
                (),
            )
            .await
            .unwrap();
        assert_eq!(
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            0
        );

        // 删除用户级联删除记账条目
        conn.execute("DELETE FROM users WHERE id = 'u1'", ())
            .await
            .unwrap();
        let mut rows = conn
            .query(
                "SELECT id FROM ledger_transactions WHERE user_id = 'u1'",
                (),
            )
            .await
            .unwrap();
        assert!(rows.next().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn existing_v11_database_is_upgraded_to_v12_with_safe_provenance_defaults() {
        let root = tempfile::tempdir().unwrap();
        let db = Builder::new_local(root.path().join("bill-import-upgrade.db"))
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
        for (version, sql) in MIGRATIONS.iter().take(11) {
            apply_migration(&conn, *version, sql).await.unwrap();
        }
        conn.execute(
            "INSERT INTO users(id, email, password_hash, timezone, created_at, updated_at) VALUES ('u1', 'import@example.com', 'hash', 'Asia/Shanghai', '2026-08-11T00:00:00Z', '2026-08-11T00:00:00Z')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO ledger_transactions(id, user_id, kind, amount_cents, occurred_on, created_at, updated_at) VALUES ('old', 'u1', 'expense', 100, '2026-08-10', '2026-08-11T00:00:00Z', '2026-08-11T00:00:00Z')",
            (),
        )
        .await
        .unwrap();

        migrate(&db).await.unwrap();

        assert_eq!(
            known_migration_versions(),
            vec![
                1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
                25, 26, 27, 28, 29, 30, 31
            ]
        );
        let mut rows = conn
            .query(
                "SELECT source_channel, external_id, import_batch_id FROM ledger_transactions WHERE id = 'old'",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "");
        assert_eq!(row.get::<String>(1).unwrap(), "");
        assert!(row.get::<Option<String>>(2).unwrap().is_none());

        let mut foreign_key_violations = conn.query("PRAGMA foreign_key_check", ()).await.unwrap();
        assert!(foreign_key_violations.next().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn transaction_events_migration_adds_event_and_normalization_schema() {
        let root = tempfile::tempdir().unwrap();
        let db = Builder::new_local(root.path().join("transaction-events.db"))
            .build()
            .await
            .unwrap();
        migrate(&db).await.unwrap();
        let conn = db.connect().unwrap();

        let mut tables = conn
            .query(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'transaction_events'",
                (),
            )
            .await
            .unwrap();
        assert!(tables.next().await.unwrap().is_some());

        let mut transaction_columns = conn
            .query(
                "SELECT name FROM pragma_table_info('ledger_transactions') WHERE name IN ('event_id', 'payee_key') ORDER BY name",
                (),
            )
            .await
            .unwrap();
        let mut transaction_column_names = Vec::new();
        while let Some(row) = transaction_columns.next().await.unwrap() {
            transaction_column_names.push(row.get::<String>(0).unwrap());
        }
        assert_eq!(transaction_column_names, vec!["event_id", "payee_key"]);

        let mut record_columns = conn
            .query(
                "SELECT name FROM pragma_table_info('import_records') WHERE name IN ('counterparty_normalized', 'normalization_version') ORDER BY name",
                (),
            )
            .await
            .unwrap();
        let mut record_column_names = Vec::new();
        while let Some(row) = record_columns.next().await.unwrap() {
            record_column_names.push(row.get::<String>(0).unwrap());
        }
        assert_eq!(
            record_column_names,
            vec!["counterparty_normalized", "normalization_version"]
        );
        let mut cluster_columns = conn
            .query(
                "SELECT name FROM pragma_table_info('duplicate_suspicions') WHERE name='cluster_key'",
                (),
            )
            .await
            .unwrap();
        assert!(cluster_columns.next().await.unwrap().is_some());

        let mut confirmation_columns = conn
            .query(
                "SELECT name FROM pragma_table_info('duplicate_suspicions') WHERE name IN ('event_id', 'revert_payload') ORDER BY name",
                (),
            )
            .await
            .unwrap();
        let mut confirmation_column_names = Vec::new();
        while let Some(row) = confirmation_columns.next().await.unwrap() {
            confirmation_column_names.push(row.get::<String>(0).unwrap());
        }
        assert_eq!(
            confirmation_column_names,
            vec!["event_id", "revert_payload"]
        );
    }

    #[tokio::test]
    async fn bill_import_constraints_enforce_idempotency_staging_and_nullable_provenance() {
        let root = tempfile::tempdir().unwrap();
        let db = Builder::new_local(root.path().join("bill-import-constraints.db"))
            .build()
            .await
            .unwrap();
        migrate(&db).await.unwrap();
        let conn = db.connect().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .await
            .unwrap();
        conn.execute(
            "INSERT INTO users(id, email, password_hash, timezone, created_at, updated_at) VALUES ('u1', 'constraints@example.com', 'hash', 'Asia/Shanghai', '2026-08-11T00:00:00Z', '2026-08-11T00:00:00Z')",
            (),
        )
        .await
        .unwrap();

        for id in ["manual-1", "manual-2"] {
            conn.execute(
                "INSERT INTO ledger_transactions(id, user_id, kind, amount_cents, occurred_on, created_at, updated_at) VALUES (?1, 'u1', 'expense', 100, '2026-08-11', '2026-08-11T00:00:00Z', '2026-08-11T00:00:00Z')",
                [id],
            )
            .await
            .unwrap();
        }
        conn.execute(
            "INSERT INTO ledger_transactions(id, user_id, kind, amount_cents, occurred_on, source_channel, external_id, created_at, updated_at) VALUES ('wechat-1', 'u1', 'expense', 100, '2026-08-11', 'wechat', 'same-id', '2026-08-11T00:00:00Z', '2026-08-11T00:00:00Z')",
            (),
        )
        .await
        .unwrap();
        assert!(conn.execute(
            "INSERT INTO ledger_transactions(id, user_id, kind, amount_cents, occurred_on, source_channel, external_id, created_at, updated_at) VALUES ('wechat-2', 'u1', 'expense', 100, '2026-08-11', 'wechat', 'same-id', '2026-08-11T00:00:00Z', '2026-08-11T00:00:00Z')",
            (),
        ).await.is_err());
        conn.execute(
            "INSERT INTO ledger_transactions(id, user_id, kind, amount_cents, occurred_on, source_channel, external_id, created_at, updated_at) VALUES ('alipay-1', 'u1', 'expense', 100, '2026-08-11', 'alipay', 'same-id', '2026-08-11T00:00:00Z', '2026-08-11T00:00:00Z')",
            (),
        )
        .await
        .unwrap();

        conn.execute(
            "INSERT INTO import_batches(id, user_id, source_channel, file_sha256, period_start, period_end, total_count, created_at, updated_at) VALUES ('b1', 'u1', 'wechat', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', '2026-08-01', '2026-08-11', 3, '2026-08-11T00:00:00Z', '2026-08-11T00:00:00Z')",
            (),
        )
        .await
        .unwrap();
        assert!(conn.execute(
            "INSERT INTO import_batches(id, user_id, source_channel, file_sha256, period_start, period_end, total_count, status, created_at, updated_at) VALUES ('bad-batch', 'u1', 'wechat', 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', '2026-08-01', '2026-08-11', 1, 'done', '2026-08-11T00:00:00Z', '2026-08-11T00:00:00Z')",
            (),
        ).await.is_err());
        conn.execute(
            "INSERT INTO import_records(id, batch_id, row_index, external_id, occurred_at, occurred_on, direction, amount_cents, source_note, disposition, transaction_id, created_at) VALUES ('r1', 'b1', 1, 'record-1', '2026-08-11T00:00:00Z', '2026-08-11', 'expense', 100, 'note', 'import', 'wechat-1', '2026-08-11T00:00:00Z')",
            (),
        )
        .await
        .unwrap();
        assert!(conn.execute(
            "INSERT INTO import_records(id, batch_id, row_index, external_id, occurred_at, occurred_on, direction, amount_cents, disposition, transaction_id, created_at) VALUES ('r2', 'b1', 2, 'record-2', '2026-08-11T00:00:00Z', '2026-08-11', 'expense', 100, 'import', 'wechat-1', '2026-08-11T00:00:00Z')",
            (),
        ).await.is_err());
        assert!(conn.execute(
            "INSERT INTO import_records(id, batch_id, row_index, external_id, occurred_at, occurred_on, direction, amount_cents, disposition, created_at) VALUES ('duplicate-row', 'b1', 1, 'record-duplicate-row', '2026-08-11T00:00:00Z', '2026-08-11', 'expense', 100, 'import', '2026-08-11T00:00:00Z')",
            (),
        ).await.is_err());
        assert!(conn.execute(
            "INSERT INTO import_records(id, batch_id, row_index, external_id, occurred_at, occurred_on, direction, amount_cents, disposition, transaction_id, created_at) VALUES ('bad-link', 'b1', 2, 'record-link', '2026-08-11T00:00:00Z', '2026-08-11', 'expense', 100, 'pending', 'alipay-1', '2026-08-11T00:00:00Z')",
            (),
        ).await.is_err());
        assert!(conn.execute(
            "INSERT INTO import_records(id, batch_id, row_index, external_id, occurred_at, occurred_on, direction, amount_cents, disposition, created_at) VALUES ('bad-neutral', 'b1', 2, 'record-3', '2026-08-11T00:00:00Z', '2026-08-11', 'expense', 0, 'neutral', '2026-08-11T00:00:00Z')",
            (),
        ).await.is_err());
        assert!(conn.execute(
            "INSERT INTO import_records(id, batch_id, row_index, external_id, occurred_at, occurred_on, direction, amount_cents, disposition, created_at) VALUES ('bad-zero', 'b1', 2, 'record-4', '2026-08-11T00:00:00Z', '2026-08-11', 'expense', 1, 'zero_amount', '2026-08-11T00:00:00Z')",
            (),
        ).await.is_err());
        assert!(conn.execute(
            "INSERT INTO import_records(id, batch_id, row_index, external_id, occurred_at, occurred_on, direction, amount_cents, disposition, created_at) VALUES ('too-large', 'b1', 2, 'record-5', '2026-08-11T00:00:00Z', '2026-08-11', 'expense', 9007199254740992, 'unknown', '2026-08-11T00:00:00Z')",
            (),
        ).await.is_err());

        conn.execute("DELETE FROM ledger_transactions WHERE id = 'wechat-1'", ())
            .await
            .unwrap();
        let mut rows = conn
            .query(
                "SELECT transaction_id FROM import_records WHERE id = 'r1'",
                (),
            )
            .await
            .unwrap();
        assert!(
            rows.next()
                .await
                .unwrap()
                .unwrap()
                .get::<Option<String>>(0)
                .unwrap()
                .is_none()
        );

        conn.execute(
            "UPDATE ledger_transactions SET import_batch_id = 'b1' WHERE id = 'alipay-1'",
            (),
        )
        .await
        .unwrap();
        conn.execute("DELETE FROM import_batches WHERE id = 'b1'", ())
            .await
            .unwrap();
        let mut rows = conn
            .query(
                "SELECT import_batch_id FROM ledger_transactions WHERE id = 'alipay-1'",
                (),
            )
            .await
            .unwrap();
        assert!(
            rows.next()
                .await
                .unwrap()
                .unwrap()
                .get::<Option<String>>(0)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn latest_balance_view_posts_both_transfer_legs() {
        let root = tempfile::tempdir().unwrap();
        let db = Builder::new_local(root.path().join("transfer-balances.db"))
            .build()
            .await
            .unwrap();
        migrate(&db).await.unwrap();
        let conn = db.connect().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .await
            .unwrap();
        conn.execute(
            "INSERT INTO users(id, email, password_hash, timezone, created_at, updated_at) VALUES ('u1', 'transfer@example.com', 'hash', 'Asia/Shanghai', '2026-08-12T00:00:00Z', '2026-08-12T00:00:00Z')",
            (),
        )
        .await
        .unwrap();
        for (id, name) in [("a", "账户 A"), ("b", "账户 B")] {
            conn.execute(
                "INSERT INTO ledger_accounts(id, user_id, name, normalized_name, created_at, updated_at) VALUES (?1, 'u1', ?2, ?2, '2026-08-12T00:00:00Z', '2026-08-12T00:00:00Z')",
                libsql::params![id, name],
            )
            .await
            .unwrap();
        }
        conn.execute(
            "INSERT INTO ledger_transactions(id, user_id, kind, amount_cents, occurred_on, transfer_from_account_id, transfer_to_account_id, created_at, updated_at) VALUES ('t1', 'u1', 'transfer', 10000, '2026-08-12', 'a', 'b', '2026-08-12T00:00:00Z', '2026-08-12T00:00:00Z')",
            (),
        )
        .await
        .unwrap();

        let mut rows = conn
            .query(
                "SELECT account_id, balance_cents FROM ledger_account_balances WHERE account_id IN ('a', 'b') ORDER BY account_id",
                (),
            )
            .await
            .unwrap();
        let from = rows.next().await.unwrap().unwrap();
        assert_eq!(from.get::<String>(0).unwrap(), "a");
        assert_eq!(from.get::<i64>(1).unwrap(), -10000);
        let to = rows.next().await.unwrap().unwrap();
        assert_eq!(to.get::<String>(0).unwrap(), "b");
        assert_eq!(to.get::<i64>(1).unwrap(), 10000);
        assert!(rows.next().await.unwrap().is_none());
    }
}
