use std::{fs, sync::Arc};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Method, Request, StatusCode, header},
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;
use zhiyu_api::{
    AppState, app, config::Config, db, domain::MAX_SAFE_CENTS, email::DevFileEmailSender,
    rate_limit::RateLimiter,
};

struct TestApp {
    router: Router,
    root: TempDir,
}

impl TestApp {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let config = Config {
            app_env: "test".into(),
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            public_base_url: "http://test.local".into(),
            database_url: format!("file:{}", root.path().join("test.db").display()),
            turso_auth_token: None,
            dev_mail_dir: root.path().join("mail"),
            web_dist_dir: root.path().join("web"),
        };
        let database = db::connect(&config).await.unwrap();
        let state = AppState {
            db: Arc::new(database),
            email: Arc::new(DevFileEmailSender::new(config.dev_mail_dir.clone())),
            config: Arc::new(config),
            rate_limiter: RateLimiter::default(),
        };
        Self {
            router: app(state),
            root,
        }
    }

    async fn register_and_login(&self, email: &str) -> String {
        let (status, _, _) = send(
            &self.router,
            Method::POST,
            "/api/v1/auth/register",
            Some(json!({ "email": email, "password": "a-very-secure-password", "timezone": "Asia/Shanghai" })),
            None,
            None,
            false,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let mail_dir = self.root.path().join("mail");
        let mail = fs::read_to_string(
            fs::read_dir(mail_dir)
                .unwrap()
                .filter_map(Result::ok)
                .max_by_key(|entry| entry.metadata().unwrap().modified().unwrap())
                .unwrap()
                .path(),
        )
        .unwrap();
        let token = mail.split("token=").nth(1).unwrap().trim();
        let (status, _, _) = send(
            &self.router,
            Method::POST,
            "/api/v1/auth/verify-email",
            Some(json!({ "token": token })),
            None,
            None,
            false,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, headers, _) = send(
            &self.router,
            Method::POST,
            "/api/v1/auth/login",
            Some(json!({ "email": email, "password": "a-very-secure-password" })),
            None,
            None,
            false,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        headers
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned()
    }

    async fn create_ledger_account(&self, cookie: &str, name: &str, account_type: &str) -> Value {
        let (status, _, account) = send(
            &self.router,
            Method::POST,
            "/api/v1/ledger-accounts",
            Some(json!({ "name": name, "accountType": account_type, "note": "测试资金账户" })),
            Some(cookie),
            Some("create-ledger-account-0001"),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(account["accountType"], account_type);
        account
    }
}

#[tokio::test]
async fn complete_ledger_flow_is_idempotent_auditable_and_user_scoped() {
    let test = TestApp::new().await;
    let cookie = test.register_and_login("first@example.com").await;
    let account = test
        .create_ledger_account(&cookie, "微信支付-完整流程", "wechat_balance")
        .await;
    let account_id = account["id"].as_str().unwrap();
    let create_body = json!({
        "direction": "lend_out",
        "counterpartyName": "阿青",
        "principalCents": 100_000,
        "occurredOn": "2026-08-02",
        "dueOn": "2026-08-09",
        "note": "测试借款",
        "accountId": account_id
    });
    let (status, _, created) = send(
        &test.router,
        Method::POST,
        "/api/v1/debts",
        Some(create_body.clone()),
        Some(&cookie),
        Some("create-debt-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let debt_id = created["id"].as_str().unwrap();
    assert_eq!(created["remainingCents"], 100_000);
    assert_eq!(created["status"], "due_soon");
    assert_eq!(created["account"]["id"], account_id);
    assert_eq!(created["account"]["name"], "微信支付-完整流程");
    assert_eq!(created["account"]["accountType"], "wechat_balance");

    let (status, _, replayed) = send(
        &test.router,
        Method::POST,
        "/api/v1/debts",
        Some(create_body),
        Some(&cookie),
        Some("create-debt-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(replayed["id"], created["id"]);

    let payment = json!({
        "amountCents": 40_000,
        "effectiveOn": "2026-08-03",
        "note": "首期",
        "accountId": account_id
    });
    let path = format!("/api/v1/debts/{debt_id}/repayments");
    let (status, _, paid) = send(
        &test.router,
        Method::POST,
        &path,
        Some(payment.clone()),
        Some(&cookie),
        Some("payment-key-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(paid["remainingCents"], 60_000);
    assert_eq!(paid["repayments"][0]["account"]["id"], account_id);
    let payment_id = paid["repayments"][0]["id"].as_str().unwrap().to_owned();
    let (status, _, duplicate) = send(
        &test.router,
        Method::POST,
        &path,
        Some(payment.clone()),
        Some(&cookie),
        Some("payment-key-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(duplicate["remainingCents"], 60_000);

    let request_a = send(
        &test.router,
        Method::POST,
        &path,
        Some(payment.clone()),
        Some(&cookie),
        Some("payment-key-0002"),
        true,
    );
    let request_b = send(
        &test.router,
        Method::POST,
        &path,
        Some(payment),
        Some(&cookie),
        Some("payment-key-0003"),
        true,
    );
    let (a, b) = tokio::join!(request_a, request_b);
    let statuses = [a.0, b.0];
    assert!(statuses.contains(&StatusCode::CREATED));
    assert!(statuses.contains(&StatusCode::CONFLICT));

    let reverse_path = format!("/api/v1/repayments/{payment_id}/reversals");
    let reverse = json!({ "effectiveOn": "2026-08-04", "note": "录入错误" });
    let (status, _, reversed) = send(
        &test.router,
        Method::POST,
        &reverse_path,
        Some(reverse.clone()),
        Some(&cookie),
        Some("reverse-key-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(reversed["remainingCents"], 60_000);
    let reversal = reversed["repayments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["kind"] == "reversal")
        .unwrap();
    assert_eq!(reversal["account"]["id"], account_id);
    assert_eq!(reversal["account"]["name"], "微信支付-完整流程");
    let (status, _, _) = send(
        &test.router,
        Method::POST,
        &reverse_path,
        Some(reverse),
        Some(&cookie),
        Some("reverse-key-0002"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let second_cookie = test.register_and_login("second@example.com").await;
    let (status, _, _) = send(
        &test.router,
        Method::GET,
        &format!("/api/v1/debts/{debt_id}"),
        None,
        Some(&second_cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let version = reversed["version"].as_i64().unwrap();
    let (status, _, body) = send(
        &test.router,
        Method::DELETE,
        &format!("/api/v1/debts/{debt_id}"),
        Some(json!({ "version": version })),
        Some(&cookie),
        Some("delete-key-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "debt_has_history");
}

#[tokio::test]
async fn debt_additions_are_idempotent_scoped_and_lock_historical_principal() {
    let test = TestApp::new().await;
    let cookie = test.register_and_login("additions@example.com").await;
    let account = test
        .create_ledger_account(&cookie, "支付宝-追加测试", "alipay_balance")
        .await;
    let account_id = account["id"].as_str().unwrap();
    let (status, _, created) = send(
        &test.router,
        Method::POST,
        "/api/v1/debts",
        Some(json!({
            "direction": "borrow_in",
            "counterpartyName": "老姑",
            "principalCents": 20_000,
            "occurredOn": "2024-09-15",
            "note": "",
            "accountId": account_id
        })),
        Some(&cookie),
        Some("addition-debt-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let debt_id = created["id"].as_str().unwrap();
    let counterparty_id = created["counterparty"]["id"].as_str().unwrap();
    let addition_path = format!("/api/v1/debts/{debt_id}/additions");
    let addition = json!({
        "amountCents": 5_000,
        "effectiveOn": "2026-08-02",
        "note": "再次借入",
        "accountId": account_id
    });

    let (status, _, added) = send(
        &test.router,
        Method::POST,
        &addition_path,
        Some(addition.clone()),
        Some(&cookie),
        Some("addition-key-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(added["direction"], "borrow_in");
    assert_eq!(added["principalCents"], 25_000);
    assert_eq!(added["paidCents"], 0);
    assert_eq!(added["remainingCents"], 25_000);
    assert_eq!(added["additions"].as_array().unwrap().len(), 1);
    assert_eq!(added["additions"][0]["amountCents"], 5_000);
    assert_eq!(added["additions"][0]["effectiveOn"], "2026-08-02");
    assert_eq!(added["additions"][0]["note"], "再次借入");
    assert_eq!(added["additions"][0]["account"]["id"], account_id);

    let (status, _, replayed) = send(
        &test.router,
        Method::POST,
        &addition_path,
        Some(addition.clone()),
        Some(&cookie),
        Some("addition-key-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(replayed["version"], added["version"]);
    assert_eq!(replayed["additions"].as_array().unwrap().len(), 1);

    let (status, _, body) = send(
        &test.router,
        Method::POST,
        &addition_path,
        Some(json!({
            "amountCents": 5_001,
            "effectiveOn": "2026-08-02",
            "note": "再次借入",
            "accountId": account_id
        })),
        Some(&cookie),
        Some("addition-key-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "idempotency_mismatch");

    let (status, _, body) = send(
        &test.router,
        Method::PATCH,
        &format!("/api/v1/debts/{debt_id}"),
        Some(json!({
            "version": added["version"],
            "counterpartyId": counterparty_id,
            "accountId": account_id,
            "principalCents": 25_001,
            "occurredOn": "2024-09-15",
            "dueOn": null,
            "note": ""
        })),
        Some(&cookie),
        Some("addition-update-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "principal_locked");

    let (status, _, body) = send(
        &test.router,
        Method::DELETE,
        &format!("/api/v1/debts/{debt_id}"),
        Some(json!({ "version": added["version"] })),
        Some(&cookie),
        Some("addition-delete-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "debt_has_history");

    let other_cookie = test.register_and_login("addition-other@example.com").await;
    let (status, _, _) = send(
        &test.router,
        Method::POST,
        &addition_path,
        Some(addition.clone()),
        Some(&other_cookie),
        Some("addition-other-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _, archived) = send(
        &test.router,
        Method::POST,
        &format!("/api/v1/debts/{debt_id}/archive"),
        Some(json!({ "version": added["version"] })),
        Some(&cookie),
        Some("addition-archive-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, body) = send(
        &test.router,
        Method::POST,
        &addition_path,
        Some(addition),
        Some(&cookie),
        Some("addition-archived-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "debt_archived");
    assert_eq!(archived["additions"].as_array().unwrap().len(), 1);

    let (status, _, at_limit) = send(
        &test.router,
        Method::POST,
        "/api/v1/debts",
        Some(json!({
            "direction": "lend_out",
            "counterpartyName": "上限测试",
            "principalCents": MAX_SAFE_CENTS,
            "occurredOn": "2026-08-02",
            "note": "",
            "accountId": account_id
        })),
        Some(&cookie),
        Some("addition-limit-debt"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _, body) = send(
        &test.router,
        Method::POST,
        &format!(
            "/api/v1/debts/{}/additions",
            at_limit["id"].as_str().unwrap()
        ),
        Some(json!({
            "amountCents": 1,
            "effectiveOn": "2026-08-02",
            "note": "",
            "accountId": account_id
        })),
        Some(&cookie),
        Some("addition-over-limit"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "validation_error");
}

#[tokio::test]
async fn addition_reopens_settled_debt_and_serializes_with_repayment() {
    let test = TestApp::new().await;
    let cookie = test
        .register_and_login("addition-balance@example.com")
        .await;
    let account = test
        .create_ledger_account(&cookie, "微信支付-余额测试", "wechat_balance")
        .await;
    let account_id = account["id"].as_str().unwrap();
    let (status, _, created) = send(
        &test.router,
        Method::POST,
        "/api/v1/debts",
        Some(json!({
            "direction": "lend_out",
            "counterpartyName": "阿青",
            "principalCents": 10_000,
            "occurredOn": "2026-08-02",
            "note": "",
            "accountId": account_id
        })),
        Some(&cookie),
        Some("addition-balance-debt"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let debt_id = created["id"].as_str().unwrap();
    let repayment_path = format!("/api/v1/debts/{debt_id}/repayments");
    let addition_path = format!("/api/v1/debts/{debt_id}/additions");

    let (status, _, settled) = send(
        &test.router,
        Method::POST,
        &repayment_path,
        Some(json!({
            "amountCents": 10_000,
            "effectiveOn": "2026-08-02",
            "note": "结清",
            "accountId": account_id
        })),
        Some(&cookie),
        Some("addition-settle-payment"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(settled["status"], "settled");

    let (status, _, reopened) = send(
        &test.router,
        Method::POST,
        &addition_path,
        Some(json!({
            "amountCents": 2_000,
            "effectiveOn": "2026-08-03",
            "note": "追加借出",
            "accountId": account_id
        })),
        Some(&cookie),
        Some("addition-reopen"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(reopened["status"], "open");
    assert_eq!(reopened["principalCents"], 12_000);
    assert_eq!(reopened["paidCents"], 10_000);
    assert_eq!(reopened["remainingCents"], 2_000);
    assert_eq!(
        reopened["principalCents"].as_i64().unwrap(),
        reopened["paidCents"].as_i64().unwrap() + reopened["remainingCents"].as_i64().unwrap()
    );

    let addition_request = send(
        &test.router,
        Method::POST,
        &addition_path,
        Some(json!({
            "amountCents": 1_000,
            "effectiveOn": "2026-08-04",
            "note": "并发追加",
            "accountId": account_id
        })),
        Some(&cookie),
        Some("addition-concurrent"),
        true,
    );
    let repayment_request = send(
        &test.router,
        Method::POST,
        &repayment_path,
        Some(json!({
            "amountCents": 2_000,
            "effectiveOn": "2026-08-04",
            "note": "并发还款",
            "accountId": account_id
        })),
        Some(&cookie),
        Some("repayment-concurrent"),
        true,
    );
    let (addition_result, repayment_result) = tokio::join!(addition_request, repayment_request);
    assert_eq!(addition_result.0, StatusCode::CREATED);
    assert_eq!(repayment_result.0, StatusCode::CREATED);

    let (status, _, final_debt) = send(
        &test.router,
        Method::GET,
        &format!("/api/v1/debts/{debt_id}"),
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(final_debt["principalCents"], 13_000);
    assert_eq!(final_debt["paidCents"], 12_000);
    assert_eq!(final_debt["remainingCents"], 1_000);
    assert_eq!(final_debt["additions"].as_array().unwrap().len(), 2);
    assert_eq!(final_debt["repayments"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn activity_records_can_be_edited_without_breaking_balances() {
    let test = TestApp::new().await;
    let cookie = test.register_and_login("activity-edit@example.com").await;
    let account = test
        .create_ledger_account(&cookie, "微信支付-流水编辑", "wechat_balance")
        .await;
    let account_id = account["id"].as_str().unwrap();
    let (status, _, created) = send(
        &test.router,
        Method::POST,
        "/api/v1/debts",
        Some(json!({
            "direction": "lend_out",
            "counterpartyName": "阿青",
            "principalCents": 10_000,
            "occurredOn": "2026-08-02",
            "note": "",
            "accountId": account_id
        })),
        Some(&cookie),
        Some("activity-edit-debt"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let debt_id = created["id"].as_str().unwrap();
    let (status, _, with_addition) = send(
        &test.router,
        Method::POST,
        &format!("/api/v1/debts/{debt_id}/additions"),
        Some(json!({
            "amountCents": 5_000,
            "effectiveOn": "2026-08-03",
            "note": "原追加",
            "accountId": account_id
        })),
        Some(&cookie),
        Some("activity-edit-addition"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let addition_id = with_addition["additions"][0]["id"].as_str().unwrap();
    let (status, _, edited_addition) = send(
        &test.router,
        Method::PATCH,
        &format!("/api/v1/debt-additions/{addition_id}"),
        Some(json!({
            "version": with_addition["version"],
            "amountCents": 3_000,
            "effectiveOn": "2026-08-04",
            "note": "修正追加",
            "accountId": account_id
        })),
        Some(&cookie),
        Some("activity-edit-addition-patch"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(edited_addition["principalCents"], 13_000);
    assert_eq!(edited_addition["remainingCents"], 13_000);
    assert_eq!(edited_addition["additions"][0]["effectiveOn"], "2026-08-04");
    assert_eq!(edited_addition["additions"][0]["note"], "修正追加");

    let (status, _, with_payment) = send(
        &test.router,
        Method::POST,
        &format!("/api/v1/debts/{debt_id}/repayments"),
        Some(json!({
            "amountCents": 4_000,
            "effectiveOn": "2026-08-05",
            "note": "原还款",
            "accountId": account_id
        })),
        Some(&cookie),
        Some("activity-edit-payment"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let payment_id = with_payment["repayments"][0]["id"].as_str().unwrap();
    let (status, _, edited_payment) = send(
        &test.router,
        Method::PATCH,
        &format!("/api/v1/repayments/{payment_id}"),
        Some(json!({
            "version": with_payment["version"],
            "amountCents": 5_000,
            "effectiveOn": "2026-08-06",
            "note": "修正还款",
            "accountId": account_id
        })),
        Some(&cookie),
        Some("activity-edit-payment-patch"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(edited_payment["paidCents"], 5_000);
    assert_eq!(edited_payment["remainingCents"], 8_000);
    assert_eq!(edited_payment["repayments"][0]["effectiveOn"], "2026-08-06");
    assert_eq!(edited_payment["repayments"][0]["note"], "修正还款");

    let (status, _, body) = send(
        &test.router,
        Method::PATCH,
        &format!("/api/v1/repayments/{payment_id}"),
        Some(json!({
            "version": edited_payment["version"],
            "amountCents": 20_000,
            "effectiveOn": "2026-08-06",
            "note": "超额",
            "accountId": account_id
        })),
        Some(&cookie),
        Some("activity-edit-payment-overpay"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "overpayment");
}

#[tokio::test]
async fn ledger_accounts_are_required_scoped_and_keep_archived_history_readable() {
    let test = TestApp::new().await;
    let cookie = test.register_and_login("accounts@example.com").await;
    let account = test
        .create_ledger_account(&cookie, "微信支付-测试号", "wechat_balance")
        .await;
    let account_id = account["id"].as_str().unwrap();

    let (status, _, _) = send(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({
            "name": "无效类型账户",
            "accountType": "bank_balance",
            "note": ""
        })),
        Some(&cookie),
        Some("invalid-ledger-account-type"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let (status, _, _) = send(
        &test.router,
        Method::POST,
        "/api/v1/debts",
        Some(json!({
            "direction": "lend_out",
            "counterpartyName": "缺少账户",
            "principalCents": 1_000,
            "occurredOn": "2026-08-02",
            "note": ""
        })),
        Some(&cookie),
        Some("missing-account-debt"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let other_cookie = test.register_and_login("accounts-other@example.com").await;
    let (status, _, body) = send(
        &test.router,
        Method::POST,
        "/api/v1/debts",
        Some(json!({
            "direction": "lend_out",
            "counterpartyName": "跨用户账户",
            "principalCents": 1_000,
            "occurredOn": "2026-08-02",
            "note": "",
            "accountId": account_id
        })),
        Some(&other_cookie),
        Some("cross-user-account-debt"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "account_unavailable");

    let (status, _, debt) = send(
        &test.router,
        Method::POST,
        "/api/v1/debts",
        Some(json!({
            "direction": "lend_out",
            "counterpartyName": "账户留痕",
            "principalCents": 10_000,
            "occurredOn": "2026-08-02",
            "note": "",
            "accountId": account_id
        })),
        Some(&cookie),
        Some("account-history-debt"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let debt_id = debt["id"].as_str().unwrap();
    assert_eq!(debt["account"]["id"], account_id);

    let (status, _, archived_account) = send(
        &test.router,
        Method::POST,
        &format!("/api/v1/ledger-accounts/{account_id}/archive"),
        Some(json!({ "version": account["version"] })),
        Some(&cookie),
        Some("archive-ledger-account"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(archived_account["archived"], true);
    assert_eq!(archived_account["usageCount"], 1);

    let (status, _, body) = send(
        &test.router,
        Method::POST,
        &format!("/api/v1/debts/{debt_id}/repayments"),
        Some(json!({
            "amountCents": 1_000,
            "effectiveOn": "2026-08-03",
            "note": "归档后不可使用",
            "accountId": account_id
        })),
        Some(&cookie),
        Some("archived-account-payment"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "account_unavailable");

    let (status, _, historical_debt) = send(
        &test.router,
        Method::GET,
        &format!("/api/v1/debts/{debt_id}"),
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(historical_debt["account"]["id"], account_id);
    assert_eq!(historical_debt["account"]["name"], "微信支付-测试号");
    assert_eq!(historical_debt["account"]["accountType"], "wechat_balance");
    assert_eq!(historical_debt["account"]["archived"], true);

    let (status, _, accounts) = send(
        &test.router,
        Method::GET,
        "/api/v1/ledger-accounts",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(accounts.as_array().unwrap().len(), 1);
    assert_eq!(accounts[0]["id"], account_id);
    assert_eq!(accounts[0]["archived"], true);

    let (status, _, body) = send(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({
            "name": "微信支付-测试号",
            "accountType": "wechat_balance",
            "note": "不应允许同名"
        })),
        Some(&cookie),
        Some("duplicate-archived-ledger-account"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "account_name_conflict");

    let (status, _, updated_account) = send(
        &test.router,
        Method::PATCH,
        &format!("/api/v1/ledger-accounts/{account_id}"),
        Some(json!({
            "name": "微信支付-主号",
            "accountType": "other",
            "note": "日常往来",
            "version": archived_account["version"]
        })),
        Some(&cookie),
        Some("update-archived-ledger-account"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated_account["name"], "微信支付-主号");
    assert_eq!(updated_account["accountType"], "other");
    assert_eq!(updated_account["archived"], true);

    let (status, _, restored_account) = send(
        &test.router,
        Method::POST,
        &format!("/api/v1/ledger-accounts/{account_id}/restore"),
        Some(json!({ "version": updated_account["version"] })),
        Some(&cookie),
        Some("restore-ledger-account"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(restored_account["archived"], false);

    let (status, _, paid) = send(
        &test.router,
        Method::POST,
        &format!("/api/v1/debts/{debt_id}/repayments"),
        Some(json!({
            "amountCents": 1_000,
            "effectiveOn": "2026-08-03",
            "note": "恢复后可使用",
            "accountId": account_id
        })),
        Some(&cookie),
        Some("restored-account-payment"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(paid["repayments"][0]["account"]["id"], account_id);
    assert_eq!(paid["repayments"][0]["account"]["name"], "微信支付-主号");
    assert_eq!(paid["repayments"][0]["account"]["accountType"], "other");
}

#[tokio::test]
async fn ledger_account_details_are_normalized_validated_and_cleared_on_type_change() {
    let test = TestApp::new().await;
    let cookie = test.register_and_login("account-details@example.com").await;

    let (status, _, bank_card) = send(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({
            "name": "工资卡",
            "accountType": "bank_card",
            "note": "详情测试",
            "bankName": "  招商银行  ",
            "branchName": "  上海世纪大道支行  ",
            "cardNumber": " 6222 0000-0000 1234 ",
            "nickname": "不应泄漏的昵称",
            "phone": "invalid-but-irrelevant",
            "email": "invalid-but-irrelevant"
        })),
        Some(&cookie),
        Some("account-details-bank-create"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(bank_card["bankName"], "招商银行");
    assert_eq!(bank_card["branchName"], "上海世纪大道支行");
    assert_eq!(bank_card["cardNumber"], "6222000000001234");
    assert!(bank_card["nickname"].is_null());
    assert!(bank_card["phone"].is_null());
    assert!(bank_card["email"].is_null());
    let account_id = bank_card["id"].as_str().unwrap();

    let (status, _, alipay) = send(
        &test.router,
        Method::PATCH,
        &format!("/api/v1/ledger-accounts/{account_id}"),
        Some(json!({
            "name": "支付宝主号",
            "accountType": "alipay_balance",
            "note": "切换类型",
            "bankName": "不应继续保留",
            "branchName": "不应继续保留",
            "cardNumber": "6222000000001234",
            "nickname": "  小余  ",
            "phone": " +86 138-0013-8000 ",
            "email": " USER@Example.com ",
            "version": bank_card["version"]
        })),
        Some(&cookie),
        Some("account-details-to-alipay"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(alipay["accountType"], "alipay_balance");
    assert!(alipay["bankName"].is_null());
    assert!(alipay["branchName"].is_null());
    assert!(alipay["cardNumber"].is_null());
    assert_eq!(alipay["nickname"], "小余");
    assert_eq!(alipay["phone"], "+86 138-0013-8000");
    assert_eq!(alipay["email"], "user@example.com");

    let (status, _, wechat) = send(
        &test.router,
        Method::PATCH,
        &format!("/api/v1/ledger-accounts/{account_id}"),
        Some(json!({
            "name": "微信零钱主号",
            "accountType": "wechat_balance",
            "note": "再次切换类型",
            "nickname": "余杭",
            "phone": "13800138000",
            "email": "not-an-email-but-irrelevant",
            "version": alipay["version"]
        })),
        Some(&cookie),
        Some("account-details-to-wechat"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(wechat["nickname"], "余杭");
    assert_eq!(wechat["phone"], "13800138000");
    assert!(wechat["email"].is_null());

    let (status, _, accounts) = send(
        &test.router,
        Method::GET,
        "/api/v1/ledger-accounts",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(accounts[0]["id"], account_id);
    assert!(accounts[0]["bankName"].is_null());
    assert!(accounts[0]["branchName"].is_null());
    assert!(accounts[0]["cardNumber"].is_null());
    assert_eq!(accounts[0]["nickname"], "余杭");
    assert_eq!(accounts[0]["phone"], "13800138000");
    assert!(accounts[0]["email"].is_null());

    let (status, _, body) = send(
        &test.router,
        Method::PATCH,
        &format!("/api/v1/ledger-accounts/{account_id}"),
        Some(json!({
            "name": "微信零钱主号",
            "accountType": "wechat_balance",
            "note": "手机号无效",
            "nickname": "余杭",
            "phone": "123456",
            "version": wechat["version"]
        })),
        Some(&cookie),
        Some("account-details-invalid-phone"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "validation_error");

    let (status, _, body) = send(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({
            "name": "无效卡号",
            "accountType": "bank_card",
            "note": "",
            "cardNumber": "123456"
        })),
        Some(&cookie),
        Some("account-details-invalid-card-number"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "validation_error");

    let (status, _, body) = send(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({
            "name": "支付宝无效邮箱",
            "accountType": "alipay_balance",
            "note": "",
            "email": "not-an-email"
        })),
        Some(&cookie),
        Some("account-details-invalid-email"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "validation_error");

    let (status, _, body) = send(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({
            "name": "银行卡名称过长",
            "accountType": "bank_card",
            "note": "",
            "bankName": "银".repeat(81)
        })),
        Some(&cookie),
        Some("account-details-long-bank"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "validation_error");

    let (status, _, blank_details) = send(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({
            "name": "空详情银行卡",
            "accountType": "bank_card",
            "note": "",
            "bankName": "   ",
            "branchName": "\n\t",
            "cardNumber": "  "
        })),
        Some(&cookie),
        Some("account-details-empty-to-null"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(blank_details["bankName"].is_null());
    assert!(blank_details["branchName"].is_null());
    assert!(blank_details["cardNumber"].is_null());
}

#[tokio::test]
async fn blank_account_names_are_derived_without_renumbering_conflicts() {
    let test = TestApp::new().await;
    let cookie = test
        .register_and_login("derived-account-names@example.com")
        .await;
    let wechat_body = json!({
        "name": "   ",
        "accountType": "wechat_balance",
        "note": "自动名称",
        "nickname": "兔子",
        "phone": "13800138000"
    });

    let (status, _, wechat) = send(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(wechat_body.clone()),
        Some(&cookie),
        Some("derived-wechat-create"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(wechat["name"], "兔子");
    assert_eq!(wechat["nameSource"], "derived");

    let (status, _, replayed) = send(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(wechat_body),
        Some(&cookie),
        Some("derived-wechat-create"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(replayed["id"], wechat["id"]);
    assert_eq!(replayed["name"], "兔子");
    assert_eq!(replayed["nameSource"], "derived");

    let (status, _, conflict) = send(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({
            "name": "",
            "accountType": "wechat_balance",
            "note": "不应自动编号",
            "nickname": "兔子"
        })),
        Some(&cookie),
        Some("derived-wechat-conflict"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["code"], "account_name_conflict");

    let account_id = wechat["id"].as_str().unwrap();
    let (status, _, rederived) = send(
        &test.router,
        Method::PATCH,
        &format!("/api/v1/ledger-accounts/{account_id}"),
        Some(json!({
            "name": "",
            "accountType": "wechat_balance",
            "note": "昵称变化后重新派生",
            "nickname": "新昵称",
            "phone": "13800138000",
            "version": wechat["version"]
        })),
        Some(&cookie),
        Some("derived-wechat-update"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rederived["name"], "新昵称");
    assert_eq!(rederived["nameSource"], "derived");

    let (status, _, custom) = send(
        &test.router,
        Method::PATCH,
        &format!("/api/v1/ledger-accounts/{account_id}"),
        Some(json!({
            "name": "  日常零钱  ",
            "accountType": "wechat_balance",
            "note": "改为自定义别名",
            "nickname": "新昵称",
            "phone": "13800138000",
            "version": rederived["version"]
        })),
        Some(&cookie),
        Some("derived-wechat-to-custom"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(custom["name"], "日常零钱");
    assert_eq!(custom["nameSource"], "custom");

    let (status, _, bank) = send(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({
            "name": "",
            "accountType": "bank_card",
            "note": "开户行兜底",
            "branchName": "上海世纪大道支行"
        })),
        Some(&cookie),
        Some("derived-bank-branch"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(bank["name"], "上海世纪大道支行");
    assert_eq!(bank["nameSource"], "derived");

    let (status, _, numbered_bank) = send(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({
            "name": "",
            "accountType": "bank_card",
            "note": "卡号优先",
            "bankName": "招商银行",
            "cardNumber": "6222 0000 0000 1234"
        })),
        Some(&cookie),
        Some("derived-bank-card-number"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(numbered_bank["name"], "6222000000001234");
    assert_eq!(numbered_bank["cardNumber"], "6222000000001234");
    assert_eq!(numbered_bank["nameSource"], "derived");

    let (status, _, alipay) = send(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({
            "accountType": "alipay_balance",
            "note": "邮箱兜底",
            "email": " USER@Example.com "
        })),
        Some(&cookie),
        Some("derived-alipay-email"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(alipay["name"], "user@example.com");
    assert_eq!(alipay["nameSource"], "derived");

    for (account_type, key) in [
        ("bank_card", "blank-bank-no-details"),
        ("wechat_balance", "blank-wechat-no-details"),
        ("alipay_balance", "blank-alipay-no-details"),
        ("cash", "blank-cash-name"),
        ("digital_cny", "blank-digital-name"),
        ("other", "blank-other-name"),
    ] {
        let (status, _, body) = send(
            &test.router,
            Method::POST,
            "/api/v1/ledger-accounts",
            Some(json!({
                "name": "",
                "accountType": account_type,
                "note": ""
            })),
            Some(&cookie),
            Some(key),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["code"], "validation_error");
    }

    let (status, _, body) = send(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({
            "name": "",
            "accountType": "bank_card",
            "note": "",
            "branchName": "支".repeat(81)
        })),
        Some(&cookie),
        Some("derived-name-too-long"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "validation_error");
}

#[tokio::test]
async fn session_mutations_require_matching_origin() {
    let test = TestApp::new().await;
    let cookie = test.register_and_login("origin@example.com").await;
    let (status, _, body) = send(
        &test.router,
        Method::POST,
        "/api/v1/debts",
        Some(json!({ "direction": "borrow_in", "counterpartyName": "甲", "principalCents": 100, "occurredOn": "2026-08-02", "note": "" })),
        Some(&cookie),
        Some("origin-test-0001"),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "forbidden");
}

#[tokio::test]
async fn cashless_debts_skip_the_principal_account_but_repayments_keep_theirs() {
    let test = TestApp::new().await;
    let cookie = test.register_and_login("cashless@example.com").await;
    let account = test
        .create_ledger_account(&cookie, "微信支付-赊账", "wechat_balance")
        .await;
    let account_id = account["id"].as_str().unwrap();

    // 默认（有实际收付款）但不给账户 → 422
    let (status, _, body) = send(
        &test.router,
        Method::POST,
        "/api/v1/debts",
        Some(json!({ "direction": "borrow_in", "counterpartyName": "代办记账", "principalCents": 150_000, "occurredOn": "2026-08-04", "accountId": null })),
        Some(&cookie),
        Some("cashless-missing-account"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "validation_error");

    // 赊账却指定账户 → 422
    let (status, _, _) = send(
        &test.router,
        Method::POST,
        "/api/v1/debts",
        Some(json!({ "direction": "borrow_in", "counterpartyName": "代办记账", "principalCents": 150_000, "occurredOn": "2026-08-04", "originKind": "no_cash_movement", "accountId": account_id })),
        Some(&cookie),
        Some("cashless-with-account"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // 新建不允许历史未指定类型 → 422
    let (status, _, _) = send(
        &test.router,
        Method::POST,
        "/api/v1/debts",
        Some(json!({ "direction": "borrow_in", "counterpartyName": "代办记账", "principalCents": 150_000, "occurredOn": "2026-08-04", "originKind": "legacy_unknown", "accountId": null })),
        Some(&cookie),
        Some("cashless-legacy-kind"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // 赊账创建成功：无账户、originKind 透出
    let (status, _, created) = send(
        &test.router,
        Method::POST,
        "/api/v1/debts",
        Some(json!({ "direction": "borrow_in", "counterpartyName": "代办记账", "principalCents": 150_000, "occurredOn": "2026-08-04", "note": "代办执照+代记账尾款", "originKind": "no_cash_movement", "accountId": null })),
        Some(&cookie),
        Some("cashless-create"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["originKind"], "no_cash_movement");
    assert!(created["account"].is_null());
    assert_eq!(created["remainingCents"], 150_000);
    let debt_id = created["id"].as_str().unwrap();
    let counterparty_id = created["counterparty"]["id"].as_str().unwrap();

    // 缺省 originKind + 账户 → 向后兼容，默认为 cash_movement
    let (status, _, cash_created) = send(
        &test.router,
        Method::POST,
        "/api/v1/debts",
        Some(json!({ "direction": "lend_out", "counterpartyName": "阿青", "principalCents": 10_000, "occurredOn": "2026-08-04", "accountId": account_id })),
        Some(&cookie),
        Some("cash-default-kind"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(cash_created["originKind"], "cash_movement");
    assert_eq!(cash_created["account"]["id"], account_id);

    // 赊账债务登记还款（有账户）→ 201，剩余减少
    let repay_path = format!("/api/v1/debts/{debt_id}/repayments");
    let (status, _, paid) = send(
        &test.router,
        Method::POST,
        &repay_path,
        Some(
            json!({ "amountCents": 50_000, "effectiveOn": "2026-08-05", "accountId": account_id }),
        ),
        Some(&cookie),
        Some("cashless-repay"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(paid["remainingCents"], 100_000);

    // 编辑：切换为有实际收付款必须补账户
    let debt_path = format!("/api/v1/debts/{debt_id}");
    let (status, _, _) = send(
        &test.router,
        Method::PATCH,
        &debt_path,
        Some(json!({ "version": paid["version"], "counterpartyId": counterparty_id, "principalCents": 150_000, "occurredOn": "2026-08-04", "originKind": "cash_movement", "accountId": null, "note": "" })),
        Some(&cookie),
        Some("cashless-to-cash-missing"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _, updated) = send(
        &test.router,
        Method::PATCH,
        &debt_path,
        Some(json!({ "version": paid["version"], "counterpartyId": counterparty_id, "principalCents": 150_000, "occurredOn": "2026-08-04", "originKind": "cash_movement", "accountId": account_id, "note": "" })),
        Some(&cookie),
        Some("cashless-to-cash"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["originKind"], "cash_movement");
    assert_eq!(updated["account"]["id"], account_id);

    // 编辑：切换回赊账需丢弃账户
    let (status, _, switched) = send(
        &test.router,
        Method::PATCH,
        &debt_path,
        Some(json!({ "version": updated["version"], "counterpartyId": counterparty_id, "principalCents": 150_000, "occurredOn": "2026-08-04", "originKind": "no_cash_movement", "accountId": null, "note": "" })),
        Some(&cookie),
        Some("cash-to-cashless"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(switched["originKind"], "no_cash_movement");
    assert!(switched["account"].is_null());
}

async fn send(
    router: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
    cookie: Option<&str>,
    idempotency_key: Option<&str>,
    with_origin: bool,
) -> (StatusCode, HeaderMap, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    if let Some(key) = idempotency_key {
        builder = builder.header("idempotency-key", key);
    }
    if with_origin {
        builder = builder.header(header::ORIGIN, "http://test.local");
    }
    let request = builder
        .body(Body::from(
            body.map(|value| value.to_string()).unwrap_or_default(),
        ))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    (status, headers, body)
}
