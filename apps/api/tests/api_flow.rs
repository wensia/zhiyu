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
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tower::ServiceExt;
use uuid::Uuid;
use zhiyu_api::{
    AppState, app, auth, config::Config, db, domain::MAX_SAFE_CENTS, email::DevFileEmailSender,
    rate_limit::RateLimiter,
};

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
    assert_eq!(list[0]["schemaVersion"], 10);
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
    let (status, _, _) = send(
        &test.router,
        Method::DELETE,
        &format!("/api/v1/transactions/{transaction_id}"),
        Some(json!({ "version": 2 })),
        Some(&cookie),
        Some("delete-tx-0001"),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
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
    assert_eq!(status, StatusCode::NO_CONTENT);

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
    assert_eq!(status, StatusCode::NO_CONTENT);
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
