use std::collections::BTreeMap;

use libsql::{Builder, Connection};
use tempfile::TempDir;
use zhiyu_api::db;

type BalanceSnapshot = BTreeMap<(String, String), i64>;
type MonthlySnapshot = BTreeMap<(String, String, String), (i64, i64, i64)>;

async fn balance_snapshot(conn: &Connection) -> BalanceSnapshot {
    let mut rows = conn
        .query(
            "SELECT a.user_id, b.account_id, b.balance_cents FROM ledger_account_balances b JOIN ledger_accounts a ON a.id = b.account_id ORDER BY a.user_id, b.account_id",
            (),
        )
        .await
        .unwrap();
    let mut snapshot = BTreeMap::new();
    while let Some(row) = rows.next().await.unwrap() {
        snapshot.insert(
            (row.get(0).unwrap(), row.get(1).unwrap()),
            row.get(2).unwrap(),
        );
    }
    snapshot
}

async fn monthly_snapshot(conn: &Connection) -> MonthlySnapshot {
    let mut rows = conn
        .query(
            "SELECT t.user_id, substr(t.occurred_on, 1, 7), CASE WHEN c.id IS NOT NULL THEN c.name ELSE COALESCE(NULLIF(t.category, ''), '') END, SUM(CASE WHEN t.kind = 'income' THEN t.amount_cents ELSE 0 END), SUM(CASE WHEN t.kind = 'expense' THEN t.amount_cents ELSE 0 END), SUM(CASE WHEN t.kind IN ('income', 'expense') THEN 1 ELSE 0 END) FROM ledger_transactions t LEFT JOIN categories c ON c.id = t.category_id WHERE t.archived_at IS NULL AND NOT EXISTS (SELECT 1 FROM repayment_events r WHERE r.transaction_id = t.id) AND NOT EXISTS (SELECT 1 FROM debt_addition_events e WHERE e.transaction_id = t.id) AND NOT EXISTS (SELECT 1 FROM debts d WHERE d.transaction_id = t.id) GROUP BY t.user_id, substr(t.occurred_on, 1, 7), c.id, CASE WHEN c.id IS NULL THEN NULLIF(t.category, '') END ORDER BY t.user_id, substr(t.occurred_on, 1, 7), c.id",
            (),
        )
        .await
        .unwrap();
    let mut snapshot = BTreeMap::new();
    while let Some(row) = rows.next().await.unwrap() {
        snapshot.insert(
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
    snapshot
}

#[tokio::test]
async fn debt_cash_migration_preserves_every_account_balance_and_monthly_statistic() {
    let root = TempDir::new().unwrap();
    let database = Builder::new_local(root.path().join("reconciliation.db"))
        .build()
        .await
        .unwrap();
    db::migrate_up_to(&database, 23).await.unwrap();
    let conn = database.connect().unwrap();
    let now = "2026-08-17T00:00:00Z";

    conn.execute(
        "INSERT INTO users(id,email,password_hash,timezone,email_verified_at,created_at,updated_at) VALUES ('migration-user','migration@example.invalid','hash','Asia/Shanghai',?1,?1,?1)",
        [now],
    ).await.unwrap();
    for (id, name) in [("account-a", "账户甲"), ("account-b", "账户乙")] {
        conn.execute(
            "INSERT INTO ledger_accounts(id,user_id,name,normalized_name,account_type,note,opening_balance_cents,version,created_at,updated_at) VALUES (?1,'migration-user',?2,?2,'cash','',0,1,?3,?3)",
            libsql::params![id, name, now],
        ).await.unwrap();
    }
    for (id, name) in [
        ("counterparty-a", "测试往来方甲"),
        ("counterparty-b", "测试往来方乙"),
        ("counterparty-c", "测试往来方丙"),
        ("counterparty-d", "测试往来方丁"),
    ] {
        conn.execute(
            "INSERT INTO counterparties(id,user_id,display_name,normalized_name,note,version,created_at,updated_at) VALUES (?1,'migration-user',?2,?2,'',1,?3,?3)",
            libsql::params![id, name, now],
        ).await.unwrap();
    }

    for (id, counterparty, direction, principal, account, origin, occurred_on) in [
        (
            "cash-borrow",
            "counterparty-a",
            "borrow_in",
            1_300_i64,
            Some("account-a"),
            "cash_movement",
            "2026-05-03",
        ),
        (
            "cash-lend",
            "counterparty-b",
            "lend_out",
            1_900_i64,
            Some("account-b"),
            "cash_movement",
            "2026-06-04",
        ),
        (
            "cashless",
            "counterparty-c",
            "borrow_in",
            700_i64,
            None,
            "no_cash_movement",
            "2026-07-05",
        ),
        (
            "legacy",
            "counterparty-d",
            "lend_out",
            800_i64,
            None,
            "legacy_unknown",
            "2026-08-06",
        ),
    ] {
        conn.execute(
            "INSERT INTO debts(id,user_id,counterparty_id,direction,principal_cents,currency,occurred_on,note,version,created_at,updated_at,account_id,origin_kind,transaction_id) VALUES (?1,'migration-user',?2,?3,?4,'CNY',?5,'',1,?6,?6,?7,?8,NULL)",
            libsql::params![id, counterparty, direction, principal, occurred_on, now, account, origin],
        ).await.unwrap();
    }

    for (id, debt_id, amount, account, effective_on) in [
        (
            "borrow-addition",
            "cash-borrow",
            300_i64,
            "account-b",
            "2026-05-10",
        ),
        (
            "lend-addition",
            "cash-lend",
            400_i64,
            "account-a",
            "2026-06-11",
        ),
    ] {
        conn.execute(
            "INSERT INTO debt_addition_events(id,user_id,debt_id,amount_cents,effective_on,note,created_at,account_id,transaction_id) VALUES (?1,'migration-user',?2,?3,?4,'',?5,?6,NULL)",
            libsql::params![id, debt_id, amount, effective_on, now, account],
        ).await.unwrap();
    }

    for (id, debt_id, kind, amount, effective_on, reverses, account) in [
        (
            "borrow-payment",
            "cash-borrow",
            "payment",
            100_i64,
            "2026-05-20",
            None,
            "account-a",
        ),
        (
            "borrow-reversal",
            "cash-borrow",
            "reversal",
            100_i64,
            "2026-05-21",
            Some("borrow-payment"),
            "account-a",
        ),
        (
            "lend-payment",
            "cash-lend",
            "payment",
            200_i64,
            "2026-06-20",
            None,
            "account-b",
        ),
        (
            "lend-reversal",
            "cash-lend",
            "reversal",
            200_i64,
            "2026-06-21",
            Some("lend-payment"),
            "account-b",
        ),
    ] {
        conn.execute(
            "INSERT INTO repayment_events(id,user_id,debt_id,kind,amount_cents,effective_on,note,reverses_event_id,created_at,account_id,transaction_id) VALUES (?1,'migration-user',?2,?3,?4,?5,'',?6,?7,?8,NULL)",
            libsql::params![id, debt_id, kind, amount, effective_on, reverses, now, account],
        ).await.unwrap();
    }

    let balances_before = balance_snapshot(&conn).await;
    let monthly_before = monthly_snapshot(&conn).await;
    drop(conn);

    db::migrate_up_to(&database, 24).await.unwrap();
    let conn = database.connect().unwrap();
    assert_eq!(balance_snapshot(&conn).await, balances_before);
    assert_eq!(monthly_snapshot(&conn).await, monthly_before);

    for (table, expected) in [
        ("debts", 2_i64),
        ("debt_addition_events", 2_i64),
        ("repayment_events", 4_i64),
    ] {
        let sql = format!(
            "SELECT COUNT(*) FROM {table} WHERE transaction_id IS NOT NULL AND transaction_auto_created = 1"
        );
        let mut rows = conn.query(&sql, ()).await.unwrap();
        assert_eq!(
            rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
            expected
        );
    }

    let mut controls = conn
        .query(
            "SELECT COUNT(*) FROM debts WHERE id IN ('cashless','legacy') AND transaction_id IS NULL AND transaction_auto_created = 0",
            (),
        )
        .await
        .unwrap();
    assert_eq!(
        controls
            .next()
            .await
            .unwrap()
            .unwrap()
            .get::<i64>(0)
            .unwrap(),
        2
    );
}
