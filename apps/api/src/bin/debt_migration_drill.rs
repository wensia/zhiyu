use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use libsql::{Builder, Connection};
use zhiyu_api::db;

type BalanceSnapshot = BTreeMap<(String, String), i64>;
type MonthlySnapshot = BTreeMap<(String, String, String), (i64, i64, i64)>;
type LinkSnapshot = BTreeSet<(String, String, String, String)>;

async fn balances(conn: &Connection) -> Result<BalanceSnapshot> {
    let mut rows = conn
        .query(
            "SELECT a.user_id, b.account_id, b.balance_cents FROM ledger_account_balances b JOIN ledger_accounts a ON a.id = b.account_id ORDER BY a.user_id, b.account_id",
            (),
        )
        .await?;
    let mut values = BTreeMap::new();
    while let Some(row) = rows.next().await? {
        values.insert((row.get(0)?, row.get(1)?), row.get(2)?);
    }
    Ok(values)
}

async fn monthly_statistics(conn: &Connection) -> Result<MonthlySnapshot> {
    let mut rows = conn
        .query(
            "SELECT t.user_id, substr(t.occurred_on, 1, 7), CASE WHEN c.id IS NOT NULL THEN c.name ELSE COALESCE(NULLIF(t.category, ''), '') END, SUM(CASE WHEN t.kind = 'income' THEN t.amount_cents ELSE 0 END), SUM(CASE WHEN t.kind = 'expense' THEN t.amount_cents ELSE 0 END), SUM(CASE WHEN t.kind IN ('income', 'expense') THEN 1 ELSE 0 END) FROM ledger_transactions t LEFT JOIN categories c ON c.id = t.category_id WHERE t.archived_at IS NULL AND NOT EXISTS (SELECT 1 FROM repayment_events r WHERE r.transaction_id = t.id) AND NOT EXISTS (SELECT 1 FROM debt_addition_events e WHERE e.transaction_id = t.id) AND NOT EXISTS (SELECT 1 FROM debts d WHERE d.transaction_id = t.id) GROUP BY t.user_id, substr(t.occurred_on, 1, 7), c.id, CASE WHEN c.id IS NULL THEN NULLIF(t.category, '') END ORDER BY t.user_id, substr(t.occurred_on, 1, 7), c.id",
            (),
        )
        .await?;
    let mut values = BTreeMap::new();
    while let Some(row) = rows.next().await? {
        values.insert(
            (row.get(0)?, row.get(1)?, row.get(2)?),
            (row.get(3)?, row.get(4)?, row.get(5)?),
        );
    }
    Ok(values)
}

async fn pnl_monthly_statistics(conn: &Connection) -> Result<MonthlySnapshot> {
    let mut rows = conn
        .query(
            "SELECT t.user_id, substr(t.occurred_on, 1, 7), CASE WHEN c.id IS NOT NULL THEN c.name ELSE COALESCE(NULLIF(t.category, ''), '') END, SUM(CASE WHEN t.kind = 'income' THEN t.amount_cents ELSE 0 END), SUM(CASE WHEN t.kind = 'expense' THEN t.amount_cents ELSE 0 END), SUM(CASE WHEN t.kind IN ('income', 'expense') THEN 1 ELSE 0 END) FROM ledger_transactions t LEFT JOIN categories c ON c.id = t.category_id WHERE t.archived_at IS NULL AND t.pnl_scope = 'counted' GROUP BY t.user_id, substr(t.occurred_on, 1, 7), c.id, CASE WHEN c.id IS NULL THEN NULLIF(t.category, '') END ORDER BY t.user_id, substr(t.occurred_on, 1, 7), c.id",
            (),
        )
        .await?;
    let mut values = BTreeMap::new();
    while let Some(row) = rows.next().await? {
        values.insert(
            (row.get(0)?, row.get(1)?, row.get(2)?),
            (row.get(3)?, row.get(4)?, row.get(5)?),
        );
    }
    Ok(values)
}

async fn scalar(conn: &Connection, sql: &str) -> Result<i64> {
    let mut rows = conn.query(sql, ()).await?;
    Ok(rows
        .next()
        .await?
        .context("count query returned no row")?
        .get(0)?)
}

