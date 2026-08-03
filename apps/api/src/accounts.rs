use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use libsql::{Connection, TransactionBehavior, params};
use uuid::Uuid;

use crate::{
    AppState,
    auth::AuthUser,
    debts::{idempotency_key, replay_idempotency, request_hash, store_idempotency, validate_note},
    domain::{
        AccountNameSource, AccountType, CreateLedgerAccountRequest, LedgerAccountView,
        UpdateLedgerAccountRequest, VersionRequest, validate_email,
    },
    error::ApiError,
};

#[utoipa::path(get, path = "/api/v1/ledger-accounts", responses((status = 200, body = [LedgerAccountView])), security(("cookieAuth" = [])))]
pub async fn list_ledger_accounts(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<LedgerAccountView>>, ApiError> {
    let conn = state.connection().await?;
    Ok(Json(load_ledger_accounts(&conn, &user.id).await?))
}

#[utoipa::path(post, path = "/api/v1/ledger-accounts", request_body = CreateLedgerAccountRequest, responses((status = 201, body = LedgerAccountView), (status = 409, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn create_ledger_account(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Json(input): Json<CreateLedgerAccountRequest>,
) -> Result<Response, ApiError> {
    validate_note(&input.note)?;
    let details = validate_account_details(
        &input.account_type,
        input.bank_name.as_deref(),
        input.branch_name.as_deref(),
        input.card_number.as_deref(),
        input.nickname.as_deref(),
        input.phone.as_deref(),
        input.email.as_deref(),
    )?;
    let (name, name_source) = resolve_account_name(&input.name, &input.account_type, &details)?;
    let key = idempotency_key(&headers)?;
    let request_hash = request_hash(&input)?;
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) =
        replay_idempotency(&tx, &user.id, &key, "create_ledger_account", &request_hash).await?
    {
        return Ok(response);
    }

    let normalized_name = normalize_account_name(&name);
    ensure_name_available(&tx, &user.id, &normalized_name, None).await?;
    let id = Uuid::now_v7().to_string();
    let now = Utc::now().to_rfc3339();
    tx.execute(
        "INSERT INTO ledger_accounts(id, user_id, name, normalized_name, name_source, account_type, note, bank_name, branch_name, card_number, nickname, phone, email, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14)",
        params![id.clone(), user.id.clone(), name, normalized_name, name_source.as_str(), input.account_type.as_str(), input.note.trim(), details.bank_name, details.branch_name, details.card_number, details.nickname, details.phone, details.email, now],
    ).await?;
    let item = load_ledger_account(&tx, &user.id, &id).await?;
    store_idempotency(
        &tx,
        &user.id,
        &key,
        "create_ledger_account",
        &request_hash,
        StatusCode::CREATED,
        &item,
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(item)).into_response())
}

#[utoipa::path(patch, path = "/api/v1/ledger-accounts/{id}", params(("id" = String, Path)), request_body = UpdateLedgerAccountRequest, responses((status = 200, body = LedgerAccountView), (status = 409, body = crate::error::ErrorBody), (status = 422, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn update_ledger_account(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<UpdateLedgerAccountRequest>,
) -> Result<Response, ApiError> {
    validate_note(&input.note)?;
    let details = validate_account_details(
        &input.account_type,
        input.bank_name.as_deref(),
        input.branch_name.as_deref(),
        input.card_number.as_deref(),
        input.nickname.as_deref(),
        input.phone.as_deref(),
        input.email.as_deref(),
    )?;
    let (name, name_source) = resolve_account_name(&input.name, &input.account_type, &details)?;
    let key = idempotency_key(&headers)?;
    let request_hash = request_hash(&input)?;
    let operation = format!("update_ledger_account:{id}");
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) =
        replay_idempotency(&tx, &user.id, &key, &operation, &request_hash).await?
    {
        return Ok(response);
    }

    load_ledger_account(&tx, &user.id, &id).await?;
    let normalized_name = normalize_account_name(&name);
    ensure_name_available(&tx, &user.id, &normalized_name, Some(&id)).await?;
    let changed = tx.execute(
        "UPDATE ledger_accounts SET name = ?1, normalized_name = ?2, name_source = ?3, account_type = ?4, note = ?5, bank_name = ?6, branch_name = ?7, card_number = ?8, nickname = ?9, phone = ?10, email = ?11, version = version + 1, updated_at = ?12 WHERE id = ?13 AND user_id = ?14 AND version = ?15",
        params![name, normalized_name, name_source.as_str(), input.account_type.as_str(), input.note.trim(), details.bank_name, details.branch_name, details.card_number, details.nickname, details.phone, details.email, Utc::now().to_rfc3339(), id.clone(), user.id.clone(), input.version],
    ).await?;
    if changed == 0 {
        return Err(ApiError::conflict(
            "version_conflict",
            "账户已在其他设备更新，请刷新后重试",
        ));
    }
    let item = load_ledger_account(&tx, &user.id, &id).await?;
    store_idempotency(
        &tx,
        &user.id,
        &key,
        &operation,
        &request_hash,
        StatusCode::OK,
        &item,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(item).into_response())
}

