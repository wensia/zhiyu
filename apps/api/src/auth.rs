use std::time::Duration as StdDuration;

use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use axum::{
    Json,
    extract::{FromRequestParts, State},
    http::{HeaderMap, HeaderValue, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use libsql::{TransactionBehavior, params};
use rand::RngCore;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    AppState,
    domain::{
        EmailRequest, LoginRequest, MessageResponse, RegisterRequest, ResetPasswordRequest,
        TokenRequest, UserView, validate_email, validate_password, validate_timezone,
    },
    email::EmailMessage,
    error::ApiError,
};

const SESSION_DAYS: i64 = 30;

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: String,
    pub email: String,
    pub timezone: String,
    pub session_hash: String,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if let Some(user) = parts.extensions.get::<Self>() {
            return Ok(user.clone());
        }
        let token = cookie_value(&parts.headers, state.config.cookie_name())
            .ok_or_else(|| ApiError::unauthorized("请先登录"))?;
        authenticate_token(state, &token).await
    }
}

pub(crate) async fn authenticate_token(
    state: &AppState,
    token: &str,
) -> Result<AuthUser, ApiError> {
    let token_hash = hash_token(token);
    let conn = state.connection().await?;
    let now = Utc::now();
    let mut rows = conn
            .query(
                "SELECT u.id, u.email, u.timezone, s.last_seen_at FROM sessions s JOIN users u ON u.id = s.user_id WHERE s.token_hash = ?1 AND s.expires_at > ?2 AND u.email_verified_at IS NOT NULL",
                params![token_hash.clone(), now.to_rfc3339()],
            )
            .await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| ApiError::unauthorized("登录已过期，请重新登录"))?;
    let user = AuthUser {
        id: row.get(0)?,
        email: row.get(1)?,
        timezone: row.get(2)?,
        session_hash: token_hash.clone(),
    };
    let last_seen: String = row.get(3)?;
    if chrono::DateTime::parse_from_rfc3339(&last_seen)
        .map(|value| now.signed_duration_since(value.with_timezone(&Utc)) > Duration::hours(24))
        .unwrap_or(true)
    {
        conn.execute(
            "UPDATE sessions SET last_seen_at = ?1, expires_at = ?2 WHERE token_hash = ?3",
            params![
                now.to_rfc3339(),
                (now + Duration::days(SESSION_DAYS)).to_rfc3339(),
                token_hash
            ],
        )
        .await?;
    }
    Ok(user)
}

#[utoipa::path(post, path = "/api/v1/auth/register", request_body = RegisterRequest, responses((status = 201, body = MessageResponse), (status = 422, body = crate::error::ErrorBody)))]
pub async fn register(
    State(state): State<AppState>,
    Json(input): Json<RegisterRequest>,
) -> Result<(StatusCode, Json<MessageResponse>), ApiError> {
    let email = validate_email(&input.email)?;
    validate_password(&input.password)?;
    let timezone = validate_timezone(input.timezone.as_deref())?;
    state
        .rate_limiter
        .check(format!("register:{email}"), 5, StdDuration::from_secs(3600))
        .await?;

    let password_hash = hash_password(input.password).await?;
    let user_id = Uuid::now_v7().to_string();
    let now = Utc::now().to_rfc3339();
    let (token, token_hash) = new_token();
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    if tx
        .execute(
            "INSERT INTO users(id, email, password_hash, timezone, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![user_id.clone(), email.clone(), password_hash, timezone, now.clone()],
        )
        .await
        .is_err()
    {
        return Err(ApiError::conflict("email_taken", "该邮箱已注册"));
    }
    insert_email_token(&tx, &user_id, "verify_email", &token_hash, 24).await?;
    tx.commit().await?;

    state
        .email
        .send(EmailMessage {
            to: email,
            subject: "验证你的知余账户".into(),
            text: format!(
                "请在 24 小时内打开以下链接完成验证：\n{}/verify-email?token={token}",
                state.config.public_base_url
            ),
        })
        .await
        .map_err(ApiError::internal)?;

    Ok((
        StatusCode::CREATED,
        Json(MessageResponse {
            message: "注册成功，请查收验证邮件".into(),
        }),
    ))
}

