use std::{
    io::{self, IsTerminal, Write},
    process::Command,
    sync::Arc,
};

use anyhow::{Context, Result, anyhow, bail};
use zhiyu_api::{
    AppState, auth, config::Config, db, email::DevFileEmailSender, rate_limit::RateLimiter,
};

struct EchoGuard {
    restore: bool,
}

impl EchoGuard {
    fn disable_for_stdin() -> Result<Self> {
        if !io::stdin().is_terminal() {
            return Ok(Self { restore: false });
        }
        let status = Command::new("stty")
            .arg("-echo")
            .status()
            .context("无法关闭终端回显")?;
        if !status.success() {
            bail!("无法关闭终端回显，已拒绝读取密码");
        }
        Ok(Self { restore: true })
    }
}

impl Drop for EchoGuard {
    fn drop(&mut self) {
        if self.restore {
            let _ = Command::new("stty").arg("echo").status();
            eprintln!();
        }
    }
}

fn read_password() -> Result<String> {
    if io::stdin().is_terminal() {
        eprint!("新密码：");
        io::stderr().flush()?;
    }
    let guard = EchoGuard::disable_for_stdin()?;
    let mut password = String::new();
    io::stdin().read_line(&mut password)?;
    drop(guard);
    while matches!(password.chars().last(), Some('\n' | '\r')) {
        password.pop();
    }
    Ok(password)
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let email = std::env::args()
        .nth(1)
        .context("用法：zhiyu-passwd <邮箱>")?;
    if std::env::args().nth(2).is_some() {
        bail!("用法：zhiyu-passwd <邮箱>；密码必须从 stdin 输入");
    }
    let password = read_password()?;
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
    auth::set_password(&state, &email, password)
        .await
        .map_err(|error| anyhow!("{}: {}", error.code, error.message))?;
    eprintln!("密码已更新，现有网页登录会话已全部失效。");
    Ok(())
}
