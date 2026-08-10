use std::time::Duration as StdDuration;

use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use axum::{
    Form, Json,
    extract::{FromRequestParts, State, rejection::FormRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use libsql::{TransactionBehavior, params};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use utoipa::ToSchema;
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
const SESSION_MAX_AGE_SECONDS: i64 = SESSION_DAYS * 86_400;
const API_KEY_DAYS: i64 = 3650;
const HANDOFF_TICKET_SECONDS: i64 = 60;

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionCookieView {
    name: String,
    value: String,
    max_age: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionFromKeyResponse {
    #[serde(flatten)]
    user: UserView,
    session_cookie: SessionCookieView,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HandoffTicketResponse {
    ticket: String,
}

#[derive(Debug, Deserialize)]
pub struct HandoffTicketForm {
    ticket: String,
}

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: String,
    pub email: String,
    pub timezone: String,
    pub session_hash: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthMechanism {
    Session,
    ApiKey,
}

#[derive(Debug, Clone)]
pub(crate) struct AuthContext {
    pub user: AuthUser,
    pub mechanism: AuthMechanism,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if let Some(context) = parts.extensions.get::<AuthContext>() {
            return Ok(context.user.clone());
        }
        if let Some(token) = cookie_value(&parts.headers, state.config.cookie_name())
            && let Ok(user) = authenticate_session_token(state, &token).await
        {
            return Ok(user);
        }
        if let Some(token) = bearer_token(&parts.headers) {
            return authenticate_api_key(state, token).await;
        }
        Err(ApiError::unauthorized("请先登录"))
    }
}

pub(crate) async fn authenticate_session_token(
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
        session_hash: Some(token_hash.clone()),
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

pub(crate) async fn authenticate_api_key(
    state: &AppState,
    token: &str,
) -> Result<AuthUser, ApiError> {
    let token_hash = hash_token(token);
    let conn = state.connection().await?;
    let now = Utc::now();
    let mut rows = conn
        .query(
            "SELECT u.id, u.email, u.timezone FROM api_keys k JOIN users u ON u.id = k.user_id WHERE k.token_hash = ?1 AND k.expires_at > ?2 AND u.email_verified_at IS NOT NULL",
            params![token_hash.clone(), now.to_rfc3339()],
        )
        .await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| ApiError::unauthorized("API 密钥无效或已过期"))?;
    let user = AuthUser {
        id: row.get(0)?,
        email: row.get(1)?,
        timezone: row.get(2)?,
        session_hash: None,
    };
    drop(rows);
    conn.execute(
        "UPDATE api_keys SET last_used_at = ?1 WHERE token_hash = ?2",
        params![now.to_rfc3339(), token_hash],
    )
    .await?;
    Ok(user)
}

#[utoipa::path(post, path = "/api/v1/auth/register", request_body = RegisterRequest, responses((status = 201, body = MessageResponse), (status = 422, body = crate::error::ErrorBody)))]
pub async fn register(
    State(state): State<AppState>,
    Json(input): Json<RegisterRequest>,
) -> Result<(StatusCode, Json<MessageResponse>), ApiError> {
    require_email_delivery(&state)?;
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
    require_email_delivery(&state)?;
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
    require_email_delivery(&state)?;
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
    drop(row);
    drop(rows);
    drop(conn);
    if !verify_password(input.password, password_hash).await? {
        return Err(ApiError::unauthorized("邮箱或密码不正确"));
    }
    if verified_at.is_none() {
        return Err(ApiError::forbidden("请先完成邮箱验证"));
    }
    let auth_user = AuthUser {
        id: user_id,
        email,
        timezone,
        session_hash: None,
    };
    let (user, token) = create_session(&state, auth_user).await?;
    let mut response = Json(user).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&session_cookie_header(&state.config, &token, false))
            .map_err(ApiError::internal)?,
    );
    Ok(response)
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/session-from-key",
    responses(
        (status = 200, body = SessionFromKeyResponse),
        (status = 401, body = crate::error::ErrorBody)
    ),
    security(("bearerAuth" = []))
)]
pub async fn session_from_key(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    // 这里故意不使用 AuthUser 提取器：它会先接受 session cookie，而这个交换端点
    // 必须只认显式 Bearer 凭证。
    let api_key =
        bearer_token(&headers).ok_or_else(|| ApiError::unauthorized("请提供 API 密钥"))?;
    let user = authenticate_api_key(&state, api_key).await?;
    let (user, token) = create_session(&state, user).await?;
    let cookie_name = state.config.cookie_name().to_owned();
    let body = SessionFromKeyResponse {
        user,
        session_cookie: SessionCookieView {
            name: cookie_name,
            value: token.clone(),
            max_age: SESSION_MAX_AGE_SECONDS,
        },
    };
    let mut response = Json(body).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&session_cookie_header(&state.config, &token, false))
            .map_err(ApiError::internal)?,
    );
    Ok(response)
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/handoff-tickets",
    responses(
        (status = 200, body = HandoffTicketResponse),
        (status = 401, body = crate::error::ErrorBody)
    ),
    security(("bearerAuth" = []))
)]
pub async fn create_handoff_ticket(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    // 与 session-from-key 一样，这里不能使用会优先接受 session cookie 的 AuthUser
    // 提取器。桌面交接票据只允许由显式 Bearer api-key 签发。
    let api_key =
        bearer_token(&headers).ok_or_else(|| ApiError::unauthorized("请提供 API 密钥"))?;
    let user = authenticate_api_key(&state, api_key).await?;
    let (ticket, token_hash) = new_token();
    let now = Utc::now();
    let conn = state.connection().await?;
    conn.execute(
        "DELETE FROM handoff_tickets WHERE expires_at <= ?1",
        [now.to_rfc3339()],
    )
    .await?;
    conn.execute(
        "INSERT INTO handoff_tickets(id, user_id, token_hash, expires_at, consumed_at, created_at) VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
        params![
            Uuid::now_v7().to_string(),
            user.id,
            token_hash,
            (now + Duration::seconds(HANDOFF_TICKET_SECONDS)).to_rfc3339(),
            now.to_rfc3339()
        ],
    )
    .await?;
    let mut response = Json(HandoffTicketResponse { ticket }).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

pub async fn desktop_handoff(
    State(state): State<AppState>,
    form: Result<Form<HandoffTicketForm>, FormRejection>,
) -> Result<Response, ApiError> {
    let Ok(Form(input)) = form else {
        return Ok(desktop_handoff_redirect(None));
    };
    if input.ticket.is_empty() {
        return Ok(desktop_handoff_redirect(None));
    }

    let token_hash = hash_token(&input.ticket);
    let now = Utc::now();
    let now_text = now.to_rfc3339();
    let conn = state.connection().await?;
    // Immediate 事务在读取前先取得写锁；同一张票据的并发请求只能有一个看到
    // consumed_at IS NULL，消费标记与 session 创建也会一起提交或回滚。
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    tx.execute(
        "DELETE FROM handoff_tickets WHERE expires_at <= ?1",
        [now_text.clone()],
    )
    .await?;
    let mut rows = tx
        .query(
            "SELECT h.id, u.id, u.email, u.timezone FROM handoff_tickets h JOIN users u ON u.id = h.user_id WHERE h.token_hash = ?1 AND h.expires_at > ?2 AND h.consumed_at IS NULL AND u.email_verified_at IS NOT NULL",
            params![token_hash, now_text.clone()],
        )
        .await?;
    let Some(row) = rows.next().await? else {
        drop(rows);
        tx.commit().await?;
        return Ok(desktop_handoff_redirect(None));
    };
    let ticket_id: String = row.get(0)?;
    let user = AuthUser {
        id: row.get(1)?,
        email: row.get(2)?,
        timezone: row.get(3)?,
        session_hash: None,
    };
    drop(row);
    drop(rows);
    let changed = tx
        .execute(
            "UPDATE handoff_tickets SET consumed_at = ?1 WHERE id = ?2 AND consumed_at IS NULL AND expires_at > ?1",
            params![now_text, ticket_id],
        )
        .await?;
    if changed != 1 {
        tx.commit().await?;
        return Ok(desktop_handoff_redirect(None));
    }
    let session = new_session(user, now);
    tx.execute(
        "INSERT INTO sessions(id, user_id, token_hash, created_at, last_seen_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
        params![
            session.id,
            session.user.id,
            session.token_hash,
            session.created_at,
            session.expires_at
        ],
    )
    .await?;
    tx.commit().await?;
    Ok(desktop_handoff_redirect(Some(session_cookie_header(
        &state.config,
        &session.token,
        false,
    ))))
}

fn desktop_handoff_redirect(cookie: Option<String>) -> Response {
    let mut response = StatusCode::SEE_OTHER.into_response();
    let headers = response.headers_mut();
    headers.insert(header::LOCATION, HeaderValue::from_static("/"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    if let Some(cookie) = cookie
        && let Ok(value) = HeaderValue::from_str(&cookie)
    {
        headers.insert(header::SET_COOKIE, value);
    }
    response
}

struct NewSession {
    id: String,
    user: UserView,
    token: String,
    token_hash: String,
    created_at: String,
    expires_at: String,
}

fn new_session(user: AuthUser, now: chrono::DateTime<Utc>) -> NewSession {
    let (token, token_hash) = new_token();
    NewSession {
        id: Uuid::now_v7().to_string(),
        user: UserView {
            id: user.id,
            email: user.email,
            timezone: user.timezone,
            email_verified: true,
        },
        token,
        token_hash,
        created_at: now.to_rfc3339(),
        expires_at: (now + Duration::days(SESSION_DAYS)).to_rfc3339(),
    }
}

async fn create_session(state: &AppState, user: AuthUser) -> Result<(UserView, String), ApiError> {
    let session = new_session(user, Utc::now());
    state
        .connection()
        .await?
        .execute(
            "INSERT INTO sessions(id, user_id, token_hash, created_at, last_seen_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
            params![
                session.id,
                session.user.id.clone(),
                session.token_hash,
                session.created_at,
                session.expires_at
            ],
        )
        .await?;
    Ok((session.user, session.token))
}

#[utoipa::path(post, path = "/api/v1/auth/logout", responses((status = 200, body = MessageResponse)), security(("cookieAuth" = [])))]
pub async fn logout(State(state): State<AppState>, user: AuthUser) -> Result<Response, ApiError> {
    if let Some(session_hash) = user.session_hash {
        let conn = state.connection().await?;
        conn.execute(
            "DELETE FROM sessions WHERE token_hash = ?1 AND user_id = ?2",
            params![session_hash, user.id],
        )
        .await?;
    }
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
    require_email_delivery(&state)?;
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
    require_email_delivery(&state)?;
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

fn require_email_delivery(state: &AppState) -> Result<(), ApiError> {
    if state.config.email_delivery_available() {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "email_unavailable",
            "self-host 模式未配置邮件发送服务，此功能不可用",
        ))
    }
}

/// 为机器调用方签发长期 API 密钥。返回值是唯一一次可见的明文，数据库只保存哈希。
///
/// 重复签发不会撤销任何仍有效的密钥；只清理已过期项，避免一次 CLI 重跑让正在使用
/// 旧密钥的集成立刻掉线。
pub async fn issue_api_key(state: &AppState, email: &str) -> Result<String, ApiError> {
    let email = validate_email(email)?;
    let conn = state.connection().await?;
    let now = Utc::now();
    let mut rows = conn
        .query("SELECT id FROM users WHERE email = ?1", [email.clone()])
        .await?;
    let user_id = match rows.next().await? {
        Some(row) => row.get::<String>(0)?,
        None => {
            // 机器用户不通过密码登录，随机口令只是为了满足 NOT NULL 且不可猜。
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
    conn.execute(
        "DELETE FROM api_keys WHERE user_id = ?1 AND expires_at <= ?2",
        params![user_id.clone(), now.to_rfc3339()],
    )
    .await?;
    let (token, token_hash) = new_token();
    conn.execute(
        "INSERT INTO api_keys(id, user_id, token_hash, created_at, last_used_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
        params![
            Uuid::now_v7().to_string(),
            user_id,
            token_hash,
            now.to_rfc3339(),
            (now + Duration::days(API_KEY_DAYS)).to_rfc3339()
        ],
    )
    .await?;
    Ok(token)
}

/// 为已有用户设置新密码，并撤销该用户的所有网页登录会话。
///
/// API 密钥属于机器凭证，不随人的密码变更撤销。
pub async fn set_password(state: &AppState, email: &str, password: String) -> Result<(), ApiError> {
    let email = validate_email(email)?;
    validate_password(&password)?;
    let password_hash = hash_password(password).await?;
    let conn = state.connection().await?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    let mut rows = tx
        .query("SELECT id FROM users WHERE email = ?1", [email.clone()])
        .await?;
    let user_id = rows
        .next()
        .await?
        .map(|row| row.get::<String>(0))
        .transpose()?
        .ok_or_else(|| ApiError::not_found(format!("用户不存在：{email}")))?;
    drop(rows);
    let now = Utc::now().to_rfc3339();
    tx.execute(
        "UPDATE users SET password_hash = ?1, updated_at = ?2 WHERE id = ?3",
        params![password_hash, now, user_id.clone()],
    )
    .await?;
    tx.execute("DELETE FROM sessions WHERE user_id = ?1", [user_id])
        .await?;
    tx.commit().await?;
    Ok(())
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

pub(crate) fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find_map(|(key, value)| (key == name).then(|| value.to_owned()))
}

pub(crate) fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    (scheme.eq_ignore_ascii_case("Bearer") && !token.trim().is_empty()).then(|| token.trim())
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
        cookie.push_str(&format!("; Max-Age={SESSION_MAX_AGE_SECONDS}"));
    }
    cookie
}
