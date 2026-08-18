use std::{future::Future, pin::Pin};

use chrono::Utc;
use libsql::{Transaction, params};
use serde_json::Value;

use crate::{error::ApiError, plugins};

#[derive(Debug, Clone)]
pub struct TransactionsWritten {
    pub user_id: String,
    pub transaction_ids: Vec<String>,
    pub origin: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategorySuggestion {
    pub transaction_id: String,
    pub category_id: String,
    pub rule_id: String,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CategorySuggestionStats {
    pub eligible: i64,
    pub matched: i64,
    pub changed: i64,
}

#[derive(Debug, Clone)]
pub struct TransactionsDeleted {
    pub user_id: String,
    pub transaction_ids: Vec<String>,
}

pub struct PreparedDeletion {
    handlers: Vec<(DeletionHandler, Value)>,
}

pub type SuggestionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<CategorySuggestion>, ApiError>> + Send + 'a>>;
pub type SuggestionProvider =
    for<'a> fn(&'a Transaction, &'a TransactionsWritten) -> SuggestionFuture<'a>;

pub type DeletionPrepareFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Value, ApiError>> + Send + 'a>>;
pub type DeletionHandleFuture<'a> = Pin<Box<dyn Future<Output = Result<(), ApiError>> + Send + 'a>>;
pub type DeletionPreparer =
    for<'a> fn(&'a Transaction, &'a TransactionsDeleted) -> DeletionPrepareFuture<'a>;
pub type DeletionHandlerFn =
    for<'a> fn(&'a Transaction, &'a TransactionsDeleted, &'a Value) -> DeletionHandleFuture<'a>;

#[derive(Clone, Copy)]
pub struct DeletionHandler {
    pub prepare: DeletionPreparer,
    pub handle: DeletionHandlerFn,
}

pub async fn after_transactions_written(
    tx: &Transaction,
    event: &TransactionsWritten,
) -> Result<CategorySuggestionStats, ApiError> {
    apply_category_suggestions(tx, event, false).await
}

pub async fn reapply_category_rules(
    tx: &Transaction,
    user_id: &str,
) -> Result<CategorySuggestionStats, ApiError> {
    let mut rows = tx
        .query(
            "SELECT id FROM ledger_transactions WHERE user_id=?1 AND category_source IN ('none','rule') ORDER BY id",
            [user_id],
        )
        .await?;
    let mut transaction_ids = Vec::new();
    while let Some(row) = rows.next().await? {
        transaction_ids.push(row.get(0)?);
    }
    drop(rows);
    let event = TransactionsWritten {
        user_id: user_id.to_owned(),
        transaction_ids,
        origin: "rule-reapply",
    };
    apply_category_suggestions(tx, &event, true).await
}

async fn apply_category_suggestions(
    tx: &Transaction,
    event: &TransactionsWritten,
    include_rule_assignments: bool,
) -> Result<CategorySuggestionStats, ApiError> {
    if event.transaction_ids.is_empty() {
        return Ok(CategorySuggestionStats::default());
    }
    let providers = plugins::suggestion_providers(tx, &event.user_id).await?;
    if providers.is_empty() {
        return Ok(CategorySuggestionStats::default());
    }
    let eligible = count_eligible(tx, event, include_rule_assignments).await?;
    let mut suggestions = Vec::new();
    for provider in providers {
        suggestions.extend(provider(tx, event).await?);
    }

    let allowed_sources = if include_rule_assignments {
        "('none','rule')"
    } else {
        "('none')"
    };
    let now = Utc::now().to_rfc3339();
    let mut changed = 0_i64;
    for suggestion in &suggestions {
        if !event
            .transaction_ids
            .iter()
            .any(|id| id == &suggestion.transaction_id)
        {
            continue;
        }
        let sql = format!(
            "UPDATE ledger_transactions SET category_id=?1,category_source='rule',category_rule_id=?2,updated_at=?3 WHERE id=?4 AND user_id=?5 AND category_source IN {allowed_sources} AND EXISTS (SELECT 1 FROM categories c WHERE c.id=?1 AND c.user_id=?5) AND (category_source<>'rule' OR category_id IS NULL OR category_id<>?1 OR category_rule_id IS NULL OR category_rule_id<>?2)"
        );
        changed += tx
            .execute(
                &sql,
                params![
                    suggestion.category_id.clone(),
                    suggestion.rule_id.clone(),
                    now.clone(),
                    suggestion.transaction_id.clone(),
                    event.user_id.clone()
                ],
            )
            .await? as i64;
    }
    Ok(CategorySuggestionStats {
        eligible,
        matched: suggestions.len() as i64,
        changed,
    })
}

async fn count_eligible(
    tx: &Transaction,
    event: &TransactionsWritten,
    include_rule_assignments: bool,
) -> Result<i64, ApiError> {
    let placeholders = (0..event.transaction_ids.len())
        .map(|index| format!("?{}", index + 2))
        .collect::<Vec<_>>()
        .join(",");
    let sources = if include_rule_assignments {
        "('none','rule')"
    } else {
        "('none')"
    };
    let sql = format!(
        "SELECT COUNT(*) FROM ledger_transactions WHERE user_id=?1 AND id IN ({placeholders}) AND category_source IN {sources}"
    );
    let mut values = Vec::with_capacity(event.transaction_ids.len() + 1);
    values.push(event.user_id.clone());
    values.extend(event.transaction_ids.iter().cloned());
    let mut rows = tx.query(&sql, values).await?;
    Ok(rows
        .next()
        .await?
        .ok_or_else(|| ApiError::internal("transaction lifecycle count returned no row"))?
        .get(0)?)
}

pub async fn prepare_transactions_deleted(
    tx: &Transaction,
    event: &TransactionsDeleted,
) -> Result<PreparedDeletion, ApiError> {
    let mut handlers = Vec::new();
    for handler in plugins::deletion_handlers(tx, &event.user_id).await? {
        handlers.push((handler, (handler.prepare)(tx, event).await?));
    }
    Ok(PreparedDeletion { handlers })
}

pub async fn after_transactions_deleted(
    tx: &Transaction,
    event: &TransactionsDeleted,
    prepared: PreparedDeletion,
) -> Result<(), ApiError> {
    for (handler, state) in &prepared.handlers {
        (handler.handle)(tx, event, state).await?;
    }
    Ok(())
}
