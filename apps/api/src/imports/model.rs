use thiserror::Error;

pub const MAX_IMPORT_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_IMPORT_RECORDS: usize = 100_000;
pub const MAX_EXTERNAL_ID_CHARS: usize = 256;
pub const MAX_STATUS_CHARS: usize = 128;
pub const MAX_DESCRIPTION_CHARS: usize = 4096;
pub const MAX_AMOUNT_CENTS: i64 = 9_007_199_254_740_991;
pub const NORMALIZATION_VERSION: i64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceChannel {
    Alipay,
    Wechat,
    Cmb,
    Cmbc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Income,
    Expense,
    Neutral,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseDisposition {
    Import,
    Pending,
    Neutral,
    Closed,
    ZeroAmount,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredDisposition {
    Import,
    Pending,
    Neutral,
    Closed,
    ZeroAmount,
    Unknown,
    Duplicate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedRecord {
    pub row_index: i64,
    pub external_id: String,
    pub merchant_order_id: String,
    pub occurred_at: String,
    pub occurred_on: String,
    pub direction: Direction,
    pub amount_cents: i64,
    pub channel_category: String,
    pub counterparty: String,
    pub product: String,
    pub pay_method: String,
    pub channel_status: String,
    pub source_note: String,
    pub counterparty_account_raw: String,
    pub occurred_at_precision: String,
    pub currency: String,
    pub external_id_source: String,
    pub counter_channel_raw: String,
    pub balance_after_cents: Option<i64>,
    pub raw_json: String,
    pub disposition: BaseDisposition,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("{code}: {message}")]
pub struct ImportParseError {
    pub code: &'static str,
    pub message: String,
}

impl ImportParseError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

pub(crate) fn validate_char_limit(
    value: &str,
    max: usize,
    field: &str,
    row: i64,
) -> Result<(), ImportParseError> {
    if value.chars().count() > max {
        return Err(ImportParseError::new(
            "import_field_too_long",
            format!("第 {row} 行字段 {field} 超过 {max} 个字符"),
        ));
    }
    Ok(())
}

pub(crate) fn truncated(value: &str) -> String {
    const MAX: usize = 80;
    let mut result: String = value.chars().take(MAX).collect();
    if value.chars().count() > MAX {
        result.push('…');
    }
    result
}

#[cfg(test)]
mod tests {
    use crate::domain::normalize_counterparty;

    #[test]
    fn normalizes_payment_platform_width_but_preserves_other_text() {
        assert_eq!(
            normalize_counterparty("alipay", "　ＡＢＣ１２３　"),
            "ABC123"
        );
        assert_eq!(
            normalize_counterparty("wechat", " 商户（分店） "),
            "商户(分店)"
        );
    }

    #[test]
    fn cmb_only_strips_a_long_numeric_bearing_ascii_suffix() {
        for (input, expected) in [
            ("AppStore_AppleMu18153357", "AppStore_AppleMu18153357"),
            (
                "AppStore_AppleMusic208842184337289",
                "AppStore_AppleMusic208842184337289",
            ),
            ("丰e足食-智慧零售WH1709094196", "丰e足食-智慧零售"),
            ("停车云管家580035495", "停车云管家"),
            ("支付宝小荷包-自动攒215500690", "支付宝小荷包-自动攒"),
            ("7分甜", "7分甜"),
            ("85度C", "85度C"),
            ("1号会员店", "1号会员店"),
            ("商户ABCDEF", "商户ABCDEF"),
            ("店123456", "店123456"),
        ] {
            assert_eq!(normalize_counterparty("cmb", input), expected, "{input}");
        }
    }

    #[test]
    fn cmb_keeps_values_when_stripping_would_leave_a_connector() {
        for connector in ['_', '-', '·', '—', '/'] {
            let input = format!("商户{connector}Account123456");
            assert_eq!(normalize_counterparty("cmb", &input), input);
        }
    }

    #[test]
    fn cmbc_keeps_the_already_split_counterparty_verbatim() {
        assert_eq!(
            normalize_counterparty("cmbc", "  对方户名  "),
            "  对方户名  "
        );
    }
}
