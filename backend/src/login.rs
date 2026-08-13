use std::{
    collections::HashMap,
    error::Error,
    fmt::{self, Display, Formatter},
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use sqlx::{FromRow, Sqlite, SqlitePool, Transaction};

use crate::{
    email::{AuthenticationLink, EmailSender, LoginLinkEmail},
    identity::UserWithPasswordRecord,
    invitations::ActiveUser,
    password,
    security::{CsrfToken, SecretKey, SecretToken, SessionCookieBuilder, TokenDomain},
};

const REQUEST_ACTION: &str = "auth.login_link.requested";
const REQUEST_LIMITED_ACTION: &str = "auth.login_link.request.rate_limited";
const REQUEST_DELIVERY_FAILED_ACTION: &str = "auth.login_link.request.delivery_failed";
const CONSUME_SUCCEEDED_ACTION: &str = "auth.login_link.consume.succeeded";
const CONSUME_FAILED_ACTION: &str = "auth.login_link.consume.failed";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoginRateLimitKey<'a> {
    Ip(&'a str),
    Email(&'a str),
}

pub trait LoginRateLimiter: Send + Sync {
    fn allow(&self, key: LoginRateLimitKey<'_>) -> bool;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AllowAllLoginRateLimiter;

impl LoginRateLimiter for AllowAllLoginRateLimiter {
    fn allow(&self, _key: LoginRateLimitKey<'_>) -> bool {
        true
    }
}

pub struct FixedWindowLoginRateLimiter {
    maximum_attempts: u32,
    window_seconds: i64,
    buckets: Mutex<HashMap<String, RateLimitBucket>>,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl FixedWindowLoginRateLimiter {
    pub fn new(maximum_attempts: u32, window_seconds: i64) -> Self {
        assert!(maximum_attempts > 0, "maximum attempts must be positive");
        assert!(window_seconds > 0, "rate-limit window must be positive");
        Self {
            maximum_attempts,
            window_seconds,
            buckets: Mutex::new(HashMap::new()),
            clock: Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system clock is before Unix epoch")
                    .as_secs() as i64
            }),
        }
    }

    pub fn new_at(maximum_attempts: u32, window_seconds: i64, now: i64) -> Self {
        let mut limiter = Self::new(maximum_attempts, window_seconds);
        limiter.clock = Arc::new(move || now);
        limiter
    }
}

impl LoginRateLimiter for FixedWindowLoginRateLimiter {
    fn allow(&self, key: LoginRateLimitKey<'_>) -> bool {
        let bucket_key = match key {
            LoginRateLimitKey::Ip(ip) => format!("ip:{ip}"),
            LoginRateLimitKey::Email(email) => format!("email:{email}"),
        };
        let now = (self.clock)();
        let mut buckets = self.buckets.lock().unwrap();
        let bucket = buckets.entry(bucket_key).or_insert(RateLimitBucket {
            window_started_at: now,
            attempts: 0,
        });
        if now - bucket.window_started_at >= self.window_seconds {
            bucket.window_started_at = now;
            bucket.attempts = 0;
        }
        if bucket.attempts >= self.maximum_attempts {
            return false;
        }
        bucket.attempts += 1;
        true
    }
}

struct RateLimitBucket {
    window_started_at: i64,
    attempts: u32,
}

pub trait LoginFlow: Send + Sync {
    fn request_link<'a>(
        &'a self,
        command: RequestLoginLink,
    ) -> Pin<Box<dyn Future<Output = Result<(), RequestLoginLinkError>> + Send + 'a>>;

    fn consume_link<'a>(
        &'a self,
        command: ConsumeLoginLink,
    ) -> Pin<Box<dyn Future<Output = Result<ConsumedLoginLink, ConsumeLoginLinkError>> + Send + 'a>>;

    fn dev_login<'a>(
        &'a self,
        _command: DevLogin,
    ) -> Pin<Box<dyn Future<Output = Result<DevLoginResult, DevLoginError>> + Send + 'a>> {
        Box::pin(async { Err(DevLoginError::Unavailable) })
    }

    fn authenticate_password<'a>(
        &'a self,
        _command: PasswordLoginCommand,
    ) -> Pin<Box<dyn Future<Output = Result<PasswordLoginResult, PasswordLoginError>> + Send + 'a>>
    {
        Box::pin(async { Err(PasswordLoginError::Unsupported) })
    }
}

