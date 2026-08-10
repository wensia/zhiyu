use std::{env, fmt, fs, net::SocketAddr, path::PathBuf};

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
    pub bill_inbox: Option<BillInboxConfig>,
}

#[derive(Clone)]
pub struct BillInboxConfig {
    pub session_url: String,
    pub username: String,
    pub password: String,
    pub address: String,
    pub owner_email: String,
    pub poll_interval_seconds: u64,
    pub max_message_bytes: usize,
}

impl fmt::Debug for BillInboxConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BillInboxConfig")
            .field("session_url", &self.session_url)
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .field("address", &self.address)
            .field("owner_email", &self.owner_email)
            .field("poll_interval_seconds", &self.poll_interval_seconds)
            .field("max_message_bytes", &self.max_message_bytes)
            .finish()
    }
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let app_env = env::var("APP_ENV").unwrap_or_else(|_| "development".into());
        let bill_inbox =
            BillInboxConfig::from_env(matches!(app_env.as_str(), "production" | "self-host"))?;
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
            bill_inbox,
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

impl BillInboxConfig {
    const DEFAULT_POLL_INTERVAL_SECONDS: u64 = 300;
    const DEFAULT_MAX_MESSAGE_BYTES: usize = 10 * 1024 * 1024;
    const MAX_ALLOWED_MESSAGE_BYTES: usize = 25 * 1024 * 1024;

    fn from_env(is_production: bool) -> Result<Option<Self>> {
        Self::from_lookup(is_production, |name| {
            env::var(name)
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        })
    }

    fn from_lookup<F>(is_production: bool, lookup: F) -> Result<Option<Self>>
    where
        F: Fn(&str) -> Option<String>,
    {
        let session_url = lookup("BILL_INBOX_JMAP_SESSION_URL");
        let username = lookup("BILL_INBOX_USERNAME");
        let password_file = lookup("BILL_INBOX_PASSWORD_FILE");
        let address = lookup("BILL_INBOX_ADDRESS");
        let owner_email = lookup("BILL_INBOX_OWNER_EMAIL");
        let poll_interval = lookup("BILL_INBOX_POLL_INTERVAL_SECONDS");
        let max_message_bytes = lookup("BILL_INBOX_MAX_MESSAGE_BYTES");
        let required = [
            ("BILL_INBOX_JMAP_SESSION_URL", session_url.as_ref()),
            ("BILL_INBOX_USERNAME", username.as_ref()),
            ("BILL_INBOX_PASSWORD_FILE", password_file.as_ref()),
            ("BILL_INBOX_ADDRESS", address.as_ref()),
            ("BILL_INBOX_OWNER_EMAIL", owner_email.as_ref()),
        ];
        let configured = required.iter().any(|(_, value)| value.is_some())
            || poll_interval.is_some()
            || max_message_bytes.is_some();
        if !configured {
            return Ok(None);
        }

        let missing = required
            .iter()
            .filter_map(|(name, value)| value.is_none().then_some(*name))
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            bail!(
                "bill inbox configuration is incomplete; missing {}",
                missing.join(", ")
            );
        }

        let session_url = session_url.unwrap();
        let username = username.unwrap();
        let password_file = PathBuf::from(password_file.unwrap());
        let address = address.unwrap();
        let owner_email = owner_email.unwrap();

        let username = crate::domain::validate_email(&username)
            .map_err(|_| anyhow::anyhow!("BILL_INBOX_USERNAME must be a valid email address"))?;
        let address = crate::domain::validate_email(&address)
            .map_err(|_| anyhow::anyhow!("BILL_INBOX_ADDRESS must be a valid email address"))?;
        let owner_email = crate::domain::validate_email(&owner_email)
            .map_err(|_| anyhow::anyhow!("BILL_INBOX_OWNER_EMAIL must be a valid email address"))?;

        if !username.eq_ignore_ascii_case(&address) {
            bail!(
                "BILL_INBOX_USERNAME and BILL_INBOX_ADDRESS must identify the same dedicated mailbox"
            );
        }

