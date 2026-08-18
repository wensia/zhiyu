use std::{collections::HashMap, io::Cursor};

use calamine::{Data, Reader, open_workbook_auto_from_rs};
use chrono::NaiveDateTime;

use super::model::{
    BaseDisposition, Direction, ImportParseError, MAX_AMOUNT_CENTS, MAX_DESCRIPTION_CHARS,
    MAX_EXTERNAL_ID_CHARS, MAX_IMPORT_BYTES, MAX_IMPORT_RECORDS, MAX_STATUS_CHARS, ParsedRecord,
    truncated, validate_char_limit,
};

const MAX_SHEETS: usize = 16;
const MAX_COLUMNS: usize = 128;
const REQUIRED_HEADERS: [&str; 11] = [
    "交易时间",
    "交易类型",
    "交易对方",
    "商品",
    "收/支",
    "金额(元)",
    "支付方式",
    "当前状态",
    "交易单号",
    "商户单号",
    "备注",
];
const SUCCESS_STATUSES: [&str; 8] = [
    "支付成功",
    "已存入零钱",
    "已转账",
    "对方已收钱",
    "提现已到账",
    "还款成功",
    "充值完成",
    "已全额退款",
];

pub(super) fn header_nickname(bytes: &[u8]) -> Option<String> {
    let mut workbook = open_workbook_auto_from_rs(Cursor::new(bytes)).ok()?;
    for name in workbook.sheet_names() {
        let range = workbook.worksheet_range(&name).ok()?;
        for row in range.rows() {
            for cell in row {
                let Data::String(value) = cell else { continue };
                let value = value.trim();
                let Some(rest) = value.strip_prefix("微信昵称：[") else {
                    continue;
                };
                let Some(nickname) = rest.strip_suffix(']') else {
                    continue;
                };
                let nickname = nickname.trim();
                if !nickname.is_empty() {
                    return Some(nickname.to_owned());
                }
            }
        }
    }
    None
}

pub fn parse_wechat_xlsx(bytes: &[u8]) -> Result<Vec<ParsedRecord>, ImportParseError> {
    if bytes.len() > MAX_IMPORT_BYTES {
        return Err(ImportParseError::new(
            "import_resource_limit",
            format!("文件超过 {MAX_IMPORT_BYTES} 字节"),
        ));
    }
    let mut workbook = open_workbook_auto_from_rs(Cursor::new(bytes))
        .map_err(|_| ImportParseError::new("invalid_import_xlsx", "xlsx 解析失败"))?;
    let names = workbook.sheet_names();
    if names.len() > MAX_SHEETS {
        return Err(resource_limit(format!("worksheet 超过 {MAX_SHEETS} 个")));
    }
    let mut sheets = Vec::with_capacity(names.len());
    for name in names {
        let range = workbook
            .worksheet_range(&name)
            .map_err(|_| ImportParseError::new("invalid_import_xlsx", "worksheet 解析失败"))?;
        sheets.push((name, range));
    }
    parse_sheets(&sheets)
}

fn parse_sheets(
    sheets: &[(String, calamine::Range<Data>)],
) -> Result<Vec<ParsedRecord>, ImportParseError> {
    if sheets.len() > MAX_SHEETS {
        return Err(resource_limit(format!("worksheet 超过 {MAX_SHEETS} 个")));
    }
    let mut candidates = Vec::new();
    for (sheet_index, (_, range)) in sheets.iter().enumerate() {
        if range.width() > MAX_COLUMNS {
            return Err(resource_limit(format!("worksheet 超过 {MAX_COLUMNS} 列")));
        }
        for (relative_row, row) in range.rows().enumerate() {
            if let Some(columns) = complete_header(row, physical_row(range, relative_row))? {
                candidates.push((sheet_index, relative_row, columns));
            }
        }
    }
    if candidates.len() != 1 {
        return Err(ImportParseError::new(
            "invalid_import_header",
            format!(
                "微信完整表头必须恰好命中一次，实际命中 {} 次",
                candidates.len()
            ),
        ));
    }
    let (sheet_index, header_row, columns) = candidates.pop().unwrap();
    let (_, range) = &sheets[sheet_index];
    let mut records = Vec::new();
    let mut external_ids = HashMap::<String, i64>::new();
    for (relative_row, row) in range.rows().enumerate().skip(header_row + 1) {
        if row.iter().all(|cell| matches!(cell, Data::Empty)) {
            continue;
        }
        if records.len() >= MAX_IMPORT_RECORDS {
            return Err(resource_limit(format!(
                "非空数据记录超过 {MAX_IMPORT_RECORDS} 条"
            )));
        }
        let physical_row = physical_row(range, relative_row);
        let record = parse_record(row, &columns, physical_row)?;
        if let Some(first_row) = external_ids.insert(record.external_id.clone(), physical_row) {
            return Err(ImportParseError::new(
                "duplicate_import_external_id",
                format!(
                    "交易单号 {} 在第 {first_row} 行和第 {physical_row} 行重复",
                    truncated(&record.external_id)
                ),
            ));
        }
        records.push(record);
    }
    if records.is_empty() {
        return Err(ImportParseError::new(
            "empty_import_file",
            "表头后没有非空数据行",
        ));
    }
    Ok(records)
}

