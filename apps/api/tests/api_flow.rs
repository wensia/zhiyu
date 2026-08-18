use std::{fs, sync::Arc};

use argon2::{
    Argon2, PasswordHasher,
    password_hash::{SaltString, rand_core::OsRng},
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Method, Request, StatusCode, header},
};
use encoding_rs::GB18030;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tower::ServiceExt;
use uuid::Uuid;
use zhiyu_api::{
    AppState, app, auth, categorize, config::Config, db, domain::MAX_SAFE_CENTS,
    email::DevFileEmailSender, rate_limit::RateLimiter,
};

#[path = "../src/bin/renormalize.rs"]
mod renormalize_bin;

#[path = "../src/bin/reself.rs"]
mod reself_bin;

struct TestApp {
    router: Router,
    root: TempDir,
    state: AppState,
}

impl TestApp {
    async fn new() -> Self {
        Self::with_env("test", "http://test.local").await
    }

    async fn self_host() -> Self {
        Self::with_env("self-host", "https://test.local").await
    }

    async fn with_env(app_env: &str, public_base_url: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let config = Config {
            app_env: app_env.into(),
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            public_base_url: public_base_url.into(),
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
            backup_status: Default::default(),
        };
        Self {
            router: app(state.clone()),
            root,
            state,
        }
    }

