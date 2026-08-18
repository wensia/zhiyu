use std::collections::BTreeMap;

use libsql::{Builder, Connection};
use tempfile::TempDir;
use zhiyu_api::db;

type BalanceSnapshot = BTreeMap<String, i64>;
type MonthlySnapshot = BTreeMap<(String, String, String), (i64, i64, i64)>;

async fn balances(conn: &Connection) -> BalanceSnapshot {
    let mut rows = conn
        .query(
            "SELECT account_id,balance_cents FROM ledger_account_balances ORDER BY account_id",
            (),
        )
        .await
        .unwrap();
    let mut values = BTreeMap::new();
    while let Some(row) = rows.next().await.unwrap() {
        values.insert(row.get(0).unwrap(), row.get(1).unwrap());
    }
    values
}

async fn monthly_statistics(conn: &Connection) -> MonthlySnapshot {
    let mut rows = conn
        .query(
            "SELECT user_id,substr(occurred_on,1,7),COALESCE(NULLIF(category,''),''),SUM(CASE WHEN kind='income' THEN amount_cents ELSE 0 END),SUM(CASE WHEN kind='expense' THEN amount_cents ELSE 0 END),SUM(CASE WHEN kind IN ('income','expense') THEN 1 ELSE 0 END) FROM ledger_transactions WHERE archived_at IS NULL AND pnl_scope='counted' GROUP BY user_id,substr(occurred_on,1,7),NULLIF(category,'') ORDER BY user_id,substr(occurred_on,1,7),NULLIF(category,'')",
            (),
        )
        .await
        .unwrap();
    let mut values = BTreeMap::new();
    while let Some(row) = rows.next().await.unwrap() {
        values.insert(
            (
                row.get(0).unwrap(),
                row.get(1).unwrap(),
                row.get(2).unwrap(),
            ),
            (
                row.get(3).unwrap(),
                row.get(4).unwrap(),
                row.get(5).unwrap(),
            ),
        );
    }
    values
}

async fn seed_v26(database: &libsql::Database) -> Connection {
    db::migrate_up_to(database, 26).await.unwrap();
    let conn = database.connect().unwrap();
    let now = "2026-08-18T00:00:00Z";
    conn.execute(
        "INSERT INTO users(id,email,password_hash,timezone,email_verified_at,created_at,updated_at) VALUES ('c3-user','c3@example.invalid','hash','Asia/Shanghai',?1,?1,?1)",
        [now],
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO ledger_accounts(id,user_id,name,normalized_name,account_type,note,opening_balance_cents,version,created_at,updated_at) VALUES ('c3-account','c3-user','测试账户','测试账户','cash','',0,1,?1,?1)",
        [now],
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO counterparties(id,user_id,display_name,normalized_name,note,version,created_at,updated_at) VALUES ('c3-counterparty','c3-user','测试往来方','测试往来方','',1,?1,?1)",
        [now],
    )
    .await
    .unwrap();
    conn
}

