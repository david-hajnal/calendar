use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::http::{HeaderMap, Method, header::ORIGIN};
use serde::Serialize;
use sqlx::{FromRow, SqlitePool};

use crate::{
    invitations::ActiveUser,
    security::{CsrfToken, SecretKey, SecretToken, TokenDomain},
};

const CSRF_HEADER: &str = "x-csrf-token";
const FETCH_SITE_HEADER: &str = "sec-fetch-site";

#[derive(Clone, Debug)]
pub struct SessionSecurityConfig {
    idle_timeout_seconds: i64,
    last_seen_write_throttle_seconds: i64,
    allowed_origin: Arc<str>,
}

impl SessionSecurityConfig {
    pub fn new(
        idle_timeout_seconds: i64,
        last_seen_write_throttle_seconds: i64,
        allowed_origin: impl Into<Arc<str>>,
    ) -> Result<Self, SessionConfigError> {
        let allowed_origin = allowed_origin.into();
        if idle_timeout_seconds <= 0 {
            return Err(SessionConfigError("idle timeout must be positive"));
        }
        if last_seen_write_throttle_seconds <= 0
            || last_seen_write_throttle_seconds > idle_timeout_seconds
        {
            return Err(SessionConfigError(
                "last-seen write throttle must be positive and no greater than idle timeout",
            ));
        }
        if allowed_origin.is_empty()
            || allowed_origin.ends_with('/')
            || !(allowed_origin.starts_with("https://") || allowed_origin.starts_with("http://"))
        {
            return Err(SessionConfigError(
                "allowed origin must be an http(s) origin without a trailing slash",
            ));
        }
        Ok(Self {
            idle_timeout_seconds,
            last_seen_write_throttle_seconds,
            allowed_origin,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionConfigError(&'static str);

impl Display for SessionConfigError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Error for SessionConfigError {}

#[derive(Clone)]
pub struct SessionManager {
    pool: SqlitePool,
    secret_key: SecretKey,
    config: SessionSecurityConfig,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl SessionManager {
    pub fn new(pool: SqlitePool, secret_key: SecretKey, config: SessionSecurityConfig) -> Self {
        Self {
            pool,
            secret_key,
            config,
            clock: Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system clock is before Unix epoch")
                    .as_secs() as i64
            }),
        }
    }

    pub fn new_at(
        pool: SqlitePool,
        secret_key: SecretKey,
        config: SessionSecurityConfig,
        now: i64,
    ) -> Self {
        let mut manager = Self::new(pool, secret_key, config);
        manager.clock = Arc::new(move || now);
        manager
    }

    pub async fn authenticate(
        &self,
        session_cookie: Option<&str>,
    ) -> Result<AuthenticatedSession, SessionError> {
        let token = session_cookie
            .and_then(|value| SecretToken::parse(value.to_owned()))
            .ok_or(SessionError::Unauthorized)?;
        let hash = self.secret_key.hash_token(TokenDomain::Session, &token);
        let record = sqlx::query_as::<_, SessionRecord>(
            "SELECT sessions.id AS session_id, sessions.user_id, sessions.created_at,
                    COALESCE(sessions.last_seen_at, sessions.created_at) AS last_seen_at,
                    sessions.expires_at, sessions.revoked_at,
                    users.normalized_email, users.display_name, users.status,
                    users.is_superadmin
             FROM sessions
             JOIN users ON users.id = sessions.user_id
             WHERE sessions.session_hash = ?",
        )
        .bind(hash.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(SessionError::Unauthorized)?;
        let now = (self.clock)();
        if record.revoked_at.is_some()
            || record.status != "active"
            || now >= record.expires_at
            || now - record.last_seen_at >= self.config.idle_timeout_seconds
        {
            return Err(SessionError::Unauthorized);
        }

        let last_seen_at =
            if now - record.last_seen_at >= self.config.last_seen_write_throttle_seconds {
                sqlx::query(
                    "UPDATE sessions SET last_seen_at = ?
                     WHERE id = ? AND revoked_at IS NULL
                       AND COALESCE(last_seen_at, created_at) <= ?",
                )
                .bind(now)
                .bind(record.session_id)
                .bind(now - self.config.last_seen_write_throttle_seconds)
                .execute(&self.pool)
                .await?;
                now
            } else {
                record.last_seen_at
            };

        let csrf_token = self
            .secret_key
            .generate_csrf_token(&token)
            .expose()
            .to_owned();
        Ok(AuthenticatedSession {
            id: record.session_id,
            token,
            csrf_token,
            user: ActiveUser {
                id: record.user_id,
                email: record.normalized_email,
                display_name: record.display_name,
                status: "active",
                is_superadmin: record.is_superadmin,
            },
            created_at: record.created_at,
            last_seen_at,
            expires_at: record.expires_at,
        })
    }

    pub fn enforce_csrf(
        &self,
        method: &Method,
        headers: &HeaderMap,
        session: &AuthenticatedSession,
    ) -> Result<(), SessionError> {
        if !matches!(
            *method,
            Method::POST | Method::PUT | Method::PATCH | Method::DELETE
        ) {
            return Ok(());
        }

        let origin = headers
            .get(ORIGIN)
            .and_then(|value| value.to_str().ok())
            .ok_or(SessionError::Forbidden)?;
        if origin != self.config.allowed_origin.as_ref() {
            return Err(SessionError::Forbidden);
        }
        let fetch_site = headers
            .get(FETCH_SITE_HEADER)
            .and_then(|value| value.to_str().ok())
            .ok_or(SessionError::Forbidden)?;
        if !matches!(fetch_site, "same-origin" | "same-site") {
            return Err(SessionError::Forbidden);
        }
        let csrf = headers
            .get(CSRF_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(CsrfToken::from_encoded)
            .ok_or(SessionError::Forbidden)?;
        if !self.secret_key.validate_csrf_token(&session.token, &csrf) {
            return Err(SessionError::Forbidden);
        }
        Ok(())
    }

    pub async fn logout_current(&self, session: &AuthenticatedSession) -> Result<(), SessionError> {
        let now = (self.clock)();
        let mut transaction = self.pool.begin().await?;
        sqlx::query("UPDATE sessions SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL")
            .bind(now)
            .bind(session.id)
            .execute(&mut *transaction)
            .await?;
        insert_logout_audit(
            &mut transaction,
            session.user.id,
            "auth.session.logout",
            now,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn logout_all(&self, session: &AuthenticatedSession) -> Result<(), SessionError> {
        let now = (self.clock)();
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "UPDATE sessions SET revoked_at = ?
             WHERE user_id = ? AND revoked_at IS NULL",
        )
        .bind(now)
        .bind(session.user.id)
        .execute(&mut *transaction)
        .await?;
        insert_logout_audit(
            &mut transaction,
            session.user.id,
            "auth.session.logout_all",
            now,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }
}

async fn insert_logout_audit(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    user_id: i64,
    action: &'static str,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO audit_log (
            actor_user_id, action, target_type, target_id, metadata_json, created_at
         ) VALUES (?, ?, 'session', NULL, ?, ?)",
    )
    .bind(user_id)
    .bind(action)
    .bind(r#"{"result":"revoked"}"#)
    .bind(now)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[derive(Clone, Debug, Serialize)]
pub struct AuthenticatedSession {
    #[serde(skip)]
    pub id: i64,
    #[serde(skip)]
    token: SecretToken,
    pub csrf_token: String,
    pub user: ActiveUser,
    pub created_at: i64,
    pub last_seen_at: i64,
    pub expires_at: i64,
}

#[derive(FromRow)]
struct SessionRecord {
    session_id: i64,
    user_id: i64,
    created_at: i64,
    last_seen_at: i64,
    expires_at: i64,
    revoked_at: Option<i64>,
    normalized_email: String,
    display_name: Option<String>,
    status: String,
    is_superadmin: bool,
}

#[derive(Debug)]
pub enum SessionError {
    Unauthorized,
    Forbidden,
    Database(sqlx::Error),
}

impl From<sqlx::Error> for SessionError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}
