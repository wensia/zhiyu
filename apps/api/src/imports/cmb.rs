use std::{cmp::Ordering, collections::HashMap, sync::OnceLock};

use chrono::NaiveDate;
use regex::Regex;
use sha2::{Digest, Sha256};

use super::{
    model::{
        BaseDisposition, Direction, ImportParseError, MAX_AMOUNT_CENTS, MAX_DESCRIPTION_CHARS,
        MAX_IMPORT_RECORDS, ParsedRecord, validate_char_limit,
    },
    pdf::{Word, extract_words},
};

const HEADERS: [&str; 6] = [
    "记账日期",
    "货币",
    "交易金额",
    "联机余额",
    "交易摘要",
    "对手信息",
];
const ENGLISH_HEADERS: [&str; 8] = [
    "Date",
    "Currency",
    "Transaction",
    "Amount",
    "Balance",
    "Type",
    "Counter",
    "Party",
];
const HEADER_Y_TOLERANCE: f64 = 6.0;
const HEADER_BLOCK_HEIGHT: f64 = 40.0;
const COLUMN_X_TOLERANCE: f64 = 6.0;
const LAST_RECORD_MARGIN: f64 = 48.0;

#[derive(Debug)]
struct Header {
    page: usize,
    y: f64,
    bottom_y: f64,
    columns: Vec<(&'static str, f64)>,
}

#[derive(Debug)]
struct RowCells {
    page: usize,
    page_sequence: usize,
    columns: HashMap<&'static str, String>,
}

pub fn parse_cmb_pdf(bytes: &[u8]) -> Result<Vec<ParsedRecord>, ImportParseError> {
    let words = extract_words(bytes)?;
    recompose(&words)
}

pub(crate) fn recompose(words: &[Word]) -> Result<Vec<ParsedRecord>, ImportParseError> {
    let headers = find_headers(words)?;
    let account_identifier = find_account_identifier(words, &headers)?;
    let account_hash = format!("{:x}", Sha256::digest(account_identifier.as_bytes()));
    let rows = collect_rows(words, &headers)?;
    if rows.len() > MAX_IMPORT_RECORDS {
        return Err(ImportParseError::new(
            "import_resource_limit",
            format!("数据记录超过 {MAX_IMPORT_RECORDS} 条"),
        ));
    }

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
            let bottom_y = words
                .iter()
                .filter(|word| {
                    word.page == anchor.page
                        && word.y_center() >= anchor.y_center() - HEADER_Y_TOLERANCE
                        && word.y_center() <= anchor.y_center() + HEADER_BLOCK_HEIGHT
                        && (HEADERS.contains(&word.text.trim())
                            || ENGLISH_HEADERS.contains(&word.text.trim()))
                })
                .map(Word::y_center)
                .max_by(|left, right| float_order(*left, *right))
                .unwrap_or_else(|| anchor.y_center());
            headers.push(Header {
                page: anchor.page,
                y: anchor.y_center(),
                bottom_y,
                columns,
            });
        }
    }
    headers.sort_by_key(|header| header.page);
    headers.dedup_by_key(|header| header.page);
    if headers.is_empty() {
        return Err(ImportParseError::new(
            "invalid_import_header",
            "招商银行 PDF 未找到完整交易表头",
        ));
    }
    Ok(headers)
}

fn find_account_identifier(words: &[Word], headers: &[Header]) -> Result<String, ImportParseError> {
    for label in words.iter().filter(|word| {
        let text = word.text.trim();
        (text.contains("账号") || text.contains("卡号") || text.contains("账户"))
            && !HEADERS.contains(&text)
    }) {
        if let Some(value) = identifier_in_text(label.text.trim()) {
            return Ok(value);
        }
        let header_y = headers
            .iter()
            .find(|header| header.page == label.page)
            .map_or(f64::INFINITY, |header| header.y);
        let mut candidates: Vec<_> = words
            .iter()
            .filter(|word| {
                word.page == label.page
                    && word.x_min > label.x_min
                    && word.y_center() < header_y
                    && (word.y_center() - label.y_center()).abs() <= HEADER_Y_TOLERANCE
            })
            .collect();
        candidates.sort_by(|left, right| float_order(left.x_min, right.x_min));
        for candidate in candidates {
            if let Some(value) = identifier_in_text(candidate.text.trim()) {
                return Ok(value);
            }
        }
    }
    Err(ImportParseError::new(
        "missing_bank_account_identifier",
        "招商银行 PDF 未找到可用于指纹的账单账户标识",
    ))
}

