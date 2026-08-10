use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};

use anyhow::{Context, Result, anyhow, bail};
use keyring::{Entry, Error as KeyringError};
use reqwest::Url;
use serde::{Deserialize, Serialize};

const KEYRING_SERVICE: &str = "app.zhiyu.desktop";
const KEYRING_USER: &str = "backup-api-key";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionConfig {
    pub server_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_fallback: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keychain_warning: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedConnection {
    pub server_url: Url,
    pub api_key: String,
    pub credential_warning: Option<String>,
}

pub fn validate_server_url(input: &str) -> Result<Url> {
    if input.trim() != input || input.is_empty() {
        bail!("服务器 URL 不能为空，且首尾不能有空格");
    }
    let mut url =
        Url::parse(input).context("服务器 URL 必须是完整形式，例如 https://ledger.example.com")?;
    if url.cannot_be_a_base() || url.host_str().is_none() {
        bail!("服务器 URL 必须包含协议和主机名");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("服务器 URL 不能包含用户名或密码");
    }
    if url.query().is_some() || url.fragment().is_some() {
        bail!("服务器 URL 不能包含查询参数或片段");
    }
    match url.scheme() {
        "https" => {}
        "http" if url.host_str() == Some("127.0.0.1") => {}
        "http" => bail!("HTTP 只允许用于 127.0.0.1 开发地址，其他服务器必须使用 HTTPS"),
        _ => bail!("服务器 URL 必须使用 HTTPS；开发时可使用 http://127.0.0.1"),
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

pub fn load_config(path: &Path) -> Result<ConnectionConfig> {
    let bytes = fs::read(path).with_context(|| format!("无法读取连接配置 {}", path.display()))?;
    let config: ConnectionConfig = serde_json::from_slice(&bytes)
        .with_context(|| format!("连接配置不是有效 JSON：{}", path.display()))?;
    validate_server_url(&config.server_url)?;
    Ok(config)
}

pub fn resolve_connection(path: &Path) -> Result<ResolvedConnection> {
    let config = load_config(path)?;
    let server_url = validate_server_url(&config.server_url)?;
    if config.api_key_fallback.is_some() {
        return fallback_connection(config, server_url, None);
    }
    let entry = Entry::new(KEYRING_SERVICE, KEYRING_USER).context("无法访问 macOS Keychain")?;
    match entry.get_password() {
        Ok(api_key) if !api_key.is_empty() => Ok(ResolvedConnection {
            server_url,
            api_key,
            credential_warning: None,
        }),
        Ok(_) | Err(KeyringError::NoEntry) => fallback_connection(config, server_url, None),
        Err(error) => fallback_connection(config, server_url, Some(error.to_string())),
    }
}

fn fallback_connection(
    config: ConnectionConfig,
    server_url: Url,
    read_error: Option<String>,
) -> Result<ResolvedConnection> {
    let api_key = config
        .api_key_fallback
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            read_error.as_ref().map_or_else(
                || anyhow!("macOS Keychain 中没有 api-key，配置文件也没有降级凭证"),
                |error| anyhow!("读取 macOS Keychain 失败，且没有配置文件降级凭证：{error}"),
            )
        })?;
    let warning = read_error.map_or_else(
        || {
            config.keychain_warning.unwrap_or_else(|| {
                "api-key 正从权限为 0600 的配置文件读取，未使用 macOS Keychain。".to_owned()
            })
        },
        |error| format!("读取 macOS Keychain 失败（{error}）；已降级使用权限为 0600 的配置文件。"),
    );
    Ok(ResolvedConnection {
        server_url,
        api_key,
        credential_warning: Some(warning),
    })
}

pub fn save_connection(path: &Path, server_url: &Url, api_key: &str) -> Result<Option<String>> {
    if api_key.is_empty() {
        bail!("api-key 不能为空");
    }
    let keychain_error = match Entry::new(KEYRING_SERVICE, KEYRING_USER) {
        Ok(entry) => match entry.set_password(api_key) {
            Ok(()) => match entry.get_password() {
                Ok(stored) if stored == api_key => None,
                Ok(_) => Some("写入后读取到的凭证内容不一致".to_owned()),
                Err(error) => Some(format!("写入后读取验证失败：{error}")),
            },
            Err(error) => Some(error.to_string()),
        },
        Err(error) => Some(error.to_string()),
    };
    let (api_key_fallback, keychain_warning) = match keychain_error {
        None => (None, None),
        Some(error) => {
            let warning = format!(
                "macOS Keychain 写入或读取验证失败（{error}）；api-key 已降级保存到权限为 0600 的配置文件。"
            );
            (Some(api_key.to_owned()), Some(warning))
        }
    };
    let config = ConnectionConfig {
        server_url: server_url.as_str().trim_end_matches('/').to_owned(),
        api_key_fallback,
        keychain_warning: keychain_warning.clone(),
    };
    write_private_json(path, &config).context("保存连接配置失败")?;
    Ok(keychain_warning)
}

pub fn clear_connection(path: &Path) -> Result<Option<String>> {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("无法删除连接配置 {}", path.display()));
        }
    }
    let partial = path.with_extension("json.partial");
    let mut warnings = Vec::new();
    match fs::remove_file(&partial) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => warnings.push(format!(
            "删除未完成的连接配置 {} 失败：{error}",
            partial.display()
        )),
    }

    match Entry::new(KEYRING_SERVICE, KEYRING_USER) {
        Ok(entry) => match entry.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => {}
            Err(error) => warnings.push(format!("删除 macOS Keychain 中的 api-key 失败：{error}")),
        },
        Err(error) => warnings.push(format!(
            "访问 macOS Keychain 失败，无法删除 api-key：{error}"
        )),
    }
    Ok((!warnings.is_empty()).then(|| warnings.join("；")))
}

pub fn write_private_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("配置文件路径没有父目录")?;
    fs::create_dir_all(parent).with_context(|| format!("无法创建配置目录 {}", parent.display()))?;
    let partial = path.with_extension("json.partial");
    let bytes = serde_json::to_vec_pretty(value)?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&partial)
        .with_context(|| format!("无法写入临时配置 {}", partial.display()))?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(&partial, path).with_context(|| format!("无法原子发布配置 {}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_https_and_loopback_development_urls() {
        assert!(validate_server_url("https://ledger.example.com").is_ok());
        assert!(validate_server_url("https://10.0.0.2:8443/base").is_ok());
        assert!(validate_server_url("http://127.0.0.1:3000").is_ok());
    }

    #[test]
    fn rejects_incomplete_or_insecure_server_urls() {
        for invalid in [
            "ledger.example.com",
            "10.0.0.2:3000",
            "http://ledger.example.com",
            "http://localhost:3000",
            "ftp://ledger.example.com",
            "https://user:secret@ledger.example.com",
            "https://ledger.example.com?debug=1",
        ] {
            assert!(
                validate_server_url(invalid).is_err(),
                "{invalid} should be rejected"
            );
        }
    }
}
