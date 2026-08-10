use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use zhiyu_api::{
    AppState, auth, config::Config, db, email::DevFileEmailSender, rate_limit::RateLimiter,
};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let email = std::env::args()
        .nth(1)
        .context("用法：zhiyu-api-key <机器用户邮箱>")?;
    let config = Config::from_env()?;
    let database = db::connect(&config).await?;
    let email_sender = DevFileEmailSender::new(config.dev_mail_dir.clone());
    let state = AppState {
        db: Arc::new(database),
        config: Arc::new(config),
        email: Arc::new(email_sender),
        rate_limiter: RateLimiter::default(),
        backup_status: Default::default(),
    };
    let key = auth::issue_api_key(&state, &email)
        .await
        .map_err(|error| anyhow!("{}: {}", error.code, error.message))?;
    eprintln!("API 密钥已签发；明文只显示这一次，请立即保存：");
    println!("{key}");
    Ok(())
}