#[utoipa::path(post, path = "/api/v1/ledger-accounts/{id}/archive", params(("id" = String, Path)), request_body = VersionRequest, responses((status = 200, body = LedgerAccountView), (status = 409, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn archive_ledger_account(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<VersionRequest>,
) -> Result<Response, ApiError> {
    let key = idempotency_key(&headers)?;
    set_archive(&state, &user, &id, &input, true, &key).await
}

#[utoipa::path(post, path = "/api/v1/ledger-accounts/{id}/restore", params(("id" = String, Path)), request_body = VersionRequest, responses((status = 200, body = LedgerAccountView), (status = 409, body = crate::error::ErrorBody)), security(("cookieAuth" = [])))]
pub async fn restore_ledger_account(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<VersionRequest>,
) -> Result<Response, ApiError> {
    let key = idempotency_key(&headers)?;
    set_archive(&state, &user, &id, &input, false, &key).await
}

async fn set_archive(
    state: &AppState,
    user: &AuthUser,
    id: &str,
    input: &VersionRequest,
    archived: bool,
    key: &str,
) -> Result<Response, ApiError> {
    let operation = format!(
        "{}_ledger_account:{id}",
        if archived { "archive" } else { "restore" }
    );
    let request_hash = request_hash(input)?;
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if let Some(response) =
        replay_idempotency(&tx, &user.id, key, &operation, &request_hash).await?
    {
        return Ok(response);
    }

    let existing = load_ledger_account(&tx, &user.id, id).await?;
    if !archived {
        ensure_name_available(
            &tx,
            &user.id,
            &normalize_account_name(&existing.name),
            Some(id),
        )
        .await?;
    }
    let archived_at = archived.then(|| Utc::now().to_rfc3339());
    let changed = tx.execute(
        "UPDATE ledger_accounts SET archived_at = ?1, version = version + 1, updated_at = ?2 WHERE id = ?3 AND user_id = ?4 AND version = ?5",
        params![archived_at, Utc::now().to_rfc3339(), id, user.id.clone(), input.version],
    ).await?;
    if changed == 0 {
        return Err(ApiError::conflict(
            "version_conflict",
            "账户已变化，请刷新后重试",
        ));
    }
    let item = load_ledger_account(&tx, &user.id, id).await?;
    store_idempotency(
        &tx,
        &user.id,
        key,
        &operation,
        &request_hash,
        StatusCode::OK,
        &item,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(item).into_response())
}

async fn load_ledger_accounts(
    conn: &Connection,
    user_id: &str,
) -> Result<Vec<LedgerAccountView>, ApiError> {
    let mut rows = conn.query(
        "SELECT a.id, a.name, a.account_type, a.note, a.bank_name, a.branch_name, a.card_number, a.nickname, a.phone, a.email, a.archived_at, a.version, a.created_at, a.updated_at, a.name_source, (SELECT COUNT(*) FROM debts d WHERE d.user_id = a.user_id AND d.account_id = a.id) + (SELECT COUNT(*) FROM debt_addition_events e WHERE e.user_id = a.user_id AND e.account_id = a.id) + (SELECT COUNT(*) FROM repayment_events e WHERE e.user_id = a.user_id AND e.account_id = a.id) AS usage_count FROM ledger_accounts a WHERE a.user_id = ?1 ORDER BY a.archived_at IS NOT NULL, a.normalized_name, a.id",
        [user_id],
    ).await?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().await? {
        items.push(ledger_account_from_row(&row)?);
    }
    Ok(items)
}

