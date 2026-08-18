use anyhow::{Context, Result, bail};
use zhiyu_api::{categorize, config::Config, db};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let mut args = std::env::args().skip(1);
    let user_id = args.next().context("用法：zhiyu-recategorize <用户 ID>")?;
    if args.next().is_some() {
        bail!("用法：zhiyu-recategorize <用户 ID>");
    }

    let config = Config::from_env()?;
    let database = db::connect(&config).await?;
    let conn = database.connect()?;
    let mut rows = conn
        .query("SELECT id FROM users WHERE id=?1", [user_id.as_str()])
        .await?;
    if rows.next().await?.is_none() {
        bail!("找不到指定用户");
    }
    drop(rows);

    let stats = categorize::recategorize_user(&conn, &user_id).await?;
    println!(
        "eligible={} matched={} changed={}",
        stats.eligible, stats.matched, stats.changed
    );
    Ok(())
}