#[derive(Clone)]
pub struct LoginService<E> {
    pool: SqlitePool,
    secret_key: SecretKey,
    login_token_lifetime_seconds: i64,
    session_lifetime_seconds: i64,
    login_url: Arc<str>,
    email_sender: Arc<E>,
    limiter: Arc<dyn LoginRateLimiter>,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    is_secure: bool,
}

impl<E> LoginService<E>
where
    E: EmailSender + Send + Sync + 'static,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pool: SqlitePool,
        secret_key: SecretKey,
        login_token_lifetime_seconds: i64,
        session_lifetime_seconds: i64,
        login_url: impl Into<Arc<str>>,
        email_sender: Arc<E>,
        limiter: Arc<dyn LoginRateLimiter>,
        is_secure: bool,
    ) -> Self {
        Self {
            pool,
            secret_key,
            login_token_lifetime_seconds,
            session_lifetime_seconds,
            login_url: login_url.into(),
            email_sender,
            limiter,
            clock: Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system clock is before Unix epoch")
                    .as_secs() as i64
            }),
            is_secure,
        }
    }

    async fn authenticate_password_impl(
        &self,
        command: PasswordLoginCommand,
    ) -> Result<PasswordLoginResult, PasswordLoginError> {
        let now = (self.clock)();
        let normalized_email = normalize_email(&command.email);

        let ip_allowed = self
            .limiter
            .allow(LoginRateLimitKey::Ip(&command.client_ip));
        let email_allowed = self
            .limiter
            .allow(LoginRateLimitKey::Email(&normalized_email));
        if !ip_allowed || !email_allowed {
            insert_audit(
                &self.pool,
                None,
                "auth.password_login.rate_limited",
                "login",
                None,
                r#"{"result":"rejected"}"#,
                now,
            )
            .await?;
            return Err(PasswordLoginError::RateLimited);
        }

        let mut user = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(|e| PasswordLoginError::Database(e.to_string()))?;

        let record = sqlx::query_as::<_, UserWithPasswordRecord>(
            "SELECT id, normalized_email, display_name, status, created_at, password_hash
             FROM users WHERE normalized_email = ? AND status = 'active'",
        )
        .bind(&normalized_email)
        .fetch_optional(&mut *user)
        .await
        .map_err(|e| PasswordLoginError::Database(e.to_string()))?;

        let Some(record) = record else {
            user.commit()
                .await
                .map_err(|e| PasswordLoginError::Database(e.to_string()))?;
            audit_generic_request(&self.pool, "auth.password_login.failed", now).await?;
            return Err(PasswordLoginError::InvalidCredentials);
        };

        let password_hash = match &record.password_hash {
            Some(hash) => hash.clone(),
            None => {
                user.commit()
                    .await
                    .map_err(|e| PasswordLoginError::Database(e.to_string()))?;
                return Err(PasswordLoginError::PasswordNotSet);
            }
        };

        let valid = password::verify_password(&command.password, &password_hash)
            .map_err(|e| PasswordLoginError::Database(e.to_string()))?;

        if !valid {
            user.commit()
                .await
                .map_err(|e| PasswordLoginError::Database(e.to_string()))?;
            audit_generic_request(&self.pool, "auth.password_login.failed", now).await?;
            return Err(PasswordLoginError::InvalidCredentials);
        }

        let session_token = self.secret_key.generate_token();
        let session_hash = self
            .secret_key
            .hash_token(TokenDomain::Session, &session_token);

        sqlx::query(
            "INSERT INTO sessions (
                user_id, session_hash, expires_at, revoked_at, created_at, last_seen_at
             ) VALUES (?, ?, ?, NULL, ?, ?)",
        )
        .bind(record.id)
        .bind(session_hash.as_bytes().as_slice())
        .bind(now + self.session_lifetime_seconds)
        .bind(now)
        .bind(now)
        .execute(&mut *user)
        .await
        .map_err(|e| PasswordLoginError::Database(e.to_string()))?;

        sqlx::query("UPDATE users SET last_login_at = ? WHERE id = ?")
            .bind(now)
            .bind(record.id)
            .execute(&mut *user)
            .await
            .map_err(|e| PasswordLoginError::Database(e.to_string()))?;

        insert_audit_in_transaction(
            &mut user,
            Some(record.id),
            "auth.password_login.succeeded",
            "login",
            None,
            r#"{"result":"authenticated"}"#,
            now,
        )
        .await
        .map_err(|e| PasswordLoginError::Database(e.to_string()))?;

        user.commit()
            .await
            .map_err(|e| PasswordLoginError::Database(e.to_string()))?;

        let csrf_token = self.secret_key.generate_csrf_token(&session_token);

        tracing::info!(
            "password login succeeded for user {}",
            record.normalized_email
        );
        Ok(PasswordLoginResult {
            user: ActiveUser {
                id: record.id,
                email: record.normalized_email,
                display_name: record.display_name,
                status: "active",
                is_superadmin: false,
            },
            session_token,
            csrf_token,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_at(
        pool: SqlitePool,
        secret_key: SecretKey,
        login_token_lifetime_seconds: i64,
        session_lifetime_seconds: i64,
        login_url: impl Into<Arc<str>>,
        email_sender: Arc<E>,
        limiter: Arc<dyn LoginRateLimiter>,
        now: i64,
        is_secure: bool,
    ) -> Self {
        let mut service = Self::new(
            pool,
            secret_key,
            login_token_lifetime_seconds,
            session_lifetime_seconds,
            login_url,
            email_sender,
            limiter,
            is_secure,
        );
        service.clock = Arc::new(move || now);
        service
    }

    async fn request(&self, command: RequestLoginLink) -> Result<(), RequestLoginLinkError> {
        let now = (self.clock)();
        let normalized_email = normalize_email(&command.email);
        let ip_allowed = self
            .limiter
            .allow(LoginRateLimitKey::Ip(&command.client_ip));
        let email_allowed = self
            .limiter
            .allow(LoginRateLimitKey::Email(&normalized_email));
        if !ip_allowed || !email_allowed {
            insert_audit(
                &self.pool,
                None,
                REQUEST_LIMITED_ACTION,
                "login",
                None,
                r#"{"result":"rejected"}"#,
                now,
            )
            .await?;
            tracing::warn!("login link request rate limited");
            return Err(RequestLoginLinkError::RateLimited);
        }

        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let user = sqlx::query_as::<_, RequestUser>(
            "SELECT id, normalized_email FROM users
             WHERE normalized_email = ? AND status = 'active'",
        )
        .bind(&normalized_email)
        .fetch_optional(&mut *transaction)
        .await?;

        let Some(user) = user else {
            audit_generic_request_in_transaction(&mut transaction, now).await?;
            transaction.commit().await?;
            tracing::info!("login link request accepted");
            return Ok(());
        };

        let token = self.secret_key.generate_token();
        let token_hash = self.secret_key.hash_token(TokenDomain::Login, &token);
        sqlx::query(
            "UPDATE login_tokens SET revoked_at = ?
             WHERE user_id = ? AND consumed_at IS NULL AND revoked_at IS NULL",
        )
        .bind(now)
        .bind(user.id)
        .execute(&mut *transaction)
        .await?;
        let inserted = sqlx::query(
            "INSERT INTO login_tokens (
                user_id, token_hash, expires_at, revoked_at, consumed_at, created_at
             ) VALUES (?, ?, ?, NULL, NULL, ?)",
        )
        .bind(user.id)
        .bind(token_hash.as_bytes().as_slice())
        .bind(now + self.login_token_lifetime_seconds)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        audit_generic_request_in_transaction(&mut transaction, now).await?;
        transaction.commit().await?;

        let authentication_link =
            AuthenticationLink::new(format!("{}?token={}", self.login_url, token.expose()));
        if self
            .email_sender
            .send_login_link(LoginLinkEmail::new(
                user.normalized_email,
                authentication_link,
            ))
            .await
            .is_err()
        {
            sqlx::query(
                "UPDATE login_tokens SET revoked_at = ?
                 WHERE id = ? AND consumed_at IS NULL AND revoked_at IS NULL",
            )
            .bind(now)
            .bind(inserted.last_insert_rowid())
            .execute(&self.pool)
            .await?;
            insert_audit(
                &self.pool,
                None,
                REQUEST_DELIVERY_FAILED_ACTION,
                "login",
                None,
                r#"{"reason":"delivery_failed"}"#,
                now,
            )
            .await?;
            tracing::error!("login link delivery failed");
            return Ok(());
        }

        tracing::info!("login link request accepted");
        Ok(())
    }

    async fn consume(
        &self,
        command: ConsumeLoginLink,
    ) -> Result<ConsumedLoginLink, ConsumeLoginLinkError> {
        let now = (self.clock)();
        let Some(token) = SecretToken::parse(command.token) else {
            audit_consume_failure(&self.pool, None, "malformed_token", now).await?;
            return Err(ConsumeLoginLinkError::Invalid);
        };
        let token_hash = self.secret_key.hash_token(TokenDomain::Login, &token);
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let record = sqlx::query_as::<_, LoginTokenUser>(
            "SELECT login_tokens.id AS token_id, login_tokens.expires_at,
                    login_tokens.revoked_at, login_tokens.consumed_at,
                    users.id AS user_id, users.normalized_email, users.display_name,
                    users.status, users.is_superadmin
             FROM login_tokens
             JOIN users ON users.id = login_tokens.user_id
             WHERE login_tokens.token_hash = ?",
        )
        .bind(token_hash.as_bytes().as_slice())
        .fetch_optional(&mut *transaction)
        .await?;

        let Some(record) = record else {
            audit_consume_failure_in_transaction(&mut transaction, None, "token_not_found", now)
                .await?;
            transaction.commit().await?;
            return Err(ConsumeLoginLinkError::Invalid);
        };
        if let Some(reason) = record.rejection_reason(now) {
            audit_consume_failure_in_transaction(
                &mut transaction,
                Some(record.token_id),
                reason,
                now,
            )
            .await?;
            transaction.commit().await?;
            return Err(ConsumeLoginLinkError::Invalid);
        }

        sqlx::query(
            "UPDATE login_tokens SET consumed_at = ?
             WHERE id = ? AND consumed_at IS NULL",
        )
        .bind(now)
        .bind(record.token_id)
        .execute(&mut *transaction)
        .await?;

        if let Some(prior_session) = command.prior_session_token.and_then(SecretToken::parse) {
            let prior_hash = self
                .secret_key
                .hash_token(TokenDomain::Session, &prior_session);
            sqlx::query(
                "UPDATE sessions SET revoked_at = ?
                 WHERE session_hash = ? AND revoked_at IS NULL",
            )
            .bind(now)
            .bind(prior_hash.as_bytes().as_slice())
            .execute(&mut *transaction)
            .await?;
        }

        let session_token = self.secret_key.generate_token();
        let session_hash = self
            .secret_key
            .hash_token(TokenDomain::Session, &session_token);
        sqlx::query(
            "INSERT INTO sessions (
                user_id, session_hash, expires_at, revoked_at, created_at, last_seen_at
             ) VALUES (?, ?, ?, NULL, ?, ?)",
        )
        .bind(record.user_id)
        .bind(session_hash.as_bytes().as_slice())
        .bind(now + self.session_lifetime_seconds)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("UPDATE users SET last_login_at = ? WHERE id = ?")
            .bind(now)
            .bind(record.user_id)
            .execute(&mut *transaction)
            .await?;
        insert_audit_in_transaction(
            &mut transaction,
            Some(record.user_id),
            CONSUME_SUCCEEDED_ACTION,
            "login_token",
            Some(record.token_id.to_string()),
            r#"{"result":"authenticated"}"#,
            now,
        )
        .await?;
        transaction.commit().await?;

        let csrf_token = self.secret_key.generate_csrf_token(&session_token);
        tracing::info!("login link consumed");
        Ok(ConsumedLoginLink {
            user: ActiveUser {
                id: record.user_id,
                email: record.normalized_email,
                display_name: record.display_name,
                status: "active",
                is_superadmin: record.is_superadmin,
            },
            session_token,
            csrf_token,
        })
    }

    async fn dev_login_impl(&self, command: DevLogin) -> Result<DevLoginResult, DevLoginError> {
        let now = (self.clock)();
        let mut transaction = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(|e| DevLoginError::Database(e.to_string()))?;

        let user = sqlx::query_as::<_, DevLoginUserRecord>(
            "SELECT id, normalized_email, display_name, status, is_superadmin
             FROM users WHERE normalized_email = ?",
        )
        .bind(&command.normalized_email)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|e| DevLoginError::Database(e.to_string()))?;

        let (user_id, _display_name) = match user {
            Some(record) => (
                record.id,
                record.display_name.or(command.display_name.clone()),
            ),
            None => {
                let result = sqlx::query(
                    "INSERT INTO users (normalized_email, display_name, status, created_at)
                     VALUES (?, ?, 'active', ?)",
                )
                .bind(&command.normalized_email)
                .bind(&command.display_name)
                .bind(now)
                .execute(&mut *transaction)
                .await
                .map_err(|e| DevLoginError::Database(e.to_string()))?;
                (result.last_insert_rowid(), command.display_name.clone())
            }
        };

        let session_token = self.secret_key.generate_token();
        let session_hash = self
            .secret_key
            .hash_token(TokenDomain::Session, &session_token);
        sqlx::query(
            "INSERT INTO sessions (
                user_id, session_hash, expires_at, revoked_at, created_at, last_seen_at
             ) VALUES (?, ?, ?, NULL, ?, ?)",
        )
        .bind(user_id)
        .bind(session_hash.as_bytes().as_slice())
        .bind(now + self.session_lifetime_seconds)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(|e| DevLoginError::Database(e.to_string()))?;

        let csrf_token = self.secret_key.generate_csrf_token(&session_token);
        let redirect_url = format!("/dev-login?csrf_token={}", csrf_token.expose());
        let cookie = SessionCookieBuilder::new(&session_token)
            .is_secure(self.is_secure)
            .build();

        transaction
            .commit()
            .await
            .map_err(|e| DevLoginError::Database(e.to_string()))?;

        Ok(DevLoginResult {
            redirect_url,
            cookie,
        })
    }
}