async fn load_ledger_account(
    conn: &Connection,
    user_id: &str,
    id: &str,
) -> Result<LedgerAccountView, ApiError> {
    let mut rows = conn.query(
        "SELECT a.id, a.name, a.account_type, a.note, a.bank_name, a.branch_name, a.card_number, a.nickname, a.phone, a.email, a.archived_at, a.version, a.created_at, a.updated_at, a.name_source, (SELECT COUNT(*) FROM debts d WHERE d.user_id = a.user_id AND d.account_id = a.id) + (SELECT COUNT(*) FROM debt_addition_events e WHERE e.user_id = a.user_id AND e.account_id = a.id) + (SELECT COUNT(*) FROM repayment_events e WHERE e.user_id = a.user_id AND e.account_id = a.id) AS usage_count FROM ledger_accounts a WHERE a.id = ?1 AND a.user_id = ?2",
        params![id, user_id],
    ).await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| ApiError::not_found("找不到该账户"))?;
    ledger_account_from_row(&row)
}

fn ledger_account_from_row(row: &libsql::Row) -> Result<LedgerAccountView, ApiError> {
    let account_type: String = row.get(2)?;
    let name_source: String = row.get(14)?;
    let archived_at: Option<String> = row.get(10)?;
    Ok(LedgerAccountView {
        id: row.get(0)?,
        name: row.get(1)?,
        name_source: AccountNameSource::from_db(&name_source)?,
        account_type: AccountType::from_db(&account_type)?,
        note: row.get(3)?,
        bank_name: row.get(4)?,
        branch_name: row.get(5)?,
        card_number: row.get(6)?,
        nickname: row.get(7)?,
        phone: row.get(8)?,
        email: row.get(9)?,
        archived: archived_at.is_some(),
        version: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
        usage_count: row.get(15)?,
    })
}

async fn ensure_name_available(
    conn: &Connection,
    user_id: &str,
    normalized_name: &str,
    except_id: Option<&str>,
) -> Result<(), ApiError> {
    let mut rows = conn
        .query(
            "SELECT id FROM ledger_accounts WHERE user_id = ?1 AND normalized_name = ?2",
            params![user_id, normalized_name],
        )
        .await?;
    while let Some(row) = rows.next().await? {
        let id: String = row.get(0)?;
        if except_id != Some(id.as_str()) {
            return Err(ApiError::conflict(
                "account_name_conflict",
                "已有同名账户（包括已归档账户），请填写自定义名称进行区分",
            ));
        }
    }
    Ok(())
}

fn validate_custom_account_name(value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.chars().count() > 80 {
        return Err(ApiError::validation("账户名称不能超过 80 个字符"));
    }
    Ok(value.to_owned())
}

fn normalize_account_name(value: &str) -> String {
    value.trim().to_lowercase()
}

#[derive(Debug, PartialEq, Eq)]
struct LedgerAccountDetails {
    bank_name: Option<String>,
    branch_name: Option<String>,
    card_number: Option<String>,
    nickname: Option<String>,
    phone: Option<String>,
    email: Option<String>,
}