async fn legacy_transaction_links(conn: &Connection) -> Result<LinkSnapshot> {
    let mut rows = conn
        .query(
            "SELECT d.transaction_id, 'principal', d.id, c.display_name FROM debts d JOIN counterparties c ON c.id=d.counterparty_id AND c.user_id=d.user_id WHERE d.transaction_id IS NOT NULL UNION ALL SELECT e.transaction_id, 'addition', e.debt_id, c.display_name FROM debt_addition_events e JOIN debts d ON d.id=e.debt_id AND d.user_id=e.user_id JOIN counterparties c ON c.id=d.counterparty_id AND c.user_id=d.user_id WHERE e.transaction_id IS NOT NULL UNION ALL SELECT e.transaction_id, 'repayment', e.debt_id, c.display_name FROM repayment_events e JOIN debts d ON d.id=e.debt_id AND d.user_id=e.user_id JOIN counterparties c ON c.id=d.counterparty_id AND c.user_id=d.user_id WHERE e.transaction_id IS NOT NULL",
            (),
        )
        .await?;
    let mut values = BTreeSet::new();
    while let Some(row) = rows.next().await? {
        values.insert((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?));
    }
    Ok(values)
}

async fn generic_transaction_links(conn: &Connection) -> Result<LinkSnapshot> {
    let mut rows = conn
        .query(
            "SELECT transaction_id, kind, ref_id, label FROM transaction_links WHERE plugin_id='debts' ORDER BY transaction_id, kind, ref_id",
            (),
        )
        .await?;
    let mut values = BTreeSet::new();
    while let Some(row) = rows.next().await? {
        values.insert((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?));
    }
    Ok(values)
}

