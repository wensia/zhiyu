use std::collections::{BTreeMap, BTreeSet};

use libsql::{Builder, Connection};
use tempfile::TempDir;
use zhiyu_api::db;

type LinkSnapshot = BTreeSet<(String, String, String, String)>;
type BalanceSnapshot = BTreeMap<String, i64>;
type StatisticsSnapshot = BTreeMap<(String, String), (i64, i64, i64)>;

async fn legacy_links(conn: &Connection) -> LinkSnapshot {
    let mut rows = conn
        .query(
            "SELECT d.transaction_id, 'principal', d.id, c.display_name FROM debts d JOIN counterparties c ON c.id=d.counterparty_id AND c.user_id=d.user_id WHERE d.transaction_id IS NOT NULL UNION ALL SELECT e.transaction_id, 'addition', e.debt_id, c.display_name FROM debt_addition_events e JOIN debts d ON d.id=e.debt_id AND d.user_id=e.user_id JOIN counterparties c ON c.id=d.counterparty_id AND c.user_id=d.user_id WHERE e.transaction_id IS NOT NULL UNION ALL SELECT e.transaction_id, 'repayment', e.debt_id, c.display_name FROM repayment_events e JOIN debts d ON d.id=e.debt_id AND d.user_id=e.user_id JOIN counterparties c ON c.id=d.counterparty_id AND c.user_id=d.user_id WHERE e.transaction_id IS NOT NULL",
            (),
        )
        .await
        .unwrap();
    let mut snapshot = BTreeSet::new();
    while let Some(row) = rows.next().await.unwrap() {
        snapshot.insert((
            row.get(0).unwrap(),
            row.get(1).unwrap(),
            row.get(2).unwrap(),
            row.get(3).unwrap(),
        ));
    }
    snapshot
}

async fn generic_links(conn: &Connection) -> LinkSnapshot {
    let mut rows = conn
        .query(
            "SELECT transaction_id, kind, ref_id, label FROM transaction_links WHERE plugin_id='debts' ORDER BY transaction_id, kind, ref_id",
            (),
        )
        .await
        .unwrap();
    let mut snapshot = BTreeSet::new();
    while let Some(row) = rows.next().await.unwrap() {
        snapshot.insert((
            row.get(0).unwrap(),
            row.get(1).unwrap(),
            row.get(2).unwrap(),
            row.get(3).unwrap(),
        ));
    }
    snapshot
}

async fn balances(conn: &Connection) -> BalanceSnapshot {
    let mut rows = conn
        .query(
            "SELECT account_id, balance_cents FROM ledger_account_balances ORDER BY account_id",
            (),
        )
        .await
        .unwrap();
    let mut snapshot = BTreeMap::new();
    while let Some(row) = rows.next().await.unwrap() {
        snapshot.insert(row.get(0).unwrap(), row.get(1).unwrap());
    }
    snapshot
}

async fn statistics(conn: &Connection) -> StatisticsSnapshot {
    let mut rows = conn
        .query(
            "SELECT user_id, substr(occurred_on,1,7), SUM(CASE WHEN kind='income' THEN amount_cents ELSE 0 END), SUM(CASE WHEN kind='expense' THEN amount_cents ELSE 0 END), COUNT(*) FROM ledger_transactions WHERE archived_at IS NULL AND pnl_scope='counted' GROUP BY user_id, substr(occurred_on,1,7) ORDER BY user_id, substr(occurred_on,1,7)",
            (),
        )
        .await
        .unwrap();
    let mut snapshot = BTreeMap::new();
    while let Some(row) = rows.next().await.unwrap() {
        snapshot.insert(
            (row.get(0).unwrap(), row.get(1).unwrap()),
            (
                row.get(2).unwrap(),
                row.get(3).unwrap(),
                row.get(4).unwrap(),
            ),
        );
    }
    snapshot
}

#[tokio::test]
async fn transaction_links_migration_backfills_all_debt_kinds_without_changing_ledger_results() {
    let root = TempDir::new().unwrap();
    let database = Builder::new_local(root.path().join("transaction-links.db"))
        .build()
        .await
        .unwrap();
    db::migrate_up_to(&database, 25).await.unwrap();
    let conn = database.connect().unwrap();
    let now = "2026-08-18T00:00:00Z";

    conn.execute(
        "INSERT INTO users(id,email,password_hash,timezone,email_verified_at,created_at,updated_at) VALUES ('links-user','links@example.invalid','hash','Asia/Shanghai',?1,?1,?1)",
        [now],
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO ledger_accounts(id,user_id,name,normalized_name,account_type,note,opening_balance_cents,version,created_at,updated_at) VALUES ('links-account','links-user','测试账户','测试账户','cash','',0,1,?1,?1)",
        [now],
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO counterparties(id,user_id,display_name,normalized_name,note,version,created_at,updated_at) VALUES ('links-counterparty','links-user','测试联系人','测试联系人','',1,?1,?1)",
        [now],
    )
    .await
    .unwrap();
    for (id, kind, amount, date) in [
        ("links-principal", "income", 1_000_i64, "2026-05-01"),
        ("links-addition", "income", 200_i64, "2026-05-02"),
        ("links-repayment", "expense", 300_i64, "2026-05-03"),
        ("links-control", "income", 400_i64, "2026-06-01"),
    ] {
        conn.execute(
            "INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,occurred_on,account_id,created_at,updated_at,pnl_scope) VALUES (?1,'links-user',?2,?3,?4,'links-account',?5,?5,?6)",
            libsql::params![id, kind, amount, date, now, if id == "links-control" { "counted" } else { "excluded" }],
        )
        .await
        .unwrap();
    }
    conn.execute(
        "INSERT INTO debts(id,user_id,counterparty_id,direction,principal_cents,currency,occurred_on,note,version,created_at,updated_at,account_id,origin_kind,transaction_id) VALUES ('links-debt','links-user','links-counterparty','borrow_in',1200,'CNY','2026-05-01','',1,?1,?1,'links-account','cash_movement','links-principal')",
        [now],
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO debt_addition_events(id,user_id,debt_id,amount_cents,effective_on,note,created_at,account_id,transaction_id) VALUES ('links-addition-event','links-user','links-debt',200,'2026-05-02','',?1,'links-account','links-addition')",
        [now],
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO repayment_events(id,user_id,debt_id,kind,amount_cents,effective_on,note,created_at,account_id,transaction_id) VALUES ('links-repayment-event','links-user','links-debt','payment',300,'2026-05-03','',?1,'links-account','links-repayment')",
        [now],
    )
    .await
    .unwrap();

    let expected_links = legacy_links(&conn).await;
    let balances_before = balances(&conn).await;
    let statistics_before = statistics(&conn).await;
    assert_eq!(expected_links.len(), 3);
    drop(conn);

    db::migrate_up_to(&database, 26).await.unwrap();
    let conn = database.connect().unwrap();
    assert_eq!(generic_links(&conn).await, expected_links);
    assert_eq!(balances(&conn).await, balances_before);
    assert_eq!(statistics(&conn).await, statistics_before);

    let mut indexes = conn
        .query(
            "SELECT name FROM sqlite_master WHERE type='index' AND name IN ('idx_transaction_links_user_transaction','idx_transaction_links_user_plugin_ref') ORDER BY name",
            (),
        )
        .await
        .unwrap();
    let mut names = Vec::new();
    while let Some(row) = indexes.next().await.unwrap() {
        names.push(row.get::<String>(0).unwrap());
    }
    assert_eq!(names.len(), 2);
}