fn resolve_account_name(
    requested_name: &str,
    account_type: &AccountType,
    details: &LedgerAccountDetails,
) -> Result<(String, AccountNameSource), ApiError> {
    let custom_name = validate_custom_account_name(requested_name)?;
    if !custom_name.is_empty() {
        return Ok((custom_name, AccountNameSource::Custom));
    }

    let derived_name = match account_type {
        AccountType::BankCard => details
            .card_number
            .as_deref()
            .or(details.bank_name.as_deref())
            .or(details.branch_name.as_deref()),
        AccountType::WechatBalance => details.nickname.as_deref().or(details.phone.as_deref()),
        AccountType::AlipayBalance => details
            .nickname
            .as_deref()
            .or(details.phone.as_deref())
            .or(details.email.as_deref()),
        AccountType::Cash | AccountType::DigitalCny | AccountType::Other => None,
    }
    .ok_or_else(|| {
        ApiError::validation(match account_type {
            AccountType::BankCard => "未填写自定义名称时，请至少填写卡号、银行或开户行",
            AccountType::WechatBalance => "未填写自定义名称时，请至少填写微信昵称或手机号",
            AccountType::AlipayBalance => "未填写自定义名称时，请至少填写支付宝昵称、手机号或邮箱",
            AccountType::Cash | AccountType::DigitalCny | AccountType::Other => {
                "该账户类型必须填写账户名称"
            }
        })
    })?;

    if derived_name.chars().count() > 80 {
        return Err(ApiError::validation(
            "自动生成的账户名称不能超过 80 个字符，请填写较短的自定义名称",
        ));
    }
    Ok((derived_name.to_owned(), AccountNameSource::Derived))
}

fn validate_account_details(
    account_type: &AccountType,
    bank_name: Option<&str>,
    branch_name: Option<&str>,
    card_number: Option<&str>,
    nickname: Option<&str>,
    phone: Option<&str>,
    email: Option<&str>,
) -> Result<LedgerAccountDetails, ApiError> {
    let mut details = LedgerAccountDetails {
        bank_name: None,
        branch_name: None,
        card_number: None,
        nickname: None,
        phone: None,
        email: None,
    };

    match account_type {
        AccountType::BankCard => {
            details.bank_name = normalize_optional_text(bank_name, 80, "银行名称")?;
            details.branch_name = normalize_optional_text(branch_name, 120, "开户行")?;
            details.card_number = normalize_optional_card_number(card_number)?;
        }
        AccountType::WechatBalance => {
            details.nickname = normalize_optional_text(nickname, 80, "微信昵称")?;
            details.phone = validate_optional_phone(phone)?;
        }
        AccountType::AlipayBalance => {
            details.nickname = normalize_optional_text(nickname, 80, "支付宝昵称")?;
            details.phone = validate_optional_phone(phone)?;
            details.email = normalize_optional_email(email)?;
        }
        AccountType::Cash | AccountType::DigitalCny | AccountType::Other => {}
    }

    Ok(details)
}

fn normalize_optional_card_number(value: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let normalized: String = value
        .chars()
        .filter(|character| character.is_ascii_digit())
        .collect();
    if value
        .chars()
        .any(|character| !character.is_ascii_digit() && character != ' ' && character != '-')
        || !(12..=23).contains(&normalized.len())
    {
        return Err(ApiError::validation(
            "银行卡号须包含 12–23 位数字，仅可使用数字、空格或短横线",
        ));
    }
    Ok(Some(normalized))
}

fn normalize_optional_text(
    value: Option<&str>,
    max_chars: usize,
    label: &str,
) -> Result<Option<String>, ApiError> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.chars().count() > max_chars || value.chars().any(char::is_control) {
        return Err(ApiError::validation(format!(
            "{label}不能超过 {max_chars} 个字符，且不能包含控制字符"
        )));
    }
    Ok(Some(value.to_owned()))
}

fn validate_optional_phone(value: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let valid_characters = value.chars().enumerate().all(|(index, character)| {
        character.is_ascii_digit()
            || character == ' '
            || character == '-'
            || (index == 0 && character == '+')
    });
    let digit_count = value
        .chars()
        .filter(|character| character.is_ascii_digit())
        .count();
    if value.chars().count() > 64 || !valid_characters || !(7..=20).contains(&digit_count) {
        return Err(ApiError::validation(
            "手机号须包含 7–20 位数字、总长不超过 64 个字符，仅可使用开头的 +、空格或短横线",
        ));
    }
    Ok(Some(value.to_owned()))
}

fn normalize_optional_email(value: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    Ok(Some(validate_email(value)?))
}

#[cfg(test)]
mod tests {
    use super::{
        AccountNameSource, AccountType, LedgerAccountDetails, resolve_account_name,
        validate_account_details,
    };