fn physical_row(range: &calamine::Range<Data>, relative_row: usize) -> i64 {
    range.start().map_or(1, |(row, _)| row as i64 + 1) + relative_row as i64
}

fn complete_header(
    row: &[Data],
    physical_row: i64,
) -> Result<Option<HashMap<String, usize>>, ImportParseError> {
    let has_anchor = row.iter().any(|cell| match cell {
        Data::String(value) => value.trim() == "交易时间",
        _ => false,
    });
    if !has_anchor {
        return Ok(None);
    }
    let mut columns = HashMap::new();
    for (index, cell) in row.iter().enumerate() {
        if let Data::String(value) = cell {
            let name = value.trim();
            if REQUIRED_HEADERS.contains(&name) && columns.insert(name.to_owned(), index).is_some()
            {
                return Err(ImportParseError::new(
                    "invalid_import_header",
                    format!("第 {physical_row} 行表头字段 {name} 重复"),
                ));
            }
        }
    }
    if REQUIRED_HEADERS
        .iter()
        .all(|name| columns.contains_key(*name))
    {
        Ok(Some(columns))
    } else {
        Ok(None)
    }
}

fn parse_record(
    row: &[Data],
    columns: &HashMap<String, usize>,
    physical_row: i64,
) -> Result<ParsedRecord, ImportParseError> {
    let cell = |name: &str| -> Result<&Data, ImportParseError> {
        row.get(columns[name]).ok_or_else(|| {
            ImportParseError::new(
                "invalid_import_row",
                format!("第 {physical_row} 行缺少字段 {name}"),
            )
        })
    };
    let occurred = parse_datetime(cell("交易时间")?, physical_row)?;
    let direction_raw = required_string(cell("收/支")?, "收/支", physical_row)?.trim();
    let direction = match direction_raw {
        "收入" => Direction::Income,
        "支出" => Direction::Expense,
        "/" => Direction::Neutral,
        _ => {
            return Err(ImportParseError::new(
                "unknown_import_direction",
                format!(
                    "第 {physical_row} 行字段 收/支 未知: {}",
                    truncated(direction_raw)
                ),
            ));
        }
    };
    let amount_cents = parse_amount(cell("金额(元)")?, physical_row)?;
    let external_id = required_string(cell("交易单号")?, "交易单号", physical_row)?
        .trim()
        .to_owned();
    if external_id.is_empty() {
        return Err(ImportParseError::new(
            "empty_import_external_id",
            format!("第 {physical_row} 行字段 交易单号 为空"),
        ));
    }
    let merchant_order_id = optional_text(cell("商户单号")?, "商户单号", physical_row)?;
    let channel_status = required_string(cell("当前状态")?, "当前状态", physical_row)?
        .trim()
        .to_owned();
    let channel_category = text(cell("交易类型")?, "交易类型", physical_row, false)?;
    let counterparty = text(cell("交易对方")?, "交易对方", physical_row, false)?;
    let product = optional_text(cell("商品")?, "商品", physical_row)?;
    let pay_method = optional_text(cell("支付方式")?, "支付方式", physical_row)?;
    let source_note = optional_text(cell("备注")?, "备注", physical_row)?;

    validate_char_limit(
        &external_id,
        MAX_EXTERNAL_ID_CHARS,
        "交易单号",
        physical_row,
    )?;
    validate_char_limit(
        &merchant_order_id,
        MAX_EXTERNAL_ID_CHARS,
        "商户单号",
        physical_row,
    )?;
    validate_char_limit(&channel_status, MAX_STATUS_CHARS, "当前状态", physical_row)?;
    for (name, value) in [
        ("交易类型", &channel_category),
        ("交易对方", &counterparty),
        ("商品", &product),
        ("支付方式", &pay_method),
        ("备注", &source_note),
    ] {
        validate_char_limit(value, MAX_DESCRIPTION_CHARS, name, physical_row)?;
    }
    let raw_json = allowlisted_raw_json(&channel_category, &channel_status, &merchant_order_id);
    let disposition = disposition(&channel_status, amount_cents, direction, &pay_method);
    Ok(ParsedRecord {
        row_index: physical_row,
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
        counterparty_account_raw: String::new(),
        occurred_at_precision: "second".to_owned(),
        currency: "CNY".to_owned(),
        external_id_source: "native".to_owned(),
        counter_channel_raw: String::new(),
        balance_after_cents: None,
        raw_json,
        disposition,
    })
}