fn identifier_in_text(value: &str) -> Option<String> {
    static IDENTIFIER_RE: OnceLock<Regex> = OnceLock::new();
    let re = IDENTIFIER_RE.get_or_init(|| {
        Regex::new(r"[0-9Xx*][0-9Xx* -]{3,}[0-9Xx*]").expect("valid account identifier regex")
    });
    re.find(value).map(|matched| {
        matched
            .as_str()
            .chars()
            .filter(|character| !matches!(character, ' ' | '-'))
            .flat_map(char::to_uppercase)
            .collect()
    })
}

fn collect_rows(words: &[Word], headers: &[Header]) -> Result<Vec<RowCells>, ImportParseError> {
    static DATE_RE: OnceLock<Regex> = OnceLock::new();
    let date_re =
        DATE_RE.get_or_init(|| Regex::new(r"^\d{4}-\d{2}-\d{2}$").expect("valid CMB date regex"));
    let mut rows = Vec::new();
    for header in headers {
        let mut anchors: Vec<_> = words
            .iter()
            .filter(|word| {
                word.page == header.page
                    && word.y_center() > header.bottom_y
                    && date_re.is_match(word.text.trim())
                    && column_for(word.x_min, &header.columns) == Some("记账日期")
            })
            .collect();
        anchors.sort_by(|left, right| float_order(left.y_center(), right.y_center()));
        if anchors.is_empty() {
            continue;
        }
        let first_boundary = (header.bottom_y + anchors[0].y_center()) / 2.0;
        let last_boundary = anchors.last().unwrap().y_center() + LAST_RECORD_MARGIN;
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
                    .map(|(name, words)| (name, join_words(words, false)))
                    .collect(),
            });
        }
    }
    if rows.is_empty() {
        return Err(ImportParseError::new(
            "empty_import_file",
            "招商银行 PDF 表头后没有交易记录",
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
    let occurred_on = field("记账日期");
    let date = NaiveDate::parse_from_str(occurred_on, "%Y-%m-%d").map_err(|_| {
        ImportParseError::new(
            "invalid_import_datetime",
            format!("第 {row_index} 条字段 记账日期 无效"),
        )
    })?;
    let signed_amount = parse_signed_cents(field("交易金额"), "交易金额", row_index)?;
    let balance = parse_signed_cents(field("联机余额"), "联机余额", row_index)?;
    let amount_cents = signed_amount
        .checked_abs()
        .ok_or_else(|| invalid_amount(row_index, "交易金额"))?;
    let direction = direction_from_signed(signed_amount);
    let summary = field("交易摘要").to_owned();
    let counterparty = field("对手信息").to_owned();
    let currency = normalize_currency(field("货币"));
    for (name, value) in [("交易摘要", &summary), ("对手信息", &counterparty)] {
        validate_char_limit(value, MAX_DESCRIPTION_CHARS, name, row_index)?;
    }
    let raw_json = allowlisted_raw_json(&summary);
    let external_id = fingerprint(
        "cmb",
        account_hash,
        occurred_on,
        amount_cents,
        balance,
        row.page,
        row.page_sequence,
    );
    Ok(ParsedRecord {
        row_index,
        external_id,
        merchant_order_id: String::new(),
        occurred_at: date
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
        occurred_on: occurred_on.to_owned(),
        direction,
        amount_cents,
        channel_category: String::new(),
        counterparty,
        product: summary,
        pay_method: String::new(),
        channel_status: String::new(),
        source_note: String::new(),
        counterparty_account_raw: String::new(),
        occurred_at_precision: "day".to_owned(),
        currency,
        external_id_source: "fingerprint".to_owned(),
        counter_channel_raw: String::new(),
        balance_after_cents: Some(balance),
        raw_json,
        disposition: if amount_cents == 0 {
            BaseDisposition::ZeroAmount
        } else {
            BaseDisposition::Import
        },
    })
}

fn column_for(x_min: f64, columns: &[(&'static str, f64)]) -> Option<&'static str> {
    columns
        .iter()
        .rev()
        .find(|(_, boundary)| *boundary <= x_min + COLUMN_X_TOLERANCE)
        .map(|(name, _)| *name)
}

fn is_header_word(word: &Word, header: &Header) -> bool {
    word.y_center() >= header.y - HEADER_Y_TOLERANCE
        && word.y_center() <= header.bottom_y
        && (HEADERS.contains(&word.text.trim()) || ENGLISH_HEADERS.contains(&word.text.trim()))
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

pub(super) fn parse_signed_cents(
    value: &str,
    field: &str,
    row: i64,
) -> Result<i64, ImportParseError> {
    let compact = value.trim().replace(',', "");
    let negative = compact.starts_with('-');
    let unsigned = compact.strip_prefix(['-', '+']).unwrap_or(&compact);
    let (yuan, fraction) = match unsigned.split_once('.') {
        Some((yuan, fraction)) if !yuan.is_empty() && (1..=2).contains(&fraction.len()) => {
            (yuan, fraction)
        }
        None if !unsigned.is_empty() => (unsigned, ""),
        _ => return Err(invalid_amount(row, field)),
    };
    if !yuan.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(invalid_amount(row, field));
    }
    let yuan = yuan
        .parse::<i64>()
        .map_err(|_| invalid_amount(row, field))?;
    let fraction = match fraction.len() {
        0 => 0,
        1 => {
            fraction
                .parse::<i64>()
                .map_err(|_| invalid_amount(row, field))?
                * 10
        }
        _ => fraction
            .parse::<i64>()
            .map_err(|_| invalid_amount(row, field))?,
    };
    let cents = yuan
        .checked_mul(100)
        .and_then(|value| value.checked_add(fraction))
        .filter(|value| *value <= MAX_AMOUNT_CENTS)
        .ok_or_else(|| invalid_amount(row, field))?;
    Ok(if negative { -cents } else { cents })
}

fn invalid_amount(row: i64, field: &str) -> ImportParseError {
    ImportParseError::new(
        "invalid_import_amount",
        format!("第 {row} 条字段 {field} 金额无效"),
    )
}

pub(super) fn direction_from_signed(amount: i64) -> Direction {
    match amount.cmp(&0) {
        Ordering::Less => Direction::Expense,
        Ordering::Greater => Direction::Income,
        Ordering::Equal => Direction::Neutral,
    }
}

fn normalize_currency(value: &str) -> String {
    match value.trim() {
        "人民币" | "RMB" | "CNY" => "CNY".to_owned(),
        other => other.to_owned(),
    }
}

fn allowlisted_raw_json(summary: &str) -> String {
    let mut raw = serde_json::Map::new();
    if !summary.is_empty() {
        raw.insert("交易摘要".to_owned(), summary.into());
    }
    serde_json::Value::Object(raw).to_string()
}

pub(super) fn fingerprint(
    source_channel: &str,
    account_hash: &str,
    occurred_on: &str,
    amount_cents: i64,
    balance_after_cents: i64,
    page: usize,
    page_sequence: usize,
) -> String {
    let payload = format!(
        "{source_channel}\0{account_hash}\0{occurred_on}\0{amount_cents}\0{balance_after_cents}\0{page}:{page_sequence}"
    );
    format!("{:x}", Sha256::digest(payload.as_bytes()))
}

pub(super) fn validate_balance_chain(records: &[ParsedRecord]) -> Result<(), ImportParseError> {
    for (index, pair) in records.windows(2).enumerate() {
        let previous = pair[0]
            .balance_after_cents
            .expect("bank balance is required");
        let actual = pair[1]
            .balance_after_cents
            .expect("bank balance is required");
        let signed = match pair[1].direction {
            Direction::Expense => -pair[1].amount_cents,
            Direction::Income => pair[1].amount_cents,
            Direction::Neutral => 0,
        };
        let expected = previous
            .checked_add(signed)
            .ok_or_else(|| ImportParseError::new("invalid_balance_chain", "余额链计算溢出"))?;
        if actual != expected {
            return Err(ImportParseError::new(
                "invalid_balance_chain",
                format!(
                    "余额链在第 {} 条断裂：期望余额 {expected} 分，实际余额 {actual} 分（前一余额 {previous} 分，交易金额 {signed} 分）",
                    index + 2
                ),
            ));
        }
    }
    Ok(())
}

fn float_order(left: f64, right: f64) -> Ordering {
    left.partial_cmp(&right).unwrap_or(Ordering::Equal)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(page: usize, x: f64, y: f64, text: &str) -> Word {
        Word {
            page,
            x_min: x,
            y_min: y - 5.0,
            y_max: y + 5.0,
            text: text.to_owned(),
        }
    }

    fn fixture(boundaries: [f64; 6], second_balance: &str) -> Vec<Word> {
        let mut words = vec![word(1, 10.0, 5.0, "账号：6225 **** 1234")];
        for (name, x) in HEADERS.into_iter().zip(boundaries) {
            words.push(word(1, x, 20.0, name));
        }
        for (y, date, amount, balance, summary, counterparty) in [
            (50.0, "2026-01-01", "+10.00", "100.00", "第一笔", "甲"),
            (100.0, "2026-01-01", "+2.00", second_balance, "第二笔", "乙"),
        ] {
            words.extend([
                word(1, boundaries[0], y, date),
                word(1, boundaries[1], y, "人民币"),
                word(1, boundaries[2], y, amount),
                word(1, boundaries[3], y, balance),
                word(1, boundaries[4], y, summary),
                word(1, boundaries[5], y, counterparty),
            ]);
        }
        words
    }

    #[test]
    fn dynamic_columns_and_midpoint_row_assignment_work() {
        let boundaries = [37.0, 93.0, 151.0, 229.0, 318.0, 433.0];
        let mut words = fixture(boundaries, "102.00");
        let second_summary = words.iter_mut().find(|word| word.text == "第二笔").unwrap();
        second_summary.y_min = 75.0;
        second_summary.y_max = 85.0;
        let records = recompose(&words).unwrap();
        assert_eq!(records[1].product, "第二笔");
        assert_eq!(records[0].occurred_at_precision, "day");
        assert_eq!(records[0].occurred_at, "2026-01-01 00:00:00");
        assert_eq!(records[0].raw_json, r#"{"交易摘要":"第一笔"}"#);

        let shifted = fixture([61.0, 121.0, 204.0, 298.0, 401.0, 535.0], "102.00");
        assert_eq!(recompose(&shifted).unwrap()[1].counterparty, "乙");
    }

    #[test]
    fn ignores_statement_period_date_outside_the_date_column() {
        let boundaries = [37.0, 93.0, 151.0, 229.0, 318.0, 433.0];
        let mut words = fixture(boundaries, "102.00");
        words.extend([
            word(1, 238.9, 10.0, "2031-02-03"),
            word(1, 300.0, 10.0, "--"),
            word(1, 330.0, 10.0, "2031-03-04"),
        ]);

        let records = recompose(&words).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].occurred_on, "2026-01-01");
    }

    #[test]
    fn ignores_bilingual_second_header_row() {
        let boundaries = [37.0, 93.0, 151.0, 229.0, 318.0, 433.0];
        let mut words = fixture(boundaries, "102.00");
        words.extend([
            word(1, boundaries[0], 32.0, "Date"),
            word(1, boundaries[1], 32.0, "Currency"),
            word(1, boundaries[2], 28.0, "Transaction"),
            word(1, boundaries[2], 40.0, "Amount"),
            word(1, boundaries[3], 32.0, "Balance"),
            word(1, boundaries[4], 32.0, "Transaction"),
            word(1, 366.0, 32.0, "Type"),
            word(1, boundaries[5], 32.0, "Counter"),
            word(1, 470.0, 32.0, "Party"),
        ]);

        let records = recompose(&words).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].amount_cents, 1_000);
        assert_eq!(records[0].balance_after_cents, Some(10_000));
    }

    #[test]
    fn broken_balance_chain_is_diagnostic_and_atomic() {
        let error = recompose(&fixture(
            [40.0, 100.0, 160.0, 220.0, 300.0, 430.0],
            "101.00",
        ))
        .unwrap_err();
        assert_eq!(error.code, "invalid_balance_chain");
        assert!(error.message.contains("第 2 条"));
        assert!(error.message.contains("期望余额 10200 分"));
        assert!(error.message.contains("实际余额 10100 分"));
    }

    #[test]
    fn fingerprint_includes_page_sequence() {
        let first = fingerprint("cmb", "account-hash", "2026-01-01", 100, 1000, 1, 1);
        let second = fingerprint("cmb", "account-hash", "2026-01-01", 100, 1000, 1, 2);
        assert_ne!(first, second);
    }
}