#[utoipa::path(post, path = "/api/v1/auth/verify-email", request_body = TokenRequest, responses((status = 200, body = MessageResponse)))]
pub async fn verify_email(
    State(state): State<AppState>,
    Json(input): Json<TokenRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    consume_email_token(&state, &input.token, "verify_email", None).await?;
    Ok(Json(MessageResponse {
        message: "邮箱验证成功，现在可以登录".into(),
    }))
}

#[utoipa::path(post, path = "/api/v1/auth/resend-verification", request_body = EmailRequest, responses((status = 200, body = MessageResponse)))]
pub async fn resend_verification(
    State(state): State<AppState>,
    Json(input): Json<EmailRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    let email = validate_email(&input.email)?;
    state
        .rate_limiter
        .check(format!("resend:{email}"), 3, StdDuration::from_secs(3600))
        .await?;
    let conn = state.connection().await?;
    let mut rows = conn
        .query(
            "SELECT id FROM users WHERE email = ?1 AND email_verified_at IS NULL",
            [email.clone()],
        )
        .await?;
    if let Some(row) = rows.next().await? {
        let user_id: String = row.get(0)?;
        let (token, token_hash) = new_token();
        insert_email_token(&conn, &user_id, "verify_email", &token_hash, 24).await?;
        state
            .email
            .send(EmailMessage {
                to: email,
                subject: "重新验证你的知余账户".into(),
                text: format!(
                    "请在 24 小时内打开以下链接完成验证：\n{}/verify-email?token={token}",
                    state.config.public_base_url
                ),
            })
            .await
            .map_err(ApiError::internal)?;
    }
    Ok(Json(generic_email_message()))
}

#[utoipa::path(post, path = "/api/v1/auth/login", request_body = LoginRequest, responses((status = 200, body = UserView), (status = 401, body = crate::error::ErrorBody)))]
pub async fn login(
    State(state): State<AppState>,
    Json(input): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    let email = validate_email(&input.email)?;
    state
        .rate_limiter
        .check(format!("login:{email}"), 10, StdDuration::from_secs(900))
        .await?;
    let conn = state.connection().await?;
    let mut rows = conn
        .query(
            "SELECT id, password_hash, timezone, email_verified_at FROM users WHERE email = ?1",
            [email.clone()],
        )
        .await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| ApiError::unauthorized("邮箱或密码不正确"))?;
    let user_id: String = row.get(0)?;
    let password_hash: String = row.get(1)?;
    let timezone: String = row.get(2)?;
    let verified_at: Option<String> = row.get(3)?;
    if !verify_password(input.password, password_hash).await? {
        return Err(ApiError::unauthorized("邮箱或密码不正确"));
    }
    if verified_at.is_none() {
        return Err(ApiError::forbidden("请先完成邮箱验证"));
    }

    let (token, token_hash) = new_token();
    let now = Utc::now();
    conn.execute(
        "INSERT INTO sessions(id, user_id, token_hash, created_at, last_seen_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
        params![Uuid::now_v7().to_string(), user_id.clone(), token_hash, now.to_rfc3339(), (now + Duration::days(SESSION_DAYS)).to_rfc3339()],
    )
    .await?;
    let user = UserView {
        id: user_id,
        email,
        timezone,
        email_verified: true,
    };
    let mut response = Json(user).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&session_cookie_header(&state.config, &token, false))
            .map_err(ApiError::internal)?,
    );
    Ok(response)
}

#[utoipa::path(post, path = "/api/v1/auth/logout", responses((status = 200, body = MessageResponse)), security(("cookieAuth" = [])))]
pub async fn logout(State(state): State<AppState>, user: AuthUser) -> Result<Response, ApiError> {
    let conn = state.connection().await?;
    conn.execute(
        "DELETE FROM sessions WHERE token_hash = ?1 AND user_id = ?2",
        params![user.session_hash, user.id],
    )
    .await?;
    let mut response = Json(MessageResponse {
        message: "已退出登录".into(),
    })
    .into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&session_cookie_header(&state.config, "", true))
            .map_err(ApiError::internal)?,
    );
    Ok(response)
}

