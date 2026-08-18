use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::error::ApiError;

pub const DUE_SOON_DAYS: i64 = 7;
pub const MAX_SAFE_CENTS: i64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserView {
    pub id: String,
    pub email: String,
    pub timezone: String,
    pub email_verified: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
    pub timezone: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct EmailRequest {
    pub email: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct TokenRequest {
    pub token: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResetPasswordRequest {
    pub token: String,
    pub new_password: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MessageResponse {
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DebtDirection {
    BorrowIn,
    LendOut,
}

impl DebtDirection {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::BorrowIn => "borrow_in",
            Self::LendOut => "lend_out",
        }
    }
}

impl TryFrom<String> for DebtDirection {
    type Error = ApiError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "borrow_in" => Ok(Self::BorrowIn),
            "lend_out" => Ok(Self::LendOut),
            _ => Err(ApiError::validation("债务方向不正确")),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DebtOriginKind {
    CashMovement,
    NoCashMovement,
    LegacyUnknown,
}

impl DebtOriginKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CashMovement => "cash_movement",
            Self::NoCashMovement => "no_cash_movement",
            Self::LegacyUnknown => "legacy_unknown",
        }
    }

    pub fn from_db(value: &str) -> Result<Self, ApiError> {
        match value {
            "cash_movement" => Ok(Self::CashMovement),
            "no_cash_movement" => Ok(Self::NoCashMovement),
            "legacy_unknown" => Ok(Self::LegacyUnknown),
            _ => Err(ApiError::internal("debt origin kind is invalid")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DebtStatus {
    Archived,
    Settled,
    Overdue,
    DueSoon,
    Open,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CounterpartyView {
    pub id: String,
    pub display_name: String,
    pub note: String,
    pub archived: bool,
    pub version: i64,
    pub lend_out_remaining_cents: i64,
    pub borrow_in_remaining_cents: i64,
    pub net_cents: i64,
    pub active_debt_count: i64,
    pub overdue_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AccountType {
    WechatBalance,
    AlipayBalance,
    BankCard,
    Cash,
    DigitalCny,
    Other,
}

impl AccountType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::WechatBalance => "wechat_balance",
            Self::AlipayBalance => "alipay_balance",
            Self::BankCard => "bank_card",
            Self::Cash => "cash",
            Self::DigitalCny => "digital_cny",
            Self::Other => "other",
        }
    }

    pub fn from_db(value: &str) -> Result<Self, ApiError> {
        match value {
            "wechat_balance" => Ok(Self::WechatBalance),
            "alipay_balance" => Ok(Self::AlipayBalance),
            "bank_card" => Ok(Self::BankCard),
            "cash" => Ok(Self::Cash),
            "digital_cny" => Ok(Self::DigitalCny),
            "other" => Ok(Self::Other),
            _ => Err(ApiError::internal("ledger account type is invalid")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AccountNameSource {
    Custom,
    Derived,
}

impl AccountNameSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Custom => "custom",
            Self::Derived => "derived",
        }
    }

    pub fn from_db(value: &str) -> Result<Self, ApiError> {
        match value {
            "custom" => Ok(Self::Custom),
            "derived" => Ok(Self::Derived),
            _ => Err(ApiError::internal("ledger account name source is invalid")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LedgerAccountBrief {
    pub id: String,
    pub name: String,
    pub account_type: AccountType,
    pub archived: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LedgerAccountView {
    pub id: String,
    pub name: String,
    pub name_source: AccountNameSource,
    pub account_type: AccountType,
    pub note: String,
    pub bank_name: Option<String>,
    pub branch_name: Option<String>,
    pub card_number: Option<String>,
    pub nickname: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub archived: bool,
    pub version: i64,
    pub usage_count: i64,
    pub opening_balance_cents: i64,
    pub balance_cents: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RepaymentEventView {
    pub id: String,
    pub kind: String,
    pub amount_cents: i64,
    pub effective_on: String,
    pub note: String,
    pub reverses_event_id: Option<String>,
    pub reversed: bool,
    pub created_at: String,
    pub account: Option<LedgerAccountBrief>,
    pub transaction_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DebtAdditionEventView {
    pub id: String,
    pub amount_cents: i64,
    pub effective_on: String,
    pub note: String,
    pub created_at: String,
    pub account: Option<LedgerAccountBrief>,
    pub transaction_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DebtView {
    pub id: String,
    pub direction: String,
    pub counterparty: CounterpartyBrief,
    pub principal_cents: i64,
    pub paid_cents: i64,
    pub remaining_cents: i64,
    pub currency: String,
    pub occurred_on: String,
    pub due_on: Option<String>,
    pub note: String,
    pub status: DebtStatus,
    pub archived: bool,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub account: Option<LedgerAccountBrief>,
    pub origin_kind: DebtOriginKind,
    pub transaction_id: Option<String>,
    pub repayments: Vec<RepaymentEventView>,
    pub additions: Vec<DebtAdditionEventView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CounterpartyBrief {
    pub id: String,
    pub display_name: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DebtListResponse {
    pub items: Vec<DebtView>,
    pub page: u32,
    pub page_size: u32,
    pub total: u64,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct DebtListQuery {
    pub query: Option<String>,
    pub direction: Option<String>,
    pub status: Option<String>,
    pub counterparty_id: Option<String>,
    pub archived: Option<bool>,
    pub page: Option<u32>,
    pub page_size: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateDebtRequest {
    pub direction: DebtDirection,
    pub counterparty_id: Option<String>,
    pub counterparty_name: Option<String>,
    pub account_id: Option<String>,
    #[serde(default)]
    pub origin_kind: Option<DebtOriginKind>,
    pub principal_cents: i64,
    pub occurred_on: String,
    pub due_on: Option<String>,
    #[serde(default)]
    pub note: String,
    pub transaction_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDebtRequest {
    pub version: i64,
    pub counterparty_id: String,
    pub account_id: Option<String>,
    #[serde(default)]
    pub origin_kind: Option<DebtOriginKind>,
    pub principal_cents: i64,
    pub occurred_on: String,
    pub due_on: Option<String>,
    #[serde(default)]
    pub note: String,
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub transaction_id: Option<Option<String>>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct VersionRequest {
    pub version: i64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateRepaymentRequest {
    pub amount_cents: i64,
    pub effective_on: String,
    pub account_id: Option<String>,
    pub transaction_id: Option<String>,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateDebtAdditionRequest {
    pub amount_cents: i64,
    pub effective_on: String,
    pub account_id: Option<String>,
    pub transaction_id: Option<String>,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDebtAdditionRequest {
    pub version: i64,
    pub amount_cents: i64,
    pub effective_on: String,
    pub account_id: Option<String>,
    pub movement_type: Option<String>,
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub transaction_id: Option<Option<String>>,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRepaymentRequest {
    pub version: i64,
    pub amount_cents: i64,
    pub effective_on: String,
    pub account_id: Option<String>,
    pub movement_type: Option<String>,
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub transaction_id: Option<Option<String>>,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReverseRepaymentRequest {
    pub effective_on: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateCounterpartyRequest {
    pub display_name: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCounterpartyRequest {
    pub display_name: String,
    #[serde(default)]
    pub note: String,
    pub version: i64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateLedgerAccountRequest {
    #[serde(default)]
    pub name: String,
    pub account_type: AccountType,
    #[serde(default)]
    pub note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bank_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_number: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nickname: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default)]
    pub opening_balance_cents: i64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateLedgerAccountRequest {
    #[serde(default)]
    pub name: String,
    pub account_type: AccountType,
    #[serde(default)]
    pub note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bank_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_number: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nickname: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default)]
    pub opening_balance_cents: i64,
    pub version: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DashboardSummary {
    pub lend_out_remaining_cents: i64,
    pub borrow_in_remaining_cents: i64,
    pub net_cents: i64,
    pub overdue_count: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransactionKind {
    Income,
    Expense,
    Transfer,
}

impl TransactionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Income => "income",
            Self::Expense => "expense",
            Self::Transfer => "transfer",
        }
    }

    pub fn from_db(value: &str) -> Result<Self, ApiError> {
        match value {
            "income" => Ok(Self::Income),
            "expense" => Ok(Self::Expense),
            "transfer" => Ok(Self::Transfer),
            _ => Err(ApiError::internal("transaction kind is invalid")),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PnlScope {
    Counted,
    Excluded,
}

impl PnlScope {
    pub fn from_db(value: &str) -> Result<Self, ApiError> {
        match value {
            "counted" => Ok(Self::Counted),
            "excluded" => Ok(Self::Excluded),
            _ => Err(ApiError::internal("transaction pnl scope is invalid")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LedgerTransactionView {
    pub id: String,
    pub kind: TransactionKind,
    pub amount_cents: i64,
    pub occurred_on: String,
    pub occurred_at: Option<String>,
    pub occurred_at_precision: String,
    pub currency: String,
    pub category: String,
    pub category_id: Option<String>,
    #[schema(required = false)]
    pub category_source: String,
    pub category_rule_id: Option<String>,
    pub category_rule_name: Option<String>,
    pub payee_name: String,
    pub payee_key: String,
    pub description: String,
    pub account: Option<LedgerAccountBrief>,
    pub transfer_from_account: Option<LedgerAccountBrief>,
    pub transfer_to_account: Option<LedgerAccountBrief>,
    pub note: String,
    pub archived: bool,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub pnl_scope: PnlScope,
    pub created_by: String,
    pub links: Vec<TransactionLinkView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TransactionLinkView {
    pub plugin_id: String,
    pub kind: String,
    pub ref_id: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TransactionLinkCandidate {
    pub id: String,
    pub kind: TransactionKind,
    pub amount_cents: i64,
    pub occurred_on: String,
    pub note: String,
    pub account: Option<LedgerAccountBrief>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct TransactionLinkCandidatesQuery {
    pub amount_cents: Option<i64>,
}

fn deserialize_nullable_string<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TransactionListResponse {
    pub items: Vec<LedgerTransactionView>,
    pub page: u32,
    pub page_size: u32,
    pub total: u64,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct TransactionListQuery {
    pub month: Option<String>,
    pub kind: Option<String>,
    pub category: Option<String>,
    pub account_id: Option<String>,
    #[param(max_length = 100)]
    pub q: Option<String>,
    pub page: Option<u32>,
    pub page_size: Option<u32>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct TransactionSummaryQuery {
    pub month: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TransactionDaySummary {
    pub date: String,
    pub income_cents: i64,
    pub expense_cents: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TransactionCategorySummary {
    pub category: String,
    pub income_cents: i64,
    pub expense_cents: i64,
    pub count: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TransactionMonthSummary {
    pub month: String,
    pub days: Vec<TransactionDaySummary>,
    pub by_category: Vec<TransactionCategorySummary>,
    pub income_cents: i64,
    pub expense_cents: i64,
    pub net_cents: i64,
    pub transaction_count: i64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateTransactionRequest {
    pub kind: TransactionKind,
    pub amount_cents: i64,
    pub occurred_on: String,
    #[serde(default)]
    pub category: String,
    pub account_id: Option<String>,
    pub transfer_from_account_id: Option<String>,
    pub transfer_to_account_id: Option<String>,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTransactionRequest {
    pub version: i64,
    pub kind: TransactionKind,
    pub amount_cents: i64,
    pub occurred_on: String,
    #[serde(default)]
    pub category: String,
    pub category_id: Option<String>,
    pub account_id: Option<String>,
    pub transfer_from_account_id: Option<String>,
    pub transfer_to_account_id: Option<String>,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CategoryView {
    pub id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub kind: String,
    pub sort_order: i64,
    pub archived: bool,
    pub version: i64,
    #[schema(no_recursion)]
    pub children: Vec<CategoryView>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateCategoryRequest {
    pub parent_id: Option<String>,
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub sort_order: i64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCategoryRequest {
    pub version: i64,
    pub name: Option<String>,
    pub sort_order: Option<i64>,
    pub archived: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SelfTransferAliasView {
    pub id: String,
    pub alias: String,
    pub normalized_alias: String,
    pub note: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateSelfTransferAliasRequest {
    pub alias: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeleteSelfTransferAliasRequest {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CategoryRuleConditionView {
    pub id: String,
    pub match_field: String,
    pub match_kind: String,
    pub match_value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CategoryRuleView {
    pub id: String,
    pub priority: i64,
    pub enabled: bool,
    pub source_channel: String,
    pub category_id: String,
    pub note: String,
    pub conditions: Vec<CategoryRuleConditionView>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CategoryRuleConditionInput {
    pub match_field: String,
    pub match_kind: String,
    pub match_value: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateCategoryRuleRequest {
    #[serde(default = "default_category_rule_priority")]
    pub priority: i64,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub source_channel: String,
    pub category_id: String,
    #[serde(default)]
    pub note: String,
    pub conditions: Vec<CategoryRuleConditionInput>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCategoryRuleRequest {
    pub priority: Option<i64>,
    pub enabled: Option<bool>,
    pub source_channel: Option<String>,
    pub category_id: Option<String>,
    pub note: Option<String>,
    pub conditions: Option<Vec<CategoryRuleConditionInput>>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecategorizeResponse {
    pub eligible: i64,
    pub matched: i64,
    pub changed: i64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RevertCategoryRuleResponse {
    pub id: String,
    pub reverted_count: i64,
}

fn default_category_rule_priority() -> i64 {
    100
}

fn default_true() -> bool {
    true
}

pub fn validate_email(email: &str) -> Result<String, ApiError> {
    let email = email.trim().to_lowercase();
    let re = regex::Regex::new(r"^[^\s@]+@[^\s@]+\.[^\s@]+$").expect("email regex");
    if email.len() > 254 || !re.is_match(&email) {
        return Err(ApiError::validation("邮箱格式不正确"));
    }
    Ok(email)
}

pub fn validate_password(password: &str) -> Result<(), ApiError> {
    if !(12..=128).contains(&password.chars().count()) {
        return Err(ApiError::validation("密码长度须为 12–128 个字符"));
    }
    Ok(())
}

pub fn validate_timezone(value: Option<&str>) -> Result<String, ApiError> {
    let timezone = value.unwrap_or("Asia/Shanghai");
    timezone
        .parse::<Tz>()
        .map_err(|_| ApiError::validation("时区不正确"))?;
    Ok(timezone.to_owned())
}

pub fn validate_date(value: &str, label: &str) -> Result<(), ApiError> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map(|_| ())
        .map_err(|_| ApiError::validation(format!("{label}格式应为 YYYY-MM-DD")))
}

pub fn validate_amount(value: i64) -> Result<(), ApiError> {
    if !(1..=MAX_SAFE_CENTS).contains(&value) {
        return Err(ApiError::validation("金额必须大于 0 且在安全范围内"));
    }
    Ok(())
}

pub(crate) fn validate_note(value: &str) -> Result<(), ApiError> {
    if value.chars().count() > 2_000 {
        return Err(ApiError::validation("备注不能超过 2000 个字符"));
    }
    Ok(())
}

/// Conservatively normalizes a counterparty name without changing the source value.
pub fn normalize_counterparty(source_channel: &str, value: &str) -> String {
    match source_channel {
        "alipay" | "wechat" | "manual" => normalize_width(value.trim()),
        "cmbc" => value.to_owned(),
        "cmb" => normalize_cmb_counterparty(value),
        _ => normalize_width(value.trim()),
    }
}

fn normalize_width(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '\u{3000}' => ' ',
            '\u{ff01}'..='\u{ff5e}' => char::from_u32(character as u32 - 0xfee0)
                .expect("full-width ASCII mapping must be valid"),
            _ => character,
        })
        .collect()
}

fn normalize_cmb_counterparty(value: &str) -> String {
    let trimmed = value.trim();
    let bytes = trimmed.as_bytes();
    let mut suffix_start = bytes.len();
    while suffix_start > 0 && bytes[suffix_start - 1].is_ascii_alphanumeric() {
        suffix_start -= 1;
    }
    let suffix = &trimmed[suffix_start..];
    let remainder = trimmed[..suffix_start].trim_end();
    let remainder_non_whitespace_len = remainder
        .chars()
        .filter(|character| !character.is_whitespace())
        .count();
    let cuts_at_connector = remainder
        .chars()
        .next_back()
        .is_some_and(|character| matches!(character, '_' | '-' | '·' | '—' | '/'));
    if suffix.len() >= 6
        && suffix.bytes().any(|byte| byte.is_ascii_digit())
        && remainder_non_whitespace_len >= 2
        && !cuts_at_connector
    {
        remainder.to_owned()
    } else {
        trimmed.to_owned()
    }
}

pub fn validate_category(value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.chars().count() > 60 || value.chars().any(char::is_control) {
        return Err(ApiError::validation(
            "分类不能超过 60 个字符，且不能包含控制字符",
        ));
    }
    Ok(value.to_owned())
}

/// 校验 YYYY-MM 月份，返回 [当月 1 日, 次月 1 日) 半开区间，供 occurred_on 范围过滤。
pub fn validate_month(value: &str) -> Result<(String, String), ApiError> {
    let invalid = || ApiError::validation("月份格式应为 YYYY-MM");
    let bytes = value.as_bytes();
    if bytes.len() != 7
        || bytes[4] != b'-'
        || !bytes.iter().all(|b| b.is_ascii_digit() || *b == b'-')
    {
        return Err(invalid());
    }
    let start =
        NaiveDate::parse_from_str(&format!("{value}-01"), "%Y-%m-%d").map_err(|_| invalid())?;
    let end = if start.month() == 12 {
        NaiveDate::from_ymd_opt(start.year() + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(start.year(), start.month() + 1, 1)
    }
    .ok_or_else(invalid)?;
    Ok((
        start.format("%Y-%m-%d").to_string(),
        end.format("%Y-%m-%d").to_string(),
    ))
}

pub fn validate_opening_balance(value: i64) -> Result<(), ApiError> {
    if !(-MAX_SAFE_CENTS..=MAX_SAFE_CENTS).contains(&value) {
        return Err(ApiError::validation("初始余额超出安全范围"));
    }
    Ok(())
}

/// 校验债务来源类型与资金账户的组合：
/// 有实际收付款必须有账户，赊账（无资金进出）不能有账户，
/// legacy_unknown 仅允许存量数据在编辑时原样保留。
pub fn validate_debt_origin(
    origin_kind: DebtOriginKind,
    account_id: Option<&str>,
    existing: Option<DebtOriginKind>,
) -> Result<(), ApiError> {
    match origin_kind {
        DebtOriginKind::CashMovement if account_id.is_none() => {
            Err(ApiError::validation("有实际收付款的债务需要选择资金账户"))
        }
        DebtOriginKind::NoCashMovement if account_id.is_some() => {
            Err(ApiError::validation("无资金进出的债务不能指定资金账户"))
        }
        DebtOriginKind::LegacyUnknown if account_id.is_some() => {
            Err(ApiError::validation("历史未指定的债务不能指定资金账户"))
        }
        DebtOriginKind::LegacyUnknown if existing != Some(DebtOriginKind::LegacyUnknown) => {
            Err(ApiError::validation("历史未指定的债务类型仅用于存量数据"))
        }
        _ => Ok(()),
    }
}

pub fn debt_status(
    archived: bool,
    remaining_cents: i64,
    due_on: Option<&str>,
    timezone: &str,
    now: DateTime<Utc>,
) -> DebtStatus {
    if archived {
        return DebtStatus::Archived;
    }
    if remaining_cents == 0 {
        return DebtStatus::Settled;
    }
    let Some(due_on) = due_on.and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
    else {
        return DebtStatus::Open;
    };
    let timezone = timezone.parse::<Tz>().unwrap_or(chrono_tz::Asia::Shanghai);
    let today = now.with_timezone(&timezone).date_naive();
    if due_on < today {
        DebtStatus::Overdue
    } else if due_on <= today + Duration::days(DUE_SOON_DAYS) {
        DebtStatus::DueSoon
    } else {
        DebtStatus::Open
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::{
        DebtStatus, MAX_SAFE_CENTS, debt_status, validate_amount, validate_category,
        validate_month, validate_opening_balance,
    };

    #[test]
    fn due_status_respects_seven_day_boundary_and_priority() {
        let now = Utc.with_ymd_and_hms(2026, 8, 2, 4, 0, 0).unwrap();
        assert_eq!(
            debt_status(false, 100, Some("2026-08-01"), "Asia/Shanghai", now),
            DebtStatus::Overdue
        );
        assert_eq!(
            debt_status(false, 100, Some("2026-08-09"), "Asia/Shanghai", now),
            DebtStatus::DueSoon
        );
        assert_eq!(
            debt_status(false, 100, Some("2026-08-10"), "Asia/Shanghai", now),
            DebtStatus::Open
        );
        assert_eq!(
            debt_status(false, 0, Some("2026-08-01"), "Asia/Shanghai", now),
            DebtStatus::Settled
        );
        assert_eq!(
            debt_status(true, 0, None, "Asia/Shanghai", now),
            DebtStatus::Archived
        );
    }

    #[test]
    fn due_status_uses_the_users_local_calendar_day() {
        let before_shanghai_midnight = Utc.with_ymd_and_hms(2026, 8, 2, 15, 59, 59).unwrap();
        let after_shanghai_midnight = Utc.with_ymd_and_hms(2026, 8, 2, 16, 0, 0).unwrap();
        assert_eq!(
            debt_status(
                false,
                100,
                Some("2026-08-02"),
                "Asia/Shanghai",
                before_shanghai_midnight,
            ),
            DebtStatus::DueSoon
        );
        assert_eq!(
            debt_status(
                false,
                100,
                Some("2026-08-02"),
                "Asia/Shanghai",
                after_shanghai_midnight,
            ),
            DebtStatus::Overdue
        );
    }

    #[test]
    fn amount_validation_uses_integer_cents_and_js_safe_bounds() {
        assert!(validate_amount(1).is_ok());
        assert!(validate_amount(MAX_SAFE_CENTS).is_ok());
        assert!(validate_amount(0).is_err());
        assert!(validate_amount(MAX_SAFE_CENTS + 1).is_err());
    }

    #[test]
    fn month_validation_returns_a_half_open_date_range() {
        assert_eq!(
            validate_month("2026-08").unwrap(),
            ("2026-08-01".to_owned(), "2026-09-01".to_owned())
        );
        assert_eq!(
            validate_month("2026-12").unwrap(),
            ("2026-12-01".to_owned(), "2027-01-01".to_owned())
        );
        assert!(validate_month("2026-8").is_err());
        assert!(validate_month("2026-13").is_err());
        assert!(validate_month("2026/08").is_err());
        assert!(validate_month("").is_err());
    }

    #[test]
    fn category_validation_trims_and_rejects_control_or_overlong_values() {
        assert_eq!(validate_category("  餐饮  ").unwrap(), "餐饮");
        assert_eq!(validate_category("").unwrap(), "");
        assert!(validate_category(&"长".repeat(61)).is_err());
        assert!(validate_category("餐饮\n换行").is_err());
    }

    #[test]
    fn opening_balance_allows_negatives_within_safe_bounds() {
        assert!(validate_opening_balance(0).is_ok());
        assert!(validate_opening_balance(-5000).is_ok());
        assert!(validate_opening_balance(MAX_SAFE_CENTS).is_ok());
        assert!(validate_opening_balance(-MAX_SAFE_CENTS).is_ok());
        assert!(validate_opening_balance(MAX_SAFE_CENTS + 1).is_err());
        assert!(validate_opening_balance(-MAX_SAFE_CENTS - 1).is_err());
    }
}
