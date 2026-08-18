use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{NaiveDate, NaiveDateTime, Utc};
use libsql::{Connection, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{
    AppState,
    auth::AuthUser,
    error::ApiError,
    idempotency::{idempotency_key, replay_idempotency, request_hash, store_idempotency},
    transactions::{
        NewTransactionRow, OnExternalConflict, TransactionPatch, hard_delete_transaction_row,
        insert_transaction_row, update_transaction_row,
    },
};

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateSuspicionListQuery {
    pub page: Option<u32>,
    pub page_size: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateSuspicionListResponse {
    pub items: Vec<DuplicateSuspicionView>,
    pub clusters: Vec<DuplicateSuspicionClusterView>,
    pub total: i64,
    pub page: u32,
    pub page_size: u32,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateSuspicionClusterView {
    pub cluster_key: String,
    pub items: Vec<DuplicateSuspicionView>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateSuspicionView {
    pub id: String,
    pub transaction_a: DuplicateTransactionView,
    pub transaction_b: DuplicateTransactionView,
    pub score: f64,
    pub match_rule: String,
    pub cluster_key: String,
    pub reason: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateTransactionView {
    pub id: String,
    pub kind: String,
    pub amount_cents: i64,
    pub currency: String,
    pub occurred_on: String,
    pub occurred_at: Option<String>,
    pub occurred_at_precision: String,
    pub source_channel: String,
    pub account_id: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateSuspicionActionResponse {
    pub suspicion_id: String,
    pub status: String,
    pub event: Option<TransactionEventView>,
    pub transactions: Vec<DuplicateActionTransactionView>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TransactionEventView {
    pub id: String,
    pub kind: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateActionTransactionView {
    pub id: String,
    pub kind: String,
    pub amount_cents: i64,
    pub account_id: Option<String>,
    pub transfer_from_account_id: Option<String>,
    pub transfer_to_account_id: Option<String>,
    pub payee_name: String,
    pub category_source: String,
    pub archived_at: Option<String>,
    pub event_id: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateDuplicateSuspicionRequest {
    pub status: String,
}

#[derive(Clone, Debug)]
struct ActionTransaction {
    id: String,
    kind: String,
    amount_cents: i64,
    currency: String,
    occurred_on: String,
    occurred_at: Option<String>,
    occurred_at_precision: String,
    source_channel: String,
    account_id: Option<String>,
    transfer_from_account_id: Option<String>,
    transfer_to_account_id: Option<String>,
    payee_name: String,
    category_source: String,
    archived_at: Option<String>,
    event_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RevertPayload {
    changed: TransactionSnapshot,
    archived: ArchivedSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    created: Option<CreatedSnapshot>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransactionSnapshot {
    id: String,
    amount_cents: i64,
    account_id: Option<String>,
    #[serde(default)]
    transfer_to_account_id: Option<String>,
    event_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArchivedSnapshot {
    id: String,
    archived_at: Option<String>,
    event_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreatedSnapshot {
    id: String,
}

#[derive(Clone, Debug)]
struct MatchTransaction {
    id: String,
    kind: String,
    amount_cents: i64,
    occurred_on: String,
    occurred_at: Option<String>,
    occurred_at_precision: String,
    source_channel: String,
    account_id: String,
    direction: String,
    channel_status: String,
    source_text: String,
    counterparty_normalized: String,
}

#[derive(Clone, Debug)]
struct MatchCandidate {
    a: MatchTransaction,
    b: MatchTransaction,
    score: f64,
    match_rule: &'static str,
    reason: String,
    cluster_key: String,
    time_difference: i64,
    amount_difference: i64,
}

pub(crate) async fn match_committed_batch(
    tx: &Transaction,
    user_id: &str,
    batch_id: &str,
) -> Result<(), ApiError> {
    let mut rows = tx.query(
        "SELECT n.id,n.kind,n.amount_cents,n.occurred_on,n.occurred_at,n.occurred_at_precision,n.source_channel,COALESCE(n.account_id,n.transfer_from_account_id,n.transfer_to_account_id),COALESCE(nr.direction,n.kind),COALESCE(nr.channel_status,''),COALESCE(nr.source_note,'')||' '||COALESCE(nr.channel_category,'')||' '||COALESCE(nr.product,'')||' '||COALESCE(nr.pay_method,''),COALESCE(nr.counterparty_normalized,n.payee_key,n.payee_name,''),o.id,o.kind,o.amount_cents,o.occurred_on,o.occurred_at,o.occurred_at_precision,o.source_channel,COALESCE(o.account_id,o.transfer_from_account_id,o.transfer_to_account_id),COALESCE(orow.direction,o.kind),COALESCE(orow.channel_status,''),COALESCE(orow.source_note,'')||' '||COALESCE(orow.channel_category,'')||' '||COALESCE(orow.product,'')||' '||COALESCE(orow.pay_method,''),COALESCE(orow.counterparty_normalized,o.payee_key,o.payee_name,'') FROM ledger_transactions n JOIN ledger_transactions o ON o.id<>n.id AND o.user_id=n.user_id AND o.source_channel<>n.source_channel AND o.currency=n.currency AND o.id IN (SELECT c.id FROM ledger_transactions c WHERE c.user_id=n.user_id AND c.account_id IN (n.account_id,n.transfer_from_account_id,n.transfer_to_account_id) UNION ALL SELECT c.id FROM ledger_transactions c WHERE c.user_id=n.user_id AND c.transfer_from_account_id IN (n.account_id,n.transfer_from_account_id,n.transfer_to_account_id) UNION ALL SELECT c.id FROM ledger_transactions c WHERE c.user_id=n.user_id AND c.transfer_to_account_id IN (n.account_id,n.transfer_from_account_id,n.transfer_to_account_id)) LEFT JOIN import_records nr ON nr.transaction_id=n.id LEFT JOIN import_records orow ON orow.transaction_id=o.id WHERE n.user_id=?1 AND n.import_batch_id=?2 AND n.archived_at IS NULL AND o.archived_at IS NULL AND (o.import_batch_id IS NULL OR o.import_batch_id<>?2) AND n.kind IN ('income','expense','transfer') AND o.kind IN ('income','expense','transfer') AND ((n.source_channel IN ('alipay','wechat') AND o.source_channel IN ('cmb','cmbc')) OR (o.source_channel IN ('alipay','wechat') AND n.source_channel IN ('cmb','cmbc'))) AND abs(julianday(n.occurred_on)-julianday(o.occurred_on))<=1 AND NOT EXISTS (SELECT 1 FROM duplicate_suspicions d WHERE d.user_id=n.user_id AND (d.transaction_id_a=n.id OR d.transaction_id_b=n.id OR d.transaction_id_a=o.id OR d.transaction_id_b=o.id))",
        params![user_id, batch_id],
    ).await?;
    let mut candidates = Vec::new();
    while let Some(row) = rows.next().await? {
        let a = MatchTransaction {
            id: row.get(0)?,
            kind: row.get(1)?,
            amount_cents: row.get(2)?,
            occurred_on: row.get(3)?,
            occurred_at: row.get(4)?,
            occurred_at_precision: row.get(5)?,
            source_channel: row.get(6)?,
            account_id: row.get(7)?,
            direction: row.get(8)?,
            channel_status: row.get(9)?,
            source_text: row.get(10)?,
            counterparty_normalized: row.get(11)?,
        };
        let b = MatchTransaction {
            id: row.get(12)?,
            kind: row.get(13)?,
            amount_cents: row.get(14)?,
            occurred_on: row.get(15)?,
            occurred_at: row.get(16)?,
            occurred_at_precision: row.get(17)?,
            source_channel: row.get(18)?,
            account_id: row.get(19)?,
            direction: row.get(20)?,
            channel_status: row.get(21)?,
            source_text: row.get(22)?,
            counterparty_normalized: row.get(23)?,
        };
        if let Some(candidate) = classify_candidate(a, b) {
            candidates.push(candidate);
        }
    }
    drop(rows);

    promote_ambiguous_clusters(&mut candidates);
    candidates.sort_by(|left, right| {
        match_rule_priority(left.match_rule)
            .cmp(&match_rule_priority(right.match_rule))
            .then_with(|| {
                right
                    .score
                    .total_cmp(&left.score)
                    .then(left.time_difference.cmp(&right.time_difference))
                    .then(left.amount_difference.cmp(&right.amount_difference))
                    .then(left.a.id.cmp(&right.a.id))
                    .then(left.b.id.cmp(&right.b.id))
            })
    });
    let mut used = HashSet::new();
    let now = Utc::now().to_rfc3339();
    for candidate in candidates {
        if candidate.match_rule != "ambiguous"
            && (used.contains(&candidate.a.id) || used.contains(&candidate.b.id))
        {
            continue;
        }
        let (transaction_id_a, transaction_id_b) = ordered_pair(&candidate.a.id, &candidate.b.id);
        tx.execute(
            "INSERT INTO duplicate_suspicions(id,user_id,transaction_id_a,transaction_id_b,score,match_rule,reason,cluster_key,status,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'open',?9,?9) ON CONFLICT(user_id,transaction_id_a,transaction_id_b) DO NOTHING",
            params![Uuid::now_v7().to_string(), user_id, transaction_id_a, transaction_id_b, candidate.score, candidate.match_rule, candidate.reason, candidate.cluster_key, now.clone()],
        ).await?;
        if candidate.match_rule != "ambiguous" {
            used.insert(candidate.a.id);
            used.insert(candidate.b.id);
        }
    }
    Ok(())
}

fn classify_candidate(a: MatchTransaction, b: MatchTransaction) -> Option<MatchCandidate> {
    let time_difference = time_difference(&a, &b)?;
    let amount_difference = (a.amount_cents - b.amount_cents).abs();
    let (platform, bank) = platform_and_bank(&a, &b)?;

    let platform_mentions_transfer = contains_any(
        &format!("{} {}", platform.channel_status, platform.source_text),
        &["提现", "充值"],
    );
    let bank_pointer = match bank.source_channel.as_str() {
        "cmb" => contains_any(&platform.counterparty_normalized, &["招商银行"]),
        "cmbc" => contains_any(&platform.counterparty_normalized, &["民生银行"]),
        _ => false,
    };
    if (platform.direction == "neutral" || platform_mentions_transfer)
        && bank_pointer
        && platform.amount_cents.checked_mul(1000) == bank.amount_cents.checked_mul(1001)
    {
        return Some(MatchCandidate {
            a,
            b,
            score: 0.99,
            match_rule: "withdraw_fee",
            reason: "平台提现或充值金额符合银行到账金额×1.001".to_owned(),
            cluster_key: String::new(),
            time_difference,
            amount_difference,
        });
    }

    // 金额守卫不可省：退款是原路退回，两侧必然同额。少了它，同账户同日的任意两笔
    // （例如平台一笔 20 元退款与银行一笔 3000 元扣款）都会拿到 0.98 分，而 refund
    // 的规则优先级高于 same_amount，还会通过 used 集合把本该命中的正确配对挤掉。
    // 提现手续费那种两侧不同额的情形由上面的 withdraw_fee 规则负责。
    // transfer 也要挡掉：转账是账户间搬钱，跟「退款」不是同一回事。放行的话一笔
    // 标了退款的支出会和同额的自转账配成 0.98，再靠优先级挤掉真正的配对。
    // 这里只排除 transfer 而不强求两侧 kind 相同——真实退款常是平台侧 expense
    // 对银行侧 income（退款到账），要求全等会杀掉真匹配。
    if amount_difference == 0
        && a.kind != "transfer"
        && b.kind != "transfer"
        && (contains_any(&a.channel_status, &["退款", "撤销", "冲正"])
            || contains_any(&b.channel_status, &["退款", "撤销", "冲正"]))
    {
        return Some(MatchCandidate {
            a,
            b,
            score: 0.98,
            match_rule: "refund",
            reason: "渠道状态包含退款、撤销或冲正语义，且两侧金额一致".to_owned(),
            cluster_key: String::new(),
            time_difference,
            amount_difference,
        });
    }

    if amount_difference != 0 || a.kind != b.kind {
        return None;
    }
    let both_second = a.occurred_at_precision == "second" && b.occurred_at_precision == "second";
    let score = if both_second {
        0.95 + 0.05 * (1.0 - time_difference as f64 / 300.0)
    } else {
        0.65 - 0.05 * (time_difference / 86_400) as f64
    };
    Some(MatchCandidate {
        a,
        b,
        score,
        match_rule: "same_amount",
        reason: if both_second {
            format!("时间差{time_difference}秒，金额相同")
        } else {
            format!("日期差{}天，金额相同", time_difference / 86_400)
        },
        cluster_key: String::new(),
        time_difference,
        amount_difference,
    })
}

fn time_difference(a: &MatchTransaction, b: &MatchTransaction) -> Option<i64> {
    if a.occurred_at_precision == "second" && b.occurred_at_precision == "second" {
        let difference = (parse_second(a.occurred_at.as_deref()?)?
            - parse_second(b.occurred_at.as_deref()?)?)
        .num_seconds()
        .abs();
        (difference <= 300).then_some(difference)
    } else {
        let a_date = NaiveDate::parse_from_str(&a.occurred_on, "%Y-%m-%d").ok()?;
        let b_date = NaiveDate::parse_from_str(&b.occurred_on, "%Y-%m-%d").ok()?;
        let difference = (a_date - b_date).num_days().abs();
        (difference <= 1).then_some(difference * 86_400)
    }
}

fn platform_and_bank<'a>(
    a: &'a MatchTransaction,
    b: &'a MatchTransaction,
) -> Option<(&'a MatchTransaction, &'a MatchTransaction)> {
    if is_payment_platform(&a.source_channel) {
        Some((a, b))
    } else if is_payment_platform(&b.source_channel) {
        Some((b, a))
    } else {
        None
    }
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

fn promote_ambiguous_clusters(candidates: &mut Vec<MatchCandidate>) {
    let mut groups: HashMap<String, (HashSet<String>, HashSet<String>)> = HashMap::new();
    for candidate in candidates.iter().filter(|candidate| {
        candidate.match_rule == "same_amount"
            && candidate.a.occurred_on == candidate.b.occurred_on
            && (candidate.a.occurred_at_precision != "second"
                || candidate.b.occurred_at_precision != "second")
    }) {
        let (platform, bank) = platform_and_bank(&candidate.a, &candidate.b)
            .expect("cross-channel query always has one payment platform side");
        let raw_key = format!(
            "{}|{}|{}",
            bank.account_id, bank.occurred_on, bank.amount_cents
        );
        let group = groups.entry(raw_key).or_default();
        group.0.insert(platform.id.clone());
        group.1.insert(bank.id.clone());
    }
    let ambiguous: HashMap<String, String> = groups
        .into_iter()
        .filter(|(_, (platform, bank))| platform.len() > 1 || bank.len() > 1)
        .map(|(raw_key, _)| {
            let cluster_key = format!("{:x}", Sha256::digest(raw_key.as_bytes()));
            (raw_key, cluster_key)
        })
        .collect();
    for candidate in candidates.iter_mut() {
        if candidate.match_rule != "same_amount"
            || candidate.a.occurred_on != candidate.b.occurred_on
        {
            continue;
        }
        let (_, bank) = platform_and_bank(&candidate.a, &candidate.b)
            .expect("cross-channel query always has one payment platform side");
        let raw_key = format!(
            "{}|{}|{}",
            bank.account_id, bank.occurred_on, bank.amount_cents
        );
        if let Some(cluster_key) = ambiguous.get(&raw_key) {
            candidate.match_rule = "ambiguous";
            candidate.reason = "同一账户、同日、同额存在多条候选，需整簇人工消歧".to_owned();
            candidate.cluster_key = cluster_key.clone();
            candidate.score = 0.5;
        }
    }
    let ambiguous_members: HashSet<String> = candidates
        .iter()
        .filter(|candidate| candidate.match_rule == "ambiguous")
        .flat_map(|candidate| [candidate.a.id.clone(), candidate.b.id.clone()])
        .collect();
    candidates.retain(|candidate| {
        candidate.match_rule != "same_amount"
            || (!ambiguous_members.contains(&candidate.a.id)
                && !ambiguous_members.contains(&candidate.b.id))
    });
}

fn match_rule_priority(rule: &str) -> u8 {
    match rule {
        "withdraw_fee" => 0,
        "refund" => 1,
        "same_amount" => 2,
        "ambiguous" => 3,
        _ => 4,
    }
}

fn parse_second(value: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S").ok()
}

fn is_payment_platform(channel: &str) -> bool {
    matches!(channel, "alipay" | "wechat")
}

fn is_bank(channel: &str) -> bool {
    matches!(channel, "cmb" | "cmbc")
}

fn ordered_pair<'a>(a: &'a str, b: &'a str) -> (&'a str, &'a str) {
    if a < b { (a, b) } else { (b, a) }
}

#[utoipa::path(get, path = "/api/v1/duplicate-suspicions", params(DuplicateSuspicionListQuery), responses((status = 200, body = DuplicateSuspicionListResponse)), security(("cookieAuth" = [])))]
pub async fn list_duplicate_suspicions(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<DuplicateSuspicionListQuery>,
) -> Result<Json<DuplicateSuspicionListResponse>, ApiError> {
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(50).clamp(1, 200);
    let conn = state.connection().await?;
    let mut rows = conn.query(
        "SELECT d.id,d.score,d.match_rule,d.reason,d.status,d.created_at,d.updated_at,d.cluster_key,a.id,a.kind,a.amount_cents,a.currency,a.occurred_on,a.occurred_at,a.occurred_at_precision,a.source_channel,COALESCE(a.account_id,a.transfer_from_account_id,a.transfer_to_account_id),b.id,b.kind,b.amount_cents,b.currency,b.occurred_on,b.occurred_at,b.occurred_at_precision,b.source_channel,COALESCE(b.account_id,b.transfer_from_account_id,b.transfer_to_account_id) FROM duplicate_suspicions d JOIN ledger_transactions a ON a.id=d.transaction_id_a AND a.user_id=d.user_id JOIN ledger_transactions b ON b.id=d.transaction_id_b AND b.user_id=d.user_id WHERE d.user_id=?1 AND d.status='open' ORDER BY d.score DESC,d.created_at DESC,d.id DESC",
        params![user.id],
    ).await?;
    // This request-local enum is short-lived and keeps the pagination assembly straightforward.
    #[allow(clippy::large_enum_variant)]
    enum ListUnit {
        Item(DuplicateSuspicionView),
        Cluster(DuplicateSuspicionClusterView),
    }
    let mut units = Vec::new();
    let mut cluster_indexes = HashMap::<String, usize>::new();
    while let Some(row) = rows.next().await? {
        let item = DuplicateSuspicionView {
            id: row.get(0)?,
            score: row.get(1)?,
            match_rule: row.get(2)?,
            reason: row.get(3)?,
            cluster_key: row.get(7)?,
            status: row.get(4)?,
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
            transaction_a: transaction_view(&row, 8)?,
            transaction_b: transaction_view(&row, 17)?,
        };
        if item.cluster_key.is_empty() {
            units.push(ListUnit::Item(item));
        } else if let Some(index) = cluster_indexes.get(&item.cluster_key).copied() {
            let ListUnit::Cluster(cluster) = &mut units[index] else {
                unreachable!("cluster index always points at a cluster")
            };
            cluster.items.push(item);
        } else {
            let cluster_key = item.cluster_key.clone();
            cluster_indexes.insert(cluster_key.clone(), units.len());
            units.push(ListUnit::Cluster(DuplicateSuspicionClusterView {
                cluster_key,
                items: vec![item],
            }));
        }
    }
    let total = i64::try_from(units.len()).map_err(ApiError::internal)?;
    let offset = (u64::from(page - 1) * u64::from(page_size)) as usize;
    let mut items = Vec::new();
    let mut clusters = Vec::new();
    for unit in units.into_iter().skip(offset).take(page_size as usize) {
        match unit {
            ListUnit::Item(item) => items.push(item),
            ListUnit::Cluster(cluster) => clusters.push(cluster),
        }
    }
    Ok(Json(DuplicateSuspicionListResponse {
        items,
        clusters,
        total,
        page,
        page_size,
    }))
}

fn transaction_view(row: &libsql::Row, offset: i32) -> Result<DuplicateTransactionView, ApiError> {
    Ok(DuplicateTransactionView {
        id: row.get(offset)?,
        kind: row.get(offset + 1)?,
        amount_cents: row.get(offset + 2)?,
        currency: row.get(offset + 3)?,
        occurred_on: row.get(offset + 4)?,
        occurred_at: row.get(offset + 5)?,
        occurred_at_precision: row.get(offset + 6)?,
        source_channel: row.get(offset + 7)?,
        account_id: row.get(offset + 8)?,
    })
}

#[utoipa::path(patch, path = "/api/v1/duplicate-suspicions/{id}", params(("id" = String, Path)), request_body = UpdateDuplicateSuspicionRequest, responses((status = 200, body = DuplicateSuspicionView), (status = 404, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn update_duplicate_suspicion(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    Json(request): Json<UpdateDuplicateSuspicionRequest>,
) -> Result<Json<DuplicateSuspicionView>, ApiError> {
    if request.status != "dismissed" {
        return Err(ApiError::validation("确认疑似重复请使用 /confirm 动作接口"));
    }
    let conn = state.connection().await?;
    let now = Utc::now().to_rfc3339();
    let changed = conn.execute(
        "UPDATE duplicate_suspicions SET status=?1,updated_at=?2 WHERE id=?3 AND user_id=?4 AND status='open'",
        params![request.status, now, id.clone(), user.id.clone()],
    ).await?;
    if changed != 1 {
        return Err(ApiError::not_found("找不到待处理的疑似重复记录"));
    }
    let mut rows = conn.query(
        "SELECT d.id,d.score,d.match_rule,d.reason,d.status,d.created_at,d.updated_at,d.cluster_key,a.id,a.kind,a.amount_cents,a.currency,a.occurred_on,a.occurred_at,a.occurred_at_precision,a.source_channel,COALESCE(a.account_id,a.transfer_from_account_id,a.transfer_to_account_id),b.id,b.kind,b.amount_cents,b.currency,b.occurred_on,b.occurred_at,b.occurred_at_precision,b.source_channel,COALESCE(b.account_id,b.transfer_from_account_id,b.transfer_to_account_id) FROM duplicate_suspicions d JOIN ledger_transactions a ON a.id=d.transaction_id_a AND a.user_id=d.user_id JOIN ledger_transactions b ON b.id=d.transaction_id_b AND b.user_id=d.user_id WHERE d.id=?1 AND d.user_id=?2",
        params![id, user.id],
    ).await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| ApiError::not_found("找不到疑似重复记录"))?;
    Ok(Json(DuplicateSuspicionView {
        id: row.get(0)?,
        score: row.get(1)?,
        match_rule: row.get(2)?,
        reason: row.get(3)?,
        cluster_key: row.get(7)?,
        status: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        transaction_a: transaction_view(&row, 8)?,
        transaction_b: transaction_view(&row, 17)?,
    }))
}

#[derive(Debug)]
struct ActionSuspicion {
    id: String,
    transaction_id_a: String,
    transaction_id_b: String,
    match_rule: String,
    status: String,
    event_id: Option<String>,
    revert_payload: String,
}

#[utoipa::path(post, path = "/api/v1/duplicate-suspicions/{id}/confirm", params(("id" = String, Path)), responses((status = 200, body = DuplicateSuspicionActionResponse), (status = 404, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn confirm_duplicate_suspicion(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let key = idempotency_key(&headers)?;
    let hash = request_hash(&())?;
    let operation = format!("confirm_duplicate_suspicion:{id}");
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) = replay_idempotency(&tx, &user.id, &key, &operation, &hash).await? {
        tx.rollback().await?;
        return Ok(response);
    }

    let suspicion = load_action_suspicion(&tx, &user.id, &id).await?;
    if suspicion.status != "open" {
        return Err(action_state_conflict());
    }
    if !matches!(
        suspicion.match_rule.as_str(),
        "same_amount" | "withdraw_fee"
    ) {
        tx.rollback().await?;
        return Err(ApiError::validation("该类型暂不支持确认"));
    }

    let transaction_a = load_action_transaction(&tx, &user.id, &suspicion.transaction_id_a).await?;
    let transaction_b = load_action_transaction(&tx, &user.id, &suspicion.transaction_id_b).await?;
    if transaction_a.archived_at.is_some() || transaction_b.archived_at.is_some() {
        return Err(ApiError::conflict(
            "duplicate_transaction_archived",
            "疑似重复记录中的交易已归档，无法确认",
        ));
    }
    let (platform, bank) = action_platform_and_bank(&transaction_a, &transaction_b)?;
    let event_id = Uuid::now_v7().to_string();
    let now = Utc::now().to_rfc3339();
    let event_kind = if suspicion.match_rule == "withdraw_fee" {
        "transfer"
    } else {
        "consume"
    };
    tx.execute(
        "INSERT INTO transaction_events(id,user_id,kind,note,created_at,updated_at) VALUES (?1,?2,?3,'',?4,?4)",
        params![event_id.clone(), user.id.clone(), event_kind, now.clone()],
    )
    .await?;

    let mut affected_ids = vec![platform.id.clone(), bank.id.clone()];
    let created = if suspicion.match_rule == "withdraw_fee" {
        let fee_id = confirm_withdraw_fee(&tx, &user.id, platform, bank, &event_id, &now).await?;
        affected_ids.push(fee_id.clone());
        Some(CreatedSnapshot { id: fee_id })
    } else {
        confirm_same_amount(&tx, &user.id, platform, bank, &event_id, &now).await?;
        None
    };
    let payload = RevertPayload {
        changed: TransactionSnapshot {
            id: platform.id.clone(),
            amount_cents: platform.amount_cents,
            account_id: platform.account_id.clone(),
            transfer_to_account_id: platform.transfer_to_account_id.clone(),
            event_id: platform.event_id.clone(),
        },
        archived: ArchivedSnapshot {
            id: bank.id.clone(),
            archived_at: bank.archived_at.clone(),
            event_id: bank.event_id.clone(),
        },
        created,
    };
    let payload = serde_json::to_string(&payload).map_err(ApiError::internal)?;
    let changed = tx
        .execute(
            "UPDATE duplicate_suspicions SET status='confirmed',event_id=?1,revert_payload=?2,updated_at=?3 WHERE id=?4 AND user_id=?5 AND status='open'",
            params![event_id.clone(), payload, now, suspicion.id.clone(), user.id.clone()],
        )
        .await?;
    if changed != 1 {
        return Err(action_state_conflict());
    }
    let body = action_response(
        &tx,
        &user.id,
        suspicion.id,
        "confirmed",
        Some(TransactionEventView {
            id: event_id,
            kind: event_kind.to_owned(),
        }),
        &affected_ids,
    )
    .await?;
    store_idempotency(
        &tx,
        &user.id,
        &key,
        &operation,
        &hash,
        StatusCode::OK,
        &body,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(body).into_response())
}

#[utoipa::path(post, path = "/api/v1/duplicate-suspicions/{id}/dismiss", params(("id" = String, Path)), responses((status = 200, body = DuplicateSuspicionActionResponse), (status = 404, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn dismiss_duplicate_suspicion(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let key = idempotency_key(&headers)?;
    let hash = request_hash(&())?;
    let operation = format!("dismiss_duplicate_suspicion:{id}");
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) = replay_idempotency(&tx, &user.id, &key, &operation, &hash).await? {
        tx.rollback().await?;
        return Ok(response);
    }
    let suspicion = load_action_suspicion(&tx, &user.id, &id).await?;
    if suspicion.status != "open" {
        return Err(action_state_conflict());
    }
    let now = Utc::now().to_rfc3339();
    let changed = tx.execute(
        "UPDATE duplicate_suspicions SET status='dismissed',updated_at=?1 WHERE id=?2 AND user_id=?3 AND status='open'",
        params![now, id.clone(), user.id.clone()],
    ).await?;
    if changed != 1 {
        return Err(action_state_conflict());
    }
    let body = action_response(
        &tx,
        &user.id,
        id,
        "dismissed",
        None,
        &[suspicion.transaction_id_a, suspicion.transaction_id_b],
    )
    .await?;
    store_idempotency(
        &tx,
        &user.id,
        &key,
        &operation,
        &hash,
        StatusCode::OK,
        &body,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(body).into_response())
}

#[utoipa::path(post, path = "/api/v1/duplicate-suspicions/{id}/revert", params(("id" = String, Path)), responses((status = 200, body = DuplicateSuspicionActionResponse), (status = 404, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn revert_duplicate_suspicion(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let key = idempotency_key(&headers)?;
    let hash = request_hash(&())?;
    let operation = format!("revert_duplicate_suspicion:{id}");
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) = replay_idempotency(&tx, &user.id, &key, &operation, &hash).await? {
        tx.rollback().await?;
        return Ok(response);
    }
    let suspicion = load_action_suspicion(&tx, &user.id, &id).await?;
    if suspicion.status != "confirmed" {
        return Err(action_state_conflict());
    }
    let event_id = suspicion
        .event_id
        .clone()
        .ok_or_else(action_state_conflict)?;
    let payload: RevertPayload =
        serde_json::from_str(&suspicion.revert_payload).map_err(|_| action_state_conflict())?;
    let pair_ids = [
        suspicion.transaction_id_a.as_str(),
        suspicion.transaction_id_b.as_str(),
    ];
    if payload.changed.id == payload.archived.id
        || !pair_ids.contains(&payload.changed.id.as_str())
        || !pair_ids.contains(&payload.archived.id.as_str())
        || payload.created.as_ref().is_some_and(|created| {
            pair_ids.contains(&created.id.as_str()) || suspicion.match_rule != "withdraw_fee"
        })
        || (suspicion.match_rule == "withdraw_fee" && payload.created.is_none())
    {
        return Err(action_state_conflict());
    }
    let now = Utc::now().to_rfc3339();
    if let Some(created) = &payload.created {
        let deleted_event = crate::lifecycle::TransactionsDeleted {
            user_id: user.id.clone(),
            transaction_ids: vec![created.id.clone()],
        };
        let prepared = crate::lifecycle::prepare_transactions_deleted(&tx, &deleted_event).await?;
        let created_transaction = load_action_transaction(&tx, &user.id, &created.id).await?;
        if created_transaction.kind != "expense"
            || created_transaction.event_id.as_deref() != Some(event_id.as_str())
        {
            return Err(action_state_conflict());
        }
        let deleted = hard_delete_transaction_row(&tx, &user.id, &created.id).await?;
        if deleted != 1 {
            return Err(action_state_conflict());
        }
        crate::lifecycle::after_transactions_deleted(&tx, &deleted_event, prepared).await?;
    }
    let restored = update_transaction_row(
        &tx,
        &user.id,
        &payload.changed.id,
        TransactionPatch::RestoreAmountAccountsAndEvent {
            amount_cents: payload.changed.amount_cents,
            account_id: payload.changed.account_id.clone(),
            transfer_to_account_id: payload.changed.transfer_to_account_id.clone(),
            event_id: payload.changed.event_id.clone(),
            expected_event_id: event_id.clone(),
            updated_at: now.clone(),
        },
    )
    .await?;
    let unarchived = update_transaction_row(
        &tx,
        &user.id,
        &payload.archived.id,
        TransactionPatch::RestoreArchiveAndEvent {
            archived_at: payload.archived.archived_at.clone(),
            event_id: payload.archived.event_id.clone(),
            expected_event_id: event_id.clone(),
            updated_at: now.clone(),
        },
    )
    .await?;
    if restored != 1 || unarchived != 1 {
        return Err(action_state_conflict());
    }
    let reopened = tx.execute(
        "UPDATE duplicate_suspicions SET status='open',event_id=NULL,revert_payload='',updated_at=?1 WHERE id=?2 AND user_id=?3 AND status='confirmed' AND event_id=?4",
        params![now, id.clone(), user.id.clone(), event_id.clone()],
    ).await?;
    if reopened != 1 {
        return Err(action_state_conflict());
    }
    tx.execute(
        "DELETE FROM transaction_events WHERE id=?1 AND user_id=?2 AND NOT EXISTS (SELECT 1 FROM ledger_transactions WHERE event_id=?1) AND NOT EXISTS (SELECT 1 FROM duplicate_suspicions WHERE event_id=?1)",
        params![event_id, user.id.clone()],
    )
    .await?;
    let body = action_response(
        &tx,
        &user.id,
        id,
        "open",
        None,
        &[payload.changed.id, payload.archived.id],
    )
    .await?;
    store_idempotency(
        &tx,
        &user.id,
        &key,
        &operation,
        &hash,
        StatusCode::OK,
        &body,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(body).into_response())
}

async fn load_action_suspicion(
    conn: &Connection,
    user_id: &str,
    id: &str,
) -> Result<ActionSuspicion, ApiError> {
    let mut rows = conn.query(
        "SELECT id,transaction_id_a,transaction_id_b,match_rule,status,event_id,revert_payload FROM duplicate_suspicions WHERE id=?1 AND user_id=?2",
        params![id, user_id],
    ).await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| ApiError::not_found("找不到疑似重复记录"))?;
    Ok(ActionSuspicion {
        id: row.get(0)?,
        transaction_id_a: row.get(1)?,
        transaction_id_b: row.get(2)?,
        match_rule: row.get(3)?,
        status: row.get(4)?,
        event_id: row.get(5)?,
        revert_payload: row.get(6)?,
    })
}

async fn load_action_transaction(
    conn: &Connection,
    user_id: &str,
    id: &str,
) -> Result<ActionTransaction, ApiError> {
    let mut rows = conn.query(
        "SELECT id,kind,amount_cents,currency,occurred_on,occurred_at,occurred_at_precision,source_channel,account_id,transfer_from_account_id,transfer_to_account_id,payee_name,category_source,archived_at,event_id FROM ledger_transactions WHERE id=?1 AND user_id=?2",
        params![id, user_id],
    ).await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| ApiError::not_found("找不到疑似重复记录对应的交易"))?;
    Ok(ActionTransaction {
        id: row.get(0)?,
        kind: row.get(1)?,
        amount_cents: row.get(2)?,
        currency: row.get(3)?,
        occurred_on: row.get(4)?,
        occurred_at: row.get(5)?,
        occurred_at_precision: row.get(6)?,
        source_channel: row.get(7)?,
        account_id: row.get(8)?,
        transfer_from_account_id: row.get(9)?,
        transfer_to_account_id: row.get(10)?,
        payee_name: row.get(11)?,
        category_source: row.get(12)?,
        archived_at: row.get(13)?,
        event_id: row.get(14)?,
    })
}

fn action_platform_and_bank<'a>(
    a: &'a ActionTransaction,
    b: &'a ActionTransaction,
) -> Result<(&'a ActionTransaction, &'a ActionTransaction), ApiError> {
    if is_payment_platform(&a.source_channel) && is_bank(&b.source_channel) {
        Ok((a, b))
    } else if is_payment_platform(&b.source_channel) && is_bank(&a.source_channel) {
        Ok((b, a))
    } else {
        Err(ApiError::validation(
            "确认动作要求一条平台侧交易和一条银行侧交易",
        ))
    }
}

async fn confirm_same_amount(
    conn: &Connection,
    user_id: &str,
    platform: &ActionTransaction,
    bank: &ActionTransaction,
    event_id: &str,
    now: &str,
) -> Result<(), ApiError> {
    if platform.kind == "transfer"
        || platform.kind != bank.kind
        || platform.currency != bank.currency
    {
        return Err(ApiError::validation(
            "same_amount 确认要求两条非 transfer 交易类型与币种相同",
        ));
    }
    let bank_account_id = bank
        .account_id
        .as_deref()
        .ok_or_else(|| ApiError::validation("银行侧交易未绑定账户"))?;
    let changed = update_transaction_row(
        conn,
        user_id,
        &platform.id,
        TransactionPatch::SetAmountAccountAndEvent {
            amount_cents: bank.amount_cents,
            account_id: bank_account_id.to_owned(),
            event_id: event_id.to_owned(),
            updated_at: now.to_owned(),
        },
    )
    .await?;
    let archived = update_transaction_row(
        conn,
        user_id,
        &bank.id,
        TransactionPatch::ArchiveAndSetEvent {
            archived_at: now.to_owned(),
            event_id: event_id.to_owned(),
        },
    )
    .await?;
    if changed != 1 || archived != 1 {
        return Err(action_state_conflict());
    }
    Ok(())
}

async fn confirm_withdraw_fee(
    conn: &Connection,
    user_id: &str,
    platform: &ActionTransaction,
    bank: &ActionTransaction,
    event_id: &str,
    now: &str,
) -> Result<String, ApiError> {
    if platform.kind != "transfer"
        || bank.kind != "income"
        || platform.currency != bank.currency
        || platform.amount_cents <= bank.amount_cents
        || !withdraw_fee_amounts_match(platform.amount_cents, bank.amount_cents)
    {
        return Err(ApiError::validation(
            "withdraw_fee 确认要求平台 transfer 与银行 income 精确符合 0.1% 手续费",
        ));
    }
    let platform_account_id = platform
        .transfer_from_account_id
        .as_deref()
        .ok_or_else(|| ApiError::validation("平台侧提现交易缺少转出账户"))?;
    let bank_account_id = bank
        .account_id
        .as_deref()
        .ok_or_else(|| ApiError::validation("银行侧到账交易未绑定账户"))?;
    if platform
        .transfer_to_account_id
        .as_deref()
        .is_some_and(|account_id| account_id != bank_account_id)
    {
        return Err(ApiError::validation("提现转入账户与银行到账账户不一致"));
    }
    if platform_account_id == bank_account_id {
        return Err(ApiError::validation("提现转出账户与银行到账账户不能相同"));
    }
    let fee_cents = platform.amount_cents - bank.amount_cents;
    let fee_id = Uuid::now_v7().to_string();
    let changed = update_transaction_row(
        conn,
        user_id,
        &platform.id,
        TransactionPatch::SetAmountTransferDestinationAndEvent {
            amount_cents: bank.amount_cents,
            transfer_to_account_id: bank_account_id.to_owned(),
            event_id: event_id.to_owned(),
            updated_at: now.to_owned(),
        },
    )
    .await?;
    let inserted = insert_transaction_row(
        conn,
        NewTransactionRow {
            id: fee_id.clone(),
            user_id: user_id.to_owned(),
            kind: "expense".to_owned(),
            amount_cents: fee_cents,
            currency: platform.currency.clone(),
            occurred_on: platform.occurred_on.clone(),
            occurred_at: platform.occurred_at.clone(),
            occurred_at_precision: platform.occurred_at_precision.clone(),
            category: String::new(),
            category_id: None,
            category_source: "rule".to_owned(),
            category_rule_id: None,
            payee_name: "提现手续费".to_owned(),
            payee_key: String::new(),
            description: String::new(),
            account_id: Some(platform_account_id.to_owned()),
            transfer_from_account_id: None,
            transfer_to_account_id: None,
            note: String::new(),
            archived_at: None,
            version: 1,
            created_at: now.to_owned(),
            updated_at: now.to_owned(),
            source_channel: String::new(),
            external_id: String::new(),
            import_batch_id: None,
            event_id: Some(event_id.to_owned()),
            pnl_scope: "counted".to_owned(),
            created_by: "user".to_owned(),
            on_external_conflict: OnExternalConflict::Error,
        },
    )
    .await?;
    let archived = update_transaction_row(
        conn,
        user_id,
        &bank.id,
        TransactionPatch::ArchiveAndSetEvent {
            archived_at: now.to_owned(),
            event_id: event_id.to_owned(),
        },
    )
    .await?;
    if changed != 1 || inserted != 1 || archived != 1 {
        return Err(action_state_conflict());
    }
    Ok(fee_id)
}

fn withdraw_fee_amounts_match(platform_cents: i64, bank_cents: i64) -> bool {
    let Some(platform_scaled) = platform_cents.checked_mul(1000) else {
        return false;
    };
    let Some(bank_scaled) = bank_cents.checked_mul(1001) else {
        return false;
    };
    platform_scaled.abs_diff(bank_scaled) < 1000
}

async fn action_response(
    conn: &Connection,
    user_id: &str,
    suspicion_id: String,
    status: &str,
    event: Option<TransactionEventView>,
    transaction_ids: &[String],
) -> Result<DuplicateSuspicionActionResponse, ApiError> {
    let mut transactions = Vec::new();
    for id in transaction_ids {
        let transaction = load_action_transaction(conn, user_id, id).await?;
        transactions.push(DuplicateActionTransactionView {
            id: transaction.id,
            kind: transaction.kind,
            amount_cents: transaction.amount_cents,
            account_id: transaction.account_id,
            transfer_from_account_id: transaction.transfer_from_account_id,
            transfer_to_account_id: transaction.transfer_to_account_id,
            payee_name: transaction.payee_name,
            category_source: transaction.category_source,
            archived_at: transaction.archived_at,
            event_id: transaction.event_id,
        });
    }
    Ok(DuplicateSuspicionActionResponse {
        suspicion_id,
        status: status.to_owned(),
        event,
        transactions,
    })
}

fn action_state_conflict() -> ApiError {
    ApiError::conflict(
        "duplicate_suspicion_state_conflict",
        "疑似重复记录状态已变更，请刷新后重试",
    )
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use libsql::{Builder, TransactionBehavior, params};

    use super::{
        MatchTransaction, TransactionPatch, classify_candidate, match_committed_batch,
        update_transaction_row,
    };
    use crate::{db::migrate, domain::normalize_counterparty, imports::NORMALIZATION_VERSION};

    fn match_transaction(
        id: &str,
        channel: &str,
        amount_cents: i64,
        status: &str,
    ) -> MatchTransaction {
        MatchTransaction {
            id: id.to_owned(),
            kind: "expense".to_owned(),
            amount_cents,
            occurred_on: "2026-08-13".to_owned(),
            occurred_at: Some("2026-08-13 10:00:00".to_owned()),
            occurred_at_precision: "second".to_owned(),
            source_channel: channel.to_owned(),
            account_id: "account-1".to_owned(),
            direction: "expense".to_owned(),
            channel_status: status.to_owned(),
            source_text: String::new(),
            counterparty_normalized: "虚构商户".to_owned(),
        }
    }

    #[test]
    fn refund_rule_requires_equal_amounts() {
        // 同额退款仍应命中 refund
        let matched = classify_candidate(
            match_transaction("a", "wechat", 2000, "已退款"),
            match_transaction("b", "cmb", 2000, ""),
        )
        .expect("同额退款应配对");
        assert_eq!(matched.match_rule, "refund");

        // 金额不同的两笔不得因为一侧带「退款」字样就被判为重复：
        // refund 优先级高于 same_amount，会通过 used 集合挤掉真正的配对
        assert!(
            classify_candidate(
                match_transaction("a", "wechat", 2000, "已退款"),
                match_transaction("b", "cmb", 300_000, ""),
            )
            .is_none(),
            "退款金额不一致时不应配对"
        );
    }

    #[test]
    fn refund_rule_rejects_transfer_legs() {
        // 转账是账户间搬钱，不该被「退款」语义拉去配对；否则 0.98 的高优先级
        // 会通过 used 集合挤掉真正的 same_amount 配对
        let mut transfer = match_transaction("b", "cmb", 2000, "");
        transfer.kind = "transfer".to_owned();
        assert!(
            classify_candidate(match_transaction("a", "wechat", 2000, "已退款"), transfer)
                .is_none(),
            "退款不应与转账配对"
        );
    }

    #[test]
    fn pagination_offset_does_not_overflow_u32() {
        // page 只有 .max(1) 没有上界，page_size clamp 到 200。乘法留在 u32 里
        // 时 page=21474838 就会溢出：debug build panic 成 500，release 回绕。
        let page: u32 = 21_474_838;
        let page_size: u32 = 200;
        assert!(
            page.checked_mul(page_size).is_none(),
            "该页号在 u32 下本就会溢出，用例前提失效"
        );
        let offset = u64::from(page - 1) * u64::from(page_size);
        assert_eq!(offset, 4_294_967_400);
    }

    #[tokio::test]
    #[ignore = "operates only on an explicitly supplied /tmp database copy"]
    async fn rerun_all_committed_batches_on_audit_copy() {
        let path = std::env::var("ZHIYU_E2_AUDIT_DB").expect("ZHIYU_E2_AUDIT_DB is required");
        assert!(
            path.starts_with("/tmp/"),
            "audit database must be under /tmp"
        );
        let database = Builder::new_local(path).build().await.unwrap();
        migrate(&database).await.unwrap();
        let connection = database.connect().unwrap();
        let mut rows = connection
            .query(
                "SELECT r.id,b.user_id,b.source_channel,r.counterparty,r.transaction_id FROM import_records r JOIN import_batches b ON b.id=r.batch_id ORDER BY r.id",
                (),
            )
            .await
            .unwrap();
        let mut records = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            records.push((
                row.get::<String>(0).unwrap(),
                row.get::<String>(1).unwrap(),
                row.get::<String>(2).unwrap(),
                row.get::<String>(3).unwrap(),
                row.get::<Option<String>>(4).unwrap(),
            ));
        }
        drop(rows);
        let mut cmb_total = 0usize;
        let mut cmb_rewritten = 0usize;
        let mut cmb_prefixes: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (_, _, channel, counterparty, _) in &records {
            if channel != "cmb" {
                continue;
            }
            cmb_total += 1;
            let normalized = normalize_counterparty(channel, counterparty);
            if normalized != *counterparty {
                cmb_rewritten += 1;
            }
            cmb_prefixes
                .entry(normalized)
                .or_default()
                .insert(counterparty.clone());
        }
        let cmb_multi_original_prefixes: Vec<_> = cmb_prefixes
            .into_iter()
            .filter(|(_, originals)| originals.len() > 1)
            .collect();
        println!(
            "CMB_NORMALIZATION total={cmb_total} rewritten={cmb_rewritten} unchanged={} multi_original_prefix_groups={}",
            cmb_total - cmb_rewritten,
            cmb_multi_original_prefixes.len()
        );
        for (prefix, originals) in &cmb_multi_original_prefixes {
            assert!(
                !prefix
                    .chars()
                    .next_back()
                    .is_some_and(|character| matches!(character, '_' | '-' | '·' | '—' | '/')),
                "normalized CMB prefix must not end at a connector: {prefix:?}"
            );
            println!(
                "CMB_NORMALIZATION_GROUP prefix={prefix:?} original_count={}",
                originals.len()
            );
        }
        for (record_id, user_id, channel, counterparty, transaction_id) in records {
            let normalized = normalize_counterparty(&channel, &counterparty);
            connection
                .execute(
                    "UPDATE import_records SET counterparty_normalized=?1,normalization_version=?2 WHERE id=?3",
                    params![normalized.clone(), NORMALIZATION_VERSION, record_id],
                )
                .await
                .unwrap();
            if let Some(transaction_id) = transaction_id {
                update_transaction_row(
                    &connection,
                    &user_id,
                    &transaction_id,
                    TransactionPatch::SetPayeeKey {
                        payee_key: normalized,
                    },
                )
                .await
                .unwrap();
            }
        }
        connection
            .execute("DELETE FROM duplicate_suspicions", ())
            .await
            .unwrap();
        let mut rows = connection
            .query(
                "SELECT id,user_id FROM import_batches WHERE status='committed' ORDER BY committed_at,id",
                (),
            )
            .await
            .unwrap();
        let mut batches = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            batches.push((row.get::<String>(0).unwrap(), row.get::<String>(1).unwrap()));
        }
        drop(rows);
        for (batch_id, user_id) in batches {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .await
                .unwrap();
            match_committed_batch(&tx, &user_id, &batch_id)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }

        let mut rows = connection
            .query(
                "SELECT d.match_rule,CASE WHEN a.source_channel IN ('alipay','wechat') THEN a.source_channel ELSE b.source_channel END,CASE WHEN a.source_channel IN ('cmb','cmbc') THEN a.source_channel ELSE b.source_channel END,count(*),count(DISTINCT CASE WHEN d.match_rule='ambiguous' THEN d.cluster_key END) FROM duplicate_suspicions d JOIN ledger_transactions a ON a.id=d.transaction_id_a JOIN ledger_transactions b ON b.id=d.transaction_id_b GROUP BY 1,2,3 ORDER BY 2,3,1",
                (),
            )
            .await
            .unwrap();
        let mut cross_stats = BTreeMap::new();
        while let Some(row) = rows.next().await.unwrap() {
            let match_rule: String = row.get(0).unwrap();
            let platform: String = row.get(1).unwrap();
            let bank: String = row.get(2).unwrap();
            let edges: i64 = row.get(3).unwrap();
            let clusters: i64 = row.get(4).unwrap();
            println!(
                "DUPLICATE_CROSS match_rule={match_rule} channels={platform}<->{bank} pair_or_edge_count={edges} ambiguous_cluster_count={clusters}"
            );
            cross_stats.insert((match_rule, platform, bank), (edges, clusters));
        }
        drop(rows);
        for (platform, bank, baseline) in [
            ("wechat", "cmb", 92),
            ("wechat", "cmbc", 46),
            ("alipay", "cmb", 10),
            ("alipay", "cmbc", 2),
        ] {
            let rule_count = |rule: &str| {
                cross_stats
                    .get(&(rule.to_owned(), platform.to_owned(), bank.to_owned()))
                    .map_or(0, |(count, _)| *count)
            };
            let same_amount = rule_count("same_amount");
            let withdraw_fee = rule_count("withdraw_fee");
            let refund = rule_count("refund");
            let definite = same_amount + withdraw_fee + refund;
            let (ambiguous_edges, ambiguous_clusters) = cross_stats
                .get(&("ambiguous".to_owned(), platform.to_owned(), bank.to_owned()))
                .copied()
                .unwrap_or_default();
            println!(
                "DUPLICATE_DEFINITE channels={platform}<->{bank} same_amount={same_amount} withdraw_fee={withdraw_fee} refund={refund} total={definite} baseline={baseline} delta={}",
                definite - baseline
            );
            println!(
                "DUPLICATE_AMBIGUOUS channels={platform}<->{bank} clusters={ambiguous_clusters} candidate_edges={ambiguous_edges}"
            );
        }
    }
}