#[derive(FromRow)]
#[allow(dead_code)]
struct DevLoginUserRecord {
    id: i64,
    normalized_email: String,
    display_name: Option<String>,
    status: String,
    is_superadmin: bool,
}

impl<E> LoginFlow for LoginService<E>
where
    E: EmailSender + Send + Sync + 'static,
{
    fn request_link<'a>(
        &'a self,
        command: RequestLoginLink,
    ) -> Pin<Box<dyn Future<Output = Result<(), RequestLoginLinkError>> + Send + 'a>> {
        Box::pin(self.request(command))
    }

    fn consume_link<'a>(
        &'a self,
        command: ConsumeLoginLink,
    ) -> Pin<Box<dyn Future<Output = Result<ConsumedLoginLink, ConsumeLoginLinkError>> + Send + 'a>>
    {
        Box::pin(self.consume(command))
    }

    fn dev_login<'a>(
        &'a self,
        command: DevLogin,
    ) -> Pin<Box<dyn Future<Output = Result<DevLoginResult, DevLoginError>> + Send + 'a>> {
        Box::pin(self.dev_login_impl(command))
    }

    fn authenticate_password<'a>(
        &'a self,
        command: PasswordLoginCommand,
    ) -> Pin<Box<dyn Future<Output = Result<PasswordLoginResult, PasswordLoginError>> + Send + 'a>>
    {
        Box::pin(self.authenticate_password_impl(command))
    }
}