    async fn insert_verified_user(&self, email: &str, password: &str) {
        let salt = SaltString::generate(&mut OsRng);
        let password_hash = Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .unwrap()
            .to_string();
        let now = chrono::Utc::now().to_rfc3339();
        self.state
            .connection()
            .await
            .unwrap()
            .execute(
                "INSERT INTO users(id, email, password_hash, timezone, email_verified_at, created_at, updated_at) VALUES (?1, ?2, ?3, 'Asia/Shanghai', ?4, ?4, ?4)",
                libsql::params![Uuid::now_v7().to_string(), email, password_hash, now],
            )
            .await
            .unwrap();
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
async fn plugins_list_requires_login_and_returns_the_built_in_registry() {
    let test = TestApp::new().await;

    let (status, _, _) = send(
        &test.router,
        Method::GET,
        "/api/v1/plugins",
        None,
        None,
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let cookie = test.register_and_login("plugins-list@zhiyu.local").await;
    let (status, _, body) = send(
        &test.router,
        Method::GET,
        "/api/v1/plugins",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!([
            {
                "id": "debts",
                "name": "债务",
                "description": "记录借入、借出及还款进度。",
                "enabled": true,
                "ownsTransactions": true,
                "routePrefixes": [
                    "/api/v1/debts",
                    "/api/v1/debt-additions",
                    "/api/v1/repayments",
                    "/api/v1/counterparties",
                    "/api/v1/dashboard/summary"
                ]
            },
            {
                "id": "bill-imports",
                "name": "账单导入",
                "description": "从受支持的账单来源导入流水。",
                "enabled": true,
                "ownsTransactions": false,
                "routePrefixes": ["/api/v1/imports", "/api/v1/duplicate-suspicions"]
            },
            {
                "id": "auto-categorize",
                "name": "自动分类",
                "description": "按规则为流水自动匹配分类。",
                "enabled": true,
                "ownsTransactions": false,
                "routePrefixes": [
                    "/api/v1/category-rules",
                    "/api/v1/categories/recategorize",
                    "/api/v1/categories/rules"
                ]
            }
        ])
    );
}

#[tokio::test]
async fn plugin_settings_are_user_scoped_idempotent_and_reject_unknown_ids() {
    let test = TestApp::new().await;
    let owner = test.register_and_login("plugin-settings@zhiyu.local").await;
    let other = test
        .register_and_login("plugin-settings-other@zhiyu.local")
        .await;

    let (status, _, disabled) = send(
        &test.router,
        Method::PATCH,
        "/api/v1/plugins/debts",
        Some(json!({ "enabled": false })),
        Some(&owner),
        Some("disable-debts-setting"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{disabled}");
    assert_eq!(disabled["enabled"], false);
    assert_eq!(disabled["reconciled"], 0);

    let (status, _, replay) = send(
        &test.router,
        Method::PATCH,
        "/api/v1/plugins/debts",
        Some(json!({ "enabled": false })),
        Some(&owner),
        Some("disable-debts-setting"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay, disabled);

    let (status, _, owner_plugins) = send(
        &test.router,
        Method::GET,
        "/api/v1/plugins",
        None,
        Some(&owner),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(owner_plugins[0]["enabled"], false);
    let (status, _, other_plugins) = send(
        &test.router,
        Method::GET,
        "/api/v1/plugins",
        None,
        Some(&other),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(other_plugins[0]["enabled"], true);

    let (status, _, unknown) = send(
        &test.router,
        Method::PATCH,
        "/api/v1/plugins/not-installed",
        Some(json!({ "enabled": false })),
        Some(&owner),
        Some("disable-unknown-plugin"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{unknown}");
}

#[tokio::test]
async fn dashboards_crud_validation_default_and_idempotency() {
    let test = TestApp::new().await;
    let cookie = test.register_and_login("dashboards@zhiyu.local").await;

    let (status, _, empty) = send(
        &test.router,
        Method::GET,
        "/api/v1/dashboards",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(empty, json!([]));

    let (status, _, default) = send(
        &test.router,
        Method::POST,
        "/api/v1/dashboards/default",
        None,
        Some(&cookie),
        Some("dashboard-default-create"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{default}");
    assert_eq!(default["name"], "月度");
    assert_eq!(default["position"], 0);
    assert_eq!(default["widgets"].as_array().unwrap().len(), 4);
    assert_eq!(
        default["widgets"][0]["widgetType"],
        "core:income-expense-trend"
    );
    assert_eq!(default["widgets"][0]["x"], 0);
    assert_eq!(default["widgets"][0]["w"], 8);
    assert_eq!(default["widgets"][3]["widgetType"], "core:month-compare");

    let (status, _, replayed_default) = send(
        &test.router,
        Method::POST,
        "/api/v1/dashboards/default",
        None,
        Some(&cookie),
        Some("dashboard-default-create"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(replayed_default, default);

    let (status, _, existing_default) = send(
        &test.router,
        Method::POST,
        "/api/v1/dashboards/default",
        None,
        Some(&cookie),
        Some("dashboard-default-existing"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(existing_default, default);

    let (status, _, second) = send(
        &test.router,
        Method::POST,
        "/api/v1/dashboards",
        Some(json!({ "name": "年度" })),
        Some(&cookie),
        Some("dashboard-create-second"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{second}");
    assert_eq!(second["position"], 1);
    let second_id = second["id"].as_str().unwrap();

    let (status, _, replayed_second) = send(
        &test.router,
        Method::POST,
        "/api/v1/dashboards",
        Some(json!({ "name": "年度" })),
        Some(&cookie),
        Some("dashboard-create-second"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(replayed_second["id"], second_id);

    let (status, _, mismatch) = send(
        &test.router,
        Method::POST,
        "/api/v1/dashboards",
        Some(json!({ "name": "不同请求" })),
        Some(&cookie),
        Some("dashboard-create-second"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(mismatch["code"], "idempotency_mismatch");

    let (status, _, moved) = send(
        &test.router,
        Method::PATCH,
        &format!("/api/v1/dashboards/{second_id}"),
        Some(json!({ "name": "年度总览", "position": 0 })),
        Some(&cookie),
        Some("dashboard-update-second"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    assert_eq!(moved["name"], "年度总览");
    assert_eq!(moved["position"], 0);

    let overflow = json!([{
        "widgetType": "core:category-share",
        "pluginId": null,
        "x": 10,
        "y": 0,
        "w": 4,
        "h": 3,
        "config": {}
    }]);
    let (status, _, invalid_grid) = send(
        &test.router,
        Method::PUT,
        &format!("/api/v1/dashboards/{second_id}/widgets"),
        Some(overflow),
        Some(&cookie),
        Some("dashboard-widget-overflow"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{invalid_grid}");

    let unknown = json!([{
        "widgetType": "plugin:not-installed:overview",
        "pluginId": "not-installed",
        "x": 0,
        "y": 0,
        "w": 4,
        "h": 3,
        "config": {}
    }]);
    let (status, _, invalid_plugin) = send(
        &test.router,
        Method::PUT,
        &format!("/api/v1/dashboards/{second_id}/widgets"),
        Some(unknown),
        Some(&cookie),
        Some("dashboard-widget-unknown"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{invalid_plugin}");

    let (status, _, _) = send(
        &test.router,
        Method::PATCH,
        "/api/v1/plugins/debts",
        Some(json!({ "enabled": false })),
        Some(&cookie),
        Some("dashboard-disable-widget-plugin"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let valid_widgets = json!([{
        "id": "stable-widget-id",
        "widgetType": "plugin:debts:overview",
        "pluginId": "debts",
        "x": 0,
        "y": 0,
        "w": 4,
        "h": 3,
        "config": { "display": "compact" }
    }]);
    let (status, _, replaced) = send(
        &test.router,
        Method::PUT,
        &format!("/api/v1/dashboards/{second_id}/widgets"),
        Some(valid_widgets.clone()),
        Some(&cookie),
        Some("dashboard-widget-replace"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replaced}");
    assert_eq!(replaced["widgets"].as_array().unwrap().len(), 1);
    assert_eq!(replaced["widgets"][0]["id"], "stable-widget-id");
    assert_eq!(replaced["widgets"][0]["config"]["display"], "compact");

    let (status, _, replayed_replace) = send(
        &test.router,
        Method::PUT,
        &format!("/api/v1/dashboards/{second_id}/widgets"),
        Some(valid_widgets),
        Some(&cookie),
        Some("dashboard-widget-replace"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replayed_replace, replaced);

    let (status, _, widget_types) = send(
        &test.router,
        Method::GET,
        "/api/v1/dashboards/widget-types",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{widget_types}");
    assert_eq!(widget_types["core"].as_array().unwrap().len(), 4);
    assert_eq!(widget_types["plugins"][0]["enabled"], false);
    assert_eq!(widget_types["plugins"][0]["widgets"][0]["id"], "overview");

    let (status, _, _) = send(
        &test.router,
        Method::DELETE,
        &format!("/api/v1/dashboards/{second_id}"),
        None,
        Some(&cookie),
        Some("dashboard-delete-second"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _) = send(
        &test.router,
        Method::DELETE,
        &format!("/api/v1/dashboards/{second_id}"),
        None,
        Some(&cookie),
        Some("dashboard-delete-second"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let default_id = default["id"].as_str().unwrap();
    let (status, _, last_page) = send(
        &test.router,
        Method::DELETE,
        &format!("/api/v1/dashboards/{default_id}"),
        None,
        Some(&cookie),
        Some("dashboard-delete-last"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(last_page["code"], "last_dashboard");
}

#[tokio::test]
async fn statistics_aggregate_supports_all_groupings_and_core_semantics() {
    let test = TestApp::new().await;
    let email = "statistics-aggregate@zhiyu.local";
    let cookie = test.register_and_login(email).await;
    let account_a = test
        .create_ledger_account(&cookie, "统计账户甲", "cash")
        .await;
    let (status, _, account_b) = send(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({ "name": "统计账户乙", "accountType": "cash", "note": "" })),
        Some(&cookie),
        Some("aggregate-account-second"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _, income_category) = send(
        &test.router,
        Method::POST,
        "/api/v1/categories",
        Some(json!({ "parentId": null, "name": "测试收入类", "kind": "income", "sortOrder": 0 })),
        Some(&cookie),
        Some("aggregate-income-category"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _, expense_category) = send(
        &test.router,
        Method::POST,
        "/api/v1/categories",
        Some(json!({ "parentId": null, "name": "测试支出类", "kind": "expense", "sortOrder": 0 })),
        Some(&cookie),
        Some("aggregate-expense-category"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let conn = test.state.connection().await.unwrap();
    let user_id: String = conn
        .query("SELECT id FROM users WHERE email=?1", [email])
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    let account_a_id = account_a["id"].as_str().unwrap();
    let account_b_id = account_b["id"].as_str().unwrap();
    let income_category_id = income_category["id"].as_str().unwrap();
    let expense_category_id = expense_category["id"].as_str().unwrap();
    let now = "2026-01-01T00:00:00Z";
    for (
        id,
        kind,
        amount,
        occurred_on,
        account_id,
        category_id,
        category,
        pnl_scope,
        archived_at,
    ) in [
        (
            "agg-income-jan",
            "income",
            1_000_i64,
            "2026-01-05",
            Some(account_a_id),
            Some(income_category_id),
            "",
            "counted",
            None,
        ),
        (
            "agg-expense-jan",
            "expense",
            400,
            "2026-01-06",
            Some(account_a_id),
            Some(expense_category_id),
            "",
            "counted",
            None,
        ),
        (
            "agg-income-feb",
            "income",
            2_000,
            "2026-02-01",
            Some(account_a_id),
            Some(income_category_id),
            "",
            "counted",
            None,
        ),
        (
            "agg-unclassified",
            "expense",
            300,
            "2026-01-07",
            Some(account_b_id),
            None,
            "",
            "counted",
            None,
        ),
        (
            "agg-excluded",
            "expense",
            9_999,
            "2026-01-08",
            Some(account_a_id),
            Some(expense_category_id),
            "",
            "excluded",
            None,
        ),
        (
            "agg-archived",
            "income",
            8_888,
            "2026-01-09",
            Some(account_a_id),
            Some(income_category_id),
            "",
            "counted",
            Some(now),
        ),
    ] {
        conn.execute(
            "INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,occurred_on,category,category_id,category_source,account_id,pnl_scope,archived_at,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,CASE WHEN ?7 IS NULL THEN 'none' ELSE 'user' END,?8,?9,?10,?11,?11)",
            libsql::params![id, user_id.clone(), kind, amount, occurred_on, category, category_id, account_id, pnl_scope, archived_at, now],
        )
        .await
        .unwrap();
    }
    conn.execute(
        "INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,occurred_on,transfer_from_account_id,transfer_to_account_id,pnl_scope,created_at,updated_at) VALUES ('agg-transfer',?1,'transfer',777,'2026-01-10',?2,?3,'counted',?4,?4)",
        libsql::params![user_id, account_a_id, account_b_id, now],
    )
    .await
    .unwrap();

    let aggregate = |group_by: &str| {
        format!("/api/v1/statistics/aggregate?from=2026-01-01&to=2026-03-01&groupBy={group_by}")
    };
    let (status, _, by_day) = send(
        &test.router,
        Method::GET,
        &aggregate("day"),
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{by_day}");
    assert_eq!(by_day.as_array().unwrap().len(), 4);
    assert_eq!(
        by_day[0],
        json!({ "key": "2026-01-05", "label": "2026-01-05", "incomeCents": 1000, "expenseCents": 0, "count": 1 })
    );

    let (status, _, by_month) = send(
        &test.router,
        Method::GET,
        &aggregate("month"),
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{by_month}");
    assert_eq!(
        by_month,
        json!([
            { "key": "2026-01", "label": "2026-01", "incomeCents": 1000, "expenseCents": 700, "count": 3 },
            { "key": "2026-02", "label": "2026-02", "incomeCents": 2000, "expenseCents": 0, "count": 1 }
        ])
    );

    let (status, _, by_category) = send(
        &test.router,
        Method::GET,
        &aggregate("category"),
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{by_category}");
    assert!(
        by_category
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["label"] == "" && item["expenseCents"] == 300)
    );
    assert!(
        by_category
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["label"] == "测试收入类" && item["incomeCents"] == 3000)
    );
    assert!(
        by_category
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["label"] == "测试支出类" && item["expenseCents"] == 400)
    );

    let (status, _, by_account) = send(
        &test.router,
        Method::GET,
        &aggregate("account"),
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{by_account}");
    assert!(
        by_account
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["label"] == "统计账户甲"
                && item["incomeCents"] == 3000
                && item["expenseCents"] == 400)
    );
    assert!(
        by_account
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["label"] == "统计账户乙" && item["expenseCents"] == 300)
    );

    let (status, _, filtered) = send(
        &test.router,
        Method::GET,
        &format!(
            "{}&accountId={account_a_id}&categoryId={income_category_id}&kind=income",
            aggregate("month")
        ),
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{filtered}");
    assert_eq!(filtered.as_array().unwrap().len(), 2);
    assert!(
        filtered
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["expenseCents"] == 0)
    );

    let (status, _, range_error) = send(
        &test.router,
        Method::GET,
        "/api/v1/statistics/aggregate?from=2025-01-01&to=2026-01-03&groupBy=day",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(range_error["code"], "validation_error");
}

#[tokio::test]
async fn debt_cash_writes_create_sync_and_archive_only_automatic_transactions() {
    let test = TestApp::new().await;
    let cookie = test.register_and_login("debt-auto@example.com").await;
    let first_account = test
        .create_ledger_account(&cookie, "自动流水账户甲", "cash")
        .await;
    let (status, _, second_account) = send(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({
            "name": "自动流水账户乙",
            "accountType": "cash",
            "note": ""
        })),
        Some(&cookie),
        Some("auto-debt-second-account"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let first_account_id = first_account["id"].as_str().unwrap();
    let second_account_id = second_account["id"].as_str().unwrap();

    let (status, _, debt) = send(
        &test.router,
        Method::POST,
        "/api/v1/debts",
        Some(json!({
            "direction": "borrow_in",
            "counterpartyName": "自动流水往来方甲",
            "principalCents": 1_000,
            "occurredOn": "2026-08-01",
            "accountId": first_account_id,
            "note": ""
        })),
        Some(&cookie),
        Some("auto-debt-create"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{debt}");
    let debt_id = debt["id"].as_str().unwrap().to_owned();
    let counterparty_id = debt["counterparty"]["id"].as_str().unwrap();
    let transaction_id = debt["transactionId"].as_str().unwrap().to_owned();

    let (status, _, updated) = send(
        &test.router,
        Method::PATCH,
        &format!("/api/v1/debts/{debt_id}"),
        Some(json!({
            "version": debt["version"],
            "counterpartyId": counterparty_id,
            "accountId": second_account_id,
            "originKind": "cash_movement",
            "principalCents": 1_200,
            "occurredOn": "2026-08-02",
            "dueOn": null,
            "note": ""
        })),
        Some(&cookie),
        Some("auto-debt-update"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["transactionId"], transaction_id);
    let conn = test.state.connection().await.unwrap();
    let mut rows = conn.query(
        "SELECT d.transaction_auto_created, t.kind, t.amount_cents, t.occurred_on, t.account_id, t.archived_at FROM debts d JOIN ledger_transactions t ON t.id = d.transaction_id WHERE d.id = ?1",
        [debt_id.clone()],
    ).await.unwrap();
    let row = rows.next().await.unwrap().unwrap();
    assert_eq!(row.get::<i64>(0).unwrap(), 1);
    assert_eq!(row.get::<String>(1).unwrap(), "income");
    assert_eq!(row.get::<i64>(2).unwrap(), 1_200);
    assert_eq!(row.get::<String>(3).unwrap(), "2026-08-02");
    assert_eq!(row.get::<String>(4).unwrap(), second_account_id);
    assert!(row.get::<Option<String>>(5).unwrap().is_none());
    drop(row);
    drop(rows);
    drop(conn);

    let (status, _, archived) = send(
        &test.router,
        Method::POST,
        &format!("/api/v1/debts/{debt_id}/archive"),
        Some(json!({ "version": updated["version"] })),
        Some(&cookie),
        Some("auto-debt-archive"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{archived}");
    let conn = test.state.connection().await.unwrap();
    let mut rows = conn
        .query(
            "SELECT archived_at FROM ledger_transactions WHERE id = ?1",
            [transaction_id.clone()],
        )
        .await
        .unwrap();
    assert!(
        rows.next()
            .await
            .unwrap()
            .unwrap()
            .get::<Option<String>>(0)
            .unwrap()
            .is_none()
    );
    drop(rows);
    drop(conn);

    let (status, _, restored) = send(
        &test.router,
        Method::POST,
        &format!("/api/v1/debts/{debt_id}/restore"),
        Some(json!({ "version": archived["version"] })),
        Some(&cookie),
        Some("auto-debt-restore"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = send(
        &test.router,
        Method::DELETE,
        &format!("/api/v1/debts/{debt_id}"),
        Some(json!({ "version": restored["version"] })),
        Some(&cookie),
        Some("auto-debt-delete"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let conn = test.state.connection().await.unwrap();
    let mut rows = conn
        .query(
            "SELECT archived_at FROM ledger_transactions WHERE id = ?1",
            [transaction_id.clone()],
        )
        .await
        .unwrap();
    assert!(
        rows.next()
            .await
            .unwrap()
            .unwrap()
            .get::<Option<String>>(0)
            .unwrap()
            .is_some()
    );
    drop(rows);
    let mut rows = conn
        .query(
            "SELECT COUNT(*) FROM transaction_links WHERE transaction_id = ?1",
            [transaction_id],
        )
        .await
        .unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        0
    );
}

#[tokio::test]
async fn plugin_owned_transaction_rejects_core_delete_and_archival_reconciliation_still_detaches() {
    let test = TestApp::new().await;
    let cookie = test
        .register_and_login("debt-reconcile@example.invalid")
        .await;
    let account = test
        .create_ledger_account(&cookie, "自检测试账户", "cash")
        .await;
    let (status, _, debt) = send(
        &test.router,
        Method::POST,
        "/api/v1/debts",
        Some(json!({
            "direction": "borrow_in",
            "counterpartyName": "自检测试往来方",
            "principalCents": 1_000,
            "occurredOn": "2026-08-18",
            "accountId": account["id"],
            "note": ""
        })),
        Some(&cookie),
        Some("debt-reconcile-create"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{debt}");
    let debt_id = debt["id"].as_str().unwrap();
    let transaction_id = debt["transactionId"].as_str().unwrap();
    let (status, _, list) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions?pageSize=200",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let transaction = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == transaction_id)
        .unwrap();
    assert_eq!(transaction["createdBy"], "plugin:debts");
    assert_eq!(transaction["links"].as_array().unwrap().len(), 1);

    let (status, _, deleted) = send(
        &test.router,
        Method::DELETE,
        &format!("/api/v1/transactions/{transaction_id}"),
        Some(json!({ "version": transaction["version"] })),
        Some(&cookie),
        Some("debt-reconcile-delete"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{deleted}");
    assert_eq!(deleted["code"], "plugin_owned_transaction");
    assert_eq!(deleted["message"], "这笔由债务创建，请在债务里删除对应记录");

    let conn = test.state.connection().await.unwrap();
    let mut rows = conn
        .query("SELECT user_id FROM debts WHERE id=?1", [debt_id])
        .await
        .unwrap();
    let user_id = rows
        .next()
        .await
        .unwrap()
        .unwrap()
        .get::<String>(0)
        .unwrap();
    drop(rows);
    conn.execute(
        "UPDATE ledger_transactions SET archived_at=?1 WHERE id=?2 AND user_id=?3",
        libsql::params![
            chrono::Utc::now().to_rfc3339(),
            transaction_id,
            user_id.clone()
        ],
    )
    .await
    .unwrap();
    let repaired = zhiyu_api::debts::reconcile_transaction_links(&conn, &user_id)
        .await
        .unwrap();
    assert_eq!(repaired, 1);
    let mut rows = conn
        .query(
            "SELECT d.transaction_id,d.transaction_auto_created,t.pnl_scope,(SELECT COUNT(*) FROM transaction_links l WHERE l.transaction_id=t.id) FROM debts d JOIN ledger_transactions t ON t.id=?1 WHERE d.id=?2",
            libsql::params![transaction_id, debt_id],
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    assert!(row.get::<Option<String>>(0).unwrap().is_none());
    assert_eq!(row.get::<i64>(1).unwrap(), 0);
    assert_eq!(row.get::<String>(2).unwrap(), "counted");
    assert_eq!(row.get::<i64>(3).unwrap(), 0);
}

#[tokio::test]
async fn debt_reconciliation_recreates_an_account_backed_transaction_that_is_missing() {
    let test = TestApp::new().await;
    let cookie = test
        .register_and_login("debt-reconcile-missing@example.invalid")
        .await;
    let account = test
        .create_ledger_account(&cookie, "缺失流水自检账户", "cash")
        .await;
    let (status, _, debt) = send(
        &test.router,
        Method::POST,
        "/api/v1/debts",
        Some(json!({
            "direction": "lend_out",
            "counterpartyName": "缺失流水自检往来方",
            "principalCents": 2_000,
            "occurredOn": "2026-08-18",
            "accountId": account["id"],
            "note": ""
        })),
        Some(&cookie),
        Some("debt-reconcile-missing-create"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{debt}");
    let debt_id = debt["id"].as_str().unwrap();
    let missing_transaction_id = debt["transactionId"].as_str().unwrap();
    let conn = test.state.connection().await.unwrap();
    let user_id: String = conn
        .query("SELECT user_id FROM debts WHERE id=?1", [debt_id])
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    conn.execute_batch("PRAGMA foreign_keys=OFF;")
        .await
        .unwrap();
    conn.execute(
        "DELETE FROM ledger_transactions WHERE id=?1 AND user_id=?2",
        libsql::params![missing_transaction_id, user_id.clone()],
    )
    .await
    .unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON;").await.unwrap();

    let repaired = zhiyu_api::debts::reconcile_transaction_links(&conn, &user_id)
        .await
        .unwrap();
    assert_eq!(repaired, 1);
    let row = conn.query(
        "SELECT d.transaction_id,d.transaction_auto_created,t.created_by,t.pnl_scope,COUNT(l.id) FROM debts d JOIN ledger_transactions t ON t.id=d.transaction_id LEFT JOIN transaction_links l ON l.transaction_id=t.id AND l.user_id=t.user_id WHERE d.id=?1 GROUP BY d.id,t.id",
        [debt_id],
    ).await.unwrap().next().await.unwrap().unwrap();
    let rebuilt_transaction_id: String = row.get(0).unwrap();
    assert_ne!(rebuilt_transaction_id, missing_transaction_id);
    assert_eq!(row.get::<i64>(1).unwrap(), 1);
    assert_eq!(row.get::<String>(2).unwrap(), "plugin:debts");
    assert_eq!(row.get::<String>(3).unwrap(), "excluded");
    assert_eq!(row.get::<i64>(4).unwrap(), 1);
}

#[tokio::test]
async fn issuing_an_api_key_keeps_existing_integrations_authenticated() {
    let test = TestApp::new().await;
    let first_key = auth::issue_api_key(&test.state, "machine@zhiyu.local")
        .await
        .unwrap();

    auth::issue_api_key(&test.state, "machine@zhiyu.local")
        .await
        .unwrap();

    let (status, _, _) = send(
        &test.router,
        Method::GET,
        "/api/v1/auth/me",
        None,
        None,
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, user) = send_with_authorization(
        &test.router,
        Method::GET,
        "/api/v1/auth/me",
        None,
        &format!("Bearer {first_key}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(user["email"], "machine@zhiyu.local");
}

#[tokio::test]
async fn valid_api_key_authenticates_and_is_not_stored_in_plaintext() {
    let test = TestApp::new().await;
    let key = auth::issue_api_key(&test.state, "valid-key@zhiyu.local")
        .await
        .unwrap();
    let conn = test.state.connection().await.unwrap();
    let mut rows = conn
        .query("SELECT token_hash FROM api_keys", ())
        .await
        .unwrap();
    let stored_hash: String = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_ne!(stored_hash, key);
    assert_eq!(stored_hash.len(), 64);
    drop(rows);
    drop(conn);

    let (status, _, user) = send_with_authorization(
        &test.router,
        Method::GET,
        "/api/v1/auth/me",
        None,
        &format!("Bearer {key}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(user["email"], "valid-key@zhiyu.local");
}

#[tokio::test]
async fn api_key_can_issue_a_self_host_session_cookie() {
    let test = TestApp::self_host().await;
    let key = auth::issue_api_key(&test.state, "desktop@zhiyu.local")
        .await
        .unwrap();

    let (status, headers, body) = send_with_authorization(
        &test.router,
        Method::POST,
        "/api/v1/auth/session-from-key",
        None,
        &format!("Bearer {key}"),
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["email"], "desktop@zhiyu.local");
    assert_eq!(body["sessionCookie"]["name"], "__Host-zhiyu_session");
    assert_eq!(body["sessionCookie"]["maxAge"], 30 * 86_400);
    let cookie = headers.get(header::SET_COOKIE).unwrap().to_str().unwrap();
    assert!(cookie.starts_with("__Host-zhiyu_session="));
    assert!(cookie.contains("; Path=/"));
    assert!(cookie.contains("; HttpOnly"));
    assert!(cookie.contains("; SameSite=Lax"));
    assert!(cookie.contains("; Secure"));
    assert!(!cookie.contains("Domain="));
}

#[tokio::test]
async fn session_from_key_rejects_invalid_and_empty_api_keys() {
    let test = TestApp::new().await;

    for authorization in ["Bearer definitely-invalid", "Bearer "] {
        let (status, _, body) = send_with_authorization(
            &test.router,
            Method::POST,
            "/api/v1/auth/session-from-key",
            None,
            authorization,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], "unauthorized");
    }
}

#[tokio::test]
async fn session_from_key_does_not_accept_a_session_cookie() {
    let test = TestApp::new().await;
    let cookie = test.register_and_login("cookie-only@zhiyu.local").await;

    let (status, _, body) = send(
        &test.router,
        Method::POST,
        "/api/v1/auth/session-from-key",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "unauthorized");
}

#[tokio::test]
async fn session_from_key_bearer_request_does_not_require_origin() {
    let test = TestApp::self_host().await;
    let key = auth::issue_api_key(&test.state, "no-origin@zhiyu.local")
        .await
        .unwrap();

    let (status, _, _) = send_with_authorization(
        &test.router,
        Method::POST,
        "/api/v1/auth/session-from-key",
        None,
        &format!("Bearer {key}"),
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn session_from_key_issued_cookie_authenticates_later_requests() {
    let test = TestApp::self_host().await;
    let key = auth::issue_api_key(&test.state, "session-user@zhiyu.local")
        .await
        .unwrap();
    let (status, _, body) = send_with_authorization(
        &test.router,
        Method::POST,
        "/api/v1/auth/session-from-key",
        None,
        &format!("Bearer {key}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let cookie = format!(
        "{}={}",
        body["sessionCookie"]["name"].as_str().unwrap(),
        body["sessionCookie"]["value"].as_str().unwrap()
    );

    let (status, _, user) = send(
        &test.router,
        Method::GET,
        "/api/v1/auth/me",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(user["email"], "session-user@zhiyu.local");
}

async fn issue_handoff_ticket(test: &TestApp, email: &str) -> String {
    let key = auth::issue_api_key(&test.state, email).await.unwrap();
    let (status, headers, body) = send_with_authorization(
        &test.router,
        Method::POST,
        "/api/v1/auth/handoff-tickets",
        None,
        &format!("Bearer {key}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
    body["ticket"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn handoff_ticket_is_short_lived_hashed_and_bearer_only() {
    let test = TestApp::new().await;
    let cookie = test.register_and_login("ticket-cookie@zhiyu.local").await;
    let (status, _, _) = send(
        &test.router,
        Method::POST,
        "/api/v1/auth/handoff-tickets",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let ticket = issue_handoff_ticket(&test, "ticket@zhiyu.local").await;
    let conn = test.state.connection().await.unwrap();
    let mut rows = conn
        .query(
            "SELECT token_hash, created_at, expires_at FROM handoff_tickets",
            (),
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let token_hash: String = row.get(0).unwrap();
    let created_at: String = row.get(1).unwrap();
    let expires_at: String = row.get(2).unwrap();
    assert_ne!(token_hash, ticket);
    assert_eq!(token_hash.len(), 64);
    let lifetime = chrono::DateTime::parse_from_rfc3339(&expires_at).unwrap()
        - chrono::DateTime::parse_from_rfc3339(&created_at).unwrap();
    assert_eq!(lifetime.num_seconds(), 60);
}

#[tokio::test]
async fn valid_desktop_handoff_sets_cookie_and_redirects_despite_local_origin() {
    let test = TestApp::self_host().await;
    let ticket = issue_handoff_ticket(&test, "handoff-valid@zhiyu.local").await;

    let (status, headers, _) = send_form(
        &test.router,
        "/desktop/handoff",
        &format!("ticket={ticket}"),
        None,
        Some("tauri://localhost"),
    )
    .await;

    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers.get(header::LOCATION).unwrap(), "/");
    assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
    assert_eq!(headers.get(header::REFERRER_POLICY).unwrap(), "no-referrer");
    let set_cookie = headers.get(header::SET_COOKIE).unwrap().to_str().unwrap();
    assert!(set_cookie.starts_with("__Host-zhiyu_session="));
    assert!(set_cookie.contains("; Path=/"));
    assert!(set_cookie.contains("; HttpOnly"));
    assert!(set_cookie.contains("; SameSite=Lax"));
    assert!(set_cookie.contains("; Secure"));
    assert!(!set_cookie.contains("Domain="));

    let cookie = set_cookie.split(';').next().unwrap();
    let (status, _, user) = send(
        &test.router,
        Method::GET,
        "/api/v1/auth/me",
        None,
        Some(cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(user["email"], "handoff-valid@zhiyu.local");
}

#[tokio::test]
async fn expired_desktop_handoff_redirects_without_cookie() {
    let test = TestApp::self_host().await;
    let ticket = issue_handoff_ticket(&test, "handoff-expired@zhiyu.local").await;
    test.state
        .connection()
        .await
        .unwrap()
        .execute(
            "UPDATE handoff_tickets SET expires_at = ?1",
            [(chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339()],
        )
        .await
        .unwrap();

    let (status, headers, _) = send_form(
        &test.router,
        "/desktop/handoff",
        &format!("ticket={ticket}"),
        None,
        Some("tauri://localhost"),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(!headers.contains_key(header::SET_COOKIE));
}

#[tokio::test]
async fn consumed_desktop_handoff_redirects_without_a_second_cookie() {
    let test = TestApp::self_host().await;
    let ticket = issue_handoff_ticket(&test, "handoff-consumed@zhiyu.local").await;
    let (_, first_headers, _) = send_form(
        &test.router,
        "/desktop/handoff",
        &format!("ticket={ticket}"),
        None,
        Some("tauri://localhost"),
    )
    .await;
    assert!(first_headers.contains_key(header::SET_COOKIE));

    let (status, second_headers, _) = send_form(
        &test.router,
        "/desktop/handoff",
        &format!("ticket={ticket}"),
        None,
        Some("tauri://localhost"),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(!second_headers.contains_key(header::SET_COOKIE));
}

#[tokio::test]
async fn concurrent_desktop_handoff_consumption_succeeds_exactly_once() {
    let test = TestApp::self_host().await;
    let ticket = issue_handoff_ticket(&test, "handoff-race@zhiyu.local").await;
    let body = format!("ticket={ticket}");
    let (first, second) = tokio::join!(
        send_form(
            &test.router,
            "/desktop/handoff",
            &body,
            None,
            Some("tauri://localhost")
        ),
        send_form(
            &test.router,
            "/desktop/handoff",
            &body,
            None,
            Some("tauri://localhost")
        )
    );
    assert_eq!(first.0, StatusCode::SEE_OTHER);
    assert_eq!(second.0, StatusCode::SEE_OTHER);
    let cookie_count = [first.1, second.1]
        .into_iter()
        .filter(|headers| headers.contains_key(header::SET_COOKIE))
        .count();
    assert_eq!(cookie_count, 1);
}

#[tokio::test]
async fn invalid_desktop_handoff_ignores_existing_session_cookie() {
    let test = TestApp::new().await;
    let cookie = test
        .register_and_login("handoff-old-cookie@zhiyu.local")
        .await;
    let (status, headers, _) = send_form(
        &test.router,
        "/desktop/handoff",
        "ticket=not-a-valid-ticket",
        Some(&cookie),
        Some("tauri://localhost"),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers.get(header::LOCATION).unwrap(), "/");
    assert!(!headers.contains_key(header::SET_COOKIE));
}

#[tokio::test]
async fn invalid_api_key_is_rejected() {
    let test = TestApp::new().await;
    let (status, _, body) = send_with_authorization(
        &test.router,
        Method::GET,
        "/api/v1/auth/me",
        None,
        "Bearer definitely-invalid",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "unauthorized");
}

#[tokio::test]
async fn empty_api_key_is_rejected() {
    let test = TestApp::new().await;
    let (status, _, body) = send_with_authorization(
        &test.router,
        Method::GET,
        "/api/v1/auth/me",
        None,
        "Bearer ",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "unauthorized");
}

#[tokio::test]
async fn expired_api_key_is_rejected() {
    let test = TestApp::new().await;
    let key = auth::issue_api_key(&test.state, "expired-key@zhiyu.local")
        .await
        .unwrap();
    test.state
        .connection()
        .await
        .unwrap()
        .execute(
            "UPDATE api_keys SET expires_at = '2000-01-01T00:00:00Z'",
            (),
        )
        .await
        .unwrap();
    let (status, _, body) = send_with_authorization(
        &test.router,
        Method::GET,
        "/api/v1/auth/me",
        None,
        &format!("Bearer {key}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "unauthorized");
}

#[tokio::test]
async fn api_key_write_does_not_require_csrf_origin() {
    let test = TestApp::new().await;
    let key = auth::issue_api_key(&test.state, "csrf-key@zhiyu.local")
        .await
        .unwrap();
    let (status, _, _) = send_with_authorization(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({ "name": "API 现金", "accountType": "cash" })),
        &format!("Bearer {key}"),
        Some("api-key-no-csrf-0001"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn expired_session_cookie_falls_back_to_bearer_without_csrf_check() {
    let test = TestApp::new().await;
    let key = auth::issue_api_key(&test.state, "expired-cookie@zhiyu.local")
        .await
        .unwrap();
    let (status, _, _) = send_with_credentials(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({ "name": "Bearer 现金", "accountType": "cash" })),
        Some("zhiyu_session=definitely-expired"),
        Some(&format!("Bearer {key}")),
        Some("https://wrong-origin.example"),
        Some("expired-cookie-bearer-0001"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn valid_session_cookie_with_wrong_origin_is_forbidden() {
    let test = TestApp::new().await;
    let cookie = test.register_and_login("wrong-origin@zhiyu.local").await;
    let (status, _, body) = send_with_credentials(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({ "name": "错误来源", "accountType": "cash" })),
        Some(&cookie),
        None,
        Some("https://wrong-origin.example"),
        Some("wrong-origin-session-0001"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "forbidden");
}

#[tokio::test]
async fn valid_session_cookie_with_matching_origin_succeeds() {
    let test = TestApp::new().await;
    let cookie = test.register_and_login("matching-origin@zhiyu.local").await;
    let (status, _, _) = send_with_credentials(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({ "name": "正确来源", "accountType": "cash" })),
        Some(&cookie),
        None,
        Some("http://test.local"),
        Some("matching-origin-session-0001"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn session_origin_matching_only_equates_exact_loopback_hosts() {
    let cases = [
        ("http://127.0.0.1:5173", "http://localhost:5173", true),
        ("http://localhost:5173", "http://127.0.0.1:5173", true),
        ("http://[::1]:5173", "http://localhost:5173", true),
        ("http://localhost", "http://127.0.0.1:80", true),
        ("http://127.0.0.1:5173", "http://localhost:6000", false),
        ("http://127.0.0.1:5173", "https://localhost:5173", false),
        ("http://127.0.0.1:5173", "http://evil.example.com", false),
        ("http://127.0.0.1:5173", "http://127.0.0.2:5173", false),
        (
            "https://app.example.com",
            "https://other.example.com",
            false,
        ),
        ("http://127.0.0.1:5173", "not a valid URL", false),
    ];

    for (index, (public_base_url, origin, should_succeed)) in cases.into_iter().enumerate() {
        let test = TestApp::with_env("test", public_base_url).await;
        let cookie = test
            .register_and_login(&format!("origin-{index}@zhiyu.local"))
            .await;
        let (status, _, body) = send_with_credentials(
            &test.router,
            Method::POST,
            "/api/v1/ledger-accounts",
            Some(json!({ "name": format!("来源测试 {index}"), "accountType": "cash" })),
            Some(&cookie),
            None,
            Some(origin),
            Some(&format!("origin-equivalence-{index:04}")),
        )
        .await;

        if should_succeed {
            assert_eq!(
                status,
                StatusCode::CREATED,
                "{public_base_url} <- {origin}: {body}"
            );
        } else {
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{public_base_url} <- {origin}: {body}"
            );
            assert_eq!(body["code"], "forbidden");
            assert_eq!(
                body["message"],
                format!("请求来源 {origin} 与服务端配置的 {public_base_url} 不一致")
            );
        }
    }
}

#[tokio::test]
async fn write_without_cookie_or_bearer_is_unauthorized() {
    let test = TestApp::new().await;
    let (status, _, body) = send_with_credentials(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({ "name": "未认证", "accountType": "cash" })),
        None,
        None,
        None,
        Some("no-credentials-0001"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "unauthorized");
}

#[tokio::test]
async fn similarly_named_cookie_with_bearer_does_not_trigger_csrf_check() {
    let test = TestApp::new().await;
    let key = auth::issue_api_key(&test.state, "similar-cookie@zhiyu.local")
        .await
        .unwrap();
    let (status, _, _) = send_with_credentials(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({ "name": "无关 Cookie", "accountType": "cash" })),
        Some("zhiyu_session_x=1"),
        Some(&format!("Bearer {key}")),
        Some("https://wrong-origin.example"),
        Some("similar-cookie-bearer-0001"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn setting_password_revokes_old_sessions_and_allows_new_login() {
    let test = TestApp::new().await;
    test.insert_verified_user("passwd@zhiyu.local", "a-very-secure-password")
        .await;
    let (status, headers, _) = send(
        &test.router,
        Method::POST,
        "/api/v1/auth/login",
        Some(json!({ "email": "passwd@zhiyu.local", "password": "a-very-secure-password" })),
        None,
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let old_cookie = headers
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();

    auth::set_password(
        &test.state,
        "passwd@zhiyu.local",
        "the-new-secure-password".into(),
    )
    .await
    .unwrap();

    let (status, _, _) = send(
        &test.router,
        Method::GET,
        "/api/v1/auth/me",
        None,
        Some(&old_cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = send(
        &test.router,
        Method::POST,
        "/api/v1/auth/login",
        Some(json!({ "email": "passwd@zhiyu.local", "password": "the-new-secure-password" })),
        None,
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn setting_password_rejects_unknown_user_without_creating_it() {
    let test = TestApp::new().await;
    let error = auth::set_password(
        &test.state,
        "missing@zhiyu.local",
        "a-strong-new-password".into(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::NOT_FOUND);
    assert_eq!(error.message, "用户不存在：missing@zhiyu.local");

    let mut rows = test
        .state
        .connection()
        .await
        .unwrap()
        .query("SELECT COUNT(*) FROM users", ())
        .await
        .unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        0
    );
}

#[tokio::test]
async fn setting_password_rejects_weak_password_and_keeps_api_keys() {
    let test = TestApp::new().await;
    let key = auth::issue_api_key(&test.state, "credentials@zhiyu.local")
        .await
        .unwrap();
    let error = auth::set_password(&test.state, "credentials@zhiyu.local", "short".into())
        .await
        .unwrap_err();
    assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);

    auth::set_password(
        &test.state,
        "credentials@zhiyu.local",
        "a-strong-human-password".into(),
    )
    .await
    .unwrap();
    let (status, _, _) = send_with_authorization(
        &test.router,
        Method::GET,
        "/api/v1/auth/me",
        None,
        &format!("Bearer {key}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn backup_endpoints_require_authentication() {
    let test = TestApp::new().await;
    let (status, _, _) = send(
        &test.router,
        Method::GET,
        "/api/v1/backups",
        None,
        None,
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = send(
        &test.router,
        Method::GET,
        "/api/v1/backups/2026-08-10T02:03:04Z",
        None,
        None,
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_key_lists_and_downloads_verified_backup() {
    let test = TestApp::new().await;
    let key = auth::issue_api_key(&test.state, "backup-reader@zhiyu.local")
        .await
        .unwrap();
    let backup_dir = zhiyu_api::backup::backup_directory(&test.state.config).unwrap();
    let snapshot = zhiyu_api::backup::create_managed_snapshot(
        &test.state.db,
        &backup_dir,
        "2026-08-10T02:03:04Z".parse().unwrap(),
    )
    .await
    .unwrap();

    let (status, _, list) = send_with_authorization(
        &test.router,
        Method::GET,
        "/api/v1/backups",
        None,
        &format!("Bearer {key}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list[0]["id"], "2026-08-10T02:03:04Z");
    assert_eq!(list[0]["size"], snapshot.manifest.database_size_bytes);
    assert_eq!(list[0]["sha256"], snapshot.manifest.database_sha256);
    // 字段名必须是 camelCase：桌面端的 RemoteSnapshot 带 rename_all = "camelCase"，
    // 服务端漏掉这个属性时两边解析不上，而各自的单测都会照样通过——所以这里既断言
    // camelCase 存在，也断言 snake_case 不存在，把跨端契约钉死在服务端这一侧。
    assert_eq!(list[0]["createdAt"], "2026-08-10T02:03:04Z");
    assert_eq!(list[0]["schemaVersion"], 31);
    assert!(list[0].get("created_at").is_none());
    assert!(list[0].get("schema_version").is_none());

    let response = test
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/backups/2026-08-10T02:03:04Z")
                .header(header::AUTHORIZATION, format!("Bearer {key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/octet-stream"
    );
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(bytes.len() as u64, snapshot.manifest.database_size_bytes);
    assert_eq!(
        format!("{:x}", Sha256::digest(&bytes)),
        snapshot.manifest.database_sha256
    );
}

#[tokio::test]
async fn backup_download_rejects_forged_ids() {
    let test = TestApp::new().await;
    let key = auth::issue_api_key(&test.state, "backup-id@zhiyu.local")
        .await
        .unwrap();
    let overlong = "a".repeat(500);
    for uri in [
        "/api/v1/backups/%2E%2E%2Fsecret".to_owned(),
        "/api/v1/backups/%2Fetc%2Fpasswd".to_owned(),
        format!("/api/v1/backups/{overlong}"),
    ] {
        let (status, _, _) = send_with_authorization(
            &test.router,
            Method::GET,
            &uri,
            None,
            &format!("Bearer {key}"),
            None,
        )
        .await;
        assert!(
            matches!(status, StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND),
            "伪造 ID 未被拒绝：{uri} -> {status}"
        );
    }
}

#[tokio::test]
async fn self_host_login_sets_secure_host_cookie_without_domain() {
    let test = TestApp::self_host().await;
    test.insert_verified_user("self-host@zhiyu.local", "a-very-secure-password")
        .await;
    let (status, headers, _) = send(
        &test.router,
        Method::POST,
        "/api/v1/auth/login",
        Some(json!({ "email": "self-host@zhiyu.local", "password": "a-very-secure-password" })),
        None,
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let cookie = headers.get(header::SET_COOKIE).unwrap().to_str().unwrap();
    assert!(cookie.starts_with("__Host-zhiyu_session="));
    assert!(cookie.contains("; Path=/"));
    assert!(cookie.contains("; Secure"));
    assert!(!cookie.to_ascii_lowercase().contains("domain="));
}

#[tokio::test]
async fn self_host_email_routes_return_explicit_unavailable_error() {
    let test = TestApp::self_host().await;
    for (path, body) in [
        (
            "/api/v1/auth/register",
            json!({ "email": "new@zhiyu.local", "password": "a-very-secure-password", "timezone": "Asia/Shanghai" }),
        ),
        ("/api/v1/auth/verify-email", json!({ "token": "token" })),
        (
            "/api/v1/auth/resend-verification",
            json!({ "email": "new@zhiyu.local" }),
        ),
        (
            "/api/v1/auth/forgot-password",
            json!({ "email": "new@zhiyu.local" }),
        ),
        (
            "/api/v1/auth/reset-password",
            json!({ "token": "token", "newPassword": "a-very-secure-password" }),
        ),
    ] {
        let (status, _, response) = send(
            &test.router,
            Method::POST,
            path,
            Some(body),
            None,
            None,
            false,
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{path}");
        assert_eq!(response["code"], "email_unavailable", "{path}");
        assert!(
            response["message"].as_str().unwrap().contains("不可用"),
            "{path}"
        );
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
    // dueOn 必须相对今天计算：DUE_SOON_DAYS 是 7，写死日期会让这个断言在到期日
    // 之后变成 overdue，测试到那天才炸。取今天 +3 天，稳定落在 due_soon 窗口内。
    let due_on = (chrono::Utc::now().date_naive() + chrono::Duration::days(3)).to_string();
    let create_body = json!({
        "direction": "lend_out",
        "counterpartyName": "阿青",
        "principalCents": 100_000,
        "occurredOn": "2026-08-02",
        "dueOn": due_on,
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
    let reversal_id = reversal["id"].as_str().unwrap();
    assert_eq!(reversal["account"]["id"], account_id);
    assert_eq!(reversal["account"]["name"], "微信支付-完整流程");
    assert!(reversal["transactionId"].is_string());
    let conn = test.state.connection().await.unwrap();
    let mut rows = conn.query(
        "SELECT e.kind, e.transaction_auto_created, t.kind FROM repayment_events e JOIN ledger_transactions t ON t.id = e.transaction_id WHERE e.id IN (?1, ?2) ORDER BY e.kind",
        libsql::params![payment_id.clone(), reversal_id],
    ).await.unwrap();
    let payment_link = rows.next().await.unwrap().unwrap();
    assert_eq!(payment_link.get::<String>(0).unwrap(), "payment");
    assert_eq!(payment_link.get::<i64>(1).unwrap(), 1);
    assert_eq!(payment_link.get::<String>(2).unwrap(), "income");
    let reversal_link = rows.next().await.unwrap().unwrap();
    assert_eq!(reversal_link.get::<String>(0).unwrap(), "reversal");
    assert_eq!(reversal_link.get::<i64>(1).unwrap(), 1);
    assert_eq!(reversal_link.get::<String>(2).unwrap(), "expense");
    drop(payment_link);
    drop(reversal_link);
    drop(rows);
    drop(conn);
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
    assert!(added["additions"][0]["transactionId"].is_string());

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
    assert_eq!(archived_account["usageCount"], 2);

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

#[tokio::test]
async fn transaction_flow_is_idempotent_versioned_and_user_scoped() {
    let test = TestApp::new().await;
    let cookie = test.register_and_login("tx-first@example.com").await;
    let account = test
        .create_ledger_account(&cookie, "微信支付-记账流程", "wechat_balance")
        .await;
    let account_id = account["id"].as_str().unwrap();

    let create_body = json!({
        "kind": "expense",
        "amountCents": 1234,
        "occurredOn": "2026-08-03",
        "category": "餐饮",
        "accountId": account_id,
        "note": "午饭"
    });
    let (status, _, created) = send(
        &test.router,
        Method::POST,
        "/api/v1/transactions",
        Some(create_body.clone()),
        Some(&cookie),
        Some("create-tx-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["kind"], "expense");
    assert_eq!(created["amountCents"], 1234);
    assert_eq!(created["account"]["id"], account_id);
    assert_eq!(created["account"]["accountType"], "wechat_balance");
    assert_eq!(created["version"], 1);
    let transaction_id = created["id"].as_str().unwrap().to_owned();

    // 幂等重放：同 key 同 body → 同响应；同 key 不同 body → 409
    let (status, _, replayed) = send(
        &test.router,
        Method::POST,
        "/api/v1/transactions",
        Some(create_body),
        Some(&cookie),
        Some("create-tx-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(replayed["id"], transaction_id);
    let (status, _, mismatch) = send(
        &test.router,
        Method::POST,
        "/api/v1/transactions",
        Some(json!({ "kind": "expense", "amountCents": 999, "occurredOn": "2026-08-03" })),
        Some(&cookie),
        Some("create-tx-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(mismatch["code"], "idempotency_mismatch");

    let (status, _, income) = send(
        &test.router,
        Method::POST,
        "/api/v1/transactions",
        Some(json!({ "kind": "income", "amountCents": 500000, "occurredOn": "2026-08-01", "category": "工资" })),
        Some(&cookie),
        Some("create-tx-0002"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(income["account"].is_null());

    // 过滤：month / kind / category
    let (status, _, list) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions?month=2026-08",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["total"], 2);
    let (_, _, empty_month) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions?month=2026-07",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(empty_month["total"], 0);
    let (_, _, expense_only) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions?month=2026-08&kind=expense",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(expense_only["total"], 1);
    let (_, _, by_category) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions?category=%E9%A4%90%E9%A5%AE",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(by_category["total"], 1);

    // 乐观锁：旧 version → 409；正确 version → 200 且 version + 1
    let (status, _, conflict) = send(
        &test.router,
        Method::PATCH,
        &format!("/api/v1/transactions/{transaction_id}"),
        Some(json!({ "version": 99, "kind": "expense", "amountCents": 2000, "occurredOn": "2026-08-03", "category": "餐饮", "accountId": account_id })),
        Some(&cookie),
        Some("update-tx-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["code"], "version_conflict");
    let (status, _, updated) = send(
        &test.router,
        Method::PATCH,
        &format!("/api/v1/transactions/{transaction_id}"),
        Some(json!({ "version": 1, "kind": "expense", "amountCents": 2000, "occurredOn": "2026-08-04", "category": "餐饮", "accountId": account_id, "note": "改期" })),
        Some(&cookie),
        Some("update-tx-0002"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["amountCents"], 2000);
    assert_eq!(updated["occurredOn"], "2026-08-04");
    assert_eq!(updated["version"], 2);

    // 软删 → 列表消失 → 恢复
    let (status, _, deleted) = send(
        &test.router,
        Method::DELETE,
        &format!("/api/v1/transactions/{transaction_id}"),
        Some(json!({ "version": 2 })),
        Some(&cookie),
        Some("delete-tx-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(deleted["createdBy"], "user");
    assert_eq!(deleted["archived"], true);
    let (status, _, conflict) = send(
        &test.router,
        Method::DELETE,
        &format!("/api/v1/transactions/{transaction_id}"),
        Some(json!({ "version": 2 })),
        Some(&cookie),
        Some("delete-tx-0002"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["code"], "version_conflict");
    let (_, _, after_delete) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions?month=2026-08",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(after_delete["total"], 1);
    let (status, _, restored) = send(
        &test.router,
        Method::POST,
        &format!("/api/v1/transactions/{transaction_id}/restore"),
        Some(json!({ "version": 3 })),
        Some(&cookie),
        Some("restore-tx-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(restored["archived"], false);

    // 多用户隔离
    let cookie2 = test.register_and_login("tx-second@example.com").await;
    let (_, _, other_list) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions?month=2026-08",
        None,
        Some(&cookie2),
        None,
        false,
    )
    .await;
    assert_eq!(other_list["total"], 0);
    let (status, _, _) = send(
        &test.router,
        Method::PATCH,
        &format!("/api/v1/transactions/{transaction_id}"),
        Some(json!({ "version": 4, "kind": "expense", "amountCents": 1, "occurredOn": "2026-08-04" })),
        Some(&cookie2),
        Some("update-tx-other"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // 校验：金额 / 日期 / 分类 / 已归档账户
    for (body, label) in [
        (
            json!({ "kind": "expense", "amountCents": 0, "occurredOn": "2026-08-03" }),
            "amount",
        ),
        (
            json!({ "kind": "expense", "amountCents": MAX_SAFE_CENTS + 1, "occurredOn": "2026-08-03" }),
            "amount-max",
        ),
        (
            json!({ "kind": "expense", "amountCents": 100, "occurredOn": "2026-13-40" }),
            "date",
        ),
        (
            json!({ "kind": "expense", "amountCents": 100, "occurredOn": "2026-08-03", "category": "长".repeat(61) }),
            "category",
        ),
    ] {
        let (status, _, _) = send(
            &test.router,
            Method::POST,
            "/api/v1/transactions",
            Some(body),
            Some(&cookie),
            Some(&format!("invalid-tx-{label}")),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "case {label}");
    }
    let (status, _, archived_account) = send(
        &test.router,
        Method::POST,
        &format!("/api/v1/ledger-accounts/{account_id}/archive"),
        Some(json!({ "version": account["version"] })),
        Some(&cookie),
        Some("archive-account-tx"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(archived_account["archived"], true);
    let (status, _, _) = send(
        &test.router,
        Method::POST,
        "/api/v1/transactions",
        Some(json!({ "kind": "expense", "amountCents": 100, "occurredOn": "2026-08-03", "accountId": account_id })),
        Some(&cookie),
        Some("create-tx-archived-account"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn transaction_search_matches_fields_combines_filters_and_escapes_like_wildcards() {
    let test = TestApp::new().await;
    let email = "tx-search@example.com";
    let cookie = test.register_and_login(email).await;
    let first_account = test
        .create_ledger_account(&cookie, "搜索账户一", "wechat_balance")
        .await;
    let (status, _, second_account) = send(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({ "name": "搜索账户二", "accountType": "cash", "note": "" })),
        Some(&cookie),
        Some("create-search-account-0002"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{second_account}");
    let first_account_id = first_account["id"].as_str().unwrap();
    let second_account_id = second_account["id"].as_str().unwrap();
    let conn = test.state.connection().await.unwrap();
    let mut user_rows = conn
        .query(
            "SELECT id FROM users WHERE email=?1",
            libsql::params![email],
        )
        .await
        .unwrap();
    let user_id: String = user_rows.next().await.unwrap().unwrap().get(0).unwrap();
    drop(user_rows);

    let now = chrono::Utc::now().to_rfc3339();
    let rows = [
        (
            "search-payee",
            "expense",
            first_account_id,
            "Merchant Alpha",
            "",
            "",
        ),
        (
            "search-description",
            "income",
            first_account_id,
            "",
            "Description Beta",
            "",
        ),
        (
            "search-note",
            "expense",
            second_account_id,
            "",
            "",
            "Note Gamma",
        ),
        (
            "search-shared-one",
            "expense",
            first_account_id,
            "Shared Shop",
            "",
            "",
        ),
        (
            "search-shared-two",
            "income",
            first_account_id,
            "",
            "Shared income",
            "",
        ),
        (
            "search-shared-three",
            "expense",
            second_account_id,
            "",
            "",
            "Shared note",
        ),
        (
            "search-percent",
            "expense",
            first_account_id,
            "100% Store",
            "",
            "",
        ),
        (
            "search-underscore",
            "expense",
            first_account_id,
            "",
            "order_42",
            "",
        ),
    ];
    for (id, kind, account_id, payee_name, description, note) in rows {
        conn.execute(
            "INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,occurred_on,account_id,payee_name,description,note,created_at,updated_at) VALUES (?1,?2,?3,100,'2026-08-13',?4,?5,?6,?7,?8,?8)",
            libsql::params![id, user_id.clone(), kind, account_id, payee_name, description, note, now.clone()],
        )
        .await
        .unwrap();
    }
    conn.execute(
        "INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,occurred_on,account_id,payee_name,payee_key,created_at,updated_at) VALUES ('search-payee-key',?1,'expense',100,'2026-08-13',?2,'Original Delta','Normalized Token',?3,?3)",
        libsql::params![user_id.clone(), first_account_id, now],
    )
    .await
    .unwrap();

    for (query, expected_id) in [
        ("merchant", "search-payee"),
        ("Description%20Beta", "search-description"),
        ("Gamma", "search-note"),
        ("Normalized%20Token", "search-payee-key"),
    ] {
        let (status, _, body) = send(
            &test.router,
            Method::GET,
            &format!("/api/v1/transactions?q={query}"),
            None,
            Some(&cookie),
            None,
            false,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["total"], 1, "query {query}: {body}");
        assert_eq!(body["items"][0]["id"], expected_id);
    }

    let (status, _, blank_search) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions?q=%20%20",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{blank_search}");
    assert_eq!(blank_search["total"], 9);

    let (status, _, combined) = send(
        &test.router,
        Method::GET,
        &format!("/api/v1/transactions?q=Shared&kind=expense&accountId={first_account_id}"),
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{combined}");
    assert_eq!(combined["total"], 1);
    assert_eq!(combined["items"][0]["id"], "search-shared-one");

    let (status, _, paged) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions?q=Shared&pageSize=1&page=2",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{paged}");
    assert_eq!(paged["total"], 3);
    assert_eq!(paged["items"].as_array().unwrap().len(), 1);

    for (query, expected_id) in [("%25", "search-percent"), ("_", "search-underscore")] {
        let (status, _, body) = send(
            &test.router,
            Method::GET,
            &format!("/api/v1/transactions?q={query}"),
            None,
            Some(&cookie),
            None,
            false,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["total"], 1, "literal wildcard {query}: {body}");
        assert_eq!(body["items"][0]["id"], expected_id);
    }

    let (status, _, body) = send(
        &test.router,
        Method::GET,
        &format!("/api/v1/transactions?q={}", "x".repeat(101)),
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "validation_error");
}

#[tokio::test]
async fn transaction_summary_aggregates_daily_by_category_and_excludes_archived() {
    let test = TestApp::new().await;
    let cookie = test.register_and_login("tx-summary@example.com").await;

    let entries = [
        (
            "create-sum-0001",
            json!({ "kind": "income", "amountCents": 5000, "occurredOn": "2026-08-01", "category": "工资" }),
        ),
        (
            "create-sum-0002",
            json!({ "kind": "expense", "amountCents": 1200, "occurredOn": "2026-08-01", "category": "餐饮" }),
        ),
        (
            "create-sum-0003",
            json!({ "kind": "expense", "amountCents": 300, "occurredOn": "2026-08-02", "category": "餐饮" }),
        ),
        (
            "create-sum-0004",
            json!({ "kind": "expense", "amountCents": 800, "occurredOn": "2026-08-02", "category": "交通" }),
        ),
        (
            "create-sum-0005",
            json!({ "kind": "expense", "amountCents": 100, "occurredOn": "2026-08-10", "category": "餐饮" }),
        ),
    ];
    let mut archived_id = String::new();
    let mut archived_version = 0_i64;
    for (key, body) in entries {
        let (status, _, created) = send(
            &test.router,
            Method::POST,
            "/api/v1/transactions",
            Some(body),
            Some(&cookie),
            Some(key),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        if key == "create-sum-0005" {
            archived_id = created["id"].as_str().unwrap().to_owned();
            archived_version = created["version"].as_i64().unwrap();
        }
    }
    let (status, _, _) = send(
        &test.router,
        Method::DELETE,
        &format!("/api/v1/transactions/{archived_id}"),
        Some(json!({ "version": archived_version })),
        Some(&cookie),
        Some("delete-sum-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _, summary) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions/summary?month=2026-08",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(summary["month"], "2026-08");
    assert_eq!(summary["incomeCents"], 5000);
    assert_eq!(summary["expenseCents"], 2300);
    assert_eq!(summary["netCents"], 2700);
    assert_eq!(summary["transactionCount"], 4);

    let days = summary["days"].as_array().unwrap();
    assert_eq!(days.len(), 2);
    assert_eq!(days[0]["date"], "2026-08-01");
    assert_eq!(days[0]["incomeCents"], 5000);
    assert_eq!(days[0]["expenseCents"], 1200);
    assert_eq!(days[1]["date"], "2026-08-02");
    assert_eq!(days[1]["incomeCents"], 0);
    assert_eq!(days[1]["expenseCents"], 1100);

    let by_category = summary["byCategory"].as_array().unwrap();
    assert_eq!(by_category.len(), 3);
    assert_eq!(by_category[0]["category"], "餐饮");
    assert_eq!(by_category[0]["expenseCents"], 1500);
    assert_eq!(by_category[0]["count"], 2);
    assert_eq!(by_category[1]["category"], "交通");
    assert_eq!(by_category[1]["expenseCents"], 800);
    assert_eq!(by_category[2]["category"], "工资");
    assert_eq!(by_category[2]["incomeCents"], 5000);

    // 分类联想：已归档条目的分类仍在（餐饮还有其他条目），独立分类仅来自未归档条目
    let (status, _, categories) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions/categories",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(categories, json!(["交通", "工资", "餐饮"]));

    // month 参数缺失/非法
    let (status, _, _) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions/summary",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    for month in ["2026-13", "2026-8"] {
        let (status, _, body) = send(
            &test.router,
            Method::GET,
            &format!("/api/v1/transactions/summary?month={month}"),
            None,
            Some(&cookie),
            None,
            false,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "month {month}");
        assert_eq!(body["code"], "validation_error");
    }
}

#[tokio::test]
async fn transaction_category_id_summary_dropdown_and_filter_keep_legacy_compatibility() {
    let test = TestApp::new().await;
    let cookie = test
        .register_and_login("tx-category-id-summary@example.com")
        .await;

    let (status, _, category) = send(
        &test.router,
        Method::POST,
        "/api/v1/categories",
        Some(json!({ "name": "订阅服务", "kind": "expense" })),
        Some(&cookie),
        Some("create-summary-category-id"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{category}");
    let category_id = category["id"].as_str().unwrap();

    let mut created = Vec::new();
    for (key, amount_cents, category) in [
        ("create-category-id-expense", 3100, ""),
        ("create-uncategorized-expense", 2200, ""),
        ("create-legacy-category-expense", 1300, "手工分类"),
    ] {
        let (status, _, transaction) = send(
            &test.router,
            Method::POST,
            "/api/v1/transactions",
            Some(json!({
                "kind": "expense",
                "amountCents": amount_cents,
                "occurredOn": "2026-08-14",
                "category": category
            })),
            Some(&cookie),
            Some(key),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{transaction}");
        created.push(transaction);
    }

    let categorized_id = created[0]["id"].as_str().unwrap();
    let (status, _, categorized) = send(
        &test.router,
        Method::PATCH,
        &format!("/api/v1/transactions/{categorized_id}"),
        Some(json!({
            "version": created[0]["version"],
            "kind": "expense",
            "amountCents": 3100,
            "occurredOn": "2026-08-14",
            "category": "",
            "categoryId": category_id
        })),
        Some(&cookie),
        Some("assign-summary-category-id"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{categorized}");

    let (status, _, summary) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions/summary?month=2026-08",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    assert_eq!(
        summary["byCategory"],
        json!([
            { "category": "订阅服务", "incomeCents": 0, "expenseCents": 3100, "count": 1 },
            { "category": "", "incomeCents": 0, "expenseCents": 2200, "count": 1 },
            { "category": "手工分类", "incomeCents": 0, "expenseCents": 1300, "count": 1 }
        ])
    );

    let (status, _, categories) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions/categories",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{categories}");
    assert_eq!(categories, json!(["手工分类", "订阅服务"]));

    for category_filter in [category_id, "%E8%AE%A2%E9%98%85%E6%9C%8D%E5%8A%A1"] {
        let (status, _, filtered) = send(
            &test.router,
            Method::GET,
            &format!("/api/v1/transactions?category={category_filter}"),
            None,
            Some(&cookie),
            None,
            false,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{filtered}");
        assert_eq!(filtered["total"], 1, "{filtered}");
        assert_eq!(filtered["items"][0]["id"], categorized_id);
    }

    let (status, _, legacy_filtered) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions?category=%E6%89%8B%E5%B7%A5%E5%88%86%E7%B1%BB",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{legacy_filtered}");
    assert_eq!(legacy_filtered["total"], 1, "{legacy_filtered}");
    assert_eq!(legacy_filtered["items"][0]["id"], created[2]["id"]);
}

#[tokio::test]
async fn transfer_moves_balances_without_affecting_income_expense_statistics() {
    let test = TestApp::new().await;
    let cookie = test.register_and_login("tx-transfer@example.com").await;

    async fn create_account(
        test: &TestApp,
        cookie: &str,
        key: &str,
        name: &str,
        opening_balance_cents: i64,
    ) -> Value {
        let (status, _, account) = send(
            &test.router,
            Method::POST,
            "/api/v1/ledger-accounts",
            Some(json!({
                "name": name,
                "accountType": "bank_card",
                "openingBalanceCents": opening_balance_cents
            })),
            Some(cookie),
            Some(key),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{account}");
        account
    }

    async fn balance_of(test: &TestApp, cookie: &str, account_id: &str) -> i64 {
        let (status, _, accounts) = send(
            &test.router,
            Method::GET,
            "/api/v1/ledger-accounts",
            None,
            Some(cookie),
            None,
            false,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        accounts
            .as_array()
            .unwrap()
            .iter()
            .find(|account| account["id"] == account_id)
            .unwrap()["balanceCents"]
            .as_i64()
            .unwrap()
    }

    let from_account = create_account(
        &test,
        &cookie,
        "transfer-account-from",
        "转账转出账户",
        10_000,
    )
    .await;
    let to_account =
        create_account(&test, &cookie, "transfer-account-to", "转账转入账户", 2_000).await;
    let from_account_id = from_account["id"].as_str().unwrap();
    let to_account_id = to_account["id"].as_str().unwrap();

    let (status, _, transfer) = send(
        &test.router,
        Method::POST,
        "/api/v1/transactions",
        Some(json!({
            "kind": "transfer",
            "amountCents": 1_500,
            "occurredOn": "2026-08-02",
            "category": "账户划转",
            "transferFromAccountId": from_account_id,
            "transferToAccountId": to_account_id
        })),
        Some(&cookie),
        Some("create-transfer"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{transfer}");
    assert_eq!(transfer["kind"], "transfer");
    assert!(transfer["account"].is_null());
    assert_eq!(transfer["transferFromAccount"]["id"], from_account_id);
    assert_eq!(transfer["transferToAccount"]["id"], to_account_id);
    assert_eq!(balance_of(&test, &cookie, from_account_id).await, 8_500);
    assert_eq!(balance_of(&test, &cookie, to_account_id).await, 3_500);
    let transfer_id = transfer["id"].as_str().unwrap();

    for (key, body) in [
        (
            "invalid-transfer-account",
            json!({
                "kind": "transfer",
                "amountCents": 100,
                "occurredOn": "2026-08-03",
                "accountId": from_account_id,
                "transferToAccountId": to_account_id
            }),
        ),
        (
            "invalid-expense-transfer-account",
            json!({
                "kind": "expense",
                "amountCents": 100,
                "occurredOn": "2026-08-03",
                "transferFromAccountId": from_account_id
            }),
        ),
    ] {
        let (status, _, body) = send(
            &test.router,
            Method::POST,
            "/api/v1/transactions",
            Some(body),
            Some(&cookie),
            Some(key),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(body["code"], "validation_error");
    }

    for (key, body) in [
        (
            "create-transfer-income",
            json!({
                "kind": "income",
                "amountCents": 5_000,
                "occurredOn": "2026-08-01",
                "category": "工资"
            }),
        ),
        (
            "create-transfer-expense",
            json!({
                "kind": "expense",
                "amountCents": 1_200,
                "occurredOn": "2026-08-03",
                "category": "餐饮"
            }),
        ),
    ] {
        let (status, _, body) = send(
            &test.router,
            Method::POST,
            "/api/v1/transactions",
            Some(body),
            Some(&cookie),
            Some(key),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }

    let (status, _, summary) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions/summary?month=2026-08",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    assert_eq!(summary["incomeCents"], 5_000);
    assert_eq!(summary["expenseCents"], 1_200);
    assert_eq!(summary["transactionCount"], 2);
    assert!(
        summary["byCategory"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["category"] != "账户划转")
    );

    let (status, _, filtered) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions?month=2026-08&kind=transfer",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{filtered}");
    assert_eq!(filtered["total"], 1);
    assert_eq!(filtered["items"][0]["id"], transfer_id);

    let (status, _, debt) = send(
        &test.router,
        Method::POST,
        "/api/v1/debts",
        Some(json!({
            "direction": "lend_out",
            "counterpartyName": "转账候选排除",
            "principalCents": 1_500,
            "occurredOn": "2026-08-04",
            "originKind": "no_cash_movement"
        })),
        Some(&cookie),
        Some("transfer-candidate-debt"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{debt}");
    let debt_id = debt["id"].as_str().unwrap();
    let (status, _, candidates) = send(
        &test.router,
        Method::GET,
        &format!("/api/v1/debts/{debt_id}/link-candidates"),
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{candidates}");
    assert!(
        candidates
            .as_array()
            .unwrap()
            .iter()
            .all(|candidate| candidate["id"] != transfer_id)
    );
}

#[tokio::test]
async fn account_balance_reflects_transactions_and_debt_cash_movements() {
    let test = TestApp::new().await;
    let cookie = test.register_and_login("tx-balance@example.com").await;

    async fn balance_of(test: &TestApp, cookie: &str, account_id: &str) -> i64 {
        let (status, _, accounts) = send(
            &test.router,
            Method::GET,
            "/api/v1/ledger-accounts",
            None,
            Some(cookie),
            None,
            false,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        accounts
            .as_array()
            .unwrap()
            .iter()
            .find(|account| account["id"] == account_id)
            .unwrap()["balanceCents"]
            .as_i64()
            .unwrap()
    }

    // 初始余额 100.00
    let (status, _, account) = send(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({ "name": "招商银行-余额联动", "accountType": "bank_card", "openingBalanceCents": 10000 })),
        Some(&cookie),
        Some("create-balance-account"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let account_id = account["id"].as_str().unwrap().to_owned();
    assert_eq!(account["balanceCents"], 10000);
    assert_eq!(account["openingBalanceCents"], 10000);
    let (status, _, second) = send(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({ "name": "现金-余额联动", "accountType": "cash" })),
        Some(&cookie),
        Some("create-balance-account-2"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let second_id = second["id"].as_str().unwrap().to_owned();

    // 收入 +5000 → 15000；支出 -1200 → 13800
    let (status, _, _) = send(
        &test.router,
        Method::POST,
        "/api/v1/transactions",
        Some(json!({ "kind": "income", "amountCents": 5000, "occurredOn": "2026-08-01", "category": "工资", "accountId": account_id })),
        Some(&cookie),
        Some("balance-tx-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(balance_of(&test, &cookie, &account_id).await, 15000);

    let (status, _, expense) = send(
        &test.router,
        Method::POST,
        "/api/v1/transactions",
        Some(json!({ "kind": "expense", "amountCents": 1200, "occurredOn": "2026-08-02", "category": "餐饮", "accountId": account_id })),
        Some(&cookie),
        Some("balance-tx-0002"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(balance_of(&test, &cookie, &account_id).await, 13800);
    let expense_id = expense["id"].as_str().unwrap().to_owned();

    // 改金额 1200 → 2000：13000；改账户 → 原账户 15000、新账户 -2000
    let (status, _, _) = send(
        &test.router,
        Method::PATCH,
        &format!("/api/v1/transactions/{expense_id}"),
        Some(json!({ "version": 1, "kind": "expense", "amountCents": 2000, "occurredOn": "2026-08-02", "category": "餐饮", "accountId": account_id })),
        Some(&cookie),
        Some("balance-tx-0003"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(balance_of(&test, &cookie, &account_id).await, 13000);
    let (status, _, _) = send(
        &test.router,
        Method::PATCH,
        &format!("/api/v1/transactions/{expense_id}"),
        Some(json!({ "version": 2, "kind": "expense", "amountCents": 2000, "occurredOn": "2026-08-02", "category": "餐饮", "accountId": second_id })),
        Some(&cookie),
        Some("balance-tx-0004"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(balance_of(&test, &cookie, &account_id).await, 15000);
    assert_eq!(balance_of(&test, &cookie, &second_id).await, -2000);

    // 归档支出 → 新账户回滚为 0
    let (status, _, _) = send(
        &test.router,
        Method::DELETE,
        &format!("/api/v1/transactions/{expense_id}"),
        Some(json!({ "version": 3 })),
        Some(&cookie),
        Some("balance-tx-0005"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(balance_of(&test, &cookie, &second_id).await, 0);

    // 债务现金流水：借出 10000 → 5000；还款 3000 → 8000；冲正 → 回到 5000
    let (status, _, debt) = send(
        &test.router,
        Method::POST,
        "/api/v1/debts",
        Some(json!({ "direction": "lend_out", "counterpartyName": "阿青", "principalCents": 10000, "occurredOn": "2026-08-03", "accountId": account_id })),
        Some(&cookie),
        Some("balance-debt-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(debt["originKind"], "cash_movement");
    assert_eq!(balance_of(&test, &cookie, &account_id).await, 5000);
    let debt_id = debt["id"].as_str().unwrap().to_owned();

    let (status, _, paid) = send(
        &test.router,
        Method::POST,
        &format!("/api/v1/debts/{debt_id}/repayments"),
        Some(json!({ "amountCents": 3000, "effectiveOn": "2026-08-04", "accountId": account_id })),
        Some(&cookie),
        Some("balance-repay-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(balance_of(&test, &cookie, &account_id).await, 8000);
    let payment_id = paid["repayments"][0]["id"].as_str().unwrap().to_owned();

    let (status, _, _) = send(
        &test.router,
        Method::POST,
        &format!("/api/v1/repayments/{payment_id}/reversals"),
        Some(json!({ "effectiveOn": "2026-08-05", "note": "录入错误" })),
        Some(&cookie),
        Some("balance-reverse-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(balance_of(&test, &cookie, &account_id).await, 5000);

    // 无资金进出的债务不影响余额
    let (status, _, _) = send(
        &test.router,
        Method::POST,
        "/api/v1/debts",
        Some(json!({ "direction": "borrow_in", "counterpartyName": "代办记账", "principalCents": 99900, "occurredOn": "2026-08-04", "originKind": "no_cash_movement", "accountId": null })),
        Some(&cookie),
        Some("balance-debt-0002"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(balance_of(&test, &cookie, &account_id).await, 5000);
}

#[tokio::test]
async fn debt_transaction_link_is_unique_excluded_from_totals_and_reversible() {
    let test = TestApp::new().await;
    let cookie = test.register_and_login("debt-link@example.com").await;
    let account = test
        .create_ledger_account(&cookie, "债务关联账户", "bank_card")
        .await;
    let account_id = account["id"].as_str().unwrap();
    let (_, _, transaction) = send(
        &test.router, Method::POST, "/api/v1/transactions",
        Some(json!({ "kind": "income", "amountCents": 3000, "occurredOn": "2026-08-07", "category": "转账", "accountId": account_id, "note": "黄英，(__old yellow，)" })),
        Some(&cookie), Some("debt-link-transaction"), true,
    ).await;
    let transaction_id = transaction["id"].as_str().unwrap();
    assert_eq!(transaction["pnlScope"], "counted");
    let (status, _, debt) = send(
        &test.router, Method::POST, "/api/v1/debts",
        Some(json!({ "direction": "lend_out", "counterpartyName": "黄英", "principalCents": 10000, "occurredOn": "2026-08-01", "accountId": account_id })),
        Some(&cookie), Some("debt-link-debt"), true,
    ).await;
    assert_eq!(status, StatusCode::CREATED, "{debt}");
    let debt_id = debt["id"].as_str().unwrap();
    let principal_transaction_id = debt["transactionId"].as_str().unwrap();
    let (_, _, before_link_summary) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions/summary?month=2026-08",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(before_link_summary["incomeCents"], 3000);
    assert_eq!(before_link_summary["expenseCents"], 0);
    assert_eq!(before_link_summary["transactionCount"], 1);
    let (_, _, before_link_list) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions?month=2026-08",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    let principal_transaction = before_link_list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == principal_transaction_id)
        .unwrap();
    assert_eq!(principal_transaction["pnlScope"], "excluded");
    assert_eq!(principal_transaction["links"][0]["pluginId"], "debts");
    assert_eq!(principal_transaction["links"][0]["kind"], "principal");
    assert_eq!(principal_transaction["links"][0]["refId"], debt_id);
    assert_eq!(principal_transaction["links"][0]["label"], "黄英");

    let (_, _, before_accounts) = send(
        &test.router,
        Method::GET,
        "/api/v1/ledger-accounts",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    let before_balance = before_accounts
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == account_id)
        .unwrap()["balanceCents"]
        .as_i64()
        .unwrap();
    assert_eq!(before_balance, -7000);

    let (status, _, linked) = send(
        &test.router, Method::POST, &format!("/api/v1/debts/{debt_id}/repayments"),
        Some(json!({ "amountCents": 3000, "effectiveOn": "2026-08-07", "accountId": null, "transactionId": transaction_id })),
        Some(&cookie), Some("debt-link-repayment"), true,
    ).await;
    assert_eq!(status, StatusCode::CREATED, "{linked}");
    assert_eq!(linked["repayments"][0]["transactionId"], transaction_id);
    assert_eq!(linked["repayments"][0]["account"]["id"], account_id);
    let payment_id = linked["repayments"][0]["id"].as_str().unwrap();

    let (_, _, accounts) = send(
        &test.router,
        Method::GET,
        "/api/v1/ledger-accounts",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(
        accounts
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == account_id)
            .unwrap()["balanceCents"],
        before_balance
    );
    let (_, _, summary) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions/summary?month=2026-08",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(summary["incomeCents"], 0);
    assert_eq!(summary["transactionCount"], 0);
    let (_, _, list) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions?month=2026-08",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(list["items"][0]["id"], transaction_id);
    assert_eq!(list["items"][0]["links"][0]["pluginId"], "debts");
    assert_eq!(list["items"][0]["links"][0]["refId"], debt_id);
    assert_eq!(list["items"][0]["links"][0]["kind"], "repayment");
    assert_eq!(list["items"][0]["links"][0]["label"], "黄英");
    assert_eq!(list["items"][0]["pnlScope"], "excluded");

    let (status, _, duplicate) = send(
        &test.router, Method::POST, &format!("/api/v1/debts/{debt_id}/additions"),
        Some(json!({ "amountCents": 3000, "effectiveOn": "2026-08-07", "accountId": null, "transactionId": transaction_id })),
        Some(&cookie), Some("debt-link-duplicate"), true,
    ).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{duplicate}");

    for (key, amount, kind) in [
        ("debt-link-wrong-amount", 3001, "income"),
        ("debt-link-wrong-direction", 3000, "expense"),
    ] {
        let (_, _, wrong) = send(
            &test.router, Method::POST, "/api/v1/transactions",
            Some(json!({ "kind": kind, "amountCents": amount, "occurredOn": "2026-08-07", "category": "转账", "accountId": account_id })),
            Some(&cookie), Some(key), true,
        ).await;
        let (status, _, body) = send(
            &test.router, Method::PATCH, &format!("/api/v1/repayments/{payment_id}"),
            Some(json!({ "version": linked["version"], "amountCents": 3000, "effectiveOn": "2026-08-07", "accountId": account_id, "transactionId": wrong["id"] })),
            Some(&cookie), Some(&format!("{key}-patch")), true,
        ).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    }

    let (_, _, replacement) = send(
        &test.router, Method::POST, "/api/v1/transactions",
        Some(json!({ "kind": "income", "amountCents": 3000, "occurredOn": "2026-08-07", "category": "转账", "accountId": account_id, "note": "另一笔候选" })),
        Some(&cookie), Some("debt-link-valid-replacement"), true,
    ).await;
    let replacement_id = replacement["id"].as_str().unwrap();
    let (status, _, switched) = send(
        &test.router, Method::PATCH, &format!("/api/v1/repayments/{payment_id}"),
        Some(json!({ "version": linked["version"], "amountCents": 3000, "effectiveOn": "2026-08-07", "accountId": account_id, "transactionId": replacement_id })),
        Some(&cookie), Some("debt-link-switch"), true,
    ).await;
    assert_eq!(status, StatusCode::OK, "{switched}");
    let (_, _, switched_list) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions?month=2026-08",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    let switched_items = switched_list["items"].as_array().unwrap();
    assert_eq!(
        switched_items
            .iter()
            .find(|item| item["id"] == transaction_id)
            .unwrap()["links"],
        json!([])
    );
    assert_eq!(
        switched_items
            .iter()
            .find(|item| item["id"] == replacement_id)
            .unwrap()["links"][0]["kind"],
        "repayment"
    );

    let (status, _, unlinked) = send(
        &test.router, Method::PATCH, &format!("/api/v1/repayments/{payment_id}"),
        Some(json!({ "version": switched["version"], "amountCents": 3000, "effectiveOn": "2026-08-07", "accountId": account_id, "transactionId": null })),
        Some(&cookie), Some("debt-link-unlink"), true,
    ).await;
    assert_eq!(status, StatusCode::OK, "{unlinked}");
    let replacement_transaction_id = unlinked["repayments"][0]["transactionId"]
        .as_str()
        .expect("cash repayment must receive an automatic replacement transaction");
    assert_ne!(replacement_transaction_id, transaction_id);
    let conn = test.state.connection().await.unwrap();
    let mut rows = conn
        .query(
            "SELECT r.transaction_auto_created, old.archived_at, replacement.archived_at, old.pnl_scope, replacement.pnl_scope FROM repayment_events r JOIN ledger_transactions old ON old.id = ?1 JOIN ledger_transactions replacement ON replacement.id = r.transaction_id WHERE r.id = ?2",
            libsql::params![transaction_id, payment_id],
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    assert_eq!(row.get::<i64>(0).unwrap(), 1);
    assert!(row.get::<Option<String>>(1).unwrap().is_none());
    assert!(row.get::<Option<String>>(2).unwrap().is_none());
    assert_eq!(row.get::<String>(3).unwrap(), "counted");
    assert_eq!(row.get::<String>(4).unwrap(), "excluded");
    drop(row);
    drop(rows);
    drop(conn);
    let (_, _, summary) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions/summary?month=2026-08",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(summary["incomeCents"], 9001);
    assert_eq!(summary["transactionCount"], 4);
    let (_, _, after_unlink_list) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions?month=2026-08",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    let after_unlink_items = after_unlink_list["items"].as_array().unwrap();
    assert_eq!(
        after_unlink_items
            .iter()
            .find(|item| item["id"] == transaction_id)
            .unwrap()["pnlScope"],
        "counted"
    );
    assert_eq!(
        after_unlink_items
            .iter()
            .find(|item| item["id"] == replacement_transaction_id)
            .unwrap()["pnlScope"],
        "excluded"
    );
    assert_eq!(
        after_unlink_items
            .iter()
            .find(|item| item["id"] == transaction_id)
            .unwrap()["links"],
        json!([])
    );
    assert_eq!(
        after_unlink_items
            .iter()
            .find(|item| item["id"] == replacement_id)
            .unwrap()["links"],
        json!([])
    );
    assert_eq!(
        after_unlink_items
            .iter()
            .find(|item| item["id"] == replacement_transaction_id)
            .unwrap()["links"][0]["kind"],
        "repayment"
    );
    let (_, _, candidates) = send(
        &test.router,
        Method::GET,
        &format!("/api/v1/debts/{debt_id}/link-candidates?amountCents=3000"),
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(
        candidates[0]["id"], transaction_id,
        "name match must outrank the other exact amount/date candidate"
    );

    let counterparty_id = debt["counterparty"]["id"].as_str().unwrap();
    let (status, _, renamed) = send(
        &test.router,
        Method::PATCH,
        &format!("/api/v1/counterparties/{counterparty_id}"),
        Some(json!({ "displayName": "黄英（更新）", "note": "", "version": 1 })),
        Some(&cookie),
        Some("debt-link-rename-counterparty"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    let (_, _, renamed_list) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions?month=2026-08",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(
        renamed_list["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == replacement_transaction_id)
            .unwrap()["links"][0]["label"],
        "黄英（更新）"
    );
}

#[tokio::test]
async fn deleting_debt_removes_its_principal_transaction_link() {
    let test = TestApp::new().await;
    let cookie = test
        .register_and_login("delete-debt-link@example.com")
        .await;
    let account = test
        .create_ledger_account(&cookie, "删除关联测试账户", "cash")
        .await;
    let (status, _, debt) = send(
        &test.router,
        Method::POST,
        "/api/v1/debts",
        Some(json!({ "direction": "lend_out", "counterpartyName": "测试联系人", "principalCents": 5000, "occurredOn": "2026-08-08", "accountId": account["id"] })),
        Some(&cookie),
        Some("delete-debt-link-create"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{debt}");
    let transaction_id = debt["transactionId"].as_str().unwrap();
    let (status, _, body) = send(
        &test.router,
        Method::DELETE,
        &format!("/api/v1/debts/{}", debt["id"].as_str().unwrap()),
        Some(json!({ "version": debt["version"] })),
        Some(&cookie),
        Some("delete-debt-link-delete"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let conn = test.state.connection().await.unwrap();
    let mut rows = conn
        .query(
            "SELECT COUNT(*) FROM transaction_links WHERE transaction_id=?1 AND plugin_id='debts'",
            [transaction_id],
        )
        .await
        .unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        0
    );
}

#[tokio::test]
async fn duplicate_suspicions_classify_four_rules_and_keep_ambiguous_cluster_unpaired() {
    let test = TestApp::new().await;
    let cookie = test
        .register_and_login("duplicate-suspicions@example.com")
        .await;
    let account = test
        .create_ledger_account(&cookie, "虚构匹配账户", "bank_card")
        .await;
    let account_id = account["id"].as_str().unwrap();
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/duplicate_suspicions_synthetic.json")).unwrap();
    let conn = test.state.connection().await.unwrap();
    let user_id: String = conn
        .query(
            "SELECT id FROM users WHERE email='duplicate-suspicions@example.com'",
            (),
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    let now = "2026-02-06T00:00:00Z";
    for item in fixture["existing"].as_array().unwrap() {
        let occurred_at = item["occurredAt"].as_str().unwrap();
        conn.execute(
            "INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,currency,occurred_on,occurred_at,occurred_at_precision,category,category_source,payee_name,description,account_id,note,version,created_at,updated_at,source_channel,external_id) VALUES (?1,?2,'expense',?3,'CNY',?4,?5,?6,'','none','','',?7,'',1,?8,?8,?9,?10)",
            libsql::params![
                item["id"].as_str().unwrap(),
                user_id.clone(),
                item["amountCents"].as_i64().unwrap(),
                &occurred_at[..10],
                occurred_at,
                item["precision"].as_str().unwrap(),
                account_id,
                now,
                item["channel"].as_str().unwrap(),
                format!("existing-{}", item["id"].as_str().unwrap()),
            ],
        )
        .await
        .unwrap();
    }
    let incoming = fixture["incoming"].as_array().unwrap();
    conn.execute(
        "INSERT INTO import_batches(id,user_id,source_channel,parser_version,file_name,file_sha256,period_start,period_end,total_count,status,created_at,updated_at) VALUES ('synthetic-duplicate-batch',?1,'wechat',1,'synthetic.json',?2,'2026-02-01','2026-02-05',?3,'preview',?4,?4)",
        libsql::params![user_id.clone(), "0".repeat(64), incoming.len() as i64, now],
    ).await.unwrap();
    for (index, item) in incoming.iter().enumerate() {
        let occurred_at = item["occurredAt"].as_str().unwrap();
        conn.execute(
            "INSERT INTO import_records(id,batch_id,row_index,external_id,occurred_at,occurred_on,direction,amount_cents,counterparty,pay_method,channel_status,source_note,occurred_at_precision,disposition,created_at) VALUES (?1,'synthetic-duplicate-batch',?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,'import',?13)",
            libsql::params![
                format!("record-{}", item["id"].as_str().unwrap()),
                index as i64 + 1,
                item["id"].as_str().unwrap(),
                occurred_at,
                &occurred_at[..10],
                item["direction"].as_str().unwrap(),
                item["amountCents"].as_i64().unwrap(),
                item["counterparty"].as_str().unwrap(),
                item["payMethod"].as_str().unwrap(),
                item["status"].as_str().unwrap(),
                item["sourceNote"].as_str().unwrap(),
                item["precision"].as_str().unwrap(),
                now,
            ],
        ).await.unwrap();
    }

    let (status, _, committed) = send(
        &test.router,
        Method::POST,
        "/api/v1/imports/synthetic-duplicate-batch/commit",
        Some(json!({ "accountId": account_id })),
        Some(&cookie),
        Some("commit-synthetic-duplicates"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{committed}");

    let (status, _, list) = send(
        &test.router,
        Method::GET,
        "/api/v1/duplicate-suspicions?pageSize=10",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["total"], 5, "four pairs plus one ambiguous cluster");
    let items = list["items"].as_array().unwrap();
    let withdraw = items
        .iter()
        .find(|item| item["matchRule"] == "withdraw_fee")
        .unwrap();
    assert!(
        withdraw["transactionA"]["amountCents"] == 100100
            || withdraw["transactionB"]["amountCents"] == 100100
    );
    assert_eq!(
        items
            .iter()
            .filter(|item| item["matchRule"] == "refund")
            .count(),
        1
    );
    assert_eq!(
        items
            .iter()
            .filter(|item| item["matchRule"] == "same_amount")
            .count(),
        2,
        "plain and combined payments must both stay in same_amount"
    );
    let clusters = list["clusters"].as_array().unwrap();
    assert_eq!(clusters.len(), 1);
    let ambiguous: Vec<_> = clusters[0]["items"].as_array().unwrap().iter().collect();
    assert_eq!(ambiguous.len(), 4, "the 2x2 cluster must retain every edge");
    let cluster_key = ambiguous[0]["clusterKey"].as_str().unwrap();
    assert!(!cluster_key.is_empty());
    assert!(
        ambiguous
            .iter()
            .all(|item| item["clusterKey"] == cluster_key)
    );
    let mut rows = conn
        .query(
            "SELECT transaction_id_a,transaction_id_b FROM duplicate_suspicions WHERE user_id=?1 AND match_rule<>'ambiguous'",
            libsql::params![user_id],
        )
        .await
        .unwrap();
    let mut seen = std::collections::HashSet::new();
    let mut pair_count = 0;
    while let Some(row) = rows.next().await.unwrap() {
        let a: String = row.get(0).unwrap();
        let b: String = row.get(1).unwrap();
        assert!(a < b, "pairs must be stored in canonical order");
        assert!(seen.insert(a));
        assert!(seen.insert(b));
        pair_count += 1;
    }
    assert_eq!(pair_count, 4, "non-ambiguous matches remain one-to-one");

    let suspicion_id = withdraw["id"].as_str().unwrap();
    let (status, _, dismissed) = send(
        &test.router,
        Method::PATCH,
        &format!("/api/v1/duplicate-suspicions/{suspicion_id}"),
        Some(json!({ "status": "dismissed" })),
        Some(&cookie),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{dismissed}");
    assert_eq!(dismissed["status"], "dismissed");
    let (_, _, after) = send(
        &test.router,
        Method::GET,
        "/api/v1/duplicate-suspicions",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(after["total"], 4);
    assert!(
        !after["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == suspicion_id)
    );
}

#[tokio::test]
async fn same_amount_confirmation_is_reversible_and_other_rules_do_not_touch_the_ledger() {
    async fn balances(conn: &libsql::Connection, user_id: &str) -> Vec<(String, i64)> {
        let mut rows = conn
            .query(
                "SELECT b.account_id,b.balance_cents FROM ledger_account_balances b JOIN ledger_accounts a ON a.id=b.account_id WHERE a.user_id=?1 ORDER BY b.account_id",
                [user_id],
            )
            .await
            .unwrap();
        let mut values = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            values.push((row.get(0).unwrap(), row.get(1).unwrap()));
        }
        values
    }

    async fn unsupported_ledger_snapshot(
        conn: &libsql::Connection,
        user_id: &str,
    ) -> Vec<(String, i64, Option<String>, Option<String>, Option<String>)> {
        let mut rows = conn
            .query(
                "SELECT id,amount_cents,account_id,archived_at,event_id FROM ledger_transactions WHERE user_id=?1 AND id LIKE 'unsupported-%' ORDER BY id",
                [user_id],
            )
            .await
            .unwrap();
        let mut values = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            values.push((
                row.get(0).unwrap(),
                row.get(1).unwrap(),
                row.get(2).unwrap(),
                row.get(3).unwrap(),
                row.get(4).unwrap(),
            ));
        }
        values
    }

    let test = TestApp::new().await;
    let cookie = test
        .register_and_login("same-amount-actions@example.com")
        .await;
    let primary = test
        .create_ledger_account(&cookie, "虚构确认账户", "bank_card")
        .await;
    let (untouched_status, _, untouched) = send(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({ "name": "虚构未变账户", "accountType": "cash", "note": "测试资金账户" })),
        Some(&cookie),
        Some("create-untouched-account-0001"),
        true,
    )
    .await;
    assert_eq!(untouched_status, StatusCode::CREATED, "{untouched}");
    let primary_id = primary["id"].as_str().unwrap();
    let untouched_id = untouched["id"].as_str().unwrap();
    let conn = test.state.connection().await.unwrap();
    let user_id: String = conn
        .query(
            "SELECT id FROM users WHERE email='same-amount-actions@example.com'",
            (),
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    let now = "2026-08-13T00:00:00Z";

    for (id, channel, account_id, payee_name, amount_cents) in [
        ("same-bank", "cmb", primary_id, "", 12_340_i64),
        (
            "same-platform",
            "alipay",
            primary_id,
            "虚构平台商户",
            12_340_i64,
        ),
    ] {
        conn.execute(
            "INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,occurred_on,account_id,payee_name,source_channel,external_id,created_at,updated_at) VALUES (?1,?2,'expense',?3,'2026-08-13',?4,?5,?6,?1,?7,?7)",
            libsql::params![id, user_id.clone(), amount_cents, account_id, payee_name, channel, now],
        ).await.unwrap();
    }
    conn.execute(
        "INSERT INTO duplicate_suspicions(id,user_id,transaction_id_a,transaction_id_b,score,match_rule,reason,created_at,updated_at) VALUES ('same-suspicion',?1,'same-bank','same-platform',1.0,'same_amount','synthetic',?2,?2)",
        libsql::params![user_id.clone(), now],
    ).await.unwrap();

    for rule in ["refund", "ambiguous"] {
        let bank_id = format!("unsupported-{rule}-bank");
        let platform_id = format!("unsupported-{rule}-platform");
        for (transaction_id, channel) in [(&bank_id, "cmb"), (&platform_id, "wechat")] {
            conn.execute(
                "INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,occurred_on,account_id,payee_name,source_channel,external_id,created_at,updated_at) VALUES (?1,?2,'expense',777,'2026-08-13',?3,'虚构不支持类型',?4,?1,?5,?5)",
                libsql::params![transaction_id.clone(), user_id.clone(), untouched_id, channel, now],
            ).await.unwrap();
        }
        conn.execute(
            "INSERT INTO duplicate_suspicions(id,user_id,transaction_id_a,transaction_id_b,score,match_rule,reason,created_at,updated_at) VALUES (?1,?2,?3,?4,0.9,?5,'synthetic unsupported',?6,?6)",
            libsql::params![format!("unsupported-{rule}"), user_id.clone(), bank_id, platform_id, rule, now],
        ).await.unwrap();
    }

    let balances_before = balances(&conn, &user_id).await;
    let primary_before = balances_before
        .iter()
        .find(|(id, _)| id == primary_id)
        .unwrap()
        .1;
    assert_eq!(primary_before, -24_680, "fixture starts double-counted");

    let (status, _, confirmed) = send(
        &test.router,
        Method::POST,
        "/api/v1/duplicate-suspicions/same-suspicion/confirm",
        None,
        Some(&cookie),
        Some("confirm-same-amount-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{confirmed}");
    assert_eq!(confirmed["status"], "confirmed");
    assert_eq!(confirmed["event"]["kind"], "consume");
    let event_id = confirmed["event"]["id"].as_str().unwrap();

    let (replay_status, _, replayed) = send(
        &test.router,
        Method::POST,
        "/api/v1/duplicate-suspicions/same-suspicion/confirm",
        None,
        Some(&cookie),
        Some("confirm-same-amount-0001"),
        true,
    )
    .await;
    assert_eq!(replay_status, StatusCode::OK);
    assert_eq!(replayed["event"]["id"], event_id);

    let mut rows = conn.query(
        "SELECT id,amount_cents,account_id,payee_name,archived_at,event_id FROM ledger_transactions WHERE id IN ('same-bank','same-platform') ORDER BY id",
        (),
    ).await.unwrap();
    let bank = rows.next().await.unwrap().unwrap();
    let bank_archived_at = bank.get::<Option<String>>(4).unwrap();
    let bank_event_id = bank.get::<Option<String>>(5).unwrap();
    let platform = rows.next().await.unwrap().unwrap();
    assert!(bank_archived_at.is_some());
    assert_eq!(bank_event_id.as_deref(), Some(event_id));
    assert_eq!(platform.get::<i64>(1).unwrap(), 12_340);
    assert_eq!(platform.get::<String>(2).unwrap(), primary_id);
    assert_eq!(platform.get::<String>(3).unwrap(), "虚构平台商户");
    assert!(platform.get::<Option<String>>(4).unwrap().is_none());
    assert_eq!(
        platform.get::<Option<String>>(5).unwrap().as_deref(),
        Some(event_id)
    );
    drop(bank);
    drop(platform);
    drop(rows);

    let balances_confirmed = balances(&conn, &user_id).await;
    let primary_confirmed = balances_confirmed
        .iter()
        .find(|(id, _)| id == primary_id)
        .unwrap()
        .1;
    assert_eq!(
        primary_confirmed, -12_340,
        "confirmed balance counts one expense"
    );
    let revert_payload: String = conn
        .query(
            "SELECT revert_payload FROM duplicate_suspicions WHERE id='same-suspicion'",
            (),
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    let payload: Value = serde_json::from_str(&revert_payload).unwrap();
    assert_eq!(payload["changed"]["id"], "same-platform");
    assert_eq!(payload["changed"]["amountCents"], 12_340);
    assert_eq!(payload["changed"]["accountId"], primary_id);
    assert_eq!(payload["archived"]["id"], "same-bank");
    assert!(payload["archived"]["archivedAt"].is_null());

    let unsupported_before = unsupported_ledger_snapshot(&conn, &user_id).await;
    let event_count_before: i64 = conn
        .query(
            "SELECT count(*) FROM transaction_events WHERE user_id=?1",
            [user_id.as_str()],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    for rule in ["refund", "ambiguous"] {
        let (status, _, error) = send(
            &test.router,
            Method::POST,
            &format!("/api/v1/duplicate-suspicions/unsupported-{rule}/confirm"),
            None,
            Some(&cookie),
            Some(&format!("unsupported-{rule}-confirm")),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{error}");
        assert_eq!(error["message"], "该类型暂不支持确认");
    }
    assert_eq!(
        unsupported_ledger_snapshot(&conn, &user_id).await,
        unsupported_before
    );
    let event_count_after: i64 = conn
        .query(
            "SELECT count(*) FROM transaction_events WHERE user_id=?1",
            [user_id.as_str()],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(event_count_after, event_count_before);
    let untouched_suspicion_count: i64 = conn.query(
        "SELECT count(*) FROM duplicate_suspicions WHERE user_id=?1 AND id LIKE 'unsupported-%' AND status='open' AND event_id IS NULL AND revert_payload=''",
        [user_id.as_str()],
    ).await.unwrap().next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(untouched_suspicion_count, 2);

    let (status, _, reverted) = send(
        &test.router,
        Method::POST,
        "/api/v1/duplicate-suspicions/same-suspicion/revert",
        None,
        Some(&cookie),
        Some("revert-same-amount-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reverted}");
    assert_eq!(reverted["status"], "open");
    assert!(reverted["event"].is_null());
    assert_eq!(balances(&conn, &user_id).await, balances_before);

    let mut rows = conn.query(
        "SELECT status,event_id,revert_payload FROM duplicate_suspicions WHERE id='same-suspicion'",
        (),
    ).await.unwrap();
    let suspicion = rows.next().await.unwrap().unwrap();
    assert_eq!(suspicion.get::<String>(0).unwrap(), "open");
    assert!(suspicion.get::<Option<String>>(1).unwrap().is_none());
    assert_eq!(suspicion.get::<String>(2).unwrap(), "");
    drop(suspicion);
    drop(rows);
    let remaining_event_count: i64 = conn
        .query(
            "SELECT count(*) FROM transaction_events WHERE id=?1",
            [event_id],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(remaining_event_count, 0);

    let (status, _, dismissed) = send(
        &test.router,
        Method::POST,
        "/api/v1/duplicate-suspicions/unsupported-refund/dismiss",
        None,
        Some(&cookie),
        Some("dismiss-suspicion-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{dismissed}");
    assert_eq!(dismissed["status"], "dismissed");
}

#[tokio::test]
async fn withdraw_fee_confirmation_splits_transfer_and_fee_and_reverts_balances() {
    async fn balances(conn: &libsql::Connection, user_id: &str) -> Vec<(String, i64)> {
        let mut rows = conn
            .query(
                "SELECT b.account_id,b.balance_cents FROM ledger_account_balances b JOIN ledger_accounts a ON a.id=b.account_id WHERE a.user_id=?1 ORDER BY b.account_id",
                [user_id],
            )
            .await
            .unwrap();
        let mut values = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            values.push((row.get(0).unwrap(), row.get(1).unwrap()));
        }
        values
    }

    fn balance_of(values: &[(String, i64)], account_id: &str) -> i64 {
        values.iter().find(|(id, _)| id == account_id).unwrap().1
    }

    let test = TestApp::new().await;
    let cookie = test
        .register_and_login("withdraw-fee-actions@example.com")
        .await;
    let platform_account = test
        .create_ledger_account(&cookie, "虚构微信零钱", "wechat_balance")
        .await;
    let (bank_status, _, bank_account) = send(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({ "name": "虚构招商卡", "accountType": "bank_card", "note": "测试资金账户" })),
        Some(&cookie),
        Some("create-withdraw-bank-account-0001"),
        true,
    )
    .await;
    assert_eq!(bank_status, StatusCode::CREATED, "{bank_account}");
    let platform_account_id = platform_account["id"].as_str().unwrap();
    let bank_account_id = bank_account["id"].as_str().unwrap();
    let conn = test.state.connection().await.unwrap();
    let user_id: String = conn
        .query(
            "SELECT id FROM users WHERE email='withdraw-fee-actions@example.com'",
            (),
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    let now = "2026-08-13T08:00:00Z";
    conn.execute(
        "INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,occurred_on,occurred_at,occurred_at_precision,transfer_from_account_id,transfer_to_account_id,payee_name,source_channel,external_id,created_at,updated_at) VALUES ('withdraw-platform',?1,'transfer',10010,'2026-08-13','2026-08-13 08:00:00','second',?2,?3,'零钱提现','wechat','synthetic-withdraw-platform',?4,?4)",
        libsql::params![user_id.clone(), platform_account_id, bank_account_id, now],
    ).await.unwrap();
    conn.execute(
        "INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,occurred_on,occurred_at,occurred_at_precision,account_id,payee_name,source_channel,external_id,created_at,updated_at) VALUES ('withdraw-bank',?1,'income',10000,'2026-08-13','2026-08-13 08:02:00','second',?2,'虚构到账','cmb','synthetic-withdraw-bank',?3,?3)",
        libsql::params![user_id.clone(), bank_account_id, now],
    ).await.unwrap();
    conn.execute(
        "INSERT INTO duplicate_suspicions(id,user_id,transaction_id_a,transaction_id_b,score,match_rule,reason,created_at,updated_at) VALUES ('withdraw-suspicion',?1,'withdraw-bank','withdraw-platform',1.0,'withdraw_fee','synthetic exact fee',?2,?2)",
        libsql::params![user_id.clone(), now],
    ).await.unwrap();

    let balances_before = balances(&conn, &user_id).await;
    assert_eq!(balance_of(&balances_before, platform_account_id), -10_010);
    assert_eq!(balance_of(&balances_before, bank_account_id), 20_010);

    let (status, _, confirmed) = send(
        &test.router,
        Method::POST,
        "/api/v1/duplicate-suspicions/withdraw-suspicion/confirm",
        None,
        Some(&cookie),
        Some("confirm-withdraw-fee-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{confirmed}");
    assert_eq!(confirmed["status"], "confirmed");
    assert_eq!(confirmed["event"]["kind"], "transfer");
    let event_id = confirmed["event"]["id"].as_str().unwrap();
    let transactions = confirmed["transactions"].as_array().unwrap();
    assert_eq!(transactions.len(), 3);
    let fee = transactions
        .iter()
        .find(|transaction| transaction["kind"] == "expense")
        .unwrap();
    let fee_id = fee["id"].as_str().unwrap().to_owned();
    assert_eq!(fee["amountCents"], 10);
    assert_eq!(fee["accountId"], platform_account_id);
    assert_eq!(fee["payeeName"], "提现手续费");
    assert_eq!(fee["categorySource"], "rule");
    assert!(
        transactions
            .iter()
            .all(|transaction| transaction["eventId"] == event_id)
    );
    let revert_payload: String = conn
        .query(
            "SELECT revert_payload FROM duplicate_suspicions WHERE id='withdraw-suspicion'",
            (),
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    let payload: Value = serde_json::from_str(&revert_payload).unwrap();
    assert_eq!(payload["changed"]["id"], "withdraw-platform");
    assert_eq!(payload["changed"]["amountCents"], 10_010);
    assert_eq!(payload["changed"]["transferToAccountId"], bank_account_id);
    assert_eq!(payload["archived"]["id"], "withdraw-bank");
    assert_eq!(payload["created"]["id"], fee_id);

    let balances_confirmed = balances(&conn, &user_id).await;
    assert_eq!(
        balance_of(&balances_confirmed, platform_account_id),
        -10_010
    );
    assert_eq!(balance_of(&balances_confirmed, bank_account_id), 10_000);
    let mut rows = conn.query(
        "SELECT id,kind,amount_cents,account_id,transfer_from_account_id,transfer_to_account_id,archived_at,event_id FROM ledger_transactions WHERE id IN ('withdraw-bank','withdraw-platform',?1) ORDER BY CASE kind WHEN 'income' THEN 0 WHEN 'expense' THEN 1 ELSE 2 END",
        [fee_id.as_str()],
    ).await.unwrap();
    let bank = rows.next().await.unwrap().unwrap();
    assert_eq!(bank.get::<String>(0).unwrap(), "withdraw-bank");
    assert!(bank.get::<Option<String>>(6).unwrap().is_some());
    assert_eq!(
        bank.get::<Option<String>>(7).unwrap().as_deref(),
        Some(event_id)
    );
    let fee_row = rows.next().await.unwrap().unwrap();
    assert_eq!(fee_row.get::<String>(0).unwrap(), fee_id);
    assert_eq!(fee_row.get::<i64>(2).unwrap(), 10);
    assert_eq!(fee_row.get::<String>(3).unwrap(), platform_account_id);
    assert_eq!(
        fee_row.get::<Option<String>>(7).unwrap().as_deref(),
        Some(event_id)
    );
    let platform = rows.next().await.unwrap().unwrap();
    assert_eq!(platform.get::<String>(0).unwrap(), "withdraw-platform");
    assert_eq!(platform.get::<i64>(2).unwrap(), 10_000);
    assert_eq!(platform.get::<String>(4).unwrap(), platform_account_id);
    assert_eq!(platform.get::<String>(5).unwrap(), bank_account_id);
    assert_eq!(
        platform.get::<Option<String>>(7).unwrap().as_deref(),
        Some(event_id)
    );
    drop(bank);
    drop(fee_row);
    drop(platform);
    drop(rows);

    let (_, _, summary) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions/summary?month=2026-08",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(summary["incomeCents"], 0);
    assert_eq!(summary["expenseCents"], 10);
    assert_eq!(summary["transactionCount"], 1);

    let (status, _, reverted) = send(
        &test.router,
        Method::POST,
        "/api/v1/duplicate-suspicions/withdraw-suspicion/revert",
        None,
        Some(&cookie),
        Some("revert-withdraw-fee-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reverted}");
    assert_eq!(reverted["status"], "open");
    assert_eq!(balances(&conn, &user_id).await, balances_before);
    let fee_count: i64 = conn
        .query(
            "SELECT count(*) FROM ledger_transactions WHERE id=?1",
            [fee_id.as_str()],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(fee_count, 0);
    let event_count: i64 = conn
        .query(
            "SELECT count(*) FROM transaction_events WHERE id=?1",
            [event_id],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(event_count, 0);
    let mut rows = conn.query(
        "SELECT id,amount_cents,transfer_to_account_id,archived_at,event_id FROM ledger_transactions WHERE id IN ('withdraw-bank','withdraw-platform') ORDER BY id",
        (),
    ).await.unwrap();
    let bank = rows.next().await.unwrap().unwrap();
    assert!(bank.get::<Option<String>>(3).unwrap().is_none());
    assert!(bank.get::<Option<String>>(4).unwrap().is_none());
    let platform = rows.next().await.unwrap().unwrap();
    assert_eq!(platform.get::<i64>(1).unwrap(), 10_010);
    assert_eq!(
        platform.get::<Option<String>>(2).unwrap().as_deref(),
        Some(bank_account_id)
    );
    assert!(platform.get::<Option<String>>(4).unwrap().is_none());
    drop(bank);
    drop(platform);
    drop(rows);

    conn.execute(
        "INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,occurred_on,transfer_from_account_id,transfer_to_account_id,source_channel,external_id,created_at,updated_at) VALUES ('invalid-withdraw-platform',?1,'transfer',10011,'2026-08-13',?2,?3,'wechat','synthetic-invalid-platform',?4,?4)",
        libsql::params![user_id.clone(), platform_account_id, bank_account_id, now],
    ).await.unwrap();
    conn.execute(
        "INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,occurred_on,account_id,source_channel,external_id,created_at,updated_at) VALUES ('invalid-withdraw-bank',?1,'income',10000,'2026-08-13',?2,'cmb','synthetic-invalid-bank',?3,?3)",
        libsql::params![user_id.clone(), bank_account_id, now],
    ).await.unwrap();
    conn.execute(
        "INSERT INTO duplicate_suspicions(id,user_id,transaction_id_a,transaction_id_b,score,match_rule,reason,created_at,updated_at) VALUES ('invalid-withdraw-suspicion',?1,'invalid-withdraw-bank','invalid-withdraw-platform',0.1,'withdraw_fee','synthetic invalid fee',?2,?2)",
        libsql::params![user_id.clone(), now],
    ).await.unwrap();
    let (status, _, error) = send(
        &test.router,
        Method::POST,
        "/api/v1/duplicate-suspicions/invalid-withdraw-suspicion/confirm",
        None,
        Some(&cookie),
        Some("confirm-invalid-withdraw-fee-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{error}");
    assert_eq!(
        error["message"],
        "withdraw_fee 确认要求平台 transfer 与银行 income 精确符合 0.1% 手续费"
    );
    let invalid_state: (String, Option<String>, String) = {
        let row = conn.query(
            "SELECT status,event_id,revert_payload FROM duplicate_suspicions WHERE id='invalid-withdraw-suspicion'",
            (),
        ).await.unwrap().next().await.unwrap().unwrap();
        (
            row.get(0).unwrap(),
            row.get(1).unwrap(),
            row.get(2).unwrap(),
        )
    };
    assert_eq!(invalid_state, ("open".to_owned(), None, String::new()));
    let invalid_event_count: i64 = conn
        .query(
            "SELECT count(*) FROM transaction_events WHERE user_id=?1",
            [user_id.as_str()],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(invalid_event_count, 0);
    let invalid_ledger_state: (i64, Option<String>, Option<String>, Option<String>) = {
        let row = conn
            .query(
                "SELECT amount_cents,transfer_to_account_id,archived_at,event_id FROM ledger_transactions WHERE id='invalid-withdraw-platform'",
                (),
            )
            .await
            .unwrap()
            .next()
            .await
            .unwrap()
            .unwrap();
        (
            row.get(0).unwrap(),
            row.get(1).unwrap(),
            row.get(2).unwrap(),
            row.get(3).unwrap(),
        )
    };
    assert_eq!(
        invalid_ledger_state,
        (10_011, Some(bank_account_id.to_owned()), None, None)
    );

    conn.execute(
        "INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,occurred_on,transfer_from_account_id,source_channel,external_id,created_at,updated_at) VALUES ('legacy-withdraw-platform',?1,'transfer',10010,'2026-08-13',?2,'wechat','synthetic-legacy-platform',?3,?3)",
        libsql::params![user_id.clone(), platform_account_id, now],
    ).await.unwrap();
    conn.execute(
        "INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,occurred_on,account_id,source_channel,external_id,created_at,updated_at) VALUES ('legacy-withdraw-bank',?1,'income',10000,'2026-08-13',?2,'cmb','synthetic-legacy-bank',?3,?3)",
        libsql::params![user_id.clone(), bank_account_id, now],
    ).await.unwrap();
    conn.execute(
        "INSERT INTO duplicate_suspicions(id,user_id,transaction_id_a,transaction_id_b,score,match_rule,reason,created_at,updated_at) VALUES ('legacy-withdraw-suspicion',?1,'legacy-withdraw-bank','legacy-withdraw-platform',1.0,'withdraw_fee','synthetic legacy shape',?2,?2)",
        libsql::params![user_id, now],
    ).await.unwrap();
    let (status, _, legacy_confirmed) = send(
        &test.router,
        Method::POST,
        "/api/v1/duplicate-suspicions/legacy-withdraw-suspicion/confirm",
        None,
        Some(&cookie),
        Some("confirm-legacy-withdraw-fee-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{legacy_confirmed}");
    let legacy_platform = legacy_confirmed["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|transaction| transaction["id"] == "legacy-withdraw-platform")
        .unwrap();
    assert_eq!(legacy_platform["amountCents"], 10_000);
    assert_eq!(legacy_platform["transferToAccountId"], bank_account_id);
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

async fn send_with_authorization(
    router: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
    authorization: &str,
    idempotency_key: Option<&str>,
) -> (StatusCode, HeaderMap, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, authorization);
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    if let Some(key) = idempotency_key {
        builder = builder.header("idempotency-key", key);
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

async fn send_form(
    router: &Router,
    uri: &str,
    body: &str,
    cookie: Option<&str>,
    origin: Option<&str>,
) -> (StatusCode, HeaderMap, Value) {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    if let Some(origin) = origin {
        builder = builder.header(header::ORIGIN, origin);
    }
    let response = router
        .clone()
        .oneshot(builder.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
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

#[allow(clippy::too_many_arguments)]
async fn send_with_credentials(
    router: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
    cookie: Option<&str>,
    authorization: Option<&str>,
    origin: Option<&str>,
    idempotency_key: Option<&str>,
) -> (StatusCode, HeaderMap, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    if let Some(authorization) = authorization {
        builder = builder.header(header::AUTHORIZATION, authorization);
    }
    if let Some(origin) = origin {
        builder = builder.header(header::ORIGIN, origin);
    }
    if let Some(key) = idempotency_key {
        builder = builder.header("idempotency-key", key);
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

async fn send_multipart(
    router: &Router,
    uri: &str,
    cookie: &str,
    key: &str,
    fields: &[(&str, Option<&str>, &[u8])],
) -> (StatusCode, Value) {
    let boundary = "zhiyu-import-test-boundary";
    let mut body = Vec::new();
    for (name, filename, value) in fields {
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"").as_bytes(),
        );
        if let Some(filename) = filename {
            body.extend_from_slice(format!("; filename=\"{filename}\"").as_bytes());
        }
        body.extend_from_slice(b"\r\n\r\n");
        body.extend_from_slice(value);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header(header::COOKIE, cookie)
                .header(header::ORIGIN, "http://test.local")
                .header("idempotency-key", key)
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn send_import_commit(
    router: &Router,
    batch_id: &str,
    cookie: &str,
    key: &str,
) -> (StatusCode, Value) {
    let (detail_status, _, detail) = send(
        router,
        Method::GET,
        &format!("/api/v1/imports/{batch_id}"),
        None,
        Some(cookie),
        None,
        false,
    )
    .await;
    assert_eq!(detail_status, StatusCode::OK, "{detail}");
    let mapped_methods = detail["payMethods"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|method| !method["accountId"].is_null())
        .filter_map(|method| method["payMethod"].as_str())
        .map(str::to_owned)
        .collect::<std::collections::BTreeSet<_>>();
    let neutral_methods = detail["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| record["direction"] == "neutral" && record["disposition"] == "import")
        .filter_map(|record| record["payMethod"].as_str())
        .map(|method| method.split('&').next().unwrap().trim())
        .filter(|method| !method.is_empty())
        .filter(|method| !mapped_methods.contains(*method))
        .map(str::to_owned)
        .collect::<std::collections::BTreeSet<_>>();
    if !neutral_methods.is_empty() {
        let account_key = format!("{key}-neutral-account");
        let (account_status, _, account) = send(
            router,
            Method::POST,
            "/api/v1/ledger-accounts",
            Some(json!({
                "name": format!("中性导入账户-{batch_id}"),
                "accountType": "other"
            })),
            Some(cookie),
            Some(&account_key),
            true,
        )
        .await;
        assert_eq!(account_status, StatusCode::CREATED, "{account}");
        for (index, pay_method) in neutral_methods.into_iter().enumerate() {
            let mapping_key = format!("{key}-neutral-mapping-{index}");
            let (mapping_status, _, mapping) = send(
                router,
                Method::POST,
                "/api/v1/imports/mappings",
                Some(json!({
                    "sourceChannel": detail["channel"],
                    "payMethod": pay_method,
                    "accountId": account["id"]
                })),
                Some(cookie),
                Some(&mapping_key),
                true,
            )
            .await;
            assert_eq!(mapping_status, StatusCode::OK, "{mapping}");
        }
    }
    let (status, _, body) = send_with_credentials(
        router,
        Method::POST,
        &format!("/api/v1/imports/{batch_id}/commit"),
        Some(json!({ "accountId": detail["accountId"] })),
        Some(cookie),
        None,
        Some("http://test.local"),
        Some(key),
    )
    .await;
    (status, body)
}

async fn send_import_discard(
    router: &Router,
    batch_id: &str,
    cookie: &str,
    key: &str,
) -> (StatusCode, Value) {
    let (status, _, body) = send_with_credentials(
        router,
        Method::DELETE,
        &format!("/api/v1/imports/{batch_id}"),
        None,
        Some(cookie),
        None,
        Some("http://test.local"),
        Some(key),
    )
    .await;
    (status, body)
}

#[tokio::test]
async fn committed_import_exposes_batch_link_and_discard_cascades_it() {
    let test = TestApp::new().await;
    let owner = test
        .register_and_login("import-batch-link@example.com")
        .await;
    let fixture = include_bytes!("fixtures/alipay_synthetic_gb18030.csv");
    let (status, preview) = send_multipart(
        &test.router,
        "/api/v1/imports",
        &owner,
        "import-batch-link-upload",
        &[
            ("file", Some("batch-link.csv"), fixture),
            ("channel", None, b"alipay"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{preview}");
    let batch_id = preview["id"].as_str().unwrap();

    let (status, committed) =
        send_import_commit(&test.router, batch_id, &owner, "import-batch-link-commit").await;
    assert_eq!(status, StatusCode::OK, "{committed}");
    assert!(committed["importedCount"].as_i64().unwrap() > 0);

    let conn = test.state.connection().await.unwrap();
    let transaction_id: String = conn
        .query(
            "SELECT transaction_id FROM import_records WHERE batch_id=?1 AND transaction_id IS NOT NULL ORDER BY row_index LIMIT 1",
            libsql::params![batch_id],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    drop(conn);

    let (status, _, list) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions?pageSize=200",
        None,
        Some(&owner),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let transaction = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == transaction_id)
        .unwrap();
    assert_eq!(transaction["createdBy"], "plugin:bill-imports");
    assert!(transaction["links"].as_array().unwrap().iter().any(|link| {
        link["pluginId"] == "bill-imports" && link["kind"] == "batch" && link["refId"] == batch_id
    }));

    let (status, discarded) =
        send_import_discard(&test.router, batch_id, &owner, "import-batch-link-discard").await;
    assert_eq!(status, StatusCode::OK, "{discarded}");
    assert_eq!(
        discarded["deletedCount"], committed["importedCount"],
        "all pristine imported transactions should be removed"
    );

    let conn = test.state.connection().await.unwrap();
    let transaction_count: i64 = conn
        .query(
            "SELECT COUNT(*) FROM ledger_transactions WHERE id=?1",
            libsql::params![transaction_id],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    let link_count: i64 = conn
        .query(
            "SELECT COUNT(*) FROM transaction_links WHERE plugin_id='bill-imports' AND kind='batch' AND ref_id=?1",
            libsql::params![batch_id],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(transaction_count, 0);
    assert_eq!(link_count, 0);
}

#[tokio::test]
async fn disabled_debt_handler_skips_import_discard_and_reopen_reconciles() {
    let test = TestApp::new().await;
    let owner = test
        .register_and_login("debt-link-import-undo@example.com")
        .await;
    let fixture = include_bytes!("fixtures/alipay_synthetic_gb18030.csv");
    let (status, preview) = send_multipart(
        &test.router,
        "/api/v1/imports",
        &owner,
        "debt-link-import-upload",
        &[
            ("file", Some("linked.csv"), fixture),
            ("channel", None, b"alipay"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{preview}");
    let batch_id = preview["id"].as_str().unwrap();
    let (status, committed) =
        send_import_commit(&test.router, batch_id, &owner, "debt-link-import-commit").await;
    assert_eq!(status, StatusCode::OK, "{committed}");
    let linked_account = test
        .create_ledger_account(&owner, "导入撤销关联账户", "alipay_balance")
        .await;
    let linked_account_id = linked_account["id"].as_str().unwrap();
    let (status, _, bound) = send_with_credentials(
        &test.router,
        Method::POST,
        &format!("/api/v1/imports/{batch_id}/account"),
        Some(json!({ "accountId": linked_account_id })),
        Some(&owner),
        Some("debt-link-import-bind"),
        Some("http://test.local"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{bound}");

    let conn = test.state.connection().await.unwrap();
    let row = conn.query(
        "SELECT id,kind,amount_cents,occurred_on,account_id FROM ledger_transactions WHERE import_batch_id=?1 AND archived_at IS NULL ORDER BY amount_cents,id LIMIT 1",
        libsql::params![batch_id],
    ).await.unwrap().next().await.unwrap().unwrap();
    let transaction_id: String = row.get(0).unwrap();
    let kind: String = row.get(1).unwrap();
    let amount_cents: i64 = row.get(2).unwrap();
    let occurred_on: String = row.get(3).unwrap();
    let account_id: String = row
        .get::<Option<String>>(4)
        .unwrap()
        .expect("committed fixture transaction has an account");
    drop(row);
    drop(conn);
    let direction = if kind == "income" {
        "lend_out"
    } else {
        "borrow_in"
    };
    let conn = test.state.connection().await.unwrap();
    let mut user_rows = conn
        .query(
            "SELECT user_id FROM import_batches WHERE id=?1",
            libsql::params![batch_id],
        )
        .await
        .unwrap();
    let user_id: String = user_rows.next().await.unwrap().unwrap().get(0).unwrap();
    drop(user_rows);
    let debt_id = Uuid::now_v7().to_string();
    let counterparty_id = Uuid::now_v7().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute("INSERT INTO counterparties(id,user_id,display_name,normalized_name,note,created_at,updated_at) VALUES (?1,?2,'导入撤销对手方','导入撤销对手方','',?3,?3)", libsql::params![counterparty_id.clone(), user_id.clone(), now.clone()]).await.unwrap();
    conn.execute("INSERT INTO debts(id,user_id,counterparty_id,direction,principal_cents,currency,occurred_on,note,created_at,updated_at,account_id,origin_kind) VALUES (?1,?2,?3,?4,1000000000,'CNY','2026-08-01','',?5,?5,NULL,'no_cash_movement')", libsql::params![debt_id.clone(), user_id.clone(), counterparty_id, direction, now]).await.unwrap();
    let balance_before: i64 = conn
        .query(
            "SELECT balance_cents FROM ledger_account_balances WHERE account_id=?1",
            libsql::params![account_id.clone()],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    let batch_delta: i64 = conn.query(
        "SELECT COALESCE(SUM(CASE WHEN kind='income' AND account_id=?2 THEN amount_cents WHEN kind='expense' AND account_id=?2 THEN -amount_cents WHEN kind='transfer' AND transfer_from_account_id=?2 THEN -amount_cents WHEN kind='transfer' AND transfer_to_account_id=?2 THEN amount_cents ELSE 0 END),0) FROM ledger_transactions WHERE import_batch_id=?1 AND archived_at IS NULL",
        libsql::params![batch_id, account_id.clone()],
    ).await.unwrap().next().await.unwrap().unwrap().get(0).unwrap();
    let linked_delta = if kind == "income" {
        amount_cents
    } else {
        -amount_cents
    };
    drop(conn);
    let (status, _, linked) = send(
        &test.router, Method::POST, &format!("/api/v1/debts/{debt_id}/repayments"),
        Some(json!({ "amountCents": amount_cents, "effectiveOn": occurred_on, "accountId": null, "transactionId": transaction_id.clone() })),
        Some(&owner), Some("debt-link-import-repayment"), true,
    ).await;
    assert_eq!(status, StatusCode::CREATED, "{linked}");
    let payment_id = linked["repayments"][0]["id"].as_str().unwrap();
    let conn = test.state.connection().await.unwrap();
    let balance_linked: i64 = conn
        .query(
            "SELECT balance_cents FROM ledger_account_balances WHERE account_id=?1",
            libsql::params![account_id.clone()],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(
        balance_linked, balance_before,
        "linked event must not double count the imported transaction"
    );
    drop(conn);

    let (status, _, disabled) = send(
        &test.router,
        Method::PATCH,
        "/api/v1/plugins/debts",
        Some(json!({ "enabled": false })),
        Some(&owner),
        Some("debt-link-disable-before-discard"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{disabled}");
    assert_eq!(disabled["enabled"], false);

    let (status, _, blocked) = send(
        &test.router,
        Method::GET,
        "/api/v1/debts",
        None,
        Some(&owner),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{blocked}");
    assert_eq!(blocked["code"], "plugin_disabled");
    let (status, _, transactions) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions?pageSize=100",
        None,
        Some(&owner),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{transactions}");
    let linked_transaction = transactions["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == transaction_id)
        .unwrap();
    assert!(
        linked_transaction["links"]
            .as_array()
            .unwrap()
            .iter()
            .any(|link| link["pluginId"] == "debts" && link["refId"] == debt_id)
    );

    let (status, discarded) =
        send_import_discard(&test.router, batch_id, &owner, "debt-link-import-discard").await;
    assert_eq!(status, StatusCode::OK, "{discarded}");
    let conn = test.state.connection().await.unwrap();
    let row = conn.query("SELECT transaction_id,amount_cents,effective_on,account_id,transaction_auto_created FROM repayment_events WHERE id=?1", libsql::params![payment_id]).await.unwrap().next().await.unwrap().unwrap();
    assert!(
        row.get::<Option<String>>(0).unwrap().is_none(),
        "the core foreign key detaches the deleted transaction without invoking the plugin"
    );
    assert_eq!(row.get::<i64>(1).unwrap(), amount_cents);
    assert_eq!(row.get::<String>(2).unwrap(), occurred_on);
    assert_eq!(row.get::<String>(3).unwrap(), account_id);
    assert_eq!(row.get::<i64>(4).unwrap(), 0);
    drop(row);
    let deleted_count: i64 = conn
        .query(
            "SELECT COUNT(*) FROM ledger_transactions WHERE id=?1",
            libsql::params![transaction_id.clone()],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(
        deleted_count, 0,
        "disabled deletion handler must not rebuild"
    );
    let balance_while_disabled: i64 = conn
        .query(
            "SELECT balance_cents FROM ledger_account_balances WHERE account_id=?1",
            libsql::params![account_id.clone()],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(balance_while_disabled, balance_before - batch_delta);
    drop(conn);

    let (status, _, enabled) = send(
        &test.router,
        Method::PATCH,
        "/api/v1/plugins/debts",
        Some(json!({ "enabled": true })),
        Some(&owner),
        Some("debt-link-reenable-after-discard"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{enabled}");
    assert_eq!(enabled["enabled"], true);
    assert!(enabled["reconciled"].as_u64().unwrap() >= 1);

    let conn = test.state.connection().await.unwrap();
    let row = conn
        .query(
            "SELECT transaction_id,transaction_auto_created FROM repayment_events WHERE id=?1",
            libsql::params![payment_id],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap();
    let rebuilt_transaction_id = row
        .get::<Option<String>>(0)
        .unwrap()
        .expect("reopening runs the debt self-check before enabling");
    assert_ne!(rebuilt_transaction_id, transaction_id);
    assert_eq!(row.get::<i64>(1).unwrap(), 1);
    drop(row);
    let rebuilt = conn.query(
        "SELECT t.pnl_scope,t.created_by,COUNT(l.id) FROM ledger_transactions t LEFT JOIN transaction_links l ON l.transaction_id=t.id AND l.user_id=t.user_id WHERE t.id=?1 AND t.user_id=?2 GROUP BY t.id",
        libsql::params![rebuilt_transaction_id, user_id],
    ).await.unwrap().next().await.unwrap().unwrap();
    assert_eq!(rebuilt.get::<String>(0).unwrap(), "excluded");
    assert_eq!(rebuilt.get::<String>(1).unwrap(), "plugin:debts");
    assert_eq!(rebuilt.get::<i64>(2).unwrap(), 1);
    let balance_after: i64 = conn
        .query(
            "SELECT balance_cents FROM ledger_account_balances WHERE account_id=?1",
            libsql::params![account_id],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(
        balance_after,
        balance_before - batch_delta + linked_delta,
        "all imported movements are removed while the linked repayment is restored as a debt movement"
    );
}

#[tokio::test]
async fn import_backfill_keeps_version_one_and_whole_batch_remains_undoable() {
    let (test, owner, preview) =
        setup_alipay_import("import-backfill@example.com", "import-backfill-upload").await;
    let batch_id = preview["id"].as_str().unwrap();
    let (status, committed) =
        send_import_commit(&test.router, batch_id, &owner, "import-backfill-commit").await;
    assert_eq!(status, StatusCode::OK, "{committed}");
    let imported_count = committed["importedCount"].as_i64().unwrap();

    let conn = test.state.connection().await.unwrap();
    let mut rows = conn
        .query(
            "SELECT user_id FROM import_batches WHERE id=?1",
            libsql::params![batch_id],
        )
        .await
        .unwrap();
    let user_id = rows
        .next()
        .await
        .unwrap()
        .unwrap()
        .get::<String>(0)
        .unwrap();
    drop(rows);
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute("INSERT INTO ledger_accounts(id,user_id,name,normalized_name,account_type,card_number,version,created_at,updated_at) VALUES ('backfill-card',?1,'回填测试卡','回填测试卡','bank_card','622200004444',1,?2,?2)", libsql::params![user_id.clone(), now]).await.unwrap();
    let (status, _, backfill) = send_with_credentials(
        &test.router,
        Method::POST,
        &format!("/api/v1/imports/{batch_id}/account"),
        Some(json!({ "accountId": "backfill-card" })),
        Some(&owner),
        Some("import-backfill-bind"),
        Some("http://test.local"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{backfill}");
    assert!(backfill["updatedCount"].as_i64().unwrap() > 0);
    let mut rows = conn.query("SELECT count(*),min(version),max(version) FROM ledger_transactions WHERE import_batch_id=?1 AND account_id='backfill-card'", libsql::params![batch_id]).await.unwrap();
    let row = rows.next().await.unwrap().unwrap();
    assert_eq!(row.get::<i64>(0).unwrap(), backfill["updatedCount"]);
    assert_eq!(row.get::<i64>(1).unwrap(), 1);
    assert_eq!(row.get::<i64>(2).unwrap(), 1);
    drop(row);
    drop(rows);

    let (status, discarded) =
        send_import_discard(&test.router, batch_id, &owner, "import-backfill-undo").await;
    assert_eq!(status, StatusCode::OK, "{discarded}");
    assert_eq!(discarded["deletedCount"], imported_count);
    assert_eq!(discarded["retainedModifiedCount"], 0);
}

#[tokio::test]
async fn disabled_auto_categorize_keeps_imported_transactions_uncategorized() {
    let (test, owner, preview) = setup_alipay_import(
        "disabled-auto-categorize@example.com",
        "disabled-auto-upload",
    )
    .await;
    let batch_id = preview["id"].as_str().unwrap();
    let (status, _, category) = send(
        &test.router,
        Method::POST,
        "/api/v1/categories",
        Some(json!({ "name": "不会自动采用", "kind": "expense", "sortOrder": 0 })),
        Some(&owner),
        Some("disabled-auto-category"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{category}");
    let (status, _, rule) = send(
        &test.router,
        Method::POST,
        "/api/v1/category-rules",
        Some(json!({
            "priority": 1,
            "enabled": true,
            "sourceChannel": "alipay",
            "categoryId": category["id"],
            "note": "",
            "conditions": [{
                "matchField": "amount_cents",
                "matchKind": "gte",
                "matchValue": "1"
            }]
        })),
        Some(&owner),
        Some("disabled-auto-rule"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{rule}");

    let (status, _, disabled) = send(
        &test.router,
        Method::PATCH,
        "/api/v1/plugins/auto-categorize",
        Some(json!({ "enabled": false })),
        Some(&owner),
        Some("disable-auto-before-import"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{disabled}");
    let (status, _, categories) = send(
        &test.router,
        Method::GET,
        "/api/v1/categories",
        None,
        Some(&owner),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{categories}");
    let (status, _, blocked) = send(
        &test.router,
        Method::GET,
        "/api/v1/category-rules",
        None,
        Some(&owner),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{blocked}");
    assert_eq!(blocked["code"], "plugin_disabled");

    let (status, committed) =
        send_import_commit(&test.router, batch_id, &owner, "disabled-auto-commit").await;
    assert_eq!(status, StatusCode::OK, "{committed}");
    assert!(committed["importedCount"].as_i64().unwrap() > 0);

    let conn = test.state.connection().await.unwrap();
    let categorized_count: i64 = conn
        .query(
            "SELECT COUNT(*) FROM ledger_transactions WHERE import_batch_id=?1 AND category_source<>'none'",
            libsql::params![batch_id],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(categorized_count, 0);
}

async fn setup_alipay_import(email: &str, key: &str) -> (TestApp, String, Value) {
    let test = TestApp::new().await;
    let owner = test.register_and_login(email).await;
    let fixture = include_bytes!("fixtures/alipay_synthetic_gb18030.csv");
    let (status, body) = send_multipart(
        &test.router,
        "/api/v1/imports",
        &owner,
        key,
        &[
            ("file", Some("../private\\bill.csv"), fixture),
            ("channel", None, b"alipay"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["fileName"], "bill.csv");
    (test, owner, body)
}

async fn seed_self_transfer_batch(
    conn: &libsql::Connection,
    user_id: &str,
    batch_id: &str,
    source_channel: &str,
    records: &[(&str, &str, &str, i64)],
) {
    let now = "2026-08-14T00:00:00Z";
    conn.execute(
        "INSERT INTO import_batches(id,user_id,source_channel,parser_version,file_name,file_sha256,period_start,period_end,total_count,status,created_at,updated_at) VALUES (?1,?2,?3,1,'synthetic.json',?4,'2026-08-14','2026-08-14',?5,'preview',?6,?6)",
        libsql::params![batch_id, user_id, source_channel, "8".repeat(64), records.len() as i64, now],
    )
    .await
    .unwrap();
    for (index, (external_id, direction, counterparty, amount_cents)) in records.iter().enumerate()
    {
        conn.execute(
            "INSERT INTO import_records(id,batch_id,row_index,external_id,occurred_at,occurred_on,direction,amount_cents,counterparty,product,pay_method,source_note,occurred_at_precision,disposition,created_at) VALUES (?1,?2,?3,?4,'2026-08-14 12:00:00','2026-08-14',?5,?6,?7,'虚构测试','','虚构测试','second','import',?8)",
            libsql::params![
                format!("{batch_id}-record-{index}"),
                batch_id,
                index as i64 + 1,
                external_id,
                direction,
                amount_cents,
                counterparty,
                now,
            ],
        )
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn cmb_import_exposes_normalized_payee_key_and_searches_by_it() {
    let test = TestApp::new().await;
    let email = "cmb-payee-key@example.com";
    let owner = test.register_and_login(email).await;
    let account = test
        .create_ledger_account(&owner, "虚构招商账单账户", "bank_card")
        .await;
    let account_id = account["id"].as_str().unwrap();
    let conn = test.state.connection().await.unwrap();
    let user_id: String = conn
        .query(
            "SELECT id FROM users WHERE email=?1",
            libsql::params![email],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    seed_self_transfer_batch(
        &conn,
        &user_id,
        "cmb-payee-key-batch",
        "cmb",
        &[(
            "CMB-PAYEE-KEY-0001",
            "expense",
            "华尔街见闻1364473102",
            1_999,
        )],
    )
    .await;
    drop(conn);

    let (status, _, committed) = send(
        &test.router,
        Method::POST,
        "/api/v1/imports/cmb-payee-key-batch/commit",
        Some(json!({ "accountId": account_id })),
        Some(&owner),
        Some("commit-cmb-payee-key"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{committed}");
    assert_eq!(committed["importedCount"], 1);

    let (status, _, monthly) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions?month=2026-08",
        None,
        Some(&owner),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{monthly}");
    assert_eq!(monthly["total"], 1);
    assert_eq!(monthly["items"][0]["payeeName"], "华尔街见闻1364473102");
    assert_eq!(monthly["items"][0]["payeeKey"], "华尔街见闻");
    assert_ne!(
        monthly["items"][0]["payeeName"],
        monthly["items"][0]["payeeKey"]
    );

    let (status, _, search) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions?q=%E5%8D%8E%E5%B0%94%E8%A1%97%E8%A7%81%E9%97%BB",
        None,
        Some(&owner),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{search}");
    assert_eq!(search["total"], 1);
    assert_eq!(search["items"][0]["payeeName"], "华尔街见闻1364473102");
    assert_eq!(search["items"][0]["payeeKey"], "华尔街见闻");
}

#[tokio::test]
async fn self_transfer_aliases_apply_exactly_to_bank_imports_only() {
    let test = TestApp::new().await;
    let email = "self-transfer-aliases@example.com";
    let owner = test.register_and_login(email).await;
    let account = test
        .create_ledger_account(&owner, "虚构账单银行卡", "bank_card")
        .await;
    let account_id = account["id"].as_str().unwrap();

    let (status, _, created) = send(
        &test.router,
        Method::POST,
        "/api/v1/self-transfer-aliases",
        Some(json!({ "alias": " 虚构本人 ", "note": "纯虚构测试" })),
        Some(&owner),
        Some("create-self-transfer-alias"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["alias"], "虚构本人");
    assert_eq!(created["normalizedAlias"], "虚构本人");

    let (status, _, aliases) = send(
        &test.router,
        Method::GET,
        "/api/v1/self-transfer-aliases",
        None,
        Some(&owner),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{aliases}");
    assert_eq!(aliases.as_array().unwrap().len(), 1);

    let conn = test.state.connection().await.unwrap();
    let user_id: String = conn
        .query(
            "SELECT id FROM users WHERE email=?1",
            libsql::params![email],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    seed_self_transfer_batch(
        &conn,
        &user_id,
        "self-transfer-cmb",
        "cmb",
        &[
            ("SELF-CMB-EXPENSE", "expense", "虚构本人", 101),
            ("SELF-CMB-INCOME", "income", "虚构本人", 202),
            ("SELF-CMB-PARTIAL", "expense", "虚构本人明", 303),
            ("SELF-CMB-WECHAT", "expense", "微信转账1000050201", 404),
        ],
    )
    .await;
    seed_self_transfer_batch(
        &conn,
        &user_id,
        "self-transfer-cmbc",
        "cmbc",
        &[("SELF-CMBC-EXPENSE", "expense", "虚构本人", 505)],
    )
    .await;
    seed_self_transfer_batch(
        &conn,
        &user_id,
        "self-transfer-alipay",
        "alipay",
        &[("SELF-ALIPAY-EXPENSE", "expense", "虚构本人", 606)],
    )
    .await;

    for (batch_id, key) in [
        ("self-transfer-cmb", "commit-self-transfer-cmb"),
        ("self-transfer-cmbc", "commit-self-transfer-cmbc"),
        ("self-transfer-alipay", "commit-self-transfer-alipay"),
    ] {
        let (status, _, committed) = send(
            &test.router,
            Method::POST,
            &format!("/api/v1/imports/{batch_id}/commit"),
            Some(json!({ "accountId": account_id })),
            Some(&owner),
            Some(key),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{committed}");
    }

    let mut rows = conn
        .query(
            "SELECT external_id,kind,account_id,transfer_from_account_id,transfer_to_account_id FROM ledger_transactions WHERE import_batch_id IN ('self-transfer-cmb','self-transfer-cmbc','self-transfer-alipay') ORDER BY external_id",
            (),
        )
        .await
        .unwrap();
    let mut transactions = std::collections::BTreeMap::new();
    while let Some(row) = rows.next().await.unwrap() {
        transactions.insert(
            row.get::<String>(0).unwrap(),
            (
                row.get::<String>(1).unwrap(),
                row.get::<Option<String>>(2).unwrap(),
                row.get::<Option<String>>(3).unwrap(),
                row.get::<Option<String>>(4).unwrap(),
            ),
        );
    }
    assert_eq!(
        transactions["SELF-CMB-EXPENSE"],
        ("transfer".into(), None, Some(account_id.into()), None)
    );
    assert_eq!(
        transactions["SELF-CMB-INCOME"],
        ("transfer".into(), None, None, Some(account_id.into()))
    );
    assert_eq!(
        transactions["SELF-CMBC-EXPENSE"],
        ("transfer".into(), None, Some(account_id.into()), None)
    );
    for external_id in ["SELF-CMB-PARTIAL", "SELF-CMB-WECHAT", "SELF-ALIPAY-EXPENSE"] {
        assert_eq!(
            transactions[external_id],
            ("expense".into(), Some(account_id.into()), None, None),
            "{external_id} must keep the existing expense behavior"
        );
    }

    let (status, _, deleted) = send(
        &test.router,
        Method::DELETE,
        "/api/v1/self-transfer-aliases",
        Some(json!({ "id": created["id"] })),
        Some(&owner),
        Some("delete-self-transfer-alias"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{deleted}");

    seed_self_transfer_batch(
        &conn,
        &user_id,
        "self-transfer-no-alias",
        "cmb",
        &[("SELF-NO-ALIAS", "expense", "虚构本人", 707)],
    )
    .await;
    let (status, _, committed) = send(
        &test.router,
        Method::POST,
        "/api/v1/imports/self-transfer-no-alias/commit",
        Some(json!({ "accountId": account_id })),
        Some(&owner),
        Some("commit-self-transfer-no-alias"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{committed}");
    let row = conn
        .query(
            "SELECT kind,account_id,transfer_from_account_id,transfer_to_account_id FROM ledger_transactions WHERE external_id='SELF-NO-ALIAS'",
            (),
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get::<String>(0).unwrap(), "expense");
    assert_eq!(row.get::<String>(1).unwrap(), account_id);
    assert!(row.get::<Option<String>>(2).unwrap().is_none());
    assert!(row.get::<Option<String>>(3).unwrap().is_none());
}

#[tokio::test]
async fn neutral_rows_are_classified_by_amount_and_only_positive_rows_commit_as_transfers() {
    let test = TestApp::new().await;
    let owner = test
        .register_and_login("neutral-amount-disposition@example.com")
        .await;
    let fixture = "支付宝账单（纯虚构测试）\r\n\
交易时间,交易分类,交易对方,对方账号,商品说明,收/支,金额,收/付款方式,交易状态,交易订单号,商家订单号,备注\r\n\
2026-08-01 10:00:00,虚构分类,虚构对象甲,/,虚构零元行,不计收支,0.00,,交易成功,FAKE-NEUTRAL-ZERO,FAKE-MERCHANT-ZERO,虚构备注\r\n\
2026-08-01 11:00:00,虚构分类,虚构对象乙,/,虚构转账行,不计收支,12.34,虚构中性账户,交易成功,FAKE-NEUTRAL-POSITIVE,FAKE-MERCHANT-POSITIVE,虚构备注\r\n\
2026-08-01 12:00:00,虚构分类,虚构对象丙,/,虚构无账户线索行,不计收支,56.78,   ,交易成功,FAKE-NEUTRAL-EMPTY-METHOD,FAKE-MERCHANT-EMPTY-METHOD,虚构备注";
    let (fixture, _, had_errors) = GB18030.encode(fixture);
    assert!(!had_errors);

    let (status, preview) = send_multipart(
        &test.router,
        "/api/v1/imports",
        &owner,
        "neutral-amount-upload",
        &[
            ("file", Some("neutral-amount.csv"), fixture.as_ref()),
            ("channel", None, b"alipay"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{preview}");
    let batch_id = preview["id"].as_str().unwrap();

    let (status, _, detail) = send(
        &test.router,
        Method::GET,
        &format!("/api/v1/imports/{batch_id}"),
        None,
        Some(&owner),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    let records = detail["records"].as_array().unwrap();
    let zero = records
        .iter()
        .find(|record| record["amountCents"] == 0)
        .unwrap();
    let positive = records
        .iter()
        .find(|record| record["amountCents"] == 1_234)
        .unwrap();
    let empty_method = records
        .iter()
        .find(|record| record["amountCents"] == 5_678)
        .unwrap();
    assert_eq!(zero["direction"], "neutral");
    assert_eq!(zero["disposition"], "zero_amount");
    assert_eq!(positive["direction"], "neutral");
    assert_eq!(positive["disposition"], "import");
    assert_eq!(empty_method["direction"], "neutral");
    assert_eq!(empty_method["payMethod"], "");
    assert_eq!(empty_method["disposition"], "neutral");

    let fallback_account = test
        .create_ledger_account(&owner, "虚构中性批次账户", "other")
        .await;
    let (status, _, committed) = send(
        &test.router,
        Method::POST,
        &format!("/api/v1/imports/{batch_id}/commit"),
        Some(json!({ "accountId": fallback_account["id"] })),
        Some(&owner),
        Some("neutral-amount-commit"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{committed}");
    assert_eq!(committed["importedCount"], 1);

    let conn = test.state.connection().await.unwrap();
    let mut rows = conn
        .query(
            "SELECT disposition,transaction_id FROM import_records WHERE batch_id=?1 ORDER BY amount_cents",
            libsql::params![batch_id],
        )
        .await
        .unwrap();
    let zero = rows.next().await.unwrap().unwrap();
    assert_eq!(zero.get::<String>(0).unwrap(), "zero_amount");
    assert!(zero.get::<Option<String>>(1).unwrap().is_none());
    let positive = rows.next().await.unwrap().unwrap();
    assert_eq!(positive.get::<String>(0).unwrap(), "import");
    assert!(positive.get::<Option<String>>(1).unwrap().is_some());
    let empty_method = rows.next().await.unwrap().unwrap();
    assert_eq!(empty_method.get::<String>(0).unwrap(), "neutral");
    assert!(empty_method.get::<Option<String>>(1).unwrap().is_none());
    drop(rows);
    let transfer_count: i64 = conn
        .query(
            "SELECT count(*) FROM ledger_transactions WHERE import_batch_id=?1 AND kind='transfer' AND transfer_from_account_id=?2",
            libsql::params![batch_id, fallback_account["id"].as_str().unwrap()],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(transfer_count, 1);
}

async fn seed_neutral_transfer_batch(
    conn: &libsql::Connection,
    user_id: &str,
    batch_id: &str,
    records: &[Value],
) {
    let now = "2026-08-13T00:00:00Z";
    conn.execute(
        "INSERT INTO import_batches(id,user_id,source_channel,parser_version,file_name,file_sha256,period_start,period_end,total_count,status,created_at,updated_at) VALUES (?1,?2,'wechat',1,'synthetic.json',?3,'2026-08-13','2026-08-13',?4,'preview',?5,?5)",
        libsql::params![batch_id, user_id, "4".repeat(64), records.len() as i64, now],
    )
    .await
    .unwrap();
    for record in records {
        let raw_json = json!({ "交易类型": record["category"] }).to_string();
        conn.execute(
            "INSERT INTO import_records(id,batch_id,row_index,external_id,occurred_at,occurred_on,direction,amount_cents,channel_category,counterparty,product,pay_method,channel_status,source_note,occurred_at_precision,raw_json,disposition,created_at) VALUES (?1,?2,?3,?4,'2026-08-13 12:00:00','2026-08-13','neutral',?5,?6,?7,?8,?9,'支付成功','虚构测试','second',?10,'import',?11)",
            libsql::params![
                format!("{batch_id}-{}", record["case"].as_str().unwrap()),
                batch_id,
                record["rowIndex"].as_i64().unwrap(),
                format!("{batch_id}-{}", record["externalId"].as_str().unwrap()),
                record["amountCents"].as_i64().unwrap(),
                record["category"].as_str().unwrap(),
                record["counterparty"].as_str().unwrap(),
                record["product"].as_str().unwrap(),
                record["payMethod"].as_str().unwrap(),
                raw_json,
                now,
            ],
        )
        .await
        .unwrap();
    }
}

async fn map_synthetic_import_account(
    router: &Router,
    owner: &str,
    pay_method: &str,
    account_id: &str,
    key: &str,
) {
    let (status, _, mapping) = send(
        router,
        Method::POST,
        "/api/v1/imports/mappings",
        Some(json!({
            "sourceChannel": "wechat",
            "payMethod": pay_method,
            "accountId": account_id
        })),
        Some(owner),
        Some(key),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{mapping}");
}

#[tokio::test]
async fn withdraw_fee_candidate_matches_the_bank_account_on_the_transfer_to_leg() {
    let test = TestApp::new().await;
    let email = "withdraw-fee-transfer-leg@example.com";
    let owner = test.register_and_login(email).await;
    let balance = test
        .create_ledger_account(&owner, "虚构渠道余额账户", "wechat_balance")
        .await;
    let (bank_status, _, bank) = send(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({ "name": "虚构银行卡", "accountType": "bank_card" })),
        Some(&owner),
        Some("withdraw-fee-create-bank-account"),
        true,
    )
    .await;
    assert_eq!(bank_status, StatusCode::CREATED, "{bank}");
    let balance_id = balance["id"].as_str().unwrap();
    let bank_id = bank["id"].as_str().unwrap();
    let conn = test.state.connection().await.unwrap();
    let user_id: String = conn
        .query(
            "SELECT id FROM users WHERE email=?1",
            libsql::params![email],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    let now = "2026-08-13T00:00:00Z";
    conn.execute(
        "INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,occurred_on,occurred_at,occurred_at_precision,account_id,source_channel,external_id,created_at,updated_at) VALUES ('withdraw-fee-bank-leg',?1,'income',10000,'2026-08-13','2026-08-13 12:01:00','second',?2,'cmb','synthetic-withdraw-fee-bank-leg',?3,?3)",
        libsql::params![user_id.clone(), bank_id, now],
    )
    .await
    .unwrap();

    map_synthetic_import_account(
        &test.router,
        &owner,
        "虚构银行卡",
        bank_id,
        "withdraw-fee-bank-mapping",
    )
    .await;
    map_synthetic_import_account(
        &test.router,
        &owner,
        "零钱",
        balance_id,
        "withdraw-fee-balance-mapping",
    )
    .await;
    seed_neutral_transfer_batch(
        &conn,
        &user_id,
        "withdraw-fee-transfer-leg",
        &[json!({
            "case": "withdrawal",
            "rowIndex": 1,
            "externalId": "FAKE-WITHDRAW-FEE-TRANSFER-LEG",
            "category": "零钱提现",
            "counterparty": "招商银行(虚构尾号)",
            "product": "虚构提现",
            "payMethod": "虚构银行卡",
            "amountCents": 10_010
        })],
    )
    .await;

    let (status, _, committed) = send(
        &test.router,
        Method::POST,
        "/api/v1/imports/withdraw-fee-transfer-leg/commit",
        Some(json!({ "accountId": null })),
        Some(&owner),
        Some("withdraw-fee-transfer-leg-commit"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{committed}");

    let transfer = conn
        .query(
            "SELECT kind,amount_cents,transfer_from_account_id,transfer_to_account_id FROM ledger_transactions WHERE import_batch_id='withdraw-fee-transfer-leg'",
            (),
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(transfer.get::<String>(0).unwrap(), "transfer");
    assert_eq!(transfer.get::<i64>(1).unwrap(), 10_010);
    assert_eq!(transfer.get::<String>(2).unwrap(), balance_id);
    assert_eq!(transfer.get::<String>(3).unwrap(), bank_id);
    drop(transfer);

    let match_rule: String = conn
        .query(
            "SELECT match_rule FROM duplicate_suspicions WHERE user_id=?1",
            libsql::params![user_id],
        )
        .await
        .unwrap()
        .next()
        .await
        .expect("bank account on transfer_to_account_id must enter the candidate set")
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(match_rule, "withdraw_fee");
}

#[tokio::test]
async fn neutral_transfer_commit_uses_channel_category_balance_mapping_and_rejects_unsafe_rows() {
    let test = TestApp::new().await;
    let email = "neutral-transfer-directions@example.com";
    let owner = test.register_and_login(email).await;
    let fixture: Value = serde_json::from_str(include_str!(
        "fixtures/neutral_transfer_directions_synthetic.json"
    ))
    .unwrap();
    let records = fixture["records"].as_array().unwrap();
    let conn = test.state.connection().await.unwrap();
    let user_id: String = conn
        .query(
            "SELECT id FROM users WHERE email=?1",
            libsql::params![email],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();

    let bank = test
        .create_ledger_account(&owner, "虚构目标银行卡", "bank_card")
        .await;
    let (status, _, balance) = send(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({ "name": "虚构微信零钱", "accountType": "cash" })),
        Some(&owner),
        Some("neutral-transfer-balance-account"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{balance}");
    let bank_id = bank["id"].as_str().unwrap();
    let balance_id = balance["id"].as_str().unwrap();

    map_synthetic_import_account(
        &test.router,
        &owner,
        "虚构银行卡",
        bank_id,
        "neutral-transfer-bank-mapping",
    )
    .await;

    seed_neutral_transfer_batch(
        &conn,
        &user_id,
        "neutral-transfer-missing-balance",
        std::slice::from_ref(&records[0]),
    )
    .await;
    let (status, _, missing) = send(
        &test.router,
        Method::POST,
        "/api/v1/imports/neutral-transfer-missing-balance/commit",
        Some(json!({ "accountId": null })),
        Some(&owner),
        Some("neutral-transfer-missing-balance-commit"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{missing}");
    assert_eq!(missing["importedCount"], 0);
    assert!(
        missing["diagnostics"][0]
            .as_str()
            .unwrap()
            .contains("wechat")
    );
    assert!(missing["diagnostics"][0].as_str().unwrap().contains("零钱"));
    let missing_row = conn
        .query(
            "SELECT disposition,transaction_id FROM import_records WHERE batch_id='neutral-transfer-missing-balance'",
            (),
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(missing_row.get::<String>(0).unwrap(), "neutral");
    assert!(missing_row.get::<Option<String>>(1).unwrap().is_none());
    drop(missing_row);

    map_synthetic_import_account(
        &test.router,
        &owner,
        "零钱",
        balance_id,
        "neutral-transfer-balance-mapping",
    )
    .await;
    seed_neutral_transfer_batch(&conn, &user_id, "neutral-transfer-directions", records).await;
    let (status, _, committed) = send(
        &test.router,
        Method::POST,
        "/api/v1/imports/neutral-transfer-directions/commit",
        Some(json!({ "accountId": null })),
        Some(&owner),
        Some("neutral-transfer-directions-commit"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{committed}");
    assert_eq!(committed["importedCount"], 3);
    assert_eq!(committed["diagnostics"], json!([]));

    let mut transfers = conn
        .query(
            "SELECT r.row_index,t.transfer_from_account_id,t.transfer_to_account_id FROM import_records r JOIN ledger_transactions t ON t.id=r.transaction_id WHERE r.batch_id='neutral-transfer-directions' ORDER BY r.row_index",
            (),
        )
        .await
        .unwrap();
    let withdrawal = transfers.next().await.unwrap().unwrap();
    assert_eq!(withdrawal.get::<String>(1).unwrap(), balance_id);
    assert_eq!(withdrawal.get::<String>(2).unwrap(), bank_id);
    let recharge = transfers.next().await.unwrap().unwrap();
    assert_eq!(recharge.get::<String>(1).unwrap(), bank_id);
    assert_eq!(recharge.get::<String>(2).unwrap(), balance_id);
    let other = transfers.next().await.unwrap().unwrap();
    assert_eq!(other.get::<String>(1).unwrap(), bank_id);
    assert!(other.get::<Option<String>>(2).unwrap().is_none());
    assert!(transfers.next().await.unwrap().is_none());
    drop(transfers);

    map_synthetic_import_account(
        &test.router,
        &owner,
        "虚构银行卡",
        balance_id,
        "neutral-transfer-self-mapping",
    )
    .await;
    seed_neutral_transfer_batch(
        &conn,
        &user_id,
        "neutral-transfer-self",
        std::slice::from_ref(&records[0]),
    )
    .await;
    let (status, _, rejected) = send(
        &test.router,
        Method::POST,
        "/api/v1/imports/neutral-transfer-self/commit",
        Some(json!({ "accountId": null })),
        Some(&owner),
        Some("neutral-transfer-self-commit"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rejected}");
    assert_eq!(rejected["importedCount"], 0);
    assert!(
        rejected["diagnostics"][0]
            .as_str()
            .unwrap()
            .contains("避免自转")
    );
    let self_transfer_count: i64 = conn
        .query(
            "SELECT count(*) FROM ledger_transactions WHERE import_batch_id='neutral-transfer-self'",
            (),
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(self_transfer_count, 0);
    let self_disposition: String = conn
        .query(
            "SELECT disposition FROM import_records WHERE batch_id='neutral-transfer-self'",
            (),
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(self_disposition, "neutral");
}

#[tokio::test]
async fn import_upload_persists_credential_layer_fields() {
    let (test, _, preview) = setup_alipay_import(
        "import-credentials@example.com",
        "import-credentials-upload",
    )
    .await;
    let batch_id = preview["id"].as_str().unwrap();
    let conn = test.state.connection().await.unwrap();
    let mut rows = conn
        .query(
            "SELECT counterparty_account_raw,occurred_at_precision,currency,external_id_source,raw_json FROM import_records WHERE batch_id=?1 ORDER BY row_index LIMIT 3",
            libsql::params![batch_id],
        )
        .await
        .unwrap();
    let expected = [
        ("虚构餐饮", "交易成功", "FAKE-MERCHANT-0001"),
        ("虚构退款", "支付成功", "FAKE-MERCHANT-0002"),
        ("虚构内部", "退款成功", ""),
    ];

    for (category, status, merchant_order_id) in expected {
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), "");
        assert_eq!(row.get::<String>(1).unwrap(), "second");
        assert_eq!(row.get::<String>(2).unwrap(), "CNY");
        assert_eq!(row.get::<String>(3).unwrap(), "native");
        let raw_json: String = row.get(4).unwrap();
        let raw: Value = serde_json::from_str(&raw_json).unwrap();
        let mut expected_raw = serde_json::Map::from_iter([
            ("交易分类".to_owned(), Value::String(category.to_owned())),
            ("交易状态".to_owned(), Value::String(status.to_owned())),
        ]);
        if !merchant_order_id.is_empty() {
            expected_raw.insert(
                "商家订单号".to_owned(),
                Value::String(merchant_order_id.to_owned()),
            );
        }
        assert_eq!(raw, Value::Object(expected_raw));
    }
    assert!(rows.next().await.unwrap().is_none());
}

#[tokio::test]
async fn import_commit_uses_normalized_row_mappings_and_structured_fields() {
    let (test, owner, preview) =
        setup_alipay_import("import-row-mapping@example.com", "row-mapping-upload").await;
    let batch_id = preview["id"].as_str().unwrap();
    let mapped = test
        .create_ledger_account(&owner, "逐行映射账户", "bank_card")
        .await;
    let mapped_id = mapped["id"].as_str().unwrap();
    let (status, _, fallback) = send(
        &test.router,
        Method::POST,
        "/api/v1/ledger-accounts",
        Some(json!({ "name": "批次兜底账户", "accountType": "cash" })),
        Some(&owner),
        Some("row-mapping-fallback-account"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{fallback}");
    let fallback_id = fallback["id"].as_str().unwrap();

    for (index, pay_method) in ["虚构余额", "虚构中性账户"].into_iter().enumerate() {
        let key = format!("row-mapping-{index}");
        let (status, _, mapping) = send(
            &test.router,
            Method::POST,
            "/api/v1/imports/mappings",
            Some(json!({
                "sourceChannel": "alipay",
                "payMethod": pay_method,
                "accountId": mapped_id
            })),
            Some(&owner),
            Some(&key),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{mapping}");
        assert_eq!(mapping["payMethod"], pay_method);
    }

    let (status, _, detail) = send(
        &test.router,
        Method::GET,
        &format!("/api/v1/imports/{batch_id}"),
        None,
        Some(&owner),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    let methods = detail["payMethods"].as_array().unwrap();
    let balance_method = methods
        .iter()
        .find(|item| item["payMethod"] == "虚构余额")
        .unwrap();
    assert!(balance_method["count"].as_i64().unwrap() >= 2);
    assert_eq!(balance_method["accountId"], mapped_id);
    assert!(
        methods
            .iter()
            .all(|item| item["payMethod"] != "虚构余额&优惠")
    );

    let conn = test.state.connection().await.unwrap();
    let user_id: String = conn
        .query(
            "SELECT id FROM users WHERE email='import-row-mapping@example.com'",
            (),
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO categories(id,user_id,name,normalized_name,kind,created_at,updated_at) VALUES ('commit-rule-category',?1,'凭证命中','凭证命中','expense',?2,?2)",
        libsql::params![user_id.clone(), now.clone()],
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO category_rules(id,user_id,priority,source_channel,category_id,created_at,updated_at) VALUES ('commit-rule',?1,1,'alipay','commit-rule-category',?2,?2)",
        libsql::params![user_id, now.clone()],
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO category_rule_conditions(id,rule_id,match_field,match_kind,match_value,created_at) VALUES ('commit-rule-condition','commit-rule','merchant_order_id','exact','FAKE-MERCHANT-0001',?1)",
        [now],
    )
    .await
    .unwrap();
    drop(conn);

    let (status, _, committed) = send(
        &test.router,
        Method::POST,
        &format!("/api/v1/imports/{batch_id}/commit"),
        Some(json!({ "accountId": null })),
        Some(&owner),
        Some("row-mapping-commit"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{committed}");

    let (status, _, transactions) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions?pageSize=200",
        None,
        Some(&owner),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{transactions}");
    let imported = transactions["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["payeeName"] == "虚构商户甲")
        .unwrap();
    assert_eq!(imported["payeeName"], "虚构商户甲");
    assert_eq!(imported["description"], "虚构早餐,加饮品");
    assert_eq!(imported["occurredAt"], "2026-01-01 08:00:00");
    assert_eq!(imported["occurredAtPrecision"], "second");
    assert_eq!(imported["currency"], "CNY");

    let conn = test.state.connection().await.unwrap();
    let row = conn.query(
        "SELECT t.account_id,t.category,t.category_source,t.payee_name,t.description,t.note,t.occurred_at,t.occurred_at_precision,t.currency,t.payee_key,r.counterparty_normalized,r.normalization_version,t.category_rule_id FROM ledger_transactions t JOIN import_records r ON r.transaction_id=t.id WHERE r.batch_id=?1 AND r.external_id='FAKE-ALI-0001'",
        libsql::params![batch_id],
    ).await.unwrap().next().await.unwrap().unwrap();
    assert_eq!(row.get::<String>(0).unwrap(), mapped_id);
    assert_eq!(row.get::<String>(1).unwrap(), "");
    assert_eq!(row.get::<String>(2).unwrap(), "rule");
    assert_eq!(row.get::<String>(3).unwrap(), "虚构商户甲");
    assert_eq!(row.get::<String>(4).unwrap(), "虚构早餐,加饮品");
    assert_eq!(row.get::<String>(5).unwrap(), "虚构备注甲");
    assert!(!row.get::<String>(5).unwrap().contains('·'));
    assert_eq!(row.get::<String>(6).unwrap(), "2026-01-01 08:00:00");
    assert_eq!(row.get::<String>(7).unwrap(), "second");
    assert_eq!(row.get::<String>(8).unwrap(), "CNY");
    assert_eq!(row.get::<String>(9).unwrap(), "虚构商户甲");
    assert_eq!(row.get::<String>(10).unwrap(), "虚构商户甲");
    assert_eq!(row.get::<i64>(11).unwrap(), 2);
    assert_eq!(row.get::<String>(12).unwrap(), "commit-rule");
    drop(row);
    let category_id: String = conn
        .query(
            "SELECT t.category_id FROM ledger_transactions t JOIN import_records r ON r.transaction_id=t.id WHERE r.batch_id=?1 AND r.external_id='FAKE-ALI-0001'",
            libsql::params![batch_id],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(category_id, "commit-rule-category");

    let unmatched: Option<String> = conn.query(
        "SELECT t.account_id FROM ledger_transactions t JOIN import_records r ON r.transaction_id=t.id WHERE r.batch_id=?1 AND r.external_id='FAKE-ALI-0002'",
        libsql::params![batch_id],
    ).await.unwrap().next().await.unwrap().unwrap().get(0).unwrap();
    assert!(unmatched.is_none());
    let transfer = conn.query(
        "SELECT t.kind,t.account_id,t.transfer_from_account_id,t.transfer_to_account_id FROM ledger_transactions t JOIN import_records r ON r.transaction_id=t.id WHERE r.batch_id=?1 AND r.external_id='FAKE-ALI-0003'",
        libsql::params![batch_id],
    ).await.unwrap().next().await.unwrap().unwrap();
    assert_eq!(transfer.get::<String>(0).unwrap(), "transfer");
    assert!(transfer.get::<Option<String>>(1).unwrap().is_none());
    assert_eq!(transfer.get::<String>(2).unwrap(), mapped_id);
    assert!(transfer.get::<Option<String>>(3).unwrap().is_none());
    drop(transfer);
    drop(conn);

    let (status, _, bound) = send(
        &test.router,
        Method::POST,
        &format!("/api/v1/imports/{batch_id}/account"),
        Some(json!({ "accountId": fallback_id })),
        Some(&owner),
        None,
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{bound}");
    assert!(bound["updatedCount"].as_i64().unwrap() > 0);
    let conn = test.state.connection().await.unwrap();
    let unmatched_after: String = conn.query(
        "SELECT t.account_id FROM ledger_transactions t JOIN import_records r ON r.transaction_id=t.id WHERE r.batch_id=?1 AND r.external_id='FAKE-ALI-0002'",
        libsql::params![batch_id],
    ).await.unwrap().next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(unmatched_after, fallback_id);
    let transfer_account: Option<String> = conn.query(
        "SELECT t.account_id FROM ledger_transactions t JOIN import_records r ON r.transaction_id=t.id WHERE r.batch_id=?1 AND r.external_id='FAKE-ALI-0003'",
        libsql::params![batch_id],
    ).await.unwrap().next().await.unwrap().unwrap().get(0).unwrap();
    assert!(transfer_account.is_none());
}

#[tokio::test]
async fn import_commit_rejects_unmapped_neutral_row_with_diagnostics() {
    let (test, owner, preview) = setup_alipay_import(
        "import-neutral-missing@example.com",
        "neutral-missing-upload",
    )
    .await;
    let batch_id = preview["id"].as_str().unwrap();
    let (status, _, body) = send(
        &test.router,
        Method::POST,
        &format!("/api/v1/imports/{batch_id}/commit"),
        Some(json!({ "accountId": null })),
        Some(&owner),
        Some("neutral-missing-commit"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "import_account_mapping_required");
    assert_eq!(body["fieldErrors"]["payMethod"], "虚构中性账户");
    assert!(body["fieldErrors"]["rowIndex"].as_i64().unwrap() > 0);
    let conn = test.state.connection().await.unwrap();
    let ledger_count: i64 = conn
        .query(
            "SELECT count(*) FROM ledger_transactions WHERE import_batch_id=?1",
            libsql::params![batch_id],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(
        ledger_count, 0,
        "the failed commit must roll back earlier rows"
    );
}

async fn seed_first_import_transaction(test: &TestApp, batch_id: &str) {
    let conn = test.state.connection().await.unwrap();
    let mut rows = conn.query("SELECT b.user_id,r.id,r.external_id,r.direction,r.amount_cents,r.occurred_on FROM import_records r JOIN import_batches b ON b.id=r.batch_id WHERE r.batch_id=?1 AND r.disposition='import' ORDER BY r.row_index LIMIT 1", libsql::params![batch_id]).await.unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let values = (
        row.get::<String>(0).unwrap(),
        row.get::<String>(1).unwrap(),
        row.get::<String>(2).unwrap(),
        row.get::<String>(3).unwrap(),
        row.get::<i64>(4).unwrap(),
        row.get::<String>(5).unwrap(),
    );
    drop(row);
    drop(rows);
    let transaction_id = Uuid::now_v7().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute("INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,occurred_on,category,note,archived_at,version,created_at,updated_at,source_channel,external_id,import_batch_id) VALUES (?1,?2,?3,?4,?5,'','',?6,1,?6,?6,'alipay',?7,?8)", libsql::params![transaction_id.clone(), values.0, values.3, values.4, values.5, now, values.2, batch_id]).await.unwrap();
    conn.execute(
        "UPDATE import_records SET transaction_id=?1 WHERE id=?2",
        libsql::params![transaction_id, values.1],
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn import_upload_idempotency_replays_and_rejects_mismatch() {
    let (test, owner, first_body) =
        setup_alipay_import("import-replay@example.com", "import-upload-key-0001").await;
    let fixture = include_bytes!("fixtures/alipay_synthetic_gb18030.csv");
    let (status, replay) = send_multipart(
        &test.router,
        "/api/v1/imports",
        &owner,
        "import-upload-key-0001",
        &[
            ("file", Some("../private\\bill.csv"), fixture),
            ("channel", None, b"alipay"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(replay, first_body);
    let (status, mismatch) = send_multipart(
        &test.router,
        "/api/v1/imports",
        &owner,
        "import-upload-key-0001",
        &[
            ("file", Some("different.csv"), fixture),
            ("channel", None, b"alipay"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(mismatch["code"], "idempotency_mismatch");
}

#[tokio::test]
async fn import_upload_is_read_only_for_ledger() {
    let (test, _, _) =
        setup_alipay_import("import-ledger@example.com", "import-upload-ledger").await;
    let count: i64 = test
        .state
        .connection()
        .await
        .unwrap()
        .query("SELECT count(*) FROM ledger_transactions", ())
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn import_upload_preview_marks_existing_transaction_duplicate() {
    let (test, owner, body) = setup_alipay_import(
        "import-duplicate@example.com",
        "import-upload-duplicate-base",
    )
    .await;
    seed_first_import_transaction(&test, body["id"].as_str().unwrap()).await;
    let fixture = include_bytes!("fixtures/alipay_synthetic_gb18030.csv");
    let (status, preview) = send_multipart(
        &test.router,
        "/api/v1/imports",
        &owner,
        "import-upload-key-duplicate",
        &[
            ("file", Some("duplicate.csv"), fixture),
            ("channel", None, b"alipay"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{preview}");
    assert_eq!(preview["summary"]["duplicate"]["count"], 1);
}

#[tokio::test]
async fn import_upload_rejects_existing_external_id_payload_mismatch() {
    let (test, owner, body) =
        setup_alipay_import("import-payload@example.com", "import-upload-payload-base").await;
    seed_first_import_transaction(&test, body["id"].as_str().unwrap()).await;
    let fixture = include_bytes!("fixtures/alipay_synthetic_gb18030.csv");
    let (decoded, _, _) = GB18030.decode(fixture);
    let mismatch_text = decoded.replacen("12.34", "12.35", 1);
    let (mismatch_fixture, _, _) = GB18030.encode(&mismatch_text);
    let (status, mismatch) = send_multipart(
        &test.router,
        "/api/v1/imports",
        &owner,
        "import-upload-key-mismatch",
        &[
            ("file", Some("mismatch.csv"), mismatch_fixture.as_ref()),
            ("channel", None, b"alipay"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(mismatch["code"], "external_id_payload_mismatch");
}

#[tokio::test]
async fn import_detail_is_paginated_and_owned() {
    let (test, owner, body) =
        setup_alipay_import("import-owner@example.com", "import-upload-owned").await;
    let other = test.register_and_login("import-other@example.com").await;
    let batch_id = body["id"].as_str().unwrap();
    let (status, _, detail) = send(
        &test.router,
        Method::GET,
        &format!("/api/v1/imports/{batch_id}?pageSize=1"),
        None,
        Some(&owner),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["records"].as_array().unwrap().len(), 1);
    assert_eq!(detail["summary"], body["summary"]);
    let (status, _, hidden) = send(
        &test.router,
        Method::GET,
        &format!("/api/v1/imports/{batch_id}"),
        None,
        Some(&other),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(hidden["code"], "not_found");
}

#[tokio::test]
async fn import_upload_rejects_invalid_multipart() {
    let test = TestApp::new().await;
    let owner = test
        .register_and_login("import-multipart@example.com")
        .await;
    let fixture = include_bytes!("fixtures/alipay_synthetic_gb18030.csv");
    let (status, body) = send_multipart(
        &test.router,
        "/api/v1/imports",
        &owner,
        "import-upload-key-0002",
        &[
            ("file", Some("bill.csv"), fixture),
            ("file", Some("again.csv"), fixture),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "invalid_multipart");
}

#[tokio::test]
async fn import_upload_rejects_payload_over_10_mib() {
    let test = TestApp::new().await;
    let owner = test.register_and_login("import-large@example.com").await;
    let oversized = vec![b'x'; 10 * 1024 * 1024 + 1];
    let (status, body) = send_multipart(
        &test.router,
        "/api/v1/imports",
        &owner,
        "import-upload-key-0003",
        &[("file", Some("large.csv"), &oversized)],
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body["code"], "payload_too_large");
}

#[tokio::test]
async fn import_blocked_batch_cannot_be_committed() {
    let test = TestApp::new().await;
    let owner = test.register_and_login("import-blocked@example.com").await;
    let fixture = include_bytes!("fixtures/alipay_synthetic_gb18030.csv");
    let (decoded, _, _) = GB18030.decode(fixture);
    let unknown_text = decoded.replacen("支付成功", "虚构未知状态", 1);
    let (unknown_fixture, _, _) = GB18030.encode(&unknown_text);
    let (status, blocked) = send_multipart(
        &test.router,
        "/api/v1/imports",
        &owner,
        "import-upload-key-0004",
        &[
            ("file", Some("unknown.csv"), unknown_fixture.as_ref()),
            ("channel", None, b"alipay"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{blocked}");
    assert_eq!(blocked["status"], "blocked");
    assert_eq!(blocked["issues"][0]["status"], "虚构未知状态");
    let (status, body) = send_import_commit(
        &test.router,
        blocked["id"].as_str().unwrap(),
        &owner,
        "blocked-commit-key",
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "import_batch_state_conflict");
}

#[tokio::test]
async fn import_commit_is_atomic_idempotent_and_resolves_post_preview_duplicates() {
    let test = TestApp::new().await;
    let owner = test.register_and_login("import-commit@example.com").await;
    let fixture = include_bytes!("fixtures/alipay_synthetic_gb18030.csv");

    let (status, first) = send_multipart(
        &test.router,
        "/api/v1/imports",
        &owner,
        "commit-upload-first",
        &[
            ("file", Some("first.csv"), fixture),
            ("channel", None, b"alipay"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    let first_id = first["id"].as_str().unwrap();
    let expected_imports = first["summary"]["importIncome"]["count"].as_i64().unwrap()
        + first["summary"]["importExpense"]["count"].as_i64().unwrap();
    let conn = test.state.connection().await.unwrap();
    let expected_transfers: i64 = conn
        .query(
            "SELECT count(*) FROM import_records WHERE batch_id=?1 AND disposition='import' AND direction='neutral'",
            libsql::params![first_id],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    drop(conn);
    let expected_ledger_rows = expected_imports + expected_transfers;
    assert!(expected_imports > 0);
    assert!(first["summary"]["zeroAmount"]["count"].as_i64().unwrap() > 0);

    // 第二批在第一批确认前创建，因此两批候选最初都还是 import。
    let (status, second) = send_multipart(
        &test.router,
        "/api/v1/imports",
        &owner,
        "commit-upload-second",
        &[
            ("file", Some("second.csv"), fixture),
            ("channel", None, b"alipay"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{second}");
    let second_id = second["id"].as_str().unwrap();
    assert_eq!(second["summary"]["duplicate"]["count"], 0);

    let (status, committed) =
        send_import_commit(&test.router, first_id, &owner, "commit-first-key").await;
    assert_eq!(status, StatusCode::OK, "{committed}");
    assert_eq!(committed["status"], "committed");
    assert_eq!(committed["importedCount"], expected_ledger_rows);
    assert_eq!(committed["duplicateCount"], 0);
    assert_eq!(
        committed["summary"]["zeroAmount"],
        first["summary"]["zeroAmount"]
    );

    let (status, replay) =
        send_import_commit(&test.router, first_id, &owner, "commit-first-key").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay, committed);
    let (status, conflict) =
        send_import_commit(&test.router, first_id, &owner, "commit-first-other-key").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["code"], "import_batch_state_conflict");

    let (status, all_duplicate) =
        send_import_commit(&test.router, second_id, &owner, "commit-second-key").await;
    assert_eq!(status, StatusCode::OK, "{all_duplicate}");
    assert_eq!(all_duplicate["status"], "committed");
    assert_eq!(all_duplicate["importedCount"], 0);
    assert_eq!(all_duplicate["duplicateCount"], expected_ledger_rows);
    assert_eq!(
        all_duplicate["summary"]["duplicate"]["count"],
        expected_ledger_rows
    );

    let conn = test.state.connection().await.unwrap();
    let ledger_count: i64 = conn
        .query("SELECT count(*) FROM ledger_transactions", ())
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(ledger_count, expected_ledger_rows);
    let account_count: i64 = conn
        .query(
            "SELECT count(*) FROM ledger_transactions WHERE account_id IS NOT NULL",
            (),
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(account_count, 0);
    let zero_ledger_count: i64 = conn
        .query(
            "SELECT count(*) FROM ledger_transactions WHERE amount_cents=0",
            (),
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(zero_ledger_count, 0);

    // NULL account_id 不进入账户余额 movement，但仍进入流水 summary。
    let movement_count: i64 = conn
        .query("SELECT count(*) FROM ledger_account_movements", ())
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(movement_count, expected_transfers);
    drop(conn);
    let (status, _, summary) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions/summary?month=2026-01",
        None,
        Some(&owner),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    assert_eq!(summary["transactionCount"], expected_imports);
}

#[tokio::test]
async fn import_commit_rolls_back_payload_mismatch_and_non_target_constraints() {
    let test = TestApp::new().await;
    let owner = test.register_and_login("import-rollback@example.com").await;
    let fixture = include_bytes!("fixtures/alipay_synthetic_gb18030.csv");
    let (status, preview) = send_multipart(
        &test.router,
        "/api/v1/imports",
        &owner,
        "rollback-upload-one",
        &[
            ("file", Some("rollback.csv"), fixture),
            ("channel", None, b"alipay"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{preview}");
    let batch_id = preview["id"].as_str().unwrap();
    let conn = test.state.connection().await.unwrap();
    let mut rows = conn.query("SELECT b.user_id,r.external_id,r.direction,r.amount_cents,r.occurred_on FROM import_records r JOIN import_batches b ON b.id=r.batch_id WHERE r.batch_id=?1 AND r.disposition='import' ORDER BY r.row_index LIMIT 1", libsql::params![batch_id]).await.unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let user_id: String = row.get(0).unwrap();
    let external_id: String = row.get(1).unwrap();
    let direction: String = row.get(2).unwrap();
    let amount: i64 = row.get(3).unwrap();
    let occurred_on: String = row.get(4).unwrap();
    drop(row);
    drop(rows);
    let provenance_batch = Uuid::now_v7().to_string();
    let provenance_record = Uuid::now_v7().to_string();
    let transaction_id = Uuid::now_v7().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute("INSERT INTO import_batches(id,user_id,source_channel,parser_version,file_name,file_sha256,period_start,period_end,total_count,status,committed_at,created_at,updated_at) VALUES (?1,?2,'alipay',1,'history.csv',?3,?4,?4,1,'committed',?5,?5,?5)", libsql::params![provenance_batch.clone(),user_id.clone(),"a".repeat(64),occurred_on.clone(),now.clone()]).await.unwrap();
    conn.execute("INSERT INTO import_records(id,batch_id,row_index,external_id,occurred_at,occurred_on,direction,amount_cents,disposition,created_at) VALUES (?1,?2,1,?3,?4,?4,?5,?6,'import',?7)", libsql::params![provenance_record.clone(),provenance_batch.clone(),external_id.clone(),occurred_on.clone(),direction.clone(),amount+1,now.clone()]).await.unwrap();
    conn.execute("INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,occurred_on,created_at,updated_at,source_channel,external_id,import_batch_id) VALUES (?1,?2,?3,?4,?5,?6,?6,'alipay',?7,?8)", libsql::params![transaction_id.clone(),user_id,direction,amount+1,occurred_on,now,external_id,provenance_batch]).await.unwrap();
    conn.execute(
        "UPDATE import_records SET transaction_id=?1 WHERE id=?2",
        libsql::params![transaction_id, provenance_record],
    )
    .await
    .unwrap();
    drop(conn);

    let (status, mismatch) =
        send_import_commit(&test.router, batch_id, &owner, "rollback-commit-one").await;
    assert_eq!(status, StatusCode::CONFLICT, "{mismatch}");
    assert_eq!(mismatch["code"], "external_id_payload_mismatch");
    let conn = test.state.connection().await.unwrap();
    let batch_status: String = conn
        .query(
            "SELECT status FROM import_batches WHERE id=?1",
            libsql::params![batch_id],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(batch_status, "preview");
    let linked: i64 = conn
        .query(
            "SELECT count(*) FROM import_records WHERE batch_id=?1 AND transaction_id IS NOT NULL",
            libsql::params![batch_id],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(linked, 0, "earlier candidate inserts must roll back");
    drop(conn);

    let (status, constrained) = send_multipart(
        &test.router,
        "/api/v1/imports",
        &owner,
        "rollback-upload-two",
        &[
            ("file", Some("constraint.csv"), fixture),
            ("channel", None, b"alipay"),
        ],
    )
    .await;
    // Existing mismatched external id also protects upload itself; use a fresh app for the DB constraint branch.
    assert_eq!(status, StatusCode::CONFLICT, "{constrained}");

    let test = TestApp::new().await;
    let owner = test
        .register_and_login("import-constraint@example.com")
        .await;
    let (status, preview) = send_multipart(
        &test.router,
        "/api/v1/imports",
        &owner,
        "constraint-upload-key",
        &[
            ("file", Some("constraint.csv"), fixture),
            ("channel", None, b"alipay"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{preview}");
    let batch_id = preview["id"].as_str().unwrap();
    let conn = test.state.connection().await.unwrap();
    conn.execute_batch("CREATE TRIGGER reject_import_ledger BEFORE INSERT ON ledger_transactions BEGIN SELECT RAISE(ABORT, 'synthetic non-target constraint'); END;").await.unwrap();
    drop(conn);
    let (status, failure) =
        send_import_commit(&test.router, batch_id, &owner, "constraint-commit-key").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{failure}");
    assert_eq!(failure["code"], "internal_error");
    let conn = test.state.connection().await.unwrap();
    let ledger_count: i64 = conn
        .query("SELECT count(*) FROM ledger_transactions", ())
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(ledger_count, 0);
    let batch_status: String = conn
        .query(
            "SELECT status FROM import_batches WHERE id=?1",
            libsql::params![batch_id],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(batch_status, "preview");
}

#[tokio::test]
async fn import_discard_abandons_uncommitted_batches_and_is_owned_idempotent() {
    let test = TestApp::new().await;
    let owner = test.register_and_login("discard-preview@example.com").await;
    let other = test
        .register_and_login("discard-preview-other@example.com")
        .await;
    let fixture = include_bytes!("fixtures/alipay_synthetic_gb18030.csv");
    let (status, preview) = send_multipart(
        &test.router,
        "/api/v1/imports",
        &owner,
        "discard-preview-upload",
        &[
            ("file", Some("preview.csv"), fixture),
            ("channel", None, b"alipay"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{preview}");
    let preview_id = preview["id"].as_str().unwrap();

    let (status, hidden) =
        send_import_discard(&test.router, preview_id, &other, "discard-hidden").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{hidden}");
    let (status, discarded) =
        send_import_discard(&test.router, preview_id, &owner, "discard-preview-key").await;
    assert_eq!(status, StatusCode::OK, "{discarded}");
    assert_eq!(discarded["deletedCount"], 0);
    assert_eq!(discarded["retainedModifiedCount"], 0);
    let (status, replay) =
        send_import_discard(&test.router, preview_id, &owner, "discard-preview-key").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay, discarded);
    let (status, conflict) =
        send_import_discard(&test.router, preview_id, &owner, "discard-preview-new").await;
    assert_eq!(status, StatusCode::CONFLICT, "{conflict}");
    assert_eq!(conflict["code"], "import_batch_state_conflict");

    let (decoded, _, _) = GB18030.decode(fixture);
    let unknown_text = decoded.replacen("支付成功", "虚构未知状态", 1);
    let (unknown_fixture, _, _) = GB18030.encode(&unknown_text);
    let (status, blocked) = send_multipart(
        &test.router,
        "/api/v1/imports",
        &owner,
        "discard-blocked-upload",
        &[
            ("file", Some("blocked.csv"), unknown_fixture.as_ref()),
            ("channel", None, b"alipay"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{blocked}");
    assert_eq!(blocked["status"], "blocked");
    let blocked_id = blocked["id"].as_str().unwrap();
    let (status, _) =
        send_import_discard(&test.router, blocked_id, &owner, "discard-blocked-key").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, detail) = send(
        &test.router,
        Method::GET,
        &format!("/api/v1/imports/{blocked_id}"),
        None,
        Some(&owner),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["status"], "discarded");
    assert!(
        detail["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["outcome"] == "abandoned")
    );
}

#[tokio::test]
async fn import_discard_removes_pristine_and_retains_modified_provenance() {
    let test = TestApp::new().await;
    let owner = test
        .register_and_login("discard-committed@example.com")
        .await;
    let fixture = include_bytes!("fixtures/alipay_synthetic_gb18030.csv");
    let (status, preview) = send_multipart(
        &test.router,
        "/api/v1/imports",
        &owner,
        "discard-committed-upload",
        &[
            ("file", Some("committed.csv"), fixture),
            ("channel", None, b"alipay"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{preview}");
    let batch_id = preview["id"].as_str().unwrap();
    let (status, committed) =
        send_import_commit(&test.router, batch_id, &owner, "discard-commit-key").await;
    assert_eq!(status, StatusCode::OK, "{committed}");

    let conn = test.state.connection().await.unwrap();
    let mut rows = conn.query("SELECT id,source_channel,external_id,import_batch_id FROM ledger_transactions WHERE import_batch_id=?1 ORDER BY id", libsql::params![batch_id]).await.unwrap();
    let mut transactions = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        transactions.push((
            row.get::<String>(0).unwrap(),
            row.get::<String>(1).unwrap(),
            row.get::<String>(2).unwrap(),
            row.get::<String>(3).unwrap(),
        ));
    }
    drop(rows);
    assert!(
        transactions.len() >= 3,
        "fixture must provide pristine, edited, and archived imports"
    );
    conn.execute(
        "UPDATE ledger_transactions SET version=2 WHERE id=?1",
        libsql::params![transactions[0].0.clone()],
    )
    .await
    .unwrap();
    conn.execute(
        "UPDATE ledger_transactions SET archived_at=?1 WHERE id=?2",
        libsql::params![chrono::Utc::now().to_rfc3339(), transactions[1].0.clone()],
    )
    .await
    .unwrap();
    drop(conn);

    let (status, discarded) =
        send_import_discard(&test.router, batch_id, &owner, "discard-committed-key").await;
    assert_eq!(status, StatusCode::OK, "{discarded}");
    assert_eq!(discarded["retainedModifiedCount"], 2);
    assert_eq!(
        discarded["deletedCount"].as_i64().unwrap(),
        transactions.len() as i64 - 2
    );

    let conn = test.state.connection().await.unwrap();
    for (id, source_channel, external_id, import_batch_id) in &transactions[..2] {
        let row = conn.query("SELECT source_channel,external_id,import_batch_id FROM ledger_transactions WHERE id=?1", libsql::params![id.clone()]).await.unwrap().next().await.unwrap().unwrap();
        assert_eq!(row.get::<String>(0).unwrap(), *source_channel);
        assert_eq!(row.get::<String>(1).unwrap(), *external_id);
        assert_eq!(row.get::<String>(2).unwrap(), *import_batch_id);
    }
    let cleared: i64 = conn.query("SELECT count(*) FROM import_records WHERE batch_id=?1 AND disposition='import' AND transaction_id IS NULL", libsql::params![batch_id]).await.unwrap().next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(cleared, transactions.len() as i64 - 2);
    drop(conn);
    let (status, _, detail) = send(
        &test.router,
        Method::GET,
        &format!("/api/v1/imports/{batch_id}"),
        None,
        Some(&owner),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["status"], "discarded");
    assert!(detail["committedAt"].is_string());
    let outcomes = detail["records"].as_array().unwrap();
    assert!(outcomes.iter().any(|r| r["outcome"] == "removed"));
    assert_eq!(
        outcomes
            .iter()
            .filter(|r| r["outcome"] == "retained_modified")
            .count(),
        2
    );
}

#[tokio::test]
async fn import_discard_wrong_provenance_rolls_back_entire_batch() {
    let test = TestApp::new().await;
    let owner = test.register_and_login("discard-chain@example.com").await;
    let fixture = include_bytes!("fixtures/alipay_synthetic_gb18030.csv");
    let (status, preview) = send_multipart(
        &test.router,
        "/api/v1/imports",
        &owner,
        "discard-chain-upload",
        &[
            ("file", Some("chain.csv"), fixture),
            ("channel", None, b"alipay"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{preview}");
    let batch_id = preview["id"].as_str().unwrap();
    let (status, committed) =
        send_import_commit(&test.router, batch_id, &owner, "discard-chain-commit").await;
    assert_eq!(status, StatusCode::OK, "{committed}");
    let conn = test.state.connection().await.unwrap();
    let transaction_id: String = conn
        .query(
            "SELECT id FROM ledger_transactions WHERE import_batch_id=?1 ORDER BY id LIMIT 1",
            libsql::params![batch_id],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    conn.execute(
        "UPDATE ledger_transactions SET import_batch_id=NULL WHERE id=?1",
        libsql::params![transaction_id],
    )
    .await
    .unwrap();
    let before: i64 = conn
        .query("SELECT count(*) FROM ledger_transactions", ())
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    drop(conn);
    let (status, failure) =
        send_import_discard(&test.router, batch_id, &owner, "discard-chain-key").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{failure}");
    let conn = test.state.connection().await.unwrap();
    let after: i64 = conn
        .query("SELECT count(*) FROM ledger_transactions", ())
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    let status_after: String = conn
        .query(
            "SELECT status FROM import_batches WHERE id=?1",
            libsql::params![batch_id],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(after, before);
    assert_eq!(status_after, "committed");
}

/// 静态资源的缓存策略必须显式声明。tower-http 默认什么都不设，客户端会退回启发式
/// 缓存（按 last-modified 自行推算过期时间），实测导致 WKWebView 长期持有旧
/// index.html，服务端换了新 bundle 也不被发现，每次部署都要手动清缓存。
/// 两类资源策略相反，这里把它们钉死。
#[tokio::test]
async fn static_assets_declare_cache_policy() {
    let test = TestApp::new().await;
    let dist = test.state.config.web_dist_dir.clone();
    std::fs::create_dir_all(dist.join("assets")).unwrap();
    std::fs::write(dist.join("index.html"), "<!doctype html>").unwrap();
    std::fs::write(dist.join("assets/index-abc123.js"), "console.log(1)").unwrap();

    // 文件名带内容 hash，内容变则文件名变，可以永久缓存。
    let response = test
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/assets/index-abc123.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("public, max-age=31536000, immutable"),
    );

    // index.html 是新 bundle 的唯一入口，必须每次回源验证。
    let response = test
        .router
        .clone()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-cache"),
    );
}

#[tokio::test]
async fn unmatched_api_paths_return_json_404_without_breaking_spa_fallback() {
    let test = TestApp::new().await;
    let dist = test.state.config.web_dist_dir.clone();
    std::fs::create_dir_all(&dist).unwrap();
    std::fs::write(dist.join("index.html"), "<!doctype html>").unwrap();

    for path in ["/api/v1/nonexistent", "/api/nonexistent"] {
        let response = test
            .router
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&header::HeaderValue::from_static("application/json")),
            "{path}",
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], "not_found", "{path}");
        assert!(body["requestId"].is_string(), "{path}");
    }

    let response = test
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/app/calendar")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE),
        Some(&header::HeaderValue::from_static("text/html")),
    );
}

#[tokio::test]
async fn category_rule_migration_creates_expected_schema() {
    let test = TestApp::new().await;
    let conn = test.state.connection().await.unwrap();

    for table in ["categories", "category_rules", "category_rule_conditions"] {
        let exists: i64 = conn
            .query(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
            )
            .await
            .unwrap()
            .next()
            .await
            .unwrap()
            .unwrap()
            .get(0)
            .unwrap();
        assert_eq!(exists, 1, "missing table {table}");
    }

    let unique_category_name_index: i64 = conn
        .query(
            "SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_categories_unique_name' AND sql LIKE 'CREATE UNIQUE INDEX%'",
            (),
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(unique_category_name_index, 1);

    let condition_rule_index: i64 = conn
        .query(
            "SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_category_rule_conditions_rule'",
            (),
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(condition_rule_index, 1);

    let conditions_sql: String = conn
        .query(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'category_rule_conditions'",
            (),
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert!(conditions_sql.contains("'merchant_order_id'"));

    let trace_column_count: i64 = conn
        .query(
            "SELECT count(*) FROM pragma_table_info('ledger_transactions') WHERE name='category_rule_id' AND type='TEXT'",
            (),
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(trace_column_count, 1);
}

#[tokio::test]
async fn category_and_rule_api_flow_is_validated_idempotent_and_preserves_manual_categories() {
    let test = TestApp::new().await;
    let cookie = test.register_and_login("category-api@zhiyu.local").await;

    let (status, _, food) = send(
        &test.router,
        Method::POST,
        "/api/v1/categories",
        Some(json!({ "name": "餐饮", "kind": "expense", "sortOrder": 20 })),
        Some(&cookie),
        Some("category-create-food"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{food}");
    let food_id = food["id"].as_str().unwrap();

    let (status, _, duplicate) = send(
        &test.router,
        Method::POST,
        "/api/v1/categories",
        Some(json!({ "name": " 餐饮 ", "kind": "expense" })),
        Some(&cookie),
        Some("category-create-duplicate"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{duplicate}");
    assert_eq!(duplicate["code"], "category_name_conflict");

    let (_, _, manual) = send(
        &test.router,
        Method::POST,
        "/api/v1/categories",
        Some(json!({ "name": "手动分类", "kind": "expense", "sortOrder": 10 })),
        Some(&cookie),
        Some("category-create-manual"),
        true,
    )
    .await;
    let manual_id = manual["id"].as_str().unwrap();

    let (status, _, child) = send(
        &test.router,
        Method::POST,
        "/api/v1/categories",
        Some(json!({ "parentId": food_id, "name": "咖啡", "kind": "expense", "sortOrder": 1 })),
        Some(&cookie),
        Some("category-create-child"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{child}");
    let (_, _, categories) = send(
        &test.router,
        Method::GET,
        "/api/v1/categories",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(categories[0]["id"], manual_id);
    assert_eq!(categories[1]["id"], food_id);
    assert_eq!(categories[1]["children"][0]["id"], child["id"]);

    let (status, _, invalid_rule) = send(
        &test.router,
        Method::POST,
        "/api/v1/category-rules",
        Some(json!({ "categoryId": food_id, "conditions": [
            { "matchField": "payee_name", "matchKind": "gte", "matchValue": "咖啡" }
        ] })),
        Some(&cookie),
        Some("category-rule-invalid"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{invalid_rule}");
    assert!(
        invalid_rule["message"]
            .as_str()
            .unwrap()
            .contains("不能用于")
    );

    let (status, _, rule) = send(
        &test.router,
        Method::POST,
        "/api/v1/category-rules",
        Some(
            json!({ "priority": 5, "categoryId": food_id, "conditions": [
            { "matchField": "note", "matchKind": "contains", "matchValue": "咖啡" }
        ] }),
        ),
        Some(&cookie),
        Some("category-rule-create"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{rule}");
    assert_eq!(rule["conditions"][0]["matchField"], "note");
    let rule_id = rule["id"].as_str().unwrap();

    let (status, _, updated_rule) = send(
        &test.router,
        Method::PATCH,
        &format!("/api/v1/category-rules/{}", rule["id"].as_str().unwrap()),
        Some(json!({ "priority": 3, "conditions": [
            { "matchField": "note", "matchKind": "prefix", "matchValue": "早晨" }
        ] })),
        Some(&cookie),
        Some("category-rule-update"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated_rule}");
    assert_eq!(updated_rule["priority"], 3);
    assert_eq!(updated_rule["conditions"][0]["matchKind"], "prefix");

    let (status, _, rules) = send(
        &test.router,
        Method::GET,
        "/api/v1/category-rules",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rules}");
    assert_eq!(rules[0]["id"], rule["id"]);
    assert_eq!(rules[0]["conditions"].as_array().unwrap().len(), 1);

    let (status, _, delete_error) = send(
        &test.router,
        Method::DELETE,
        &format!("/api/v1/categories/{food_id}"),
        None,
        Some(&cookie),
        Some("category-delete-referenced"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{delete_error}");
    assert!(delete_error["message"].as_str().unwrap().contains("归档"));

    let (status, _, transaction) = send(
        &test.router,
        Method::POST,
        "/api/v1/transactions",
        Some(
            json!({ "kind": "expense", "amountCents": 3500, "occurredOn": "2026-08-14",
            "category": "", "accountId": null, "transferFromAccountId": null,
            "transferToAccountId": null, "note": "早晨咖啡" }),
        ),
        Some(&cookie),
        Some("category-transaction-create"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{transaction}");
    let transaction_id = transaction["id"].as_str().unwrap();

    let (status, _, first) = send(
        &test.router,
        Method::POST,
        "/api/v1/categories/recategorize",
        None,
        Some(&cookie),
        Some("category-recategorize-first"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first, json!({ "eligible": 1, "matched": 1, "changed": 1 }));
    let (_, _, second) = send(
        &test.router,
        Method::POST,
        "/api/v1/categories/recategorize",
        None,
        Some(&cookie),
        Some("category-recategorize-second"),
        true,
    )
    .await;
    assert_eq!(second["changed"], 0);

    let (_, _, traced_transactions) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(traced_transactions["items"][0]["categoryRuleId"], rule_id);
    let (status, _, reverted) = send(
        &test.router,
        Method::POST,
        &format!("/api/v1/categories/rules/{rule_id}/revert"),
        None,
        Some(&cookie),
        Some("category-rule-revert"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reverted}");
    assert_eq!(reverted, json!({ "id": rule_id, "revertedCount": 1 }));
    let (_, _, replayed_revert) = send(
        &test.router,
        Method::POST,
        &format!("/api/v1/categories/rules/{rule_id}/revert"),
        None,
        Some(&cookie),
        Some("category-rule-revert"),
        true,
    )
    .await;
    assert_eq!(replayed_revert, reverted);
    let (_, _, after_revert) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert!(after_revert["items"][0]["categoryId"].is_null());
    assert_eq!(after_revert["items"][0]["categorySource"], "none");
    assert!(after_revert["items"][0]["categoryRuleId"].is_null());
    let (_, _, reapplied) = send(
        &test.router,
        Method::POST,
        "/api/v1/categories/recategorize",
        None,
        Some(&cookie),
        Some("category-recategorize-after-revert"),
        true,
    )
    .await;
    assert_eq!(reapplied["changed"], 1);

    let (status, _, updated) = send(
        &test.router,
        Method::PATCH,
        &format!("/api/v1/transactions/{transaction_id}"),
        Some(
            json!({ "version": transaction["version"], "kind": "expense", "amountCents": 3500,
            "occurredOn": "2026-08-14", "category": "", "categoryId": manual_id,
            "accountId": null, "transferFromAccountId": null, "transferToAccountId": null,
            "note": "早晨咖啡" }),
        ),
        Some(&cookie),
        Some("category-transaction-manual"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["categoryId"], manual_id);
    assert_eq!(updated["categorySource"], "user");
    assert!(updated["categoryRuleId"].is_null());

    let (_, _, user_safe_revert) = send(
        &test.router,
        Method::POST,
        &format!("/api/v1/categories/rules/{rule_id}/revert"),
        None,
        Some(&cookie),
        Some("category-rule-revert-after-user"),
        true,
    )
    .await;
    assert_eq!(user_safe_revert["revertedCount"], 0);

    let (_, _, after_manual) = send(
        &test.router,
        Method::POST,
        "/api/v1/categories/recategorize",
        None,
        Some(&cookie),
        Some("category-recategorize-manual"),
        true,
    )
    .await;
    assert_eq!(after_manual["eligible"], 0);
    assert_eq!(after_manual["changed"], 0);
    let (_, _, transactions) = send(
        &test.router,
        Method::GET,
        "/api/v1/transactions",
        None,
        Some(&cookie),
        None,
        false,
    )
    .await;
    assert_eq!(transactions["items"][0]["categoryId"], manual_id);
    assert_eq!(transactions["items"][0]["categorySource"], "user");

    let (status, _, archived) = send(
        &test.router,
        Method::PATCH,
        &format!("/api/v1/categories/{food_id}"),
        Some(json!({ "version": food["version"], "archived": true })),
        Some(&cookie),
        Some("category-archive-food"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{archived}");
    assert_eq!(archived["archived"], true);
    let (_, _, unmatched_transaction) = send(
        &test.router,
        Method::POST,
        "/api/v1/transactions",
        Some(
            json!({ "kind": "expense", "amountCents": 2000, "occurredOn": "2026-08-14",
            "category": "", "accountId": null, "transferFromAccountId": null,
            "transferToAccountId": null, "note": "早晨茶饮" }),
        ),
        Some(&cookie),
        Some("category-transaction-archived-target"),
        true,
    )
    .await;
    assert!(!unmatched_transaction["id"].as_str().unwrap().is_empty());
    let (_, _, archived_run) = send(
        &test.router,
        Method::POST,
        "/api/v1/categories/recategorize",
        None,
        Some(&cookie),
        Some("category-recategorize-archived"),
        true,
    )
    .await;
    assert_eq!(
        archived_run,
        json!({ "eligible": 1, "matched": 0, "changed": 0 })
    );

    let (status, _, deleted_rule) = send(
        &test.router,
        Method::DELETE,
        &format!("/api/v1/category-rules/{}", rule["id"].as_str().unwrap()),
        None,
        Some(&cookie),
        Some("category-rule-delete"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{deleted_rule}");
    let (status, _, deleted_child) = send(
        &test.router,
        Method::DELETE,
        &format!("/api/v1/categories/{}", child["id"].as_str().unwrap()),
        None,
        Some(&cookie),
        Some("category-child-delete"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{deleted_child}");
}

#[tokio::test]
async fn categorization_rules_cover_required_matching_and_protection_semantics() {
    let test = TestApp::new().await;
    test.insert_verified_user("categorize@example.com", "Password123!")
        .await;
    let conn = test.state.connection().await.unwrap();
    let user_id: String = conn
        .query(
            "SELECT id FROM users WHERE email='categorize@example.com'",
            (),
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    let now = "2026-08-14T00:00:00Z";

    for (id, name) in [
        ("cat-and", "严格且"),
        ("cat-fallback", "且回退"),
        ("cat-high", "高优先级"),
        ("cat-low", "低优先级"),
        ("cat-channel", "渠道"),
        ("cat-merchant", "凭证"),
        ("cat-amount", "区间"),
        ("cat-manual", "手工"),
        ("cat-percent", "百分号"),
    ] {
        conn.execute(
            "INSERT INTO categories(id,user_id,name,normalized_name,kind,created_at,updated_at) VALUES (?1,?2,?3,?3,'expense',?4,?4)",
            libsql::params![id, user_id.clone(), name, now],
        )
        .await
        .unwrap();
    }

    for (id, priority, channel, category) in [
        ("rule-and", 1_i64, "", "cat-and"),
        ("rule-priority-high", 2, "", "cat-high"),
        ("rule-channel", 3, "alipay", "cat-channel"),
        ("rule-merchant", 4, "alipay", "cat-merchant"),
        ("rule-amount", 5, "", "cat-amount"),
        ("rule-manual", 6, "", "cat-low"),
        ("rule-percent", 7, "", "cat-percent"),
        ("rule-and-fallback", 10, "", "cat-fallback"),
        ("rule-priority-low", 20, "", "cat-low"),
    ] {
        conn.execute(
            "INSERT INTO category_rules(id,user_id,priority,source_channel,category_id,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?6)",
            libsql::params![id, user_id.clone(), priority, channel, category, now],
        )
        .await
        .unwrap();
    }

    for (id, rule, field, kind, value) in [
        ("c-and-payee", "rule-and", "payee_name", "exact", "AND only"),
        ("c-and-note", "rule-and", "note", "exact", "also required"),
        (
            "c-and-fallback",
            "rule-and-fallback",
            "payee_name",
            "exact",
            "and ONLY",
        ),
        (
            "c-priority-high",
            "rule-priority-high",
            "payee_name",
            "exact",
            "Priority",
        ),
        (
            "c-priority-low",
            "rule-priority-low",
            "payee_name",
            "exact",
            "priority",
        ),
        (
            "c-channel",
            "rule-channel",
            "payee_name",
            "exact",
            "Channel target",
        ),
        (
            "c-merchant",
            "rule-merchant",
            "merchant_order_id",
            "prefix",
            "T200P",
        ),
        ("c-amount-gte", "rule-amount", "amount_cents", "gte", "100"),
        ("c-amount-lte", "rule-amount", "amount_cents", "lte", "200"),
        ("c-manual", "rule-manual", "payee_name", "exact", "Manual"),
        (
            "c-percent",
            "rule-percent",
            "description",
            "contains",
            "100%",
        ),
    ] {
        conn.execute(
            "INSERT INTO category_rule_conditions(id,rule_id,match_field,match_kind,match_value,created_at) VALUES (?1,?2,?3,?4,?5,?6)",
            libsql::params![id, rule, field, kind, value, now],
        )
        .await
        .unwrap();
    }

    conn.execute(
        "INSERT INTO import_batches(id,user_id,source_channel,file_sha256,period_start,period_end,total_count,status,committed_at,created_at,updated_at) VALUES ('category-batch',?1,'alipay',?2,'2026-08-14','2026-08-14',1,'committed',?3,?3,?3)",
        libsql::params![user_id.clone(), "a".repeat(64), now],
    )
    .await
    .unwrap();

    for (id, payee, description, amount, source, external_id, category_id, category_source) in [
        ("tx-and", "AND only", "", 50_i64, "", "", None, "none"),
        ("tx-priority", "PRIORITY", "", 50, "", "", None, "none"),
        (
            "tx-channel",
            "Channel target",
            "",
            50,
            "wechat",
            "wechat-category",
            None,
            "none",
        ),
        (
            "tx-merchant",
            "",
            "",
            50,
            "alipay",
            "merchant-category",
            None,
            "none",
        ),
        ("tx-amount", "", "", 150, "", "", None, "none"),
        (
            "tx-manual",
            "Manual",
            "",
            50,
            "",
            "",
            Some("cat-manual"),
            "user",
        ),
        (
            "tx-percent",
            "",
            "Sale 100% genuine",
            50,
            "",
            "",
            None,
            "none",
        ),
        (
            "tx-percent-impostor",
            "",
            "Sale 100X genuine",
            50,
            "",
            "",
            None,
            "none",
        ),
    ] {
        conn.execute(
            "INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,occurred_on,category_id,category_source,payee_name,description,created_at,updated_at,source_channel,external_id,import_batch_id) VALUES (?1,?2,'expense',?3,'2026-08-14',?4,?5,?6,?7,?8,?8,?9,?10,CASE WHEN ?1='tx-merchant' THEN 'category-batch' ELSE NULL END)",
            libsql::params![id, user_id.clone(), amount, category_id, category_source, payee, description, now, source, external_id],
        )
        .await
        .unwrap();
    }
    conn.execute(
        "INSERT INTO import_records(id,batch_id,row_index,external_id,merchant_order_id,occurred_at,occurred_on,direction,amount_cents,disposition,transaction_id,created_at) VALUES ('category-record','category-batch',1,'merchant-category','T200P-123','2026-08-14','2026-08-14','expense',50,'import','tx-merchant',?1)",
        [now],
    )
    .await
    .unwrap();

    let first = categorize::recategorize_user(&conn, &user_id)
        .await
        .unwrap();
    assert_eq!(first.eligible, 7);
    assert_eq!(first.matched, 5);
    assert_eq!(first.changed, 5);
    let second = categorize::recategorize_user(&conn, &user_id)
        .await
        .unwrap();
    assert_eq!(second.matched, 5);
    assert_eq!(second.changed, 0);

    let mut rows = conn
        .query(
            "SELECT id,category_id,category_source FROM ledger_transactions WHERE user_id=?1 ORDER BY id",
            [user_id.clone()],
        )
        .await
        .unwrap();
    let mut actual = Vec::new();
    while let Some(row) = rows.next().await.unwrap() {
        actual.push((
            row.get::<String>(0).unwrap(),
            row.get::<Option<String>>(1).unwrap(),
            row.get::<String>(2).unwrap(),
        ));
    }
    assert_eq!(
        actual,
        vec![
            ("tx-amount".into(), Some("cat-amount".into()), "rule".into()),
            ("tx-and".into(), Some("cat-fallback".into()), "rule".into()),
            ("tx-channel".into(), None, "none".into()),
            ("tx-manual".into(), Some("cat-manual".into()), "user".into()),
            (
                "tx-merchant".into(),
                Some("cat-merchant".into()),
                "rule".into()
            ),
            (
                "tx-percent".into(),
                Some("cat-percent".into()),
                "rule".into()
            ),
            ("tx-percent-impostor".into(), None, "none".into()),
            ("tx-priority".into(), Some("cat-high".into()), "rule".into()),
        ]
    );
    let traced_rule_assignments: i64 = conn
        .query(
            "SELECT COUNT(*) FROM ledger_transactions WHERE user_id=?1 AND category_source='rule' AND category_rule_id IS NOT NULL",
            [user_id],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(traced_rule_assignments, 5);
}

#[tokio::test]
async fn renormalize_backfills_legacy_receipts_and_manual_transactions_idempotently() {
    let test = TestApp::new().await;
    test.insert_verified_user("renormalize@example.com", "Password123!")
        .await;
    let conn = test.state.connection().await.unwrap();
    let user_id: String = conn
        .query(
            "SELECT id FROM users WHERE email='renormalize@example.com'",
            (),
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    let now = "2026-08-14T00:00:00Z";

    conn.execute(
        "INSERT INTO import_batches(id,user_id,source_channel,file_sha256,period_start,period_end,total_count,status,committed_at,created_at,updated_at) \
         VALUES ('renormalize-batch',?1,'cmb',?2,'2026-08-01','2026-08-14',5,'committed',?3,?3,?3)",
        libsql::params![user_id.clone(), "b".repeat(64), now],
    )
    .await
    .unwrap();

    for (index, id, external_id, counterparty, version, payee_key) in [
        (
            1_i64,
            "wall-legacy",
            "renormalize-1",
            "华尔街见闻1364473102",
            0_i64,
            "legacy-wall-key",
        ),
        (
            2,
            "protected-seven",
            "renormalize-2",
            "7分甜",
            1,
            "legacy-seven-key",
        ),
        (
            3,
            "protected-eighty-five",
            "renormalize-3",
            "85度C",
            1,
            "legacy-eighty-five-key",
        ),
        (
            4,
            "protected-member",
            "renormalize-4",
            "1号会员店",
            1,
            "legacy-member-key",
        ),
        (
            5,
            "already-current",
            "renormalize-5",
            "华尔街见闻430115617",
            2,
            "current-version-sentinel",
        ),
    ] {
        conn.execute(
            "INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,occurred_on,payee_name,payee_key,created_at,updated_at,source_channel,external_id,import_batch_id) \
             VALUES (?1,?2,'expense',100,'2026-08-14',?3,?4,?5,?5,'cmb',?6,'renormalize-batch')",
            libsql::params![id, user_id.clone(), counterparty, payee_key, now, external_id],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO import_records(id,batch_id,row_index,external_id,occurred_at,occurred_on,direction,amount_cents,counterparty,disposition,transaction_id,counterparty_normalized,normalization_version,created_at) \
             VALUES (?1,'renormalize-batch',?2,?3,?4,'2026-08-14','expense',100,?5,'import',?1,?6,?7,?4)",
            libsql::params![id, index, external_id, now, counterparty, payee_key, version],
        )
        .await
        .unwrap();
    }

    for (id, payee_name, payee_key) in [
        ("manual-width", "　ＡＢＣ１２３　", "stale-manual-key"),
        ("manual-empty", "", ""),
    ] {
        conn.execute(
            "INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,occurred_on,payee_name,payee_key,created_at,updated_at) \
             VALUES (?1,?2,'expense',100,'2026-08-14',?3,?4,?5,?5)",
            libsql::params![id, user_id.clone(), payee_name, payee_key, now],
        )
        .await
        .unwrap();
    }

    let first = renormalize_bin::renormalize_user(&conn, &user_id)
        .await
        .unwrap();
    assert_eq!(first.eligible_records, 4);
    assert_eq!(first.changed_records, 4);
    assert_eq!(first.changed_linked_transactions, 4);
    assert_eq!(first.scanned_manual_transactions, 2);
    assert_eq!(first.changed_manual_transactions, 1);
    assert_eq!(first.changed(), 9);

    let second = renormalize_bin::renormalize_user(&conn, &user_id)
        .await
        .unwrap();
    assert_eq!(second.eligible_records, 0);
    assert_eq!(second.changed_records, 0);
    assert_eq!(second.changed_linked_transactions, 0);
    assert_eq!(second.scanned_manual_transactions, 2);
    assert_eq!(second.changed_manual_transactions, 0);
    assert_eq!(second.changed(), 0);

    let mut rows = conn
        .query(
            "SELECT id,payee_key FROM ledger_transactions WHERE user_id=?1 ORDER BY id",
            [user_id.clone()],
        )
        .await
        .unwrap();
    let mut payee_keys = std::collections::BTreeMap::new();
    while let Some(row) = rows.next().await.unwrap() {
        payee_keys.insert(row.get::<String>(0).unwrap(), row.get::<String>(1).unwrap());
    }
    assert_eq!(payee_keys["wall-legacy"], "华尔街见闻");
    assert_eq!(payee_keys["protected-seven"], "7分甜");
    assert_eq!(payee_keys["protected-eighty-five"], "85度C");
    assert_eq!(payee_keys["protected-member"], "1号会员店");
    assert_eq!(payee_keys["already-current"], "current-version-sentinel");
    assert_eq!(payee_keys["manual-width"], "ABC123");
    assert_eq!(payee_keys["manual-empty"], "");

    let legacy_version_count: i64 = conn
        .query(
            "SELECT count(*) FROM import_records r \
             JOIN import_batches b ON b.id=r.batch_id \
             WHERE b.user_id=?1 AND r.normalization_version<2",
            [user_id],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(legacy_version_count, 0);
}

#[tokio::test]
async fn reself_backfills_exact_bank_aliases_and_preserves_protected_transactions() {
    let test = TestApp::new().await;
    let email = "reself@example.com";
    let owner = test.register_and_login(email).await;
    let account = test
        .create_ledger_account(&owner, "历史账单银行卡", "bank_card")
        .await;
    let account_id = account["id"].as_str().unwrap();
    let conn = test.state.connection().await.unwrap();
    let user_id: String = conn
        .query("SELECT id FROM users WHERE email=?1", [email])
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    let now = "2026-08-14T00:00:00Z";

    conn.execute(
        "INSERT INTO self_transfer_aliases(id,user_id,alias,normalized_alias,created_at,updated_at) VALUES ('reself-alias',?1,'虚构本人','虚构本人',?2,?2)",
        libsql::params![user_id.clone(), now],
    )
    .await
    .unwrap();
    conn.execute(
        "INSERT INTO categories(id,user_id,name,normalized_name,kind,created_at,updated_at) VALUES ('reself-category',?1,'历史分类','历史分类','expense',?2,?2)",
        libsql::params![user_id.clone(), now],
    )
    .await
    .unwrap();
    for event_id in ["reself-event", "reself-both-event"] {
        conn.execute(
            "INSERT INTO transaction_events(id,user_id,kind,created_at,updated_at) VALUES (?1,?2,'consume',?3,?3)",
            libsql::params![event_id, user_id.clone(), now],
        )
        .await
        .unwrap();
    }
    for (batch_id, channel, total_count) in [
        ("reself-cmb", "cmb", 6_i64),
        ("reself-cmbc", "cmbc", 1),
        ("reself-alipay", "alipay", 1),
    ] {
        conn.execute(
            "INSERT INTO import_batches(id,user_id,source_channel,file_sha256,period_start,period_end,total_count,status,committed_at,created_at,updated_at) VALUES (?1,?2,?3,?4,'2026-08-14','2026-08-14',?5,'committed',?6,?6,?6)",
            libsql::params![batch_id, user_id.clone(), channel, format!("{channel:0<64}"), total_count, now],
        )
        .await
        .unwrap();
    }

    for (index, id, batch_id, channel, kind, counterparty, event_id, archived_at) in [
        (
            1_i64,
            "reself-expense",
            "reself-cmb",
            "cmb",
            "expense",
            "虚构本人",
            None,
            None,
        ),
        (
            2,
            "reself-partial",
            "reself-cmb",
            "cmb",
            "expense",
            "虚构本人甲",
            None,
            None,
        ),
        (
            3,
            "reself-wechat-name",
            "reself-cmb",
            "cmb",
            "expense",
            "微信转账",
            None,
            None,
        ),
        (
            4,
            "reself-confirmed",
            "reself-cmb",
            "cmb",
            "expense",
            "虚构本人",
            Some("reself-event"),
            None,
        ),
        (
            5,
            "reself-archived",
            "reself-cmb",
            "cmb",
            "income",
            "虚构本人",
            None,
            Some(now),
        ),
        (
            6,
            "reself-both",
            "reself-cmb",
            "cmb",
            "expense",
            "虚构本人",
            Some("reself-both-event"),
            Some(now),
        ),
        (
            7,
            "reself-income",
            "reself-cmbc",
            "cmbc",
            "income",
            "虚构本人",
            None,
            None,
        ),
        (
            8,
            "reself-non-bank",
            "reself-alipay",
            "alipay",
            "expense",
            "虚构本人",
            None,
            None,
        ),
    ] {
        conn.execute(
            "INSERT INTO ledger_transactions(id,user_id,kind,amount_cents,occurred_on,category_id,category_source,account_id,archived_at,event_id,created_at,updated_at,source_channel,external_id,import_batch_id) VALUES (?1,?2,?3,100,'2026-08-14','reself-category','user',?4,?5,?6,?7,?7,?8,?1,?9)",
            libsql::params![id, user_id.clone(), kind, account_id, archived_at, event_id, now, channel, batch_id],
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO import_records(id,batch_id,row_index,external_id,occurred_at,occurred_on,direction,amount_cents,counterparty,disposition,transaction_id,counterparty_normalized,normalization_version,created_at) VALUES (?1,?2,?3,?4,?5,'2026-08-14',?6,100,?7,'import',?4,?7,2,?5)",
            libsql::params![format!("{id}-record"), batch_id, index, id, now, kind, counterparty],
        )
        .await
        .unwrap();
    }

    let transaction_count_before: i64 = conn
        .query(
            "SELECT count(*) FROM ledger_transactions WHERE user_id=?1",
            [user_id.clone()],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    let first = reself_bin::reself_user(&conn, &user_id).await.unwrap();
    assert_eq!(first.eligible, 7);
    assert_eq!(first.matched, 5);
    assert_eq!(first.changed, 2);
    assert_eq!(first.skipped_total, 3);
    assert_eq!(first.skipped_confirmed, 2);
    assert_eq!(first.skipped_archived, 2);

    let second = reself_bin::reself_user(&conn, &user_id).await.unwrap();
    assert_eq!(second.changed, 0);
    assert_eq!(second.matched, 3);
    assert_eq!(second.skipped_total, 3);

    let mut rows = conn
        .query(
            "SELECT id,kind,account_id,transfer_from_account_id,transfer_to_account_id,category_id,category_source FROM ledger_transactions WHERE user_id=?1 ORDER BY id",
            [user_id.clone()],
        )
        .await
        .unwrap();
    let mut transactions = std::collections::BTreeMap::new();
    while let Some(row) = rows.next().await.unwrap() {
        transactions.insert(
            row.get::<String>(0).unwrap(),
            (
                row.get::<String>(1).unwrap(),
                row.get::<Option<String>>(2).unwrap(),
                row.get::<Option<String>>(3).unwrap(),
                row.get::<Option<String>>(4).unwrap(),
                row.get::<Option<String>>(5).unwrap(),
                row.get::<String>(6).unwrap(),
            ),
        );
    }
    assert_eq!(
        transactions["reself-expense"],
        (
            "transfer".into(),
            None,
            Some(account_id.into()),
            None,
            None,
            "none".into()
        )
    );
    assert_eq!(
        transactions["reself-income"],
        (
            "transfer".into(),
            None,
            None,
            Some(account_id.into()),
            None,
            "none".into()
        )
    );
    for id in [
        "reself-archived",
        "reself-both",
        "reself-confirmed",
        "reself-non-bank",
        "reself-partial",
        "reself-wechat-name",
    ] {
        assert_ne!(transactions[id].0, "transfer", "{id} must remain unchanged");
        assert_eq!(transactions[id].1.as_deref(), Some(account_id));
        assert_eq!(transactions[id].4.as_deref(), Some("reself-category"));
        assert_eq!(transactions[id].5, "user");
    }

    let transaction_count_after: i64 = conn
        .query(
            "SELECT count(*) FROM ledger_transactions WHERE user_id=?1",
            [user_id],
        )
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(transaction_count_after, transaction_count_before);
    assert!(
        conn.query("PRAGMA foreign_key_check", ())
            .await
            .unwrap()
            .next()
            .await
            .unwrap()
            .is_none()
    );
}
