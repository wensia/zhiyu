use axum::{
    Json,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use libsql::{Connection, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::ApiError;

pub(crate) fn idempotency_key(headers: &HeaderMap) -> Result<String, ApiError> {
    let value = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .unwrap_or_default();
    if !(8..=128).contains(&value.len()) || !value.is_ascii() {
        return Err(ApiError::bad_request(
            "idempotency_key_required",
            "请提供 8–128 位 Idempotency-Key",
        ));
    }
    Ok(value.to_owned())
}

pub(crate) fn request_hash<T: Serialize>(value: &T) -> Result<String, ApiError> {
    let json = serde_json::to_vec(value).map_err(ApiError::internal)?;
    Ok(format!("{:x}", Sha256::digest(json)))
}

pub(crate) async fn replay_idempotency(
    conn: &Connection,
    user_id: &str,
    key: &str,
    operation: &str,
    hash: &str,
) -> Result<Option<Response>, ApiError> {
    let mut rows = conn.query("SELECT request_hash, response_status, response_body FROM idempotency_records WHERE user_id = ?1 AND idempotency_key = ?2 AND operation = ?3", params![user_id, key, operation]).await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    let existing_hash: String = row.get(0)?;
    if existing_hash != hash {
        return Err(ApiError::conflict(
            "idempotency_mismatch",
            "该 Idempotency-Key 已用于不同请求",
        ));
    }
    let status: i64 = row.get(1)?;
    let body: String = row.get(2)?;
    let status = StatusCode::from_u16(status as u16).map_err(ApiError::internal)?;
    let mut body: serde_json::Value = serde_json::from_str(&body).map_err(ApiError::internal)?;
    if let Some(object) = body.as_object_mut()
        && object.contains_key("repayments")
    {
        object
            .entry("additions".to_owned())
            .or_insert_with(|| serde_json::Value::Array(Vec::new()));
        object
            .entry("account".to_owned())
            .or_insert(serde_json::Value::Null);
        for key in ["repayments", "additions"] {
            if let Some(events) = object
                .get_mut(key)
                .and_then(serde_json::Value::as_array_mut)
            {
                for event in events {
                    if let Some(event) = event.as_object_mut() {
                        event
                            .entry("account".to_owned())
                            .or_insert(serde_json::Value::Null);
                    }
                }
            }
        }
    }
    backfill_account_types_in_idempotency_response(&mut body);
    Ok(Some((status, Json(body)).into_response()))
}

fn backfill_account_types_in_idempotency_response(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                backfill_account_types_in_idempotency_response(item);
            }
        }
        serde_json::Value::Object(object) => {
            for child in object.values_mut() {
                backfill_account_types_in_idempotency_response(child);
            }
            if !object.contains_key("accountType")
                && object.contains_key("archived")
                && let Some(name) = object.get("name").and_then(serde_json::Value::as_str)
            {
                object.insert(
                    "accountType".to_owned(),
                    serde_json::Value::String(infer_legacy_account_type(name).to_owned()),
                );
            }
            if object.contains_key("usageCount") && object.contains_key("name") {
                object
                    .entry("nameSource".to_owned())
                    .or_insert_with(|| serde_json::Value::String("custom".to_owned()));
            }
        }
        _ => {}
    }
}

fn infer_legacy_account_type(name: &str) -> &'static str {
    if name.starts_with("微信") {
        "wechat_balance"
    } else if name.starts_with("支付宝") {
        "alipay_balance"
    } else if name == "现金" {
        "cash"
    } else if name.starts_with("数字人民币") {
        "digital_cny"
    } else {
        "other"
    }
}

pub(crate) async fn store_idempotency<T: Serialize>(
    conn: &Connection,
    user_id: &str,
    key: &str,
    operation: &str,
    hash: &str,
    status: StatusCode,
    body: &T,
) -> Result<(), ApiError> {
    conn.execute("INSERT INTO idempotency_records(user_id, idempotency_key, operation, request_hash, response_status, response_body, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", params![user_id, key, operation, hash, status.as_u16() as i64, serde_json::to_string(body).map_err(ApiError::internal)?, Utc::now().to_rfc3339()]).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::backfill_account_types_in_idempotency_response;

    #[test]
    fn legacy_account_idempotency_responses_default_to_custom_name_source() {
        let mut account = json!({
            "id": "account-1",
            "name": "旧账户",
            "accountType": "other",
            "archived": false,
            "usageCount": 0
        });
        backfill_account_types_in_idempotency_response(&mut account);
        assert_eq!(account["nameSource"], "custom");

        let mut brief = json!({
            "id": "account-1",
            "name": "旧账户",
            "accountType": "other",
            "archived": false
        });
        backfill_account_types_in_idempotency_response(&mut brief);
        assert!(brief.get("nameSource").is_none());
    }
}
