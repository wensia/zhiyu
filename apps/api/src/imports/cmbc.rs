use std::{cmp::Ordering, collections::HashMap};

use chrono::NaiveDateTime;
use regex::Regex;
use sha2::{Digest, Sha256};

use super::{
    cmb::{direction_from_signed, fingerprint, parse_signed_cents, validate_balance_chain},
    model::{
        BaseDisposition, ImportParseError, MAX_AMOUNT_CENTS, MAX_DESCRIPTION_CHARS,
        MAX_IMPORT_RECORDS, ParsedRecord, validate_char_limit,
    },
    pdf::{Word, extract_words},
};

const HEADERS: [&str; 11] = [
    "凭证类型",
    "凭证号码",
    "交易时间",
    "摘要",
    "交易金额",
    "账户余额",
    "现转标志",
    "交易渠道",
    "交易机构",
    "对方户名/账号",
    "对方行名",
];
const HEADER_Y_TOLERANCE: f64 = 6.0;
const COLUMN_X_TOLERANCE: f64 = 6.0;
const MIN_LAST_RECORD_MARGIN: f64 = 12.0;
const LAST_RECORD_MARGIN: f64 = 48.0;

#[derive(Debug)]
struct Header {
    page: usize,
    y: f64,
    columns: Vec<(&'static str, f64)>,
}

#[derive(Debug)]
struct RowCells {
    page: usize,
    page_sequence: usize,
    columns: HashMap<&'static str, String>,
}

pub fn parse_cmbc_pdf(bytes: &[u8]) -> Result<Vec<ParsedRecord>, ImportParseError> {
    let words = extract_words(bytes)?;
    recompose(&words)
}

pub(crate) fn recompose(words: &[Word]) -> Result<Vec<ParsedRecord>, ImportParseError> {
    let headers = find_headers(words)?;
    let rows = collect_rows(words, &headers)?;
    if rows.len() > MAX_IMPORT_RECORDS {
        return Err(ImportParseError::new(
            "import_resource_limit",
            format!("数据记录超过 {MAX_IMPORT_RECORDS} 条"),
        ));
    }
    let mut expected_account_hash = None;
    for (index, row) in rows.iter().enumerate() {
        let credential = row
            .columns
            .get("凭证号码")
            .map_or("", String::as_str)
            .trim();
        if credential.is_empty() {
            continue;
        }
        let account_hash = format!("{:x}", Sha256::digest(credential.as_bytes()));
        match &expected_account_hash {
            None => expected_account_hash = Some(account_hash),
            Some(expected) if expected == &account_hash => {}
            Some(_) => {
                return Err(ImportParseError::new(
                    "multiple_bank_accounts",
                    format!("第 {} 条账单账户标识与首条不一致", index + 1),
                ));
            }
        }
    }
    let account_hash = expected_account_hash.ok_or_else(|| {
        ImportParseError::new(
            "missing_bank_account_identifier",
            "民生银行 PDF 未找到可用于指纹的账单账户标识",
        )
    })?;
    let mut records = Vec::with_capacity(rows.len());
    for (index, row) in rows.into_iter().enumerate() {
        records.push(parse_row(row, index as i64 + 1, &account_hash)?);
    }
    validate_balance_chain(&records)?;
    Ok(records)
}

fn find_headers(words: &[Word]) -> Result<Vec<Header>, ImportParseError> {
    let mut headers = Vec::new();
    for anchor in words.iter().filter(|word| word.text.trim() == HEADERS[0]) {
        let mut columns = Vec::with_capacity(HEADERS.len());
        for name in HEADERS {
            let matches: Vec<_> = words
                .iter()
                .filter(|word| {
                    word.page == anchor.page
                        && word.text.trim() == name
                        && (word.y_center() - anchor.y_center()).abs() <= HEADER_Y_TOLERANCE
                })
                .collect();
            if matches.len() != 1 {
                columns.clear();
                break;
            }
            columns.push((name, matches[0].x_min));
        }
        if columns.len() == HEADERS.len() {
            columns.sort_by(|left, right| float_order(left.1, right.1));
            headers.push(Header {
                page: anchor.page,
                y: anchor.y_center(),
                columns,
            });
        }
    }
    headers.sort_by_key(|header| header.page);
    headers.dedup_by_key(|header| header.page);
    if headers.is_empty() {
        return Err(ImportParseError::new(
            "invalid_import_header",
            "民生银行 PDF 未找到完整交易表头",
        ));
    }
    Ok(headers)
}

fn collect_rows(words: &[Word], headers: &[Header]) -> Result<Vec<RowCells>, ImportParseError> {
    let date_re = Regex::new(r"^\d{4}/\d{2}/\d{2}$").expect("valid CMBC date regex");
    let mut rows = Vec::new();
    for header in headers {
        let mut anchors: Vec<_> = words
            .iter()
            .filter(|word| {
                word.page == header.page
                    && word.y_center() > header.y
                    && date_re.is_match(word.text.trim())
                    && column_for(word.x_min, &header.columns) == Some("交易时间")
            })
            .collect();
        anchors.sort_by(|left, right| float_order(left.y_center(), right.y_center()));
        if anchors.is_empty() {
            continue;
        }
        let first_boundary = (header.y + anchors[0].y_center()) / 2.0;
        let last_record_margin = anchors
            .windows(2)
            .last()
            .map(|pair| (pair[1].y_center() - pair[0].y_center()) / 2.0)
            .unwrap_or(LAST_RECORD_MARGIN)
            .clamp(MIN_LAST_RECORD_MARGIN, LAST_RECORD_MARGIN);
        let last_boundary = anchors.last().unwrap().y_center() + last_record_margin;
        let midpoints: Vec<_> = anchors
            .windows(2)
            .map(|pair| (pair[0].y_center() + pair[1].y_center()) / 2.0)
            .collect();
        let mut buckets = vec![Vec::<&Word>::new(); anchors.len()];
        for word in words.iter().filter(|word| word.page == header.page) {
            let y = word.y_center();
            if y < first_boundary || y > last_boundary || is_header_word(word, header) {
                continue;
            }
            let record_index = midpoints.partition_point(|midpoint| y >= *midpoint);
            buckets[record_index].push(word);
        }
        for (page_sequence, bucket) in buckets.into_iter().enumerate() {
            let mut columns: HashMap<&'static str, Vec<&Word>> = HashMap::new();
            for word in bucket {
                if let Some(column) = column_for(word.x_min, &header.columns) {
                    columns.entry(column).or_default().push(word);
                }
            }
            rows.push(RowCells {
                page: header.page,
                page_sequence: page_sequence + 1,
                columns: columns
                    .into_iter()
                    .map(|(name, words)| (name, join_words(words, name == "交易时间")))
                    .collect(),
            });
        }
    }
    if rows.is_empty() {
        return Err(ImportParseError::new(
            "empty_import_file",
            "民生银行 PDF 表头后没有交易记录",
        ));
    }
    Ok(rows)
}

fn parse_row(
    row: RowCells,
    row_index: i64,
    account_hash: &str,
) -> Result<ParsedRecord, ImportParseError> {
    let field = |name: &'static str| row.columns.get(name).map_or("", String::as_str).trim();
    let occurred_raw = field("交易时间");
    let occurred =
        NaiveDateTime::parse_from_str(occurred_raw, "%Y/%m/%d %H:%M:%S").map_err(|_| {
            ImportParseError::new(
                "invalid_import_datetime",
                format!("第 {row_index} 条字段 交易时间 无效"),
            )
        })?;
    let signed_amount = parse_signed_cents(field("交易金额"), "交易金额", row_index)?;
    let balance = parse_signed_cents(field("账户余额"), "账户余额", row_index)?;
    let amount_cents = signed_amount.checked_abs().ok_or_else(|| {
        ImportParseError::new(
            "invalid_import_amount",
            format!("第 {row_index} 条字段 交易金额 金额无效"),
        )
    })?;
    if amount_cents > MAX_AMOUNT_CENTS {
        return Err(ImportParseError::new(
            "invalid_import_amount",
            format!("第 {row_index} 条字段 交易金额 金额无效"),
        ));
    }
    let summary = field("摘要").to_owned();
    let cash_transfer = field("现转标志").to_owned();
    let transaction_channel = field("交易渠道").to_owned();
    let counter_channel_raw = field("对方行名").to_owned();
    let (counterparty, counterparty_account_raw) = split_counterparty(field("对方户名/账号"));
    for (name, value) in [
        ("摘要", summary.as_str()),
        ("现转标志", cash_transfer.as_str()),
        ("交易渠道", transaction_channel.as_str()),
        ("对方户名", counterparty.as_str()),
        ("账号", counterparty_account_raw.as_str()),
        ("对方行名", counter_channel_raw.as_str()),
    ] {
        validate_char_limit(value, MAX_DESCRIPTION_CHARS, name, row_index)?;
    }
    let occurred_on = occurred.format("%Y-%m-%d").to_string();
    let external_id = fingerprint(
        "cmbc",
        account_hash,
        &occurred_on,
        amount_cents,
        balance,
        row.page,
        row.page_sequence,
    );
    let raw_json = allowlisted_raw_json(&summary, &cash_transfer, &transaction_channel);
    Ok(ParsedRecord {
        row_index,
        external_id,
        merchant_order_id: String::new(),
        occurred_at: occurred.format("%Y-%m-%d %H:%M:%S").to_string(),
        occurred_on,
        direction: direction_from_signed(signed_amount),
        amount_cents,
        channel_category: String::new(),
        counterparty,
        product: summary,
        pay_method: String::new(),
        channel_status: String::new(),
        source_note: String::new(),
        counterparty_account_raw,
        occurred_at_precision: "second".to_owned(),
        currency: "CNY".to_owned(),
        external_id_source: "fingerprint".to_owned(),
        counter_channel_raw,
        balance_after_cents: Some(balance),
        raw_json,
        disposition: if amount_cents == 0 {
            BaseDisposition::ZeroAmount
        } else {
            BaseDisposition::Import
        },
    })
}

fn split_counterparty(value: &str) -> (String, String) {
    value
        .trim()
        .rsplit_once('/')
        .map(|(name, account)| (name.trim().to_owned(), account.trim().to_owned()))
        .unwrap_or_else(|| (value.trim().to_owned(), String::new()))
}

fn allowlisted_raw_json(summary: &str, cash_transfer: &str, channel: &str) -> String {
    let mut raw = serde_json::Map::new();
    for (key, value) in [
        ("摘要", summary),
        ("现转标志", cash_transfer),
        ("交易渠道", channel),
    ] {
        if !value.is_empty() {
            raw.insert(key.to_owned(), value.into());
        }
    }
    serde_json::Value::Object(raw).to_string()
}

fn column_for(x_min: f64, columns: &[(&'static str, f64)]) -> Option<&'static str> {
    columns
        .iter()
        .rev()
        .find(|(_, boundary)| *boundary <= x_min + COLUMN_X_TOLERANCE)
        .map(|(name, _)| *name)
}

fn is_header_word(word: &Word, header: &Header) -> bool {
    (word.y_center() - header.y).abs() <= HEADER_Y_TOLERANCE && HEADERS.contains(&word.text.trim())
}

fn join_words(mut words: Vec<&Word>, separate_lines: bool) -> String {
    words.sort_by(|left, right| {
        float_order(left.y_center(), right.y_center())
            .then_with(|| float_order(left.x_min, right.x_min))
    });
    let mut result = String::new();
    let mut previous_y = None;
    for word in words {
        if separate_lines
            && previous_y.is_some_and(|y: f64| (word.y_center() - y).abs() > 2.0)
            && !result.is_empty()
        {
            result.push(' ');
        }
        result.push_str(word.text.trim());
        previous_y = Some(word.y_center());
    }
    result
}

fn float_order(left: f64, right: f64) -> Ordering {
    left.partial_cmp(&right).unwrap_or(Ordering::Equal)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(x: f64, y: f64, text: &str) -> Word {
        Word {
            page: 1,
            x_min: x,
            y_min: y - 4.0,
            y_max: y + 4.0,
            text: text.to_owned(),
        }
    }

    fn fixture(second_balance: &str) -> Vec<Word> {
        let xs = [
            10.0, 60.0, 120.0, 205.0, 300.0, 365.0, 430.0, 485.0, 545.0, 610.0, 720.0,
        ];
        let mut words = Vec::new();
        for (name, x) in HEADERS.into_iter().zip(xs) {
            words.push(word(x, 20.0, name));
        }
        for (y, date, time, amount, balance, party) in [
            (
                50.0,
                "2026/01/01",
                "10:00:00",
                "+10.00",
                "100.00",
                "研发/一组/62250001",
            ),
            (
                100.0,
                "2026/01/01",
                "10:00:01",
                "+2.00",
                second_balance,
                "商户/62250002",
            ),
        ] {
            words.extend([
                word(xs[0], y, "转账"),
                word(xs[1], y, "6225****1234"),
                word(xs[2], y - 3.0, date),
                word(xs[2], y + 7.0, time),
                word(xs[3], y, "财付通-快捷支付-虚构商户"),
                word(xs[4], y, amount),
                word(xs[5], y, balance),
                word(xs[6], y, "现转"),
                word(xs[7], y, "手机银行"),
                word(xs[8], y, "虚构机构"),
                word(xs[9], y, party),
                word(xs[10], y, "中国银联"),
            ]);
        }
        words
    }

    #[test]
    fn parses_seconds_last_slash_and_allowlist() {
        let records = recompose(&fixture("102.00")).unwrap();
        assert_eq!(records[0].occurred_at, "2026-01-01 10:00:00");
        assert_eq!(records[0].occurred_at_precision, "second");
        assert_eq!(records[0].counterparty, "研发/一组");
        assert_eq!(records[0].counterparty_account_raw, "62250001");
        assert_eq!(records[0].counter_channel_raw, "中国银联");
        let raw: serde_json::Value = serde_json::from_str(&records[0].raw_json).unwrap();
        assert_eq!(
            raw.as_object().unwrap().keys().cloned().collect::<Vec<_>>(),
            ["交易渠道", "摘要", "现转标志"]
        );
        assert!(!records[0].raw_json.contains("6225"));
        assert!(!records[0].raw_json.contains("中国银联"));
    }

    #[test]
    fn uses_the_statement_account_when_a_record_credential_cell_is_blank() {
        let mut words = fixture("102.00");
        let second_credential = words
            .iter()
            .enumerate()
            .filter(|(_, word)| word.text == "6225****1234")
            .nth(1)
            .map(|(index, _)| index)
            .unwrap();
        words.remove(second_credential);

        let records = recompose(&words).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].external_id.len(), 64);
        assert_eq!(records[1].external_id.len(), 64);
    }

    #[test]
    fn ignores_page_footer_after_the_last_record() {
        let mut words = fixture("102.00");
        words.push(word(120.0, 148.0, "第 1 页 / 共 2 页"));

        let records = recompose(&words).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].occurred_at, "2026-01-01 10:00:01");
    }

    #[test]
    fn broken_balance_chain_reports_position_and_values() {
        let error = recompose(&fixture("101.00")).unwrap_err();
        assert!(error.message.contains("第 2 条"));
        assert!(error.message.contains("10200"));
        assert!(error.message.contains("10100"));
    }
}
