use libsql::{Connection, Transaction, TransactionBehavior};

use crate::{
    error::ApiError,
    lifecycle::{
        CategorySuggestion, CategorySuggestionStats, SuggestionFuture, TransactionsWritten,
    },
};

pub type CategorizeStats = CategorySuggestionStats;

#[derive(Debug)]
struct Rule {
    id: String,
    category_id: String,
    source_channel: String,
    conditions: Vec<Condition>,
}

#[derive(Debug)]
struct Condition {
    field: String,
    kind: String,
    value: String,
}

#[derive(Debug)]
struct Candidate {
    id: String,
    source_channel: String,
    payee_key: String,
    payee_name: String,
    description: String,
    note: String,
    kind: String,
    amount_cents: i64,
    channel_category: Option<String>,
    pay_method: Option<String>,
    merchant_order_id: Option<String>,
}

/// Re-run enabled rules for all rule-eligible transactions belonging to one user.
/// The transaction makes the command atomic and serializes it with other writers.
pub async fn recategorize_user(
    conn: &Connection,
    user_id: &str,
) -> Result<CategorizeStats, libsql::Error> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    let stats = crate::lifecycle::reapply_category_rules(&tx, user_id)
        .await
        .map_err(api_error_to_libsql)?;
    tx.commit().await?;
    Ok(stats)
}

pub fn suggest_categories<'a>(
    tx: &'a Transaction,
    event: &'a TransactionsWritten,
) -> SuggestionFuture<'a> {
    Box::pin(async move { suggest_categories_inner(tx, event).await })
}

async fn suggest_categories_inner(
    conn: &Connection,
    event: &TransactionsWritten,
) -> Result<Vec<CategorySuggestion>, ApiError> {
    let rules = load_rules(conn, &event.user_id).await?;
    let candidates = load_candidates(conn, event).await?;
    let mut suggestions = Vec::new();
    for candidate in candidates {
        let Some(rule) = rules.iter().find(|rule| rule.matches(&candidate)) else {
            continue;
        };
        suggestions.push(CategorySuggestion {
            transaction_id: candidate.id,
            category_id: rule.category_id.clone(),
            rule_id: rule.id.clone(),
        });
    }
    Ok(suggestions)
}

async fn load_rules(conn: &Connection, user_id: &str) -> Result<Vec<Rule>, libsql::Error> {
    let mut rows = conn
        .query(
            "SELECT r.id,r.category_id,r.source_channel,rc.match_field,rc.match_kind,rc.match_value FROM category_rules r JOIN categories c ON c.id=r.category_id AND c.user_id=r.user_id AND c.archived_at IS NULL LEFT JOIN category_rule_conditions rc ON rc.rule_id=r.id WHERE r.user_id=?1 AND r.enabled=1 ORDER BY r.priority ASC,r.created_at ASC,r.id ASC,rc.created_at ASC,rc.id ASC",
            [user_id],
        )
        .await?;
    let mut rules: Vec<(String, Rule)> = Vec::new();
    while let Some(row) = rows.next().await? {
        let rule_id: String = row.get(0)?;
        if rules.last().is_none_or(|(id, _)| id != &rule_id) {
            rules.push((
                rule_id.clone(),
                Rule {
                    id: rule_id,
                    category_id: row.get(1)?,
                    source_channel: row.get(2)?,
                    conditions: Vec::new(),
                },
            ));
        }
        let field: Option<String> = row.get(3)?;
        if let Some(field) = field {
            rules
                .last_mut()
                .expect("rule was just inserted")
                .1
                .conditions
                .push(Condition {
                    field,
                    kind: row.get(4)?,
                    value: row.get(5)?,
                });
        }
    }
    Ok(rules.into_iter().map(|(_, rule)| rule).collect())
}

async fn load_candidates(
    conn: &Connection,
    event: &TransactionsWritten,
) -> Result<Vec<Candidate>, libsql::Error> {
    if event.transaction_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = (0..event.transaction_ids.len())
        .map(|index| format!("?{}", index + 2))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT t.id,t.source_channel,t.payee_key,t.payee_name,t.description,t.note,t.kind,t.amount_cents,r.channel_category,r.pay_method,r.merchant_order_id FROM ledger_transactions t LEFT JOIN import_records r ON r.transaction_id=t.id WHERE t.user_id=?1 AND t.id IN ({placeholders}) AND t.category_source IN ('none','rule') ORDER BY t.id"
    );
    let mut values = Vec::with_capacity(event.transaction_ids.len() + 1);
    values.push(event.user_id.clone());
    values.extend(event.transaction_ids.iter().cloned());
    let mut rows = conn.query(&sql, values).await?;
    let mut candidates = Vec::new();
    while let Some(row) = rows.next().await? {
        candidates.push(Candidate {
            id: row.get(0)?,
            source_channel: row.get(1)?,
            payee_key: row.get(2)?,
            payee_name: row.get(3)?,
            description: row.get(4)?,
            note: row.get(5)?,
            kind: row.get(6)?,
            amount_cents: row.get(7)?,
            channel_category: row.get(8)?,
            pay_method: row.get(9)?,
            merchant_order_id: row.get(10)?,
        });
    }
    Ok(candidates)
}

fn api_error_to_libsql(error: ApiError) -> libsql::Error {
    libsql::Error::Misuse(error.message)
}

impl Rule {
    fn matches(&self, candidate: &Candidate) -> bool {
        (self.source_channel.is_empty() || self.source_channel == candidate.source_channel)
            && self
                .conditions
                .iter()
                .all(|condition| condition.matches(candidate))
    }
}

impl Condition {
    fn matches(&self, candidate: &Candidate) -> bool {
        if self.field == "amount_cents" {
            let Ok(expected) = self.value.trim().parse::<i64>() else {
                return false;
            };
            return match self.kind.as_str() {
                "gte" => candidate.amount_cents >= expected,
                "lte" => candidate.amount_cents <= expected,
                "exact" => candidate.amount_cents == expected,
                _ => false,
            };
        }
        if matches!(self.kind.as_str(), "gte" | "lte") {
            return false;
        }
        let actual = match self.field.as_str() {
            "payee_key" => Some(candidate.payee_key.as_str()),
            "payee_name" => Some(candidate.payee_name.as_str()),
            "description" => Some(candidate.description.as_str()),
            "note" => Some(candidate.note.as_str()),
            "kind" => Some(candidate.kind.as_str()),
            "channel_category" => candidate.channel_category.as_deref(),
            "pay_method" => candidate.pay_method.as_deref(),
            "merchant_order_id" => candidate.merchant_order_id.as_deref(),
            _ => None,
        };
        let Some(actual) = actual else {
            return false;
        };
        let actual = actual.trim().to_lowercase();
        let expected = self.value.trim().to_lowercase();
        match self.kind.as_str() {
            "exact" => actual == expected,
            "contains" => actual.contains(&expected),
            "prefix" => actual.starts_with(&expected),
            _ => false,
        }
    }
}