    #[test]
    fn details_are_trimmed_normalized_and_scoped_to_account_type() {
        let alipay = validate_account_details(
            &AccountType::AlipayBalance,
            Some("不应保留的银行"),
            Some("不应保留的开户行"),
            Some("6222 0000 0000 1234"),
            Some("  小余  "),
            Some(" +86 138-0013-8000 "),
            Some(" USER@Example.com "),
        )
        .unwrap();
        assert_eq!(
            alipay,
            LedgerAccountDetails {
                bank_name: None,
                branch_name: None,
                card_number: None,
                nickname: Some("小余".to_owned()),
                phone: Some("+86 138-0013-8000".to_owned()),
                email: Some("user@example.com".to_owned()),
            }
        );

        let cash = validate_account_details(
            &AccountType::Cash,
            Some("不应保留的银行"),
            Some("不应保留的开户行"),
            Some("6222 0000 0000 1234"),
            Some("不应保留的昵称"),
            Some("not-a-phone"),
            Some("not-an-email"),
        )
        .unwrap();
        assert_eq!(
            cash,
            LedgerAccountDetails {
                bank_name: None,
                branch_name: None,
                card_number: None,
                nickname: None,
                phone: None,
                email: None,
            }
        );
    }

    #[test]
    fn applicable_phone_and_email_are_validated() {
        let invalid_phone = validate_account_details(
            &AccountType::WechatBalance,
            None,
            None,
            None,
            None,
            Some("123456"),
            None,
        );
        assert!(invalid_phone.is_err());

        let too_long_phone = format!("1234567{}", "-".repeat(58));
        let too_long_phone = validate_account_details(
            &AccountType::WechatBalance,
            None,
            None,
            None,
            None,
            Some(&too_long_phone),
            None,
        );
        assert!(too_long_phone.is_err());

        let invalid_email = validate_account_details(
            &AccountType::AlipayBalance,
            None,
            None,
            None,
            None,
            None,
            Some("not-an-email"),
        );
        assert!(invalid_email.is_err());
    }

    #[test]
    fn blank_names_are_derived_from_type_specific_details_in_priority_order() {
        let bank = validate_account_details(
            &AccountType::BankCard,
            Some("招商银行"),
            Some("上海世纪大道支行"),
            Some("6222 0000 0000-1234"),
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            resolve_account_name("  ", &AccountType::BankCard, &bank).unwrap(),
            ("6222000000001234".to_owned(), AccountNameSource::Derived)
        );

        let wechat = validate_account_details(
            &AccountType::WechatBalance,
            None,
            None,
            None,
            Some("兔子"),
            Some("13800138000"),
            None,
        )
        .unwrap();
        assert_eq!(
            resolve_account_name("", &AccountType::WechatBalance, &wechat).unwrap(),
            ("兔子".to_owned(), AccountNameSource::Derived)
        );

        let alipay = validate_account_details(
            &AccountType::AlipayBalance,
            None,
            None,
            None,
            None,
            Some("13800138000"),
            Some("user@example.com"),
        )
        .unwrap();
        assert_eq!(
            resolve_account_name("", &AccountType::AlipayBalance, &alipay).unwrap(),
            ("13800138000".to_owned(), AccountNameSource::Derived)
        );

        assert_eq!(
            resolve_account_name("  日常零钱  ", &AccountType::WechatBalance, &wechat).unwrap(),
            ("日常零钱".to_owned(), AccountNameSource::Custom)
        );
    }

    #[test]
    fn blank_names_require_usable_details_and_derived_names_fit_the_name_limit() {
        let empty_wechat = validate_account_details(
            &AccountType::WechatBalance,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(resolve_account_name("", &AccountType::WechatBalance, &empty_wechat).is_err());
        assert!(resolve_account_name("", &AccountType::Cash, &empty_wechat).is_err());

        let long_branch = "支".repeat(81);
        let bank = validate_account_details(
            &AccountType::BankCard,
            None,
            Some(&long_branch),
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(resolve_account_name("", &AccountType::BankCard, &bank).is_err());

        let invalid_card = validate_account_details(
            &AccountType::BankCard,
            None,
            None,
            Some("6214 8622 x624 4444"),
            None,
            None,
            None,
        );
        assert!(invalid_card.is_err());
    }
}
