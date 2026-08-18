use std::collections::HashSet;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use libsql::{Connection, TransactionBehavior, params};
use zhiyu_api::{config::Config, db, imports::is_self_transfer};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReselfStats {
    pub eligible: u64,
    pub matched: u64,
    pub changed: u64,
    pub skipped_total: u64,
    pub skipped_confirmed: u64,
    pub skipped_archived: u64,
}

struct Candidate {
    id: String,
    source_channel: String,
    kind: String,
    account_id: Option<String>,
    counterparty_normalized: String,
    event_id: Option<String>,
    archived_at: Option<String>,
}

pub async fn reself_user(connection: &Connection, user_id: &str) -> Result<ReselfStats> {
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

    let mut alias_rows = tx
        .query(
            "SELECT normalized_alias FROM self_transfer_aliases WHERE user_id=?1",
            [user_id],
        )
        .await?;
    let mut aliases = HashSet::new();
    while let Some(row) = alias_rows.next().await? {
        aliases.insert(row.get::<String>(0)?);
    }
    drop(alias_rows);

    let mut candidate_rows = tx
        .query(
            "SELECT t.id,b.source_channel,t.kind,t.account_id,r.counterparty_normalized,t.event_id,t.archived_at \
             FROM ledger_transactions t \
             JOIN import_records r ON r.transaction_id=t.id \
             JOIN import_batches b ON b.id=r.batch_id \
             WHERE t.user_id=?1 \
               AND b.user_id=?1 \
               AND b.source_channel IN ('cmb','cmbc') \
               AND t.kind IN ('income','expense') \
             ORDER BY t.id,r.id",
            [user_id],
        )
        .await?;
    let mut candidates = Vec::new();
    while let Some(row) = candidate_rows.next().await? {
        candidates.push(Candidate {
            id: row.get(0)?,
            source_channel: row.get(1)?,
            kind: row.get(2)?,
            account_id: row.get(3)?,
            counterparty_normalized: row.get(4)?,
            event_id: row.get(5)?,
            archived_at: row.get(6)?,
        });
    }
    drop(candidate_rows);

    let mut stats = ReselfStats {
        eligible: candidates.len() as u64,
        ..ReselfStats::default()
    };
    let updated_at = Utc::now().to_rfc3339();
    for candidate in candidates {
        if !is_self_transfer(
            &candidate.source_channel,
            &candidate.kind,
            &candidate.counterparty_normalized,
            &aliases,
        ) {
            continue;
        }
        stats.matched += 1;

        let is_confirmed = candidate.event_id.is_some();
        let is_archived = candidate.archived_at.is_some();
        if is_confirmed {
            stats.skipped_confirmed += 1;
        }
        if is_archived {
            stats.skipped_archived += 1;
        }
        if is_confirmed || is_archived {
            stats.skipped_total += 1;
            continue;
        }

        let account_id = candidate
            .account_id
            .context("命中的历史自转交易缺少账单账户")?;
        let (transfer_from_account_id, transfer_to_account_id) = match candidate.kind.as_str() {
            "expense" => (Some(account_id), None),
            "income" => (None, Some(account_id)),
            _ => unreachable!("candidate query only accepts income or expense"),
        };
        stats.changed += tx
            .execute(
                "UPDATE ledger_transactions \
                 SET kind='transfer',account_id=NULL,transfer_from_account_id=?1,transfer_to_account_id=?2,category_id=NULL,category_source='none',category_rule_id=NULL,version=version+1,updated_at=?3 \
                 WHERE id=?4 AND user_id=?5 AND kind=?6 AND event_id IS NULL AND archived_at IS NULL",
                params![
                    transfer_from_account_id,
                    transfer_to_account_id,
                    updated_at.clone(),
                    candidate.id,
                    user_id,
                    candidate.kind,
                ],
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
    let user_id = args.next().context("用法：zhiyu-reself <用户 ID>")?;
    if args.next().is_some() {
        bail!("用法：zhiyu-reself <用户 ID>");
    }

    let config = Config::from_env()?;
    let database = db::connect(&config).await?;
    let connection = database.connect()?;
    let stats = reself_user(&connection, &user_id).await?;
    println!(
        "eligible={} matched={} changed={} skipped_total={} skipped_confirmed={} skipped_archived={}",
        stats.eligible,
        stats.matched,
        stats.changed,
        stats.skipped_total,
        stats.skipped_confirmed,
        stats.skipped_archived,
    );
    Ok(())
}
