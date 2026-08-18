use libsql::{Builder, Connection};
use tempfile::TempDir;
use zhiyu_api::db;

async fn scalar(conn: &Connection, sql: &str) -> i64 {
    conn.query(sql, ())
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap()
}

#[tokio::test]
async fn dashboard_migration_preserves_balance_and_statistics() {
    let root = TempDir::new().unwrap();
    let database = Builder::new_local(root.path().join("dashboards.db"))
        .build()
        .await
        .unwrap();
    db::migrate_up_to(&database, 30).await.unwrap();
    let conn = database.connect().unwrap();
    let now = "2026-08-18T00:00:00Z";

    conn.execute(
        "INSERT INTO users(id,email,password_hash,timezone,email_verified_at,created_at,updated_at) VALUES ('dashboard-migration-user','dashboard-migration@example.invalid','hash','Asia/Shanghai',?1,?1,?1)",
        [now],
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO ledger_accounts(id,user_id,name,normalized_name,account_type,note,opening_balance_cents,version,created_at,updated_at) VALUES ('dashboard-migration-account','dashboard-migration-user','迁移测试账户','迁移测试账户','cash','',250,1,?1,?1)",
        [now],
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,occurred_on,account_id,pnl_scope,created_by,created_at,updated_at) VALUES ('dashboard-migration-income','dashboard-migration-user','income',1200,'2026-08-18','dashboard-migration-account','counted','user',?1,?1)",
        [now],
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,occurred_on,account_id,pnl_scope,created_by,created_at,updated_at) VALUES ('dashboard-migration-excluded','dashboard-migration-user','expense',500,'2026-08-18','dashboard-migration-account','excluded','plugin:debts',?1,?1)",
        [now],
    )
    .await
    .unwrap();

    let balance_before = scalar(
        &conn,
        "SELECT balance_cents FROM ledger_account_balances WHERE account_id='dashboard-migration-account'",
    )
    .await;
    let income_before = scalar(
        &conn,
        "SELECT COALESCE(SUM(amount_cents),0) FROM ledger_transactions WHERE user_id='dashboard-migration-user' AND kind='income' AND archived_at IS NULL AND pnl_scope='counted'",
    )
    .await;
    let expense_before = scalar(
        &conn,
        "SELECT COALESCE(SUM(amount_cents),0) FROM ledger_transactions WHERE user_id='dashboard-migration-user' AND kind='expense' AND archived_at IS NULL AND pnl_scope='counted'",
    )
    .await;
    drop(conn);

    db::migrate_up_to(&database, 31).await.unwrap();
    let conn = database.connect().unwrap();
    assert_eq!(
        scalar(
            &conn,
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('dashboards','dashboard_widgets')",
        )
        .await,
        2
    );
    assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM dashboards").await, 0);
    assert_eq!(
        scalar(&conn, "SELECT COUNT(*) FROM dashboard_widgets").await,
        0
    );
    assert_eq!(
        scalar(
            &conn,
            "SELECT balance_cents FROM ledger_account_balances WHERE account_id='dashboard-migration-account'",
        )
        .await,
        balance_before
    );
    assert_eq!(
        scalar(
            &conn,
            "SELECT COALESCE(SUM(amount_cents),0) FROM ledger_transactions WHERE user_id='dashboard-migration-user' AND kind='income' AND archived_at IS NULL AND pnl_scope='counted'",
        )
        .await,
        income_before
    );
    assert_eq!(
        scalar(
            &conn,
            "SELECT COALESCE(SUM(amount_cents),0) FROM ledger_transactions WHERE user_id='dashboard-migration-user' AND kind='expense' AND archived_at IS NULL AND pnl_scope='counted'",
        )
        .await,
        expense_before
    );
    assert!(
        conn.query("PRAGMA foreign_key_check", ())
            .await
            .unwrap()
            .next()
            .await
            .unwrap()
            .is_none()
    );
}