pub struct RequestLoginLink {
    pub email: String,
    pub client_ip: String,
}

pub struct ConsumeLoginLink {
    pub token: String,
    pub prior_session_token: Option<String>,
}

pub struct ConsumedLoginLink {
    pub user: ActiveUser,
    pub session_token: SecretToken,
    pub csrf_token: CsrfToken,
}

#[derive(Clone, Debug)]
pub struct DevLogin {
    pub normalized_email: String,
    pub display_name: Option<String>,
}

#[derive(Clone, Debug)]
pub struct DevLoginResult {
    pub redirect_url: String,
    pub cookie: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DevLoginError {
    Unavailable,
    Database(String),
}

impl Display for DevLoginError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            DevLoginError::Unavailable => write!(f, "dev login unavailable"),
            DevLoginError::Database(msg) => write!(f, "database error: {msg}"),
        }
    }
}

impl Error for DevLoginError {}

#[derive(FromRow)]
struct RequestUser {
    id: i64,
    normalized_email: String,
}

#[derive(FromRow)]
struct LoginTokenUser {
    token_id: i64,
    expires_at: i64,
    revoked_at: Option<i64>,
    consumed_at: Option<i64>,
    user_id: i64,
    normalized_email: String,
    display_name: Option<String>,
    status: String,
    is_superadmin: bool,
}