#[utoipa::path(get, path = "/api/v1/auth/me", responses((status = 200, body = UserView)), security(("cookieAuth" = [])))]
pub async fn me(user: AuthUser) -> Json<UserView> {
    Json(UserView {
        id: user.id,
        email: user.email,
        timezone: user.timezone,
        email_verified: true,
    })
}

#[utoipa::path(post, path = "/api/v1/auth/forgot-password", request_body = EmailRequest, responses((status = 200, body = MessageResponse)))]
pub async fn forgot_password(
    State(state): State<AppState>,
    Json(input): Json<EmailRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    let email = validate_email(&input.email)?;
    state
        .rate_limiter
        .check(format!("forgot:{email}"), 3, StdDuration::from_secs(3600))
        .await?;
    let conn = state.connection().await?;
    let mut rows = conn
        .query(
            "SELECT id FROM users WHERE email = ?1 AND email_verified_at IS NOT NULL",
            [email.clone()],
        )
        .await?;
    if let Some(row) = rows.next().await? {
        let user_id: String = row.get(0)?;
        let (token, token_hash) = new_token();
        insert_email_token(&conn, &user_id, "reset_password", &token_hash, 1).await?;
        state
            .email
            .send(EmailMessage {
                to: email,
                subject: "重置你的知余密码".into(),
                text: format!(
                    "请在 1 小时内打开以下链接重置密码：\n{}/reset-password?token={token}",
                    state.config.public_base_url
                ),
            })
            .await
            .map_err(ApiError::internal)?;
    }
    Ok(Json(generic_email_message()))
}

#[utoipa::path(post, path = "/api/v1/auth/reset-password", request_body = ResetPasswordRequest, responses((status = 200, body = MessageResponse)))]
pub async fn reset_password(
    State(state): State<AppState>,
    Json(input): Json<ResetPasswordRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    validate_password(&input.new_password)?;
    let password_hash = hash_password(input.new_password).await?;
    consume_email_token(&state, &input.token, "reset_password", Some(password_hash)).await?;
    Ok(Json(MessageResponse {
        message: "密码已重置，请重新登录".into(),
    }))
}

async fn insert_email_token(
    conn: &libsql::Connection,
    user_id: &str,
    purpose: &str,
    token_hash: &str,
    hours: i64,
) -> Result<(), ApiError> {
    let now = Utc::now();
    conn.execute(
        "INSERT INTO email_tokens(id, user_id, purpose, token_hash, expires_at, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![Uuid::now_v7().to_string(), user_id, purpose, token_hash, (now + Duration::hours(hours)).to_rfc3339(), now.to_rfc3339()],
    )
    .await?;
    Ok(())
}

