use std::sync::Arc;

use anyhow::Result;
use tracing_subscriber::EnvFilter;
use zhiyu_api::{
    AppState, app, config::Config, db, email::DevFileEmailSender, rate_limit::RateLimiter,
};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let config = Config::from_env()?;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "zhiyu_api=info,tower_http=info".into());
    if config.is_production() {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .json()
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .compact()
            .init();
    }

    let database = db::connect(&config).await?;
    let address = config.bind_addr;
    let email = DevFileEmailSender::new(config.dev_mail_dir.clone());
    let state = AppState {
        db: Arc::new(database),
        config: Arc::new(config),
        email: Arc::new(email),
        rate_limiter: RateLimiter::default(),
        backup_status: Default::default(),
    };
    zhiyu_api::backup::spawn_backup_scheduler(state.clone());
    zhiyu_api::bill_inbox::spawn_scheduler(state.clone());
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(%address, "zhiyu API listening");
    axum::serve(listener, app(state).into_make_service()).await?;
    Ok(())
}