impl LoginTokenUser {
    fn rejection_reason(&self, now: i64) -> Option<&'static str> {
        if self.status != "active" {
            Some("account_ineligible")
        } else if self.revoked_at.is_some() {
            Some("revoked")
        } else if self.consumed_at.is_some() {
            Some("already_consumed")
        } else if now >= self.expires_at {
            Some("expired")
        } else {
            None
        }
    }
}

fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

async fn audit_generic_request_in_transaction(
    transaction: &mut Transaction<'_, Sqlite>,
    now: i64,
) -> Result<(), sqlx::Error> {
    insert_audit_in_transaction(
        transaction,
        None,
        REQUEST_ACTION,
        "login",
        None,
        r#"{"result":"accepted"}"#,
        now,
    )
    .await
}

async fn audit_consume_failure(
    pool: &SqlitePool,
    token_id: Option<i64>,
    reason: &'static str,
    now: i64,
) -> Result<(), sqlx::Error> {
    let metadata = format!(r#"{{"reason":"{reason}"}}"#);
    insert_audit(
        pool,
        None,
        CONSUME_FAILED_ACTION,
        "login_token",
        token_id.map(|id| id.to_string()),
        &metadata,
        now,
    )
    .await
}

async fn audit_consume_failure_in_transaction(
    transaction: &mut Transaction<'_, Sqlite>,
    token_id: Option<i64>,
    reason: &'static str,
    now: i64,
) -> Result<(), sqlx::Error> {
    let metadata = format!(r#"{{"reason":"{reason}"}}"#);
    insert_audit_in_transaction(
        transaction,
        None,
        CONSUME_FAILED_ACTION,
        "login_token",
        token_id.map(|id| id.to_string()),
        &metadata,
        now,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn insert_audit<'e, E>(
    executor: E,
    actor_user_id: Option<i64>,
    action: &'static str,
    target_type: &'static str,
    target_id: Option<String>,
    metadata: &str,
    now: i64,
) -> Result<(), sqlx::Error>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    sqlx::query(
        "INSERT INTO audit_log (
            actor_user_id, action, target_type, target_id, metadata_json, created_at
         ) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(actor_user_id)
    .bind(action)
    .bind(target_type)
    .bind(target_id)
    .bind(metadata)
    .bind(now)
    .execute(executor)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_audit_in_transaction(
    transaction: &mut Transaction<'_, Sqlite>,
    actor_user_id: Option<i64>,
    action: &'static str,
    target_type: &'static str,
    target_id: Option<String>,
    metadata: &str,
    now: i64,
) -> Result<(), sqlx::Error> {
    insert_audit(
        &mut **transaction,
        actor_user_id,
        action,
        target_type,
        target_id,
        metadata,
        now,
    )
    .await
}

#[derive(Debug)]
pub enum RequestLoginLinkError {
    RateLimited,
    Database(sqlx::Error),
}

impl Display for RequestLoginLinkError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::RateLimited => formatter.write_str("login link request rate limited"),
            Self::Database(_) => formatter.write_str("login link request failed"),
        }
    }
}