async fn consume_email_token(
    state: &AppState,
    token: &str,
    purpose: &str,
    new_password_hash: Option<String>,
) -> Result<(), ApiError> {
    let token_hash = hash_token(token);
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    let now = Utc::now().to_rfc3339();
    let mut rows = tx
        .query(
            "SELECT id, user_id, consumed_at FROM email_tokens WHERE token_hash = ?1 AND purpose = ?2 AND expires_at > ?3",
            params![token_hash, purpose, now.clone()],
        )
        .await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| ApiError::bad_request("invalid_token", "链接无效或已过期"))?;
    let token_id: String = row.get(0)?;
    let user_id: String = row.get(1)?;
    let consumed_at: Option<String> = row.get(2)?;
    drop(rows);
    if consumed_at.is_some() {
        if purpose == "verify_email" {
            let mut verified = tx
                .query(
                    "SELECT 1 FROM users WHERE id = ?1 AND email_verified_at IS NOT NULL",
                    [user_id],
                )
                .await?;
            if verified.next().await?.is_some() {
                tx.rollback().await?;
                return Ok(());
            }
        }
        return Err(ApiError::bad_request("invalid_token", "链接无效或已过期"));
    }
    tx.execute(
        "UPDATE email_tokens SET consumed_at = ?1 WHERE id = ?2",
        params![now.clone(), token_id],
    )
    .await?;
    if purpose == "verify_email" {
        tx.execute(
            "UPDATE users SET email_verified_at = COALESCE(email_verified_at, ?1), updated_at = ?1 WHERE id = ?2",
            params![now, user_id],
        )
        .await?;
    } else if let Some(password_hash) = new_password_hash {
        tx.execute(
            "UPDATE users SET password_hash = ?1, updated_at = ?2 WHERE id = ?3",
            params![password_hash, now, user_id.clone()],
        )
        .await?;
        tx.execute("DELETE FROM sessions WHERE user_id = ?1", [user_id])
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

fn generic_email_message() -> MessageResponse {
    MessageResponse {
        message: "如果账户存在，相关邮件已发送".into(),
    }
}

/// 桌面端单机模式的本地用户引导。
///
/// 桌面版没有注册与邮箱验证的意义：首次启动静默建一个已验证的本地用户，之后每次
/// 启动复用它并签发新会话。返回值是可直接下发的 `Set-Cookie` 头，调用方负责让
/// webview 带上它——此后所有请求都与网页版走同一套 `AuthUser` 提取逻辑。
pub async fn ensure_local_session(state: &AppState, email: &str) -> Result<String, ApiError> {
    let conn = state.connection().await?;
    let now = Utc::now();
    let mut rows = conn
        .query("SELECT id FROM users WHERE email = ?1", [email])
        .await?;
    let user_id = match rows.next().await? {
        Some(row) => row.get::<String>(0)?,
        None => {
            // 本地用户从不通过密码登录，随机口令只是为了满足 NOT NULL 且不可猜。
            let mut secret = [0_u8; 32];
            rand::rngs::OsRng.fill_bytes(&mut secret);
            let password_hash = hash_password(URL_SAFE_NO_PAD.encode(secret)).await?;
            let id = Uuid::now_v7().to_string();
            conn.execute(
                "INSERT INTO users(id, email, password_hash, timezone, email_verified_at, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?5)",
                params![
                    id.clone(),
                    email,
                    password_hash,
                    "Asia/Shanghai",
                    now.to_rfc3339()
                ],
            )
            .await?;
            id
        }
    };
    // 单机模式下只该存在一条会话。旧 token 的明文没有留存、无法复用，所以每次启动
    // 都要重新签发；但必须同时清掉上一条，否则每启动一次就在表里堆一行永不回收的
    // 凭据哈希。
    conn.execute("DELETE FROM sessions WHERE user_id = ?1", [user_id.clone()])
        .await?;
    let (token, token_hash) = new_token();
    conn.execute(
        "INSERT INTO sessions(id, user_id, token_hash, created_at, last_seen_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
        params![
            Uuid::now_v7().to_string(),
            user_id,
            token_hash,
            now.to_rfc3339(),
            (now + Duration::days(SESSION_DAYS)).to_rfc3339()
        ],
    )
    .await?;
    Ok(session_cookie_header(&state.config, &token, false))
}

async fn hash_password(password: String) -> Result<String, ApiError> {
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(ApiError::internal)
    })
    .await
    .map_err(ApiError::internal)?
}

async fn verify_password(password: String, hash: String) -> Result<bool, ApiError> {
    tokio::task::spawn_blocking(move || {
        let parsed = PasswordHash::new(&hash).map_err(ApiError::internal)?;
        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok())
    })
    .await
    .map_err(ApiError::internal)?
}

fn new_token() -> (String, String) {
    let mut bytes = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let token = URL_SAFE_NO_PAD.encode(bytes);
    let hash = hash_token(&token);
    (token, hash)
}

fn hash_token(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find_map(|(key, value)| (key == name).then(|| value.to_owned()))
}

pub(crate) fn session_cookie_header(
    config: &crate::config::Config,
    token: &str,
    clear: bool,
) -> String {
    let mut cookie = format!(
        "{}={token}; Path=/; HttpOnly; SameSite=Lax",
        config.cookie_name()
    );
    if config.is_production() {
        cookie.push_str("; Secure");
    }
    if clear {
        cookie.push_str("; Max-Age=0");
    } else {
        cookie.push_str(&format!("; Max-Age={}", SESSION_DAYS * 86_400));
    }
    cookie
}
