use std::{env, net::SocketAddr, path::PathBuf};

use anyhow::{Context, Result, bail};

#[derive(Clone, Debug)]
pub struct Config {
    pub app_env: String,
    pub bind_addr: SocketAddr,
    pub public_base_url: String,
    pub database_url: String,
    pub turso_auth_token: Option<String>,
    pub dev_mail_dir: PathBuf,
    pub web_dist_dir: PathBuf,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let app_env = env::var("APP_ENV").unwrap_or_else(|_| "development".into());
        if app_env == "production" {
            bail!("production requires a real EmailSender; dev-file is intentionally disabled");
        }

        Ok(Self {
            app_env,
            bind_addr: env::var("BIND_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:8787".into())
                .parse()
                .context("invalid BIND_ADDR")?,
            public_base_url: env::var("PUBLIC_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:5173".into())
                .trim_end_matches('/')
                .to_owned(),
            database_url: env::var("DATABASE_URL").unwrap_or_else(|_| "file:./var/zhiyu.db".into()),
            turso_auth_token: env::var("TURSO_AUTH_TOKEN")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            dev_mail_dir: env::var("DEV_MAIL_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("./var/dev-mail")),
            web_dist_dir: env::var("WEB_DIST_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("./apps/web/dist")),
        })
    }

    pub fn is_production(&self) -> bool {
        matches!(self.app_env.as_str(), "production" | "self-host")
    }

    pub fn email_delivery_available(&self) -> bool {
        self.app_env != "self-host"
    }

    pub fn cookie_name(&self) -> &'static str {
        if self.is_production() {
            "__Host-zhiyu_session"
        } else {
            "zhiyu_session"
        }
    }
}