fn allowlisted_raw_json(category: &str, status: &str, merchant_order_id: &str) -> String {
    let mut raw = serde_json::Map::new();
    for (key, value) in [
        ("交易类型", category),
        ("当前状态", status),
        ("商户单号", merchant_order_id),
    ] {
        if !value.is_empty() {
            raw.insert(key.to_owned(), serde_json::Value::String(value.to_owned()));
        }
    }
    serde_json::Value::Object(raw).to_string()
}

fn parse_datetime(cell: &Data, row: i64) -> Result<NaiveDateTime, ImportParseError> {
    match cell {
        Data::DateTime(value) => value
            .as_datetime()
            .ok_or_else(|| invalid_type(row, "交易时间")),
        Data::DateTimeIso(value) => {
            NaiveDateTime::parse_from_str(value.trim(), "%Y-%m-%dT%H:%M:%S")
                .or_else(|_| NaiveDateTime::parse_from_str(value.trim(), "%Y-%m-%d %H:%M:%S"))
                .map_err(|_| {
                    ImportParseError::new(
                        "invalid_import_datetime",
                        format!("第 {row} 行字段 交易时间 无效: {}", truncated(value)),
                    )
                })
        }
        _ => Err(invalid_type(row, "交易时间")),
    }
}

fn parse_amount(cell: &Data, row: i64) -> Result<i64, ImportParseError> {
    let invalid = || {
        ImportParseError::new(
            "invalid_import_amount",
            format!("第 {row} 行字段 金额(元) 无效"),
        )
    };
    let cents = match cell {
        Data::Float(value) => {
            if !value.is_finite() || *value < 0.0 {
                return Err(invalid());
            }
            let rounded = (*value * 100.0).round();
            if !rounded.is_finite() || rounded < 0.0 || rounded > MAX_AMOUNT_CENTS as f64 {
                return Err(invalid());
            }
            rounded as i64
        }
        Data::Int(value) if *value >= 0 => value.checked_mul(100).ok_or_else(invalid)?,
        Data::Int(_) => return Err(invalid()),
        _ => return Err(invalid_type(row, "金额(元)")),
    };
    if cents > MAX_AMOUNT_CENTS {
        return Err(invalid());
    }
    Ok(cents)
}

fn required_string<'a>(cell: &'a Data, field: &str, row: i64) -> Result<&'a str, ImportParseError> {
    match cell {
        Data::String(value) => Ok(value),
        _ => Err(invalid_type(row, field)),
    }
}

fn text(cell: &Data, field: &str, row: i64, optional: bool) -> Result<String, ImportParseError> {
    match cell {
        Data::String(value) => Ok(normalize_optional(value)),
        Data::Empty if optional => Ok(String::new()),
        _ => Err(invalid_type(row, field)),
    }
}

fn optional_text(cell: &Data, field: &str, row: i64) -> Result<String, ImportParseError> {
    text(cell, field, row, true)
}

fn normalize_optional(value: &str) -> String {
    let value = value.trim();
    if value == "/" {
        String::new()
    } else {
        value.to_owned()
    }
}

fn valid_refund_status(status: &str) -> bool {
    let amount = status
        .strip_prefix("已退款(¥")
        .and_then(|rest| rest.strip_suffix(')'))
        .or_else(|| status.strip_prefix("已退款¥"));
    amount.is_some_and(valid_decimal)
}

