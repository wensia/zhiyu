use std::path::PathBuf;

use zhiyu_api::{config::Config, db};

#[tokio::test]
#[ignore = "requires TURSO_DATABASE_URL and TURSO_AUTH_TOKEN"]
async fn turso_remote_readiness_smoke() {
    let database_url = std::env::var("TURSO_DATABASE_URL")
        .expect("TURSO_DATABASE_URL must be set for the ignored remote smoke test");
    let token = std::env::var("TURSO_AUTH_TOKEN")
        .expect("TURSO_AUTH_TOKEN must be set for the ignored remote smoke test");
    assert!(
        database_url.starts_with("libsql://") || database_url.starts_with("https://"),
        "TURSO_DATABASE_URL must be a remote libSQL URL"
    );

    let config = Config {
        app_env: "test".into(),
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        public_base_url: "http://test.local".into(),
        database_url,
        turso_auth_token: Some(token),
        dev_mail_dir: PathBuf::from("./var/dev-mail"),
        web_dist_dir: PathBuf::from("./apps/web/dist"),
        bill_inbox: None,
    };
    let database = db::connect(&config).await.unwrap();
    let conn = database.connect().unwrap();
    let mut rows = conn.query("SELECT 1", ()).await.unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        1
    );
}
