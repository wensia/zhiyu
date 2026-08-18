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
async fn import_transaction_links_migration_backfills_batches_without_changing_ledger_results() {
    let root = TempDir::new().unwrap();
    let database = Builder::new_local(root.path().join("import-transaction-links.db"))
        .build()
        .await
        .unwrap();
    db::migrate_up_to(&database, 28).await.unwrap();
    let conn = database.connect().unwrap();
    let now = "2026-08-18T00:00:00Z";

    conn.execute(
        "INSERT INTO users(id,email,password_hash,timezone,email_verified_at,created_at,updated_at) VALUES ('import-links-user','import-links@example.invalid','hash','Asia/Shanghai',?1,?1,?1)",
        [now],
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO ledger_accounts(id,user_id,name,normalized_name,account_type,note,opening_balance_cents,version,created_at,updated_at) VALUES ('import-links-account','import-links-user','测试账户','测试账户','cash','',0,1,?1,?1)",
        [now],
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO import_batches(id,user_id,source_channel,file_sha256,period_start,period_end,total_count,status,committed_at,created_at,updated_at) VALUES ('import-links-batch','import-links-user','alipay',?1,'2026-07-01','2026-07-31',1,'committed',?2,?2,?2)",
        libsql::params!["a".repeat(64), now],
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,occurred_on,account_id,created_at,updated_at,source_channel,external_id,import_batch_id,created_by) VALUES ('import-links-transaction','import-links-user','expense',500,'2026-07-18','import-links-account',?1,?1,'alipay','external-test','import-links-batch','plugin:bill-imports')",
        [now],
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO import_records(id,batch_id,row_index,external_id,occurred_at,occurred_on,direction,amount_cents,disposition,transaction_id,created_at) VALUES ('import-links-record','import-links-batch',1,'external-test','2026-07-18 12:00:00','2026-07-18','expense',500,'import','import-links-transaction',?1)",
        [now],
    )
    .await
    .unwrap();

    let balance_before = scalar(
        &conn,
        "SELECT balance_cents FROM ledger_account_balances WHERE account_id='import-links-account'",
    )
    .await;
    let expense_before = scalar(
        &conn,
        "SELECT SUM(amount_cents) FROM ledger_transactions WHERE user_id='import-links-user' AND kind='expense' AND archived_at IS NULL AND pnl_scope='counted'",
    )
    .await;
    drop(conn);

    db::migrate_up_to(&database, 29).await.unwrap();
    let conn = database.connect().unwrap();
    let mut rows = conn
        .query(
            "SELECT transaction_id,plugin_id,kind,ref_id,label FROM transaction_links WHERE plugin_id='bill-imports'",
            (),
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    assert_eq!(row.get::<String>(0).unwrap(), "import-links-transaction");
    assert_eq!(row.get::<String>(1).unwrap(), "bill-imports");
    assert_eq!(row.get::<String>(2).unwrap(), "batch");
    assert_eq!(row.get::<String>(3).unwrap(), "import-links-batch");
    assert_eq!(
        row.get::<String>(4).unwrap(),
        "支付宝 · 2026-07-01 至 2026-07-31"
    );
    assert!(rows.next().await.unwrap().is_none());
    drop(rows);

    assert_eq!(
        scalar(
            &conn,
            "SELECT balance_cents FROM ledger_account_balances WHERE account_id='import-links-account'",
        )
        .await,
        balance_before
    );
    assert_eq!(
        scalar(
            &conn,
            "SELECT SUM(amount_cents) FROM ledger_transactions WHERE user_id='import-links-user' AND kind='expense' AND archived_at IS NULL AND pnl_scope='counted'",
        )
        .await,
        expense_before
    );
}