fn valid_decimal(value: &str) -> bool {
    let (whole, fraction) = value
        .split_once('.')
        .map_or((value, None), |(a, b)| (a, Some(b)));
    !whole.is_empty()
        && whole.bytes().all(|byte| byte.is_ascii_digit())
        && fraction.is_none_or(|part| {
            (1..=2).contains(&part.len()) && part.bytes().all(|b| b.is_ascii_digit())
        })
}

fn disposition(
    status: &str,
    amount_cents: i64,
    direction: Direction,
    pay_method: &str,
) -> BaseDisposition {
    if !SUCCESS_STATUSES.contains(&status) && !valid_refund_status(status) {
        BaseDisposition::Unknown
    } else if amount_cents == 0 {
        BaseDisposition::ZeroAmount
    } else if direction == Direction::Neutral && pay_method.trim().is_empty() {
        BaseDisposition::Neutral
    } else {
        BaseDisposition::Import
    }
}

fn invalid_type(row: i64, field: &str) -> ImportParseError {
    ImportParseError::new(
        "invalid_import_cell_type",
        format!("第 {row} 行字段 {field} 单元格类型无效"),
    )
}

fn resource_limit(message: String) -> ImportParseError {
    ImportParseError::new("import_resource_limit", message)
}

#[cfg(test)]
mod tests {
    use calamine::{Cell, ExcelDateTime, ExcelDateTimeType, Range};

    use super::*;

    fn fixture() -> Vec<ParsedRecord> {
        parse_wechat_xlsx(include_bytes!("../../tests/fixtures/wechat.xlsx")).unwrap()
    }

    #[test]
    fn parses_static_fixture_with_native_types_refunds_and_slash_boundaries() {
        let records = fixture();
        assert_eq!(records.len(), 5);
        assert_eq!(records.iter().map(|r| r.amount_cents).sum::<i64>(), 28866);
        assert_eq!(records[0].occurred_at, "2026-01-02 03:04:05");
        assert_eq!(records[0].amount_cents, 28371);
        assert_eq!(records[0].source_note, "纯虚构备注");
        assert!(records.iter().any(|r| r.direction == Direction::Neutral));
        assert!(records.iter().any(|r| {
            r.direction == Direction::Neutral && r.disposition == BaseDisposition::Neutral
        }));
        assert!(records.iter().any(|r| r.channel_status == "已退款(¥0.95)"));
        assert!(records.iter().any(|r| r.channel_status == "已退款¥1.05"));
        let dynamic = records
            .iter()
            .find(|r| r.channel_category.ends_with("-退款"))
            .unwrap();
        assert!(dynamic.product.is_empty());
        assert!(dynamic.pay_method.is_empty());
    }

    #[test]
    fn populates_credential_fields_with_allowlisted_raw_json() {
        let record = fixture().remove(0);
        assert_eq!(record.counterparty_account_raw, "");
        assert_eq!(record.occurred_at_precision, "second");
        assert_eq!(record.currency, "CNY");
        assert_eq!(record.external_id_source, "native");
        assert_eq!(record.counter_channel_raw, "");
        assert_eq!(record.balance_after_cents, None);

        let raw: serde_json::Value = serde_json::from_str(&record.raw_json).unwrap();
        assert_eq!(
            raw,
            serde_json::json!({
                "交易类型": record.channel_category,
                "当前状态": record.channel_status,
                "商户单号": record.merchant_order_id
            })
        );
        for forbidden in ["交易对方", "商品", "支付方式", "备注"] {
            assert!(raw.get(forbidden).is_none());
        }
        for forbidden_value in [
            record.counterparty,
            record.product,
            record.pay_method,
            record.source_note,
        ] {
            if !forbidden_value.is_empty() {
                assert!(!record.raw_json.contains(&forbidden_value));
            }
        }
    }

    #[test]
    fn amount_types_round_and_reject_invalid_values() {
        assert_eq!(parse_amount(&Data::Float(283.71), 2).unwrap(), 28_371);
        assert_eq!(parse_amount(&Data::Int(12), 2).unwrap(), 1_200);
        for value in [f64::NAN, f64::INFINITY, -1.0, 90_071_992_547_409.92] {
            assert_eq!(
                parse_amount(&Data::Float(value), 2).unwrap_err().code,
                "invalid_import_amount"
            );
        }
        assert!(parse_amount(&Data::Int(i64::MAX), 2).is_err());
        assert_eq!(
            parse_amount(&Data::String("1.00".into()), 2)
                .unwrap_err()
                .code,
            "invalid_import_cell_type"
        );
    }

