use chrono::{DateTime, Duration, NaiveDate, Utc};
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
    pub account_id: String,
    pub principal_cents: i64,
    pub occurred_on: String,
    pub due_on: Option<String>,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDebtRequest {
    pub version: i64,
    pub counterparty_id: String,
    pub account_id: String,
    pub principal_cents: i64,
    pub occurred_on: String,
    pub due_on: Option<String>,
    #[serde(default)]
    pub note: String,
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
    pub account_id: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateDebtAdditionRequest {
    pub amount_cents: i64,
    pub effective_on: String,
    pub account_id: String,
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

    use super::{DebtStatus, MAX_SAFE_CENTS, debt_status, validate_amount};

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
}
