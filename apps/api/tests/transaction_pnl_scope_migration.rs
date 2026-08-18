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

async fn legacy_monthly_snapshot(conn: &Connection) -> MonthlySnapshot {
    monthly_snapshot(
        conn,
        "AND NOT EXISTS (SELECT 1 FROM repayment_events r WHERE r.transaction_id = t.id) AND NOT EXISTS (SELECT 1 FROM debt_addition_events e WHERE e.transaction_id = t.id) AND NOT EXISTS (SELECT 1 FROM debts d WHERE d.transaction_id = t.id)",
    )
    .await
}

async fn pnl_monthly_snapshot(conn: &Connection) -> MonthlySnapshot {
    monthly_snapshot(conn, "AND t.pnl_scope = 'counted'").await
}

async fn monthly_snapshot(conn: &Connection, scope_predicate: &str) -> MonthlySnapshot {
    let sql = format!(
        "SELECT t.user_id, substr(t.occurred_on, 1, 7), COALESCE(NULLIF(t.category, ''), ''), SUM(CASE WHEN t.kind = 'income' THEN t.amount_cents ELSE 0 END), SUM(CASE WHEN t.kind = 'expense' THEN t.amount_cents ELSE 0 END), SUM(CASE WHEN t.kind IN ('income', 'expense') THEN 1 ELSE 0 END) FROM ledger_transactions t WHERE t.archived_at IS NULL {scope_predicate} GROUP BY t.user_id, substr(t.occurred_on, 1, 7), NULLIF(t.category, '') ORDER BY t.user_id, substr(t.occurred_on, 1, 7), NULLIF(t.category, '')"
    );
    let mut rows = conn.query(&sql, ()).await.unwrap();
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
async fn transaction_pnl_scope_migration_preserves_balances_and_monthly_statistics() {
    let root = TempDir::new().unwrap();
    let database = Builder::new_local(root.path().join("pnl-scope-reconciliation.db"))
        .build()
        .await
        .unwrap();
    db::migrate_up_to(&database, 24).await.unwrap();
    let conn = database.connect().unwrap();
    let now = "2026-08-17T00:00:00Z";

    conn.execute(
        "INSERT INTO users(id,email,password_hash,timezone,email_verified_at,created_at,updated_at) VALUES ('pnl-user','pnl@example.invalid','hash','Asia/Shanghai',?1,?1,?1)",
        [now],
    )
    .await
    .unwrap();
    for (id, name) in [
        ("pnl-account-a", "测试账户甲"),
        ("pnl-account-b", "测试账户乙"),
    ] {
        conn.execute(
            "INSERT INTO ledger_accounts(id,user_id,name,normalized_name,account_type,note,opening_balance_cents,version,created_at,updated_at) VALUES (?1,'pnl-user',?2,?2,'cash','',0,1,?3,?3)",
            libsql::params![id, name, now],
        )
        .await
        .unwrap();
    }
    conn.execute(
        "INSERT INTO counterparties(id,user_id,display_name,normalized_name,note,version,created_at,updated_at) VALUES ('pnl-counterparty','pnl-user','测试往来方','测试往来方','',1,?1,?1)",
        [now],
    )
    .await
    .unwrap();

    for (id, kind, amount, occurred_on, category, account_id) in [
        (
            "pnl-principal",
            "income",
            1_000_i64,
            "2026-05-03",
            "本金",
            "pnl-account-a",
        ),
        (
            "pnl-addition",
            "income",
            200_i64,
            "2026-06-04",
            "追加",
            "pnl-account-a",
        ),
        (
            "pnl-repayment",
            "expense",
            300_i64,
            "2026-06-20",
            "还款",
            "pnl-account-a",
        ),
        (
            "pnl-counted",
            "expense",
            400_i64,
            "2026-07-05",
            "日常",
            "pnl-account-b",
        ),
    ] {
        conn.execute(
            "INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,occurred_on,category,account_id,created_at,updated_at) VALUES (?1,'pnl-user',?2,?3,?4,?5,?6,?7,?7)",
            libsql::params![id, kind, amount, occurred_on, category, account_id, now],
        )
        .await
        .unwrap();
    }
    conn.execute(
        "INSERT INTO debts(id,user_id,counterparty_id,direction,principal_cents,currency,occurred_on,note,version,created_at,updated_at,account_id,origin_kind,transaction_id) VALUES ('pnl-debt','pnl-user','pnl-counterparty','borrow_in',1200,'CNY','2026-05-03','',1,?1,?1,'pnl-account-a','cash_movement','pnl-principal')",
        [now],
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO debt_addition_events(id,user_id,debt_id,amount_cents,effective_on,note,created_at,account_id,transaction_id) VALUES ('pnl-addition-event','pnl-user','pnl-debt',200,'2026-06-04','',?1,'pnl-account-a','pnl-addition')",
        [now],
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO repayment_events(id,user_id,debt_id,kind,amount_cents,effective_on,note,created_at,account_id,transaction_id) VALUES ('pnl-repayment-event','pnl-user','pnl-debt','payment',300,'2026-06-20','',?1,'pnl-account-a','pnl-repayment')",
        [now],
    )
    .await
    .unwrap();

    let balances_before = balance_snapshot(&conn).await;
    let monthly_before = legacy_monthly_snapshot(&conn).await;
    drop(conn);

    db::migrate_up_to(&database, 25).await.unwrap();
    let conn = database.connect().unwrap();
    assert_eq!(balance_snapshot(&conn).await, balances_before);
    assert_eq!(pnl_monthly_snapshot(&conn).await, monthly_before);

    let mut rows = conn
        .query(
            "SELECT id,pnl_scope FROM ledger_transactions WHERE user_id='pnl-user' ORDER BY id",
            (),
        )
        .await
        .unwrap();
    let mut scopes = BTreeMap::new();
    while let Some(row) = rows.next().await.unwrap() {
        scopes.insert(row.get::<String>(0).unwrap(), row.get::<String>(1).unwrap());
    }
    assert_eq!(scopes.get("pnl-principal").unwrap(), "excluded");
    assert_eq!(scopes.get("pnl-addition").unwrap(), "excluded");
    assert_eq!(scopes.get("pnl-repayment").unwrap(), "excluded");
    assert_eq!(scopes.get("pnl-counted").unwrap(), "counted");

    let mut indexes = conn
        .query(
            "SELECT 1 FROM sqlite_master WHERE type='index' AND name='idx_ledger_transactions_user_pnl_scope'",
            (),
        )
        .await
        .unwrap();
    assert!(indexes.next().await.unwrap().is_some());
    assert!(
        conn.execute(
            "UPDATE ledger_transactions SET pnl_scope='invalid' WHERE id='pnl-counted'",
            (),
        )
        .await
        .is_err()
    );
}