        let parsed_session_url = reqwest::Url::parse(&session_url)
            .context("BILL_INBOX_JMAP_SESSION_URL must be a valid absolute URL")?;
        if parsed_session_url.host_str().is_none()
            || !parsed_session_url.username().is_empty()
            || parsed_session_url.password().is_some()
        {
            bail!("BILL_INBOX_JMAP_SESSION_URL must have a host and no embedded credentials");
        }
        if !matches!(parsed_session_url.scheme(), "http" | "https") {
            bail!("BILL_INBOX_JMAP_SESSION_URL must use http or https");
        }
        if is_production && parsed_session_url.scheme() != "https" {
            bail!("BILL_INBOX_JMAP_SESSION_URL must use https in production/self-host");
        }
        let session_url = parsed_session_url.to_string();

        let password = fs::read_to_string(&password_file).with_context(|| {
            format!(
                "failed to read BILL_INBOX_PASSWORD_FILE {}",
                password_file.display()
            )
        })?;
        // Secret 文件通常由编辑器或 Docker secret 带一个行尾；只去掉这一处协议性
        // 换行，不能用 trim() 改写本来就以空格开头/结尾的合法邮箱密码。
        let password = password
            .strip_suffix("\r\n")
            .or_else(|| password.strip_suffix('\n'))
            .unwrap_or(&password)
            .to_owned();
        if password.is_empty() {
            bail!("BILL_INBOX_PASSWORD_FILE must contain a non-empty password");
        }

        let poll_interval_seconds = parse_positive(
            "BILL_INBOX_POLL_INTERVAL_SECONDS",
            poll_interval,
            Self::DEFAULT_POLL_INTERVAL_SECONDS,
        )?;
        let max_message_bytes = parse_positive(
            "BILL_INBOX_MAX_MESSAGE_BYTES",
            max_message_bytes,
            Self::DEFAULT_MAX_MESSAGE_BYTES,
        )?;
        if max_message_bytes > Self::MAX_ALLOWED_MESSAGE_BYTES {
            bail!("BILL_INBOX_MAX_MESSAGE_BYTES must not exceed 26214400");
        }

        Ok(Some(Self {
            session_url,
            username,
            password,
            address,
            owner_email,
            poll_interval_seconds,
            max_message_bytes,
        }))
    }
}