impl Error for RequestLoginLinkError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            _ => None,
        }
    }
}

impl From<sqlx::Error> for RequestLoginLinkError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

#[derive(Debug)]
pub enum ConsumeLoginLinkError {
    Invalid,
    Database(sqlx::Error),
}

impl Display for ConsumeLoginLinkError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid => formatter.write_str("login link is invalid or expired"),
            Self::Database(_) => formatter.write_str("login link consumption failed"),
        }
    }
}

impl Error for ConsumeLoginLinkError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            Self::Invalid => None,
        }
    }
}

impl From<sqlx::Error> for ConsumeLoginLinkError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub user: ActiveUser,
    pub csrf_token: String,
}

async fn audit_generic_request(
    pool: &SqlitePool,
    action: &'static str,
    now: i64,
) -> Result<(), sqlx::Error> {
    insert_audit(
        pool,
        None,
        action,
        "login",
        None,
        r#"{"result":"accepted"}"#,
        now,
    )
    .await
}

#[derive(Clone, Debug)]
pub struct PasswordLoginCommand {
    pub email: String,
    pub password: String,
    pub client_ip: String,
}

#[derive(Clone, Debug)]
pub struct PasswordLoginResult {
    pub user: ActiveUser,
    pub session_token: SecretToken,
    pub csrf_token: CsrfToken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasswordLoginError {
    Unsupported,
    InvalidCredentials,
    PasswordNotSet,
    RateLimited,
    Database(String),
}

impl From<sqlx::Error> for PasswordLoginError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error.to_string())
    }
}

impl fmt::Display for PasswordLoginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => write!(f, "password login unsupported"),
            Self::InvalidCredentials => write!(f, "invalid credentials"),
            Self::PasswordNotSet => write!(f, "password not set for this account"),
            Self::RateLimited => write!(f, "too many attempts, try again later"),
            Self::Database(msg) => write!(f, "database error: {msg}"),
        }
    }
}

impl std::error::Error for PasswordLoginError {}