    #[test]
    fn datetime_uses_calamine_api_and_strict_iso() {
        let native = Data::DateTime(ExcelDateTime::new(
            46_024.5,
            ExcelDateTimeType::DateTime,
            false,
        ));
        assert_eq!(
            parse_datetime(&native, 2)
                .unwrap()
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
            "2026-01-02 12:00:00"
        );
        assert!(parse_datetime(&Data::DateTimeIso("2026-01-02T03:04:05".into()), 2).is_ok());
        assert!(parse_datetime(&Data::Float(46_024.5), 2).is_err());
    }

    #[test]
    fn refund_statuses_are_bounded_and_unknown_status_is_preserved() {
        for status in ["已退款(¥0.95)", "已退款¥1.05"] {
            assert_eq!(
                disposition(status, 100, Direction::Expense, ""),
                BaseDisposition::Import
            );
        }
        for status in [
            "垃圾已退款¥1.00",
            "已退款垃圾",
            "已退款¥-1",
            "已退款(¥1.234)",
        ] {
            assert_eq!(
                disposition(status, 100, Direction::Expense, ""),
                BaseDisposition::Unknown
            );
        }
    }

    #[test]
    fn successful_rows_use_amount_to_choose_disposition() {
        assert_eq!(
            disposition("支付成功", 1, Direction::Expense, ""),
            BaseDisposition::Import
        );
        assert_eq!(
            disposition("支付成功", 0, Direction::Neutral, ""),
            BaseDisposition::ZeroAmount
        );
        assert_eq!(
            disposition(
                "支付成功",
                1,
                Direction::Neutral,
                &normalize_optional(" / ")
            ),
            BaseDisposition::Neutral
        );
        assert_eq!(
            disposition("支付成功", 1, Direction::Neutral, "虚构零钱"),
            BaseDisposition::Import
        );
    }

    fn range(rows: Vec<Vec<Data>>) -> Range<Data> {
        let cells = rows
            .into_iter()
            .enumerate()
            .flat_map(|(r, row)| {
                row.into_iter().enumerate().filter_map(move |(c, value)| {
                    (!matches!(value, Data::Empty)).then(|| Cell::new((r as u32, c as u32), value))
                })
            })
            .collect();
        Range::from_sparse(cells)
    }

    fn header() -> Vec<Data> {
        REQUIRED_HEADERS
            .iter()
            .map(|s| Data::String((*s).into()))
            .collect()
    }

    #[test]
    fn rejects_multiple_sheets_headers_and_resource_limits() {
        let one = range(vec![header()]);
        let sheets = vec![("一".into(), one.clone()), ("二".into(), one)];
        assert_eq!(
            parse_sheets(&sheets).unwrap_err().code,
            "invalid_import_header"
        );
        let too_many = (0..17)
            .map(|i| (i.to_string(), Range::empty()))
            .collect::<Vec<_>>();
        assert_eq!(
            parse_sheets(&too_many).unwrap_err().code,
            "import_resource_limit"
        );
        let wide = Range::<Data>::new((0, 0), (0, 128));
        assert_eq!(
            parse_sheets(&[("宽".into(), wide)]).unwrap_err().code,
            "import_resource_limit"
        );
    }

    #[test]
    fn rejects_unknown_direction_empty_duplicate_id_and_wrong_text_types() {
        let mut records = fixture();
        let base = &records.remove(0);
        assert_eq!(base.row_index, 4);
        assert_eq!(
            header_nickname(include_bytes!("../../tests/fixtures/wechat.xlsx")),
            Some("虚构昵称".to_owned())
        );
        assert_eq!(
            required_string(&Data::Int(1), "交易单号", 2)
                .unwrap_err()
                .code,
            "invalid_import_cell_type"
        );
        assert_eq!(
            disposition("未知状态", 100, Direction::Expense, ""),
            BaseDisposition::Unknown
        );
        assert!(matches!(normalize_optional(" / ").as_str(), ""));
        assert_eq!(normalize_optional("前/后"), "前/后");
    }
}
