use std::collections::{HashMap, HashSet};

use chrono::NaiveDateTime;
use csv::StringRecord;
use encoding_rs::GB18030;

use super::model::{
    BaseDisposition, Direction, ImportParseError, MAX_AMOUNT_CENTS, MAX_DESCRIPTION_CHARS,
    MAX_EXTERNAL_ID_CHARS, MAX_IMPORT_BYTES, MAX_IMPORT_RECORDS, MAX_STATUS_CHARS, ParsedRecord,
    truncated, validate_char_limit,
};

const REQUIRED_HEADERS: [&str; 12] = [
    "交易时间",
    "交易分类",
    "交易对方",
    "对方账号",
    "商品说明",
    "收/支",
    "金额",
    "收/付款方式",
    "交易状态",
    "交易订单号",
    "商家订单号",
    "备注",
];

const SUCCESS_STATUSES: [&str; 5] = ["交易成功", "支付成功", "退款成功", "还款成功", "放款成功"];
const PENDING_STATUSES: [&str; 3] = ["等待发货", "等待对方确认收货", "等待确认收货"];

pub(super) fn header_email(bytes: &[u8]) -> Option<String> {
    let (decoded, _, had_errors) = GB18030.decode(bytes);
    if had_errors {
        return None;
    }
    decoded.lines().find_map(|line| {
        let (_, value) = line.trim().split_once("支付宝账户：")?;
        let value = value.trim().trim_matches(',').trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

pub fn parse_alipay_csv(bytes: &[u8]) -> Result<Vec<ParsedRecord>, ImportParseError> {
    if bytes.len() > MAX_IMPORT_BYTES {
        return Err(ImportParseError::new(
            "import_resource_limit",
            format!("文件超过 {MAX_IMPORT_BYTES} 字节"),
        ));
    }

    let (decoded, _, had_errors) = GB18030.decode(bytes);
    if had_errors {
        return Err(ImportParseError::new(
            "invalid_import_encoding",
            "文件不是有效的 GB18030 编码",
        ));
    }
    let decoded = decoded.strip_prefix('\u{feff}').unwrap_or(&decoded);
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(decoded.as_bytes());

    let mut header: Option<(HashMap<String, usize>, usize)> = None;
    let mut records = Vec::new();
    let mut external_ids = HashMap::<String, i64>::new();

    for result in reader.records() {
        let record = result.map_err(|error| {
            ImportParseError::new("invalid_import_csv", format!("CSV 解析失败: {error}"))
        })?;
        let row = record
            .position()
            .map_or(1, |position| position.line() as i64 + 1);

        if header.is_none() {
            if record.iter().any(|value| value.trim() == "交易时间") {
                header = Some(build_header_map(&record, row)?);
            }
            continue;
        }
        if record.iter().all(|value| value.trim().is_empty()) {
            continue;
        }
        if records.len() >= MAX_IMPORT_RECORDS {
            return Err(ImportParseError::new(
                "import_resource_limit",
                format!("非空数据记录超过 {MAX_IMPORT_RECORDS} 条"),
            ));
        }

        let (columns, header_width) = header.as_ref().expect("header was checked");
        if record.len() > *header_width {
            return Err(ImportParseError::new(
                "invalid_import_row",
                format!("第 {row} 行数据列数超过表头列数"),
            ));
        }
        let parsed = parse_record(&record, columns, row)?;
        if let Some(first_row) = external_ids.insert(parsed.external_id.clone(), row) {
            return Err(ImportParseError::new(
                "duplicate_import_external_id",
                format!(
                    "交易订单号 {} 在第 {first_row} 行和第 {row} 行重复",
                    truncated(&parsed.external_id)
                ),
            ));
        }
        records.push(parsed);
    }

    if header.is_none() {
        return Err(ImportParseError::new(
            "invalid_import_header",
            "找不到支付宝完整表头",
        ));
    }
    if records.is_empty() {
        return Err(ImportParseError::new(
            "empty_import_file",
            "表头后没有非空数据行",
        ));
    }
    Ok(records)
}

fn build_header_map(
    record: &StringRecord,
    row: i64,
) -> Result<(HashMap<String, usize>, usize), ImportParseError> {
    let mut columns = HashMap::new();
    let required: HashSet<&str> = REQUIRED_HEADERS.into_iter().collect();
    for (index, raw) in record.iter().enumerate() {
        let name = raw.trim();
        if required.contains(name) && columns.insert(name.to_owned(), index).is_some() {
            return Err(ImportParseError::new(
                "invalid_import_header",
                format!("第 {row} 行表头字段 {name} 重复"),
            ));
        }
    }
    let missing: Vec<_> = REQUIRED_HEADERS
        .iter()
        .filter(|name| !columns.contains_key(**name))
        .copied()
        .collect();
    if !missing.is_empty() {
        return Err(ImportParseError::new(
            "invalid_import_header",
            format!("第 {row} 行缺少表头字段: {}", missing.join(", ")),
        ));
    }
    Ok((columns, record.len()))
}

fn parse_record(
    record: &StringRecord,
    columns: &HashMap<String, usize>,
    row: i64,
) -> Result<ParsedRecord, ImportParseError> {
    let field = |name: &str| -> Result<&str, ImportParseError> {
        let index = columns[name];
        record.get(index).ok_or_else(|| {
            ImportParseError::new("invalid_import_row", format!("第 {row} 行缺少字段 {name}"))
        })
    };

    let occurred_at = field("交易时间")?.trim();
    let occurred =
        NaiveDateTime::parse_from_str(occurred_at, "%Y-%m-%d %H:%M:%S").map_err(|_| {
            ImportParseError::new(
                "invalid_import_datetime",
                format!("第 {row} 行字段 交易时间 无效: {}", truncated(occurred_at)),
            )
        })?;
    let direction_raw = field("收/支")?.trim();
    let direction = match direction_raw {
        "收入" => Direction::Income,
        "支出" => Direction::Expense,
        "不计收支" => Direction::Neutral,
        _ => {
            return Err(ImportParseError::new(
                "unknown_import_direction",
                format!("第 {row} 行字段 收/支 未知: {}", truncated(direction_raw)),
            ));
        }
    };
    let amount_cents = parse_amount(field("金额")?.trim(), row)?;
    let external_id = field("交易订单号")?.trim().to_owned();
    if external_id.is_empty() {
        return Err(ImportParseError::new(
            "empty_import_external_id",
            format!("第 {row} 行字段 交易订单号 为空"),
        ));
    }
    let merchant_order_id = field("商家订单号")?.trim().to_owned();
    let channel_status = field("交易状态")?.trim().to_owned();
    let channel_category = field("交易分类")?.trim().to_owned();
    let counterparty = field("交易对方")?.trim().to_owned();
    let counterparty_account_raw = normalize_optional(field("对方账号")?);
    let product = field("商品说明")?.trim().to_owned();
    let pay_method = field("收/付款方式")?.trim().to_owned();
    let source_note = field("备注")?.trim().to_owned();

    validate_char_limit(&external_id, MAX_EXTERNAL_ID_CHARS, "交易订单号", row)?;
    validate_char_limit(&merchant_order_id, MAX_EXTERNAL_ID_CHARS, "商家订单号", row)?;
    validate_char_limit(&channel_status, MAX_STATUS_CHARS, "交易状态", row)?;
    for (name, value) in [
        ("交易分类", channel_category.as_str()),
        ("交易对方", counterparty.as_str()),
        ("对方账号", counterparty_account_raw.as_str()),
        ("商品说明", product.as_str()),
        ("收/付款方式", pay_method.as_str()),
        ("备注", source_note.as_str()),
    ] {
        validate_char_limit(value, MAX_DESCRIPTION_CHARS, name, row)?;
    }

    let raw_json = allowlisted_raw_json(&channel_category, &channel_status, &merchant_order_id);
    let disposition = disposition(&channel_status, amount_cents, direction, &pay_method);
    Ok(ParsedRecord {
        row_index: row,
        external_id,
        merchant_order_id,
        occurred_at: occurred.format("%Y-%m-%d %H:%M:%S").to_string(),
        occurred_on: occurred.date().format("%Y-%m-%d").to_string(),
        direction,
        amount_cents,
        channel_category,
        counterparty,
        product,
        pay_method,
        channel_status,
        source_note,
        counterparty_account_raw,
        occurred_at_precision: "second".to_owned(),
        currency: "CNY".to_owned(),
        external_id_source: "native".to_owned(),
        counter_channel_raw: String::new(),
        balance_after_cents: None,
        raw_json,
        disposition,
    })
}

fn normalize_optional(value: &str) -> String {
    let value = value.trim();
    if value == "/" {
        String::new()
    } else {
        value.to_owned()
    }
}

fn allowlisted_raw_json(category: &str, status: &str, merchant_order_id: &str) -> String {
    let mut raw = serde_json::Map::new();
    for (key, value) in [
        ("交易分类", category),
        ("交易状态", status),
        ("商家订单号", merchant_order_id),
    ] {
        if !value.is_empty() {
            raw.insert(key.to_owned(), serde_json::Value::String(value.to_owned()));
        }
    }
    serde_json::Value::Object(raw).to_string()
}

fn parse_amount(value: &str, row: i64) -> Result<i64, ImportParseError> {
    let invalid = || {
        ImportParseError::new(
            "invalid_import_amount",
            format!("第 {row} 行字段 金额 无效: {}", truncated(value)),
        )
    };
    let (yuan, fraction) = match value.split_once('.') {
        Some((yuan, fraction)) if !yuan.is_empty() && (1..=2).contains(&fraction.len()) => {
            (yuan, fraction)
        }
        None if !value.is_empty() => (value, ""),
        _ => return Err(invalid()),
    };
    if !yuan.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(invalid());
    }
    let yuan: i64 = yuan.parse().map_err(|_| invalid())?;
    let fraction = match fraction.len() {
        0 => 0,
        1 => fraction.parse::<i64>().map_err(|_| invalid())? * 10,
        2 => fraction.parse::<i64>().map_err(|_| invalid())?,
        _ => unreachable!(),
    };
    let cents = yuan
        .checked_mul(100)
        .and_then(|amount| amount.checked_add(fraction))
        .ok_or_else(invalid)?;
    if cents > MAX_AMOUNT_CENTS {
        return Err(invalid());
    }
    Ok(cents)
}

fn disposition(
    status: &str,
    amount_cents: i64,
    direction: Direction,
    pay_method: &str,
) -> BaseDisposition {
    if status == "交易关闭" {
        BaseDisposition::Closed
    } else if PENDING_STATUSES.contains(&status) {
        BaseDisposition::Pending
    } else if !SUCCESS_STATUSES.contains(&status) {
        BaseDisposition::Unknown
    } else if amount_cents == 0 {
        BaseDisposition::ZeroAmount
    } else if direction == Direction::Neutral && pay_method.trim().is_empty() {
        BaseDisposition::Neutral
    } else {
        BaseDisposition::Import
    }
}

#[cfg(test)]
mod tests {
    use encoding_rs::GB18030;

    use super::*;

    const HEADER: &str = "交易时间,交易分类,交易对方,对方账号,商品说明,收/支,金额,收/付款方式,交易状态,交易订单号,商家订单号,备注";

    fn encoded(text: &str) -> Vec<u8> {
        let (bytes, _, had_errors) = GB18030.encode(text);
        assert!(!had_errors);
        bytes.into_owned()
    }

    fn document(header: &str, rows: &[&str]) -> Vec<u8> {
        encoded(&format!(
            "支付宝账单（虚构测试）\r\n{header}\r\n{}",
            rows.join("\r\n")
        ))
    }

    fn valid_row() -> &'static str {
        "2026-01-02 03:04:05,虚构分类,虚构商户,不保存的虚构账号,虚构商品,支出,12.34,虚构余额,交易成功,FAKE-ORDER-001,FAKE-MERCHANT-001,虚构备注"
    }

    #[test]
    fn parses_gb18030_fixture_and_all_alipay_statuses() {
        let records = parse_alipay_csv(include_bytes!(
            "../../tests/fixtures/alipay_synthetic_gb18030.csv"
        ))
        .unwrap();
        assert_eq!(records.len(), 9);
        assert_eq!(records[0].row_index, 5);
        assert_eq!(records[8].row_index, 13);
        assert_eq!(
            header_email(include_bytes!(
                "../../tests/fixtures/alipay_synthetic_gb18030.csv"
            )),
            Some("fake@example.test".to_owned())
        );
        assert!(records.iter().any(|record| record.amount_cents == 0));
        assert!(records.iter().any(|record| record.pay_method.is_empty()));
        assert!(records.iter().any(|record| record.product.contains(',')));
        assert_eq!(records[0].external_id, "FAKE-ALI-0001");
        assert_eq!(records[0].source_note, "虚构备注甲");
        let statuses: HashSet<_> = records
            .iter()
            .map(|record| record.channel_status.as_str())
            .collect();
        assert_eq!(statuses.len(), 9);
        assert_eq!(
            records
                .iter()
                .filter(|r| r.disposition == BaseDisposition::Import)
                .count(),
            4
        );
        assert_eq!(
            records
                .iter()
                .filter(|r| r.disposition == BaseDisposition::Pending)
                .count(),
            3
        );
        assert_eq!(
            records
                .iter()
                .filter(|r| r.disposition == BaseDisposition::Closed)
                .count(),
            1
        );
        assert_eq!(
            records
                .iter()
                .filter(|r| r.disposition == BaseDisposition::Neutral)
                .count(),
            0
        );
        assert_eq!(
            records
                .iter()
                .filter(|r| r.disposition == BaseDisposition::ZeroAmount)
                .count(),
            1
        );
    }

    #[test]
    fn successful_rows_use_amount_to_choose_disposition() {
        assert_eq!(
            disposition("交易成功", 1, Direction::Expense, ""),
            BaseDisposition::Import
        );
        assert_eq!(
            disposition("交易成功", 0, Direction::Neutral, ""),
            BaseDisposition::ZeroAmount
        );
        assert_eq!(
            disposition("交易成功", 1, Direction::Neutral, "  "),
            BaseDisposition::Neutral
        );
        assert_eq!(
            disposition("交易成功", 1, Direction::Neutral, "虚构余额"),
            BaseDisposition::Import
        );
    }

    #[test]
    fn parses_counterparty_account_and_allowlists_credential_fields() {
        let slash = valid_row()
            .replace("不保存的虚构账号", "/")
            .replace("FAKE-ORDER-001", "FAKE-ORDER-002");
        let empty = valid_row()
            .replace("不保存的虚构账号", "")
            .replace("FAKE-ORDER-001", "FAKE-ORDER-003");
        let records = parse_alipay_csv(&document(HEADER, &[valid_row(), &slash, &empty])).unwrap();

        assert_eq!(records[0].counterparty_account_raw, "不保存的虚构账号");
        assert_eq!(records[1].counterparty_account_raw, "");
        assert_eq!(records[2].counterparty_account_raw, "");
        assert_eq!(records[0].occurred_at_precision, "second");
        assert_eq!(records[0].currency, "CNY");
        assert_eq!(records[0].external_id_source, "native");
        assert_eq!(records[0].counter_channel_raw, "");
        assert_eq!(records[0].balance_after_cents, None);

        let raw: serde_json::Value = serde_json::from_str(&records[0].raw_json).unwrap();
        assert_eq!(
            raw,
            serde_json::json!({
                "交易分类": "虚构分类",
                "交易状态": "交易成功",
                "商家订单号": "FAKE-MERCHANT-001"
            })
        );
        for forbidden in ["对方账号", "收/付款方式", "商品说明", "交易对方", "备注"]
        {
            assert!(raw.get(forbidden).is_none());
        }
        for forbidden_value in [
            "不保存的虚构账号",
            "虚构余额",
            "虚构商品",
            "虚构商户",
            "虚构备注",
        ] {
            assert!(!records[0].raw_json.contains(forbidden_value));
        }
    }

    #[test]
    fn unknown_status_is_a_record_not_a_parser_error() {
        let row = valid_row().replace("交易成功", "虚构未知状态");
        let records = parse_alipay_csv(&document(HEADER, &[&row])).unwrap();
        assert_eq!(records[0].disposition, BaseDisposition::Unknown);
    }

    #[test]
    fn rejects_bad_gb18030_decoding() {
        let error = parse_alipay_csv(&[0x81]).unwrap_err();
        assert_eq!(error.code, "invalid_import_encoding");
    }

    #[test]
    fn rejects_missing_and_duplicate_headers() {
        let missing = HEADER.replace("交易时间", "错误时间");
        assert_eq!(
            parse_alipay_csv(&document(&missing, &[valid_row()]))
                .unwrap_err()
                .code,
            "invalid_import_header"
        );
        let duplicate = HEADER.replace("交易分类", "交易时间");
        assert_eq!(
            parse_alipay_csv(&document(&duplicate, &[valid_row()]))
                .unwrap_err()
                .code,
            "invalid_import_header"
        );
    }

    #[test]
    fn rejects_unknown_or_empty_direction() {
        for direction in ["其他", ""] {
            let row = valid_row().replace(",支出,", &format!(",{direction},"));
            let error = parse_alipay_csv(&document(HEADER, &[&row])).unwrap_err();
            assert_eq!(error.code, "unknown_import_direction");
            assert!(error.message.contains("第 3 行"));
        }
    }

    #[test]
    fn rejects_bad_datetime() {
        let row = valid_row().replace("2026-01-02 03:04:05", "2026-02-30 03:04:05");
        assert_eq!(
            parse_alipay_csv(&document(HEADER, &[&row]))
                .unwrap_err()
                .code,
            "invalid_import_datetime"
        );
    }

    #[test]
    fn rejects_bad_amount_forms_and_overflow() {
        for amount in [
            "-1",
            "+1",
            "1e2",
            "\"1,000\"",
            "1.234",
            "90071992547409.92",
            "",
        ] {
            let row = valid_row().replace("12.34", amount);
            assert_eq!(
                parse_alipay_csv(&document(HEADER, &[&row]))
                    .unwrap_err()
                    .code,
                "invalid_import_amount",
                "{amount}"
            );
        }
    }

    #[test]
    fn rejects_empty_and_duplicate_external_ids_with_both_rows() {
        let empty = valid_row().replace("FAKE-ORDER-001", "   ");
        assert_eq!(
            parse_alipay_csv(&document(HEADER, &[&empty]))
                .unwrap_err()
                .code,
            "empty_import_external_id"
        );
        let error = parse_alipay_csv(&document(HEADER, &[valid_row(), valid_row()])).unwrap_err();
        assert_eq!(error.code, "duplicate_import_external_id");
        assert!(error.message.contains("第 3 行") && error.message.contains("第 4 行"));
    }

    #[test]
    fn skips_fully_empty_rows_but_rejects_partially_empty_rows_atomically() {
        let bytes = document(
            HEADER,
            &[",,,,,,,,,,,,", valid_row(), ",,,,,,,,,,,部分内容"],
        );
        let error = parse_alipay_csv(&bytes).unwrap_err();
        assert_eq!(error.code, "invalid_import_datetime");
        assert!(error.message.contains("第 5 行"));
    }
}