fn parse_positive<T>(name: &str, value: Option<String>, default: T) -> Result<T>
where
    T: Copy + PartialEq + From<u8> + std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    let Some(value) = value else {
        return Ok(default);
    };
    let parsed = value
        .parse::<T>()
        .with_context(|| format!("invalid {name}"))?;
    if parsed == T::from(0) {
        bail!("{name} must be greater than zero");
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::BillInboxConfig;

    fn lookup(values: &HashMap<&str, String>, name: &str) -> Option<String> {
        values.get(name).cloned()
    }

    fn complete_values(password_file: &std::path::Path) -> HashMap<&'static str, String> {
        HashMap::from([
            (
                "BILL_INBOX_JMAP_SESSION_URL",
                "https://mail.example.com/jmap/session".into(),
            ),
            ("BILL_INBOX_USERNAME", "zhiyu-bills@example.com".into()),
            (
                "BILL_INBOX_PASSWORD_FILE",
                password_file.display().to_string(),
            ),
            ("BILL_INBOX_ADDRESS", "zhiyu-bills@example.com".into()),
            ("BILL_INBOX_OWNER_EMAIL", "owner@example.com".into()),
        ])
    }

    #[test]
    fn bill_inbox_is_disabled_when_no_related_values_are_set() {
        let values = HashMap::new();
        assert!(
            BillInboxConfig::from_lookup(false, |name| lookup(&values, name))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn bill_inbox_rejects_partial_configuration() {
        let values = HashMap::from([(
            "BILL_INBOX_JMAP_SESSION_URL",
            "https://mail.example.com/jmap/session".into(),
        )]);
        let error = BillInboxConfig::from_lookup(false, |name| lookup(&values, name))
            .unwrap_err()
            .to_string();
        assert!(error.contains("BILL_INBOX_USERNAME"));
        assert!(error.contains("BILL_INBOX_PASSWORD_FILE"));
    }

    #[test]
    fn bill_inbox_loads_password_defaults_and_redacts_debug_output() {
        let root = tempfile::tempdir().unwrap();
        let password_file = root.path().join("password");
        std::fs::write(&password_file, "secret-from-file\n").unwrap();
        let mut values = complete_values(&password_file);
        values.insert("BILL_INBOX_USERNAME", "ZHIYU-BILLS@EXAMPLE.COM".into());
        values.insert("BILL_INBOX_ADDRESS", "ZHIYU-BILLS@EXAMPLE.COM".into());
        values.insert("BILL_INBOX_OWNER_EMAIL", "OWNER@EXAMPLE.COM".into());

        let config = BillInboxConfig::from_lookup(true, |name| lookup(&values, name))
            .unwrap()
            .unwrap();
        assert_eq!(config.password, "secret-from-file");
        assert_eq!(config.username, "zhiyu-bills@example.com");
        assert_eq!(config.address, "zhiyu-bills@example.com");
        assert_eq!(config.owner_email, "owner@example.com");
        assert_eq!(config.poll_interval_seconds, 300);
        assert_eq!(config.max_message_bytes, 10 * 1024 * 1024);
        let debug = format!("{config:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("secret-from-file"));

        std::fs::write(&password_file, " secret with spaces \n").unwrap();
        let config = BillInboxConfig::from_lookup(true, |name| lookup(&values, name))
            .unwrap()
            .unwrap();
        assert_eq!(config.password, " secret with spaces ");
    }

    #[test]
    fn bill_inbox_requires_https_in_production_and_validates_positive_limits() {
        let root = tempfile::tempdir().unwrap();
        let password_file = root.path().join("password");
        std::fs::write(&password_file, "secret").unwrap();
        let mut values = complete_values(&password_file);
        values.insert(
            "BILL_INBOX_JMAP_SESSION_URL",
            "http://stalwart:8080/jmap/session".into(),
        );
        assert!(
            BillInboxConfig::from_lookup(true, |name| lookup(&values, name))
                .unwrap_err()
                .to_string()
                .contains("must use https")
        );

        values.insert(
            "BILL_INBOX_JMAP_SESSION_URL",
            "https://mail.example.com/jmap/session".into(),
        );
        values.insert("BILL_INBOX_POLL_INTERVAL_SECONDS", "0".into());
        assert!(
            BillInboxConfig::from_lookup(false, |name| lookup(&values, name))
                .unwrap_err()
                .to_string()
                .contains("greater than zero")
        );

        values.remove("BILL_INBOX_POLL_INTERVAL_SECONDS");
        values.insert("BILL_INBOX_MAX_MESSAGE_BYTES", "26214400".into());
        assert_eq!(
            BillInboxConfig::from_lookup(false, |name| lookup(&values, name))
                .unwrap()
                .unwrap()
                .max_message_bytes,
            26_214_400
        );
        values.insert("BILL_INBOX_MAX_MESSAGE_BYTES", "26214401".into());
        assert!(
            BillInboxConfig::from_lookup(false, |name| lookup(&values, name))
                .unwrap_err()
                .to_string()
                .contains("must not exceed")
        );

        values.remove("BILL_INBOX_MAX_MESSAGE_BYTES");
        values.insert("BILL_INBOX_JMAP_SESSION_URL", "https://".into());
        assert!(
            BillInboxConfig::from_lookup(false, |name| lookup(&values, name))
                .unwrap_err()
                .to_string()
                .contains("valid absolute URL")
        );
    }

    #[test]
    fn bill_inbox_rejects_a_shared_mailbox_alias_configuration() {
        let root = tempfile::tempdir().unwrap();
        let password_file = root.path().join("password");
        std::fs::write(&password_file, "secret").unwrap();
        let mut values = complete_values(&password_file);
        values.insert("BILL_INBOX_USERNAME", "shared@example.com".into());

        assert!(
            BillInboxConfig::from_lookup(true, |name| lookup(&values, name))
                .unwrap_err()
                .to_string()
                .contains("same dedicated mailbox")
        );
    }
}
