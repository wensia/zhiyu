use anyhow::{Context, Result, bail};
use libsql::{Connection, TransactionBehavior, params};
use zhiyu_api::{
    config::Config, db, domain::normalize_counterparty, imports::model::NORMALIZATION_VERSION,
};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RenormalizeStats {
    pub eligible_records: u64,
    pub changed_records: u64,
    pub changed_linked_transactions: u64,
    pub scanned_manual_transactions: u64,
    pub changed_manual_transactions: u64,
}

impl RenormalizeStats {
    pub fn changed(&self) -> u64 {
        self.changed_records + self.changed_linked_transactions + self.changed_manual_transactions
    }
}

pub async fn renormalize_user(connection: &Connection, user_id: &str) -> Result<RenormalizeStats> {
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;

    let mut user_rows = tx
        .query("SELECT 1 FROM users WHERE id=?1", [user_id])
        .await?;
    if user_rows.next().await?.is_none() {
        bail!("找不到指定用户");
    }
    drop(user_rows);

    let mut record_rows = tx
        .query(
            "SELECT r.id,b.source_channel,r.counterparty,r.transaction_id \
             FROM import_records r \
             JOIN import_batches b ON b.id=r.batch_id \
             WHERE b.user_id=?1 AND r.normalization_version<?2 \
             ORDER BY r.id",
            params![user_id, NORMALIZATION_VERSION],
        )
        .await?;
    let mut records = Vec::new();
    while let Some(row) = record_rows.next().await? {
        records.push((
            row.get::<String>(0)?,
            row.get::<String>(1)?,
            row.get::<String>(2)?,
            row.get::<Option<String>>(3)?,
        ));
    }
    drop(record_rows);

    let mut stats = RenormalizeStats {
        eligible_records: records.len() as u64,
        ..RenormalizeStats::default()
    };
    for (record_id, source_channel, counterparty, transaction_id) in records {
        let normalized = normalize_counterparty(&source_channel, &counterparty);
        stats.changed_records += tx
            .execute(
                "UPDATE import_records \
                 SET counterparty_normalized=?1,normalization_version=?2 \
                 WHERE id=?3 AND normalization_version<?2",
                params![normalized.clone(), NORMALIZATION_VERSION, record_id],
            )
            .await?;
        if let Some(transaction_id) = transaction_id {
            stats.changed_linked_transactions += tx
                .execute(
                    "UPDATE ledger_transactions \
                     SET payee_key=?1 \
                     WHERE id=?2 AND user_id=?3 AND payee_key<>?1",
                    params![normalized, transaction_id, user_id],
                )
                .await?;
        }
    }

    let mut manual_rows = tx
        .query(
            "SELECT t.id,t.payee_name \
             FROM ledger_transactions t \
             WHERE t.user_id=?1 \
               AND NOT EXISTS (SELECT 1 FROM import_records r WHERE r.transaction_id=t.id) \
             ORDER BY t.id",
            [user_id],
        )
        .await?;
    let mut manual_transactions = Vec::new();
    while let Some(row) = manual_rows.next().await? {
        manual_transactions.push((row.get::<String>(0)?, row.get::<String>(1)?));
    }
    drop(manual_rows);

    stats.scanned_manual_transactions = manual_transactions.len() as u64;
    for (transaction_id, payee_name) in manual_transactions {
        let normalized = normalize_counterparty("manual", &payee_name);
        stats.changed_manual_transactions += tx
            .execute(
                "UPDATE ledger_transactions \
                 SET payee_key=?1 \
                 WHERE id=?2 AND user_id=?3 AND payee_key<>?1",
                params![normalized, transaction_id, user_id],
            )
            .await?;
    }

    tx.commit().await?;
    Ok(stats)
}

#[tokio::main]
#[allow(dead_code)]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let mut args = std::env::args().skip(1);
    let user_id = args.next().context("用法：zhiyu-renormalize <用户 ID>")?;
    if args.next().is_some() {
        bail!("用法：zhiyu-renormalize <用户 ID>");
    }

    let config = Config::from_env()?;
    let database = db::connect(&config).await?;
    let connection = database.connect()?;
    let stats = renormalize_user(&connection, &user_id).await?;
    println!(
        "normalization_version={} eligible_records={} changed_records={} changed_linked_transactions={} scanned_manual_transactions={} changed_manual_transactions={} changed={}",
        NORMALIZATION_VERSION,
        stats.eligible_records,
        stats.changed_records,
        stats.changed_linked_transactions,
        stats.scanned_manual_transactions,
        stats.changed_manual_transactions,
        stats.changed(),
    );
    Ok(())
}