#[tokio::test]
async fn migration_backfills_sources_and_keeps_balances_and_statistics_unchanged() {
    let root = TempDir::new().unwrap();
    let database = Builder::new_local(root.path().join("c3-reconciliation.db"))
        .build()
        .await
        .unwrap();
    let conn = seed_v26(&database).await;
    let now = "2026-08-18T00:00:00Z";

    for (id, kind, amount, category, scope, source_channel, external_id) in [
        (
            "c3-auto",
            "income",
            1_000_i64,
            "自动本金",
            "excluded",
            "",
            "",
        ),
        (
            "c3-user-linked",
            "expense",
            200,
            "用户关联",
            "excluded",
            "",
            "",
        ),
        (
            "c3-import",
            "income",
            300,
            "导入",
            "counted",
            "alipay",
            "import-row",
        ),
        ("c3-manual", "expense", 50, "手工", "counted", "", ""),
        (
            "c3-zero-addition",
            "income",
            500,
            "零差额追加",
            "excluded",
            "",
            "",
        ),
    ] {
        conn.execute(
            "INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,occurred_on,category,account_id,created_at,updated_at,pnl_scope,source_channel,external_id) VALUES (?1,'c3-user',?2,?3,'2026-08-18',?4,'c3-account',?5,?5,?6,?7,?8)",
            libsql::params![id, kind, amount, category, now, scope, source_channel, external_id],
        )
        .await
        .unwrap();
    }
    for (id, direction, principal, transaction_id, auto_created) in [
        (
            "c3-debt-auto",
            "borrow_in",
            1_000_i64,
            Some("c3-auto"),
            1_i64,
        ),
        ("c3-debt-user", "lend_out", 200, Some("c3-user-linked"), 0),
        ("c3-debt-zero", "borrow_in", 500, None, 0),
    ] {
        conn.execute(
            "INSERT INTO debts(id,user_id,counterparty_id,direction,principal_cents,currency,occurred_on,note,version,created_at,updated_at,account_id,origin_kind,transaction_id,transaction_auto_created) VALUES (?1,'c3-user','c3-counterparty',?2,?3,'CNY','2026-08-18','',1,?4,?4,'c3-account','cash_movement',?5,?6)",
            libsql::params![id, direction, principal, now, transaction_id, auto_created],
        )
        .await
        .unwrap();
    }
    conn.execute(
        "INSERT INTO debt_addition_events(id,user_id,debt_id,amount_cents,effective_on,note,created_at,account_id,transaction_id,transaction_auto_created) VALUES ('c3-zero-addition-event','c3-user','c3-debt-zero',500,'2026-08-18','',?1,'c3-account','c3-zero-addition',1)",
        [now],
    )
    .await
    .unwrap();
    for (id, transaction_id, kind, ref_id) in [
        ("c3-link-auto", "c3-auto", "principal", "c3-debt-auto"),
        (
            "c3-link-user",
            "c3-user-linked",
            "principal",
            "c3-debt-user",
        ),
        (
            "c3-link-zero",
            "c3-zero-addition",
            "addition",
            "c3-debt-zero",
        ),
    ] {
        conn.execute(
            "INSERT INTO transaction_links(id,user_id,transaction_id,plugin_id,kind,ref_id,label,created_at) VALUES (?1,'c3-user',?2,'debts',?3,?4,'测试往来方',?5)",
            libsql::params![id, transaction_id, kind, ref_id, now],
        )
        .await
        .unwrap();
    }

    let balances_before = balances(&conn).await;
    let statistics_before = monthly_statistics(&conn).await;
    drop(conn);
    db::migrate_up_to(&database, 27).await.unwrap();
    let conn = database.connect().unwrap();

    assert_eq!(balances(&conn).await, balances_before);
    assert_eq!(monthly_statistics(&conn).await, statistics_before);
    let mut rows = conn
        .query(
            "SELECT id,created_by FROM ledger_transactions WHERE user_id='c3-user' ORDER BY id",
            (),
        )
        .await
        .unwrap();
    let mut sources = BTreeMap::new();
    while let Some(row) = rows.next().await.unwrap() {
        sources.insert(row.get::<String>(0).unwrap(), row.get::<String>(1).unwrap());
    }
    assert_eq!(sources["c3-auto"], "plugin:debts");
    assert_eq!(sources["c3-zero-addition"], "plugin:debts");
    assert_eq!(sources["c3-import"], "plugin:bill-imports");
    assert_eq!(sources["c3-user-linked"], "user");
    assert_eq!(sources["c3-manual"], "user");

    let mut rows = conn
        .query(
            "SELECT sql FROM sqlite_master WHERE type='view' AND name='ledger_account_movements'",
            (),
        )
        .await
        .unwrap();
    let view_sql = rows
        .next()
        .await
        .unwrap()
        .unwrap()
        .get::<String>(0)
        .unwrap();
    assert_eq!(view_sql.matches("ledger_transactions").count(), 4);
    for plugin_table in ["debts", "debt_addition_events", "repayment_events"] {
        assert!(!view_sql.contains(plugin_table), "{view_sql}");
    }
    assert!(
        conn.execute(
            "UPDATE ledger_transactions SET created_by='plugin:unknown' WHERE id='c3-manual'",
            (),
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn migration_refuses_negative_unmigrated_principal_with_an_actionable_count() {
    let root = TempDir::new().unwrap();
    let database = Builder::new_local(root.path().join("c3-guard.db"))
        .build()
        .await
        .unwrap();
    let conn = seed_v26(&database).await;
    let now = "2026-08-18T00:00:00Z";
    conn.execute(
        "INSERT INTO debts(id,user_id,counterparty_id,direction,principal_cents,currency,occurred_on,note,version,created_at,updated_at,account_id,origin_kind,transaction_id,transaction_auto_created) VALUES ('c3-negative','c3-user','c3-counterparty','borrow_in',100,'CNY','2026-08-18','',1,?1,?1,'c3-account','cash_movement',NULL,0)",
        [now],
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO debt_addition_events(id,user_id,debt_id,amount_cents,effective_on,note,created_at,account_id,transaction_id,transaction_auto_created) VALUES ('c3-negative-addition','c3-user','c3-negative',200,'2026-08-18','',?1,'c3-account',NULL,0)",
        [now],
    )
    .await
    .unwrap();
    drop(conn);

    let error = db::migrate_up_to(&database, 27).await.unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("found 1"), "{message}");
    assert!(
        message.contains("这些债务的追加借款之和超过本金，请先在债务页修正后重启"),
        "{message}"
    );
    let conn = database.connect().unwrap();
    let mut rows = conn
        .query("SELECT 1 FROM schema_migrations WHERE version=27", ())
        .await
        .unwrap();
    assert!(rows.next().await.unwrap().is_none());
}