#[tokio::main]
async fn main() -> Result<()> {
    let path = std::env::var_os("ZHIYU_DRILL_DB").context("ZHIYU_DRILL_DB is required")?;
    let from_version = std::env::var("ZHIYU_DRILL_FROM_VERSION")
        .unwrap_or_else(|_| "23".to_owned())
        .parse::<i64>()
        .context("ZHIYU_DRILL_FROM_VERSION must be an integer")?;
    if !matches!(from_version, 23..=28) {
        bail!("ZHIYU_DRILL_FROM_VERSION must be between 23 and 28");
    }
    let target_version = from_version + 1;
    let database = Builder::new_local(path).build().await?;
    db::migrate_up_to(&database, from_version).await?;
    let conn = database.connect()?;
    if scalar(
        &conn,
        &format!("SELECT COUNT(*) FROM schema_migrations WHERE version >= {target_version}"),
    )
    .await?
        != 0
    {
        bail!("drill database must not have migration {target_version} applied");
    }

    let principal_count = if from_version == 23 {
        scalar(
            &conn,
            "SELECT COUNT(*) FROM debts WHERE origin_kind='cash_movement' AND account_id IS NOT NULL AND transaction_id IS NULL",
        )
        .await?
    } else {
        0
    };
    let addition_count = if from_version == 23 {
        scalar(
            &conn,
            "SELECT COUNT(*) FROM debt_addition_events e JOIN debts d ON d.id=e.debt_id AND d.user_id=e.user_id WHERE d.origin_kind='cash_movement' AND e.account_id IS NOT NULL AND e.transaction_id IS NULL",
        )
        .await?
    } else {
        0
    };
    let repayment_count = if from_version == 23 {
        scalar(
            &conn,
            "SELECT COUNT(*) FROM repayment_events e JOIN debts d ON d.id=e.debt_id AND d.user_id=e.user_id WHERE d.origin_kind='cash_movement' AND e.account_id IS NOT NULL AND e.transaction_id IS NULL",
        )
        .await?
    } else {
        0
    };
    let linked_transaction_count = if from_version == 24 {
        scalar(
            &conn,
            "SELECT COUNT(*) FROM ledger_transactions t WHERE EXISTS (SELECT 1 FROM debts d WHERE d.transaction_id=t.id AND d.user_id=t.user_id) OR EXISTS (SELECT 1 FROM debt_addition_events e WHERE e.transaction_id=t.id AND e.user_id=t.user_id) OR EXISTS (SELECT 1 FROM repayment_events r WHERE r.transaction_id=t.id AND r.user_id=t.user_id)",
        )
        .await?
    } else {
        0
    };
    let links_before = if from_version == 25 {
        legacy_transaction_links(&conn).await?
    } else {
        LinkSnapshot::new()
    };
    let invalid_negative_principal_count = if from_version == 26 {
        scalar(
            &conn,
            "SELECT COUNT(*) FROM debts d WHERE d.origin_kind='cash_movement' AND d.account_id IS NOT NULL AND d.transaction_id IS NULL AND d.principal_cents - COALESCE((SELECT SUM(a.amount_cents) FROM debt_addition_events a WHERE a.debt_id=d.id),0) < 0",
        )
        .await?
    } else {
        0
    };
    let import_transaction_count = if from_version == 28 {
        scalar(
            &conn,
            "SELECT COUNT(*) FROM import_records WHERE transaction_id IS NOT NULL",
        )
        .await?
    } else {
        0
    };
    let before_balances = balances(&conn).await?;
    let before_statistics = if from_version >= 25 {
        pnl_monthly_statistics(&conn).await?
    } else {
        monthly_statistics(&conn).await?
    };
    let account_count = before_balances.len();
    let month_count = before_statistics
        .keys()
        .map(|(user_id, month, _)| (user_id, month))
        .collect::<BTreeSet<_>>()
        .len();
    drop(conn);

    db::migrate_up_to(&database, target_version).await?;
    let conn = database.connect()?;
    let balances_match = before_balances == balances(&conn).await?;
    let after_statistics = if target_version >= 25 {
        pnl_monthly_statistics(&conn).await?
    } else {
        monthly_statistics(&conn).await?
    };
    let statistics_match = before_statistics == after_statistics;
    let import_link_count = if target_version == 29 {
        scalar(
            &conn,
            "SELECT COUNT(*) FROM transaction_links WHERE plugin_id='bill-imports' AND kind='batch'",
        )
        .await?
    } else {
        0
    };
    let links_match = match target_version {
        26 => links_before == generic_transaction_links(&conn).await?,
        29 => import_transaction_count == import_link_count,
        _ => true,
    };

    if from_version == 23 {
        println!("迁移条数：债务本金 {principal_count}");
        println!("迁移条数：追加借款 {addition_count}");
        println!("迁移条数：还款及撤销 {repayment_count}");
    } else if from_version == 24 {
        println!("回填为不计入收支的流水数：{linked_transaction_count}");
    } else if from_version == 25 {
        println!("债务关联数：{}", links_before.len());
        println!(
            "债务关联逐项一致：{}",
            if links_match { "是" } else { "否" }
        );
    } else if from_version == 26 {
        let user_count = scalar(
            &conn,
            "SELECT COUNT(*) FROM ledger_transactions WHERE created_by='user'",
        )
        .await?;
        let debts_count = scalar(
            &conn,
            "SELECT COUNT(*) FROM ledger_transactions WHERE created_by='plugin:debts'",
        )
        .await?;
        let imports_count = scalar(
            &conn,
            "SELECT COUNT(*) FROM ledger_transactions WHERE created_by='plugin:bill-imports'",
        )
        .await?;
        println!("created_by user：{user_count}");
        println!("created_by plugin:debts：{debts_count}");
        println!("created_by plugin:bill-imports：{imports_count}");
        println!("追加之和超过本金的异常记录数：{invalid_negative_principal_count}");
    } else if from_version == 28 {
        println!("有 transaction_id 的导入流水数：{import_transaction_count}");
        println!("账单导入关联数：{import_link_count}");
        println!(
            "账单导入关联条数一致：{}",
            if links_match { "是" } else { "否" }
        );
    }
    println!("账户数：{account_count}");
    println!("月份数：{month_count}");
    println!("余额逐项一致：{}", if balances_match { "是" } else { "否" });
    println!(
        "统计逐项一致：{}",
        if statistics_match { "是" } else { "否" }
    );
    println!(
        "一致：{}",
        if balances_match && statistics_match && links_match {
            "是"
        } else {
            "否"
        }
    );
    if !balances_match || !statistics_match || !links_match {
        bail!("reconciliation failed");
    }
    Ok(())
}
