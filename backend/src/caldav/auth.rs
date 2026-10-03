use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use axum::http::HeaderValue;
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use sqlx::SqlitePool;
use url::Url;

use crate::{
    caldav::{
        MAX_LABEL_LENGTH, TOKEN_PREFIX_LENGTH,
        types::{
            CaldavAccount, CaldavAuthError, CaldavCalendar, CaldavMetrics, ConnectionStatus,
            CredentialMetadata, DavSession, IssuedCredential, PrincipalInfo,
        },
    },
    security::{SecretKey, SecretToken, TokenDomain},
};

type CalendarRow = (
    i64,
    i64,
    Option<String>,
    String,
    Option<String>,
    String,
    String,
);

/// Maximum failed DAV auth attempts per client key before throttling.
pub const DAV_AUTH_MAX_ATTEMPTS: u32 = 10;
/// Window (seconds) for the DAV auth rate limiter.
pub const DAV_AUTH_WINDOW_SECONDS: i64 = 60;

#[derive(Clone)]
pub struct CaldavAccountService {
    pool: SqlitePool,
    key: SecretKey,
    public_origin: Url,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    auth_buckets: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<AuthFailureBucket>>>>>,
    metrics: Arc<CaldavMetrics>,
}

/// Per-client lock serializes admission and credential verification. Successful
/// requests never reserve budget; concurrent failures cannot overshoot the cap.
#[derive(Default)]
struct AuthFailureBucket {
    window_started_at: i64,
    failures: u32,
}

impl CaldavAccountService {
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub fn now(&self) -> i64 {
        (self.clock)()
    }

    pub fn metrics(&self) -> &CaldavMetrics {
        &self.metrics
    }

    /// Missing credentials are a free discovery challenge, unless already blocked.
    /// Success does not clear failures. Infrastructure errors do not spend budget.
    /// The tenth failure receives 401; subsequent requests receive 429 until expiry.
    pub async fn authenticate_limited(
        &self,
        client_key: &str,
        authorization: Option<&HeaderValue>,
    ) -> Result<DavSession, (i64, CaldavAuthError)> {
        let bucket = {
            let now = self.now();
            let mut buckets = self.auth_buckets.lock().unwrap();
            // Never evict a lock with active or queued requests: doing so would
            // create a second lock for the same client and bypass serialization.
            buckets.retain(|_, bucket| {
                Arc::strong_count(bucket) > 1
                    || bucket.try_lock().map_or(true, |state| {
                        state.failures > 0
                            && now - state.window_started_at < DAV_AUTH_WINDOW_SECONDS
                    })
            });
            buckets.entry(client_key.to_owned()).or_default().clone()
        };
        let mut state = bucket.lock().await;
        let now = self.now();
        if now - state.window_started_at >= DAV_AUTH_WINDOW_SECONDS {
            state.failures = 0;
        }
        if state.failures >= DAV_AUTH_MAX_ATTEMPTS {
            self.metrics.record_rate_limited();
            return Err((
                (DAV_AUTH_WINDOW_SECONDS - (now - state.window_started_at)).max(1),
                CaldavAuthError::RateLimited,
            ));
        }
        let Some(authorization) = authorization else {
            return Err((0, CaldavAuthError::InvalidCredentials));
        };
        let result = self.authenticate(authorization).await;
        if matches!(result, Err(CaldavAuthError::InvalidCredentials)) {
            let now = self.now();
            if state.failures == 0 || now - state.window_started_at >= DAV_AUTH_WINDOW_SECONDS {
                state.window_started_at = now;
                state.failures = 0;
            }
            state.failures += 1;
        }
        result.map_err(|error| (0, error))
    }

    /// Record a failed authentication attempt in metrics.
    pub fn record_auth_failure(&self) {
        self.metrics.record_auth_failure();
    }

    /// Record a successful authentication in metrics.
    pub fn record_auth_success(&self) {
        self.metrics.record_success();
    }

    pub fn new(pool: SqlitePool, key: SecretKey, public_origin: Url) -> Self {
        Self {
            pool,
            key,
            public_origin,
            clock: Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system clock is before Unix epoch")
                    .as_secs() as i64
            }),
            auth_buckets: Arc::new(Mutex::new(HashMap::new())),
            metrics: Arc::new(CaldavMetrics::new()),
        }
    }

    pub fn new_at(pool: SqlitePool, key: SecretKey, public_origin: Url, now: i64) -> Self {
        let mut service = Self::new(pool, key, public_origin);
        service.clock = Arc::new(move || now);
        service
    }

    #[cfg(test)]
    pub(crate) fn with_clock(mut self, clock: Arc<dyn Fn() -> i64 + Send + Sync>) -> Self {
        self.clock = clock;
        self
    }

    pub async fn status(&self, actor_user_id: i64) -> Result<ConnectionStatus, CaldavAuthError> {
        let principal_id: Option<String> =
            sqlx::query_scalar("SELECT principal_id FROM caldav_accounts WHERE user_id = ?")
                .bind(actor_user_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|_| CaldavAuthError::Persistence)?;
        let credentials = self.list_credentials(actor_user_id).await?;
        let last_successful_access_at = credentials
            .iter()
            .filter_map(|credential| credential.last_used_at)
            .max();
        Ok(ConnectionStatus {
            server_url: self.server_url(),
            principal_id,
            credentials,
            last_successful_access_at,
        })
    }

    pub async fn issue_credential(
        &self,
        actor_user_id: i64,
        label: String,
    ) -> Result<IssuedCredential, CaldavAuthError> {
        let label = label.trim().to_owned();
        if label.is_empty() || label.chars().count() > MAX_LABEL_LENGTH {
            return Err(CaldavAuthError::InvalidCredentials);
        }
        self.ensure_account(actor_user_id).await?;
        let username = self
            .user_email(actor_user_id)
            .await
            .map_err(|_| CaldavAuthError::Persistence)?;
        let now = (self.clock)();
        let password = self.key.generate_token();
        let token_prefix = password
            .expose()
            .chars()
            .take(TOKEN_PREFIX_LENGTH)
            .collect::<String>();
        let token_hash = self
            .key
            .hash_token(TokenDomain::CaldavConnection, &password);
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| CaldavAuthError::Persistence)?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO caldav_credentials (
                user_id, label, token_prefix, token_hash, created_at
             ) VALUES (?, ?, ?, ?, ?)
             RETURNING id",
        )
        .bind(actor_user_id)
        .bind(&label)
        .bind(&token_prefix)
        .bind(token_hash.as_bytes().to_vec())
        .bind(now)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| CaldavAuthError::Persistence)?;
        insert_audit(
            &mut transaction,
            actor_user_id,
            "caldav.credential.issue",
            id,
            now,
        )
        .await?;
        transaction
            .commit()
            .await
            .map_err(|_| CaldavAuthError::Persistence)?;
        Ok(IssuedCredential {
            metadata: CredentialMetadata {
                id,
                label,
                created_at: now,
                last_used_at: None,
            },
            username,
            password,
            server_url: self.server_url(),
        })
    }

    pub async fn revoke_credential(
        &self,
        actor_user_id: i64,
        credential_id: i64,
    ) -> Result<(), CaldavAuthError> {
        let now = (self.clock)();
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| CaldavAuthError::Persistence)?;
        let result = sqlx::query(
            "UPDATE caldav_credentials
                 SET revoked_at = ?
              WHERE id = ? AND user_id = ? AND revoked_at IS NULL",
        )
        .bind(now)
        .bind(credential_id)
        .bind(actor_user_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| CaldavAuthError::Persistence)?;
        if result.rows_affected() == 0 {
            return Err(CaldavAuthError::InvalidCredentials);
        }
        insert_audit(
            &mut transaction,
            actor_user_id,
            "caldav.credential.revoke",
            credential_id,
            now,
        )
        .await?;
        transaction
            .commit()
            .await
            .map_err(|_| CaldavAuthError::Persistence)?;
        Ok(())
    }

    pub async fn revoke_all(&self, actor_user_id: i64) -> Result<u64, CaldavAuthError> {
        let now = (self.clock)();
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| CaldavAuthError::Persistence)?;
        let result = sqlx::query(
            "UPDATE caldav_credentials
                 SET revoked_at = ?
              WHERE user_id = ? AND revoked_at IS NULL",
        )
        .bind(now)
        .bind(actor_user_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| CaldavAuthError::Persistence)?;
        insert_audit(
            &mut transaction,
            actor_user_id,
            "caldav.credential.revoke_all",
            actor_user_id,
            now,
        )
        .await?;
        transaction
            .commit()
            .await
            .map_err(|_| CaldavAuthError::Persistence)?;
        Ok(result.rows_affected())
    }

    pub async fn authenticate(
        &self,
        authorization: &HeaderValue,
    ) -> Result<DavSession, CaldavAuthError> {
        let (username, password) =
            parse_basic_authorization(authorization).ok_or(CaldavAuthError::InvalidCredentials)?;
        let username = username.to_ascii_lowercase();
        let token = SecretToken::parse(&password).ok_or(CaldavAuthError::InvalidCredentials)?;
        let token_prefix = password
            .chars()
            .take(TOKEN_PREFIX_LENGTH)
            .collect::<String>();
        let user_id: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM users WHERE normalized_email = ? AND status = 'active'",
        )
        .bind(&username)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| CaldavAuthError::Persistence)?;
        let candidates: Vec<(i64, Vec<u8>)> = match user_id {
            Some(user_id) => sqlx::query_as(
                "SELECT id, token_hash FROM caldav_credentials
                 WHERE user_id = ? AND token_prefix = ? AND revoked_at IS NULL",
            )
            .bind(user_id)
            .bind(&token_prefix)
            .fetch_all(&self.pool)
            .await
            .map_err(|_| CaldavAuthError::Persistence)?,
            None => Vec::new(),
        };
        let principal_id: Option<String> = match user_id {
            Some(user_id) => {
                sqlx::query_scalar("SELECT principal_id FROM caldav_accounts WHERE user_id = ?")
                    .bind(user_id)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|_| CaldavAuthError::Persistence)?
            }
            None => None,
        };
        let mut verified: Option<i64> = None;
        let mut checked_candidate = false;
        for (credential_id, token_hash) in &candidates {
            checked_candidate = true;
            let expected = crate::security::TokenHash::from_bytes(
                token_hash
                    .as_slice()
                    .try_into()
                    .map_err(|_| CaldavAuthError::Persistence)?,
            );
            if self
                .key
                .verify_token(TokenDomain::CaldavConnection, &token, &expected)
            {
                verified = Some(*credential_id);
                break;
            }
        }
        if !checked_candidate {
            // Verify against a dummy hash so unknown-user, unknown-prefix, and
            // wrong-password failures take comparable time and do not reveal
            // which lookup failed.
            let dummy = crate::security::TokenHash::from_bytes([0_u8; 32]);
            let _ = self
                .key
                .verify_token(TokenDomain::CaldavConnection, &token, &dummy);
        }
        let credential_id = verified.ok_or(CaldavAuthError::InvalidCredentials)?;
        let principal_id = principal_id.ok_or(CaldavAuthError::InvalidCredentials)?;
        let user_id = user_id.ok_or(CaldavAuthError::InvalidCredentials)?;
        let now = (self.clock)();
        sqlx::query(
            "UPDATE caldav_credentials SET last_used_at = ? WHERE id = ? AND revoked_at IS NULL",
        )
        .bind(now)
        .bind(credential_id)
        .execute(&self.pool)
        .await
        .map_err(|_| CaldavAuthError::Persistence)?;
        Ok(DavSession {
            user_id,
            credential_id,
            principal_id,
        })
    }

    async fn ensure_account(&self, actor_user_id: i64) -> Result<CaldavAccount, CaldavAuthError> {
        let now = (self.clock)();
        let principal_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO caldav_accounts (user_id, principal_id, created_at, updated_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(user_id) DO NOTHING",
        )
        .bind(actor_user_id)
        .bind(&principal_id)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|_| CaldavAuthError::Persistence)?;
        let principal_id: String =
            sqlx::query_scalar("SELECT principal_id FROM caldav_accounts WHERE user_id = ?")
                .bind(actor_user_id)
                .fetch_one(&self.pool)
                .await
                .map_err(|_| CaldavAuthError::Persistence)?;
        Ok(CaldavAccount {
            user_id: actor_user_id,
            principal_id,
        })
    }

    async fn list_credentials(
        &self,
        actor_user_id: i64,
    ) -> Result<Vec<CredentialMetadata>, CaldavAuthError> {
        let rows: Vec<(i64, String, i64, Option<i64>)> = sqlx::query_as(
            "SELECT id, label, created_at, last_used_at
             FROM caldav_credentials
             WHERE user_id = ? AND revoked_at IS NULL
             ORDER BY id",
        )
        .bind(actor_user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| CaldavAuthError::Persistence)?;
        Ok(rows
            .into_iter()
            .map(|(id, label, created_at, last_used_at)| CredentialMetadata {
                id,
                label,
                created_at,
                last_used_at,
            })
            .collect())
    }

    async fn user_email(&self, user_id: i64) -> Result<String, sqlx::Error> {
        sqlx::query_scalar("SELECT normalized_email FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_one(&self.pool)
            .await
    }

    fn server_url(&self) -> String {
        self.public_origin
            .join("/dav/")
            .map(|url| url.to_string())
            .unwrap_or_else(|_| format!("{}/dav/", self.public_origin))
    }

    pub fn dav_root_url(&self) -> String {
        self.server_url()
    }

    pub fn principal_url(&self, principal_id: &str) -> String {
        self.public_origin
            .join(&format!("/dav/principals/{}/", principal_id))
            .map(|url| url.to_string())
            .unwrap_or_else(|_| format!("{}/dav/principals/{}/", self.public_origin, principal_id))
    }

    pub fn calendar_home_url(&self, principal_id: &str) -> String {
        self.public_origin
            .join(&format!("/dav/calendars/{}/", principal_id))
            .map(|url| url.to_string())
            .unwrap_or_else(|_| format!("{}/dav/calendars/{}/", self.public_origin, principal_id))
    }

    pub async fn list_calendars(
        &self,
        user_id: i64,
    ) -> Result<Vec<CaldavCalendar>, CaldavAuthError> {
        let rows: Vec<CalendarRow> = sqlx::query_as(
            "SELECT calendars.id, calendars.owner_user_id, owner_accounts.principal_id,
                    calendars.name, calendars.description, calendars.color,
                    calendar_acl.role
              FROM calendars
              JOIN calendar_acl ON calendar_acl.calendar_id = calendars.id
              LEFT JOIN caldav_accounts owner_accounts
                ON owner_accounts.user_id = calendars.owner_user_id
              WHERE calendar_acl.user_id = ?
                AND calendars.archived = 0
              ORDER BY calendars.id",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| CaldavAuthError::Persistence)?;
        Ok(rows
            .into_iter()
            .map(
                |(
                    calendar_id,
                    owner_user_id,
                    owner_principal_id,
                    name,
                    description,
                    color,
                    role,
                )| CaldavCalendar {
                    calendar_id,
                    owner_user_id,
                    owner_principal_id,
                    name,
                    description,
                    color,
                    role,
                },
            )
            .collect())
    }

    pub async fn resolve_calendar(
        &self,
        user_id: i64,
        calendar_id: i64,
    ) -> Result<Option<CaldavCalendar>, CaldavAuthError> {
        let row: Option<CalendarRow> = sqlx::query_as(
            "SELECT calendars.id, calendars.owner_user_id, owner_accounts.principal_id,
                    calendars.name, calendars.description, calendars.color,
                    calendar_acl.role
             FROM calendars
             JOIN calendar_acl ON calendar_acl.calendar_id = calendars.id
             LEFT JOIN caldav_accounts owner_accounts
               ON owner_accounts.user_id = calendars.owner_user_id
             WHERE calendars.id = ? AND calendar_acl.user_id = ? AND calendars.archived = 0",
        )
        .bind(calendar_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| CaldavAuthError::Persistence)?;
        Ok(row.map(
            |(calendar_id, owner_user_id, owner_principal_id, name, description, color, role)| {
                CaldavCalendar {
                    calendar_id,
                    owner_user_id,
                    owner_principal_id,
                    name,
                    description,
                    color,
                    role,
                }
            },
        ))
    }

    pub async fn resolve_principal(
        &self,
        principal_id: &str,
    ) -> Result<Option<PrincipalInfo>, CaldavAuthError> {
        let row: Option<(i64, Option<String>, String)> = sqlx::query_as(
            "SELECT a.user_id, u.display_name, u.normalized_email
              FROM caldav_accounts a
              JOIN users u ON u.id = a.user_id
              WHERE a.principal_id = ?",
        )
        .bind(principal_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| CaldavAuthError::Persistence)?;
        Ok(row.map(|(user_id, display_name, email)| PrincipalInfo {
            user_id,
            display_name: display_name
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(email),
        }))
    }

    /// URI token signed over collection, visibility epoch, revision and expiry.
    pub fn collection_sync_token(&self, context: &str, revision: i64) -> String {
        self.encode_collection_state(context, revision, None)
    }
    pub fn collection_snapshot_token(&self, context: &str, revision: i64, cursor: i64) -> String {
        self.encode_collection_state(context, revision, Some(cursor))
    }
    fn encode_collection_state(&self, context: &str, revision: i64, cursor: Option<i64>) -> String {
        let payload = format!(
            "{context}\n{revision}\n{}\n{}",
            self.now() + 30 * 86400,
            cursor.map(|cursor| cursor.to_string()).unwrap_or_default()
        );
        let tag = self.key.sign_sync_payload(payload.as_bytes());
        format!(
            "urn:happening:sync:v1:{}.{}",
            URL_SAFE_NO_PAD.encode(payload),
            URL_SAFE_NO_PAD.encode(tag)
        )
    }
    pub fn collection_sync_state(&self, token: &str, context: &str) -> Option<(i64, Option<i64>)> {
        let encoded = token.strip_prefix("urn:happening:sync:v1:")?;
        let (payload, tag) = encoded.split_once('.')?;
        let payload = URL_SAFE_NO_PAD.decode(payload).ok()?;
        let tag = URL_SAFE_NO_PAD.decode(tag).ok()?;
        if !self.key.verify_sync_revision(&payload, &tag) {
            return None;
        }
        let payload = std::str::from_utf8(&payload).ok()?;
        let mut parts = payload.split('\n');
        if parts.next()? != context {
            return None;
        }
        let revision: i64 = parts.next()?.parse().ok()?;
        let expiry: i64 = parts.next()?.parse().ok()?;
        let cursor = match parts.next()? {
            "" => None,
            value => Some(value.parse::<i64>().ok().filter(|cursor| *cursor > 0)?),
        };
        if parts.next().is_some() || revision < 0 || expiry <= self.now() {
            return None;
        }
        Some((revision, cursor))
    }
    pub fn collection_sync_revision(&self, token: &str, context: &str) -> Option<i64> {
        let (revision, cursor) = self.collection_sync_state(token, context)?;
        cursor.is_none().then_some(revision)
    }

    /// Encode a change-log revision into an opaque, tamper-evident sync token.
    ///
    /// The token is `base64url(revision) . base64url(hmac)`. Clients treat it
    /// as an opaque string; the server verifies the HMAC before trusting the
    /// revision, so a forged or truncated token fails safely.
    pub fn encode_sync_token(&self, revision: i64) -> String {
        let (revision_bytes, tag) = self.key.sign_sync_revision(revision);
        format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(revision_bytes),
            URL_SAFE_NO_PAD.encode(tag)
        )
    }

    /// Decode and verify a sync token, returning the revision it carries.
    ///
    /// Returns `None` for any malformed, truncated, or tampered token so the
    /// caller can fail safely without leaking which check failed.
    pub fn decode_sync_token(&self, token: &str) -> Option<i64> {
        let (encoded_revision, encoded_tag) = token.split_once('.')?;
        let revision_bytes = URL_SAFE_NO_PAD.decode(encoded_revision).ok()?;
        let tag = URL_SAFE_NO_PAD.decode(encoded_tag).ok()?;
        if revision_bytes.len() != 8 {
            return None;
        }
        if !self.key.verify_sync_revision(&revision_bytes, &tag) {
            return None;
        }
        let mut bytes = [0_u8; 8];
        bytes.copy_from_slice(&revision_bytes);
        Some(i64::from_be_bytes(bytes))
    }
}

fn parse_basic_authorization(authorization: &HeaderValue) -> Option<(String, String)> {
    let encoded = authorization.to_str().ok()?;
    let (scheme, credentials) = encoded.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = STANDARD.decode(credentials).ok()?;
    let string = String::from_utf8(decoded).ok()?;
    let (username, password) = string.split_once(':')?;
    if username.is_empty() || password.is_empty() {
        return None;
    }
    Some((username.to_owned(), password.to_owned()))
}

async fn insert_audit(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    actor_user_id: i64,
    action: &'static str,
    credential_id: i64,
    now: i64,
) -> Result<(), CaldavAuthError> {
    sqlx::query(
        "INSERT INTO audit_log (
            actor_user_id, action, target_type, target_id, metadata_json, created_at
         ) VALUES (?, ?, 'caldav_credential', ?, NULL, ?)",
    )
    .bind(actor_user_id)
    .bind(action)
    .bind(credential_id.to_string())
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(|_| CaldavAuthError::Persistence)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use base64::engine::general_purpose::STANDARD as B64;
    use sqlx::SqlitePool;
    use tempfile::NamedTempFile;

    struct TestDb {
        _file: NamedTempFile,
        pool: SqlitePool,
    }

    impl TestDb {
        async fn new() -> Self {
            let file = NamedTempFile::new().unwrap();
            let conn_str = format!("sqlite:{}", file.path().to_str().unwrap());
            let pool = SqlitePool::connect(&conn_str).await.unwrap();
            sqlx::query(
                "CREATE TABLE users (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    normalized_email TEXT NOT NULL UNIQUE COLLATE NOCASE,
                    display_name TEXT,
                    status TEXT NOT NULL CHECK (status IN ('invited', 'active', 'suspended', 'deleted')),
                    created_at INTEGER NOT NULL
                )",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "CREATE TABLE caldav_accounts (
                    user_id INTEGER PRIMARY KEY REFERENCES users(id),
                    principal_id TEXT NOT NULL UNIQUE CHECK (length(principal_id) > 0),
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                )",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "CREATE TABLE caldav_credentials (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    user_id INTEGER NOT NULL REFERENCES users(id),
                    label TEXT NOT NULL CHECK (length(trim(label)) > 0),
                    token_prefix TEXT NOT NULL CHECK (length(token_prefix) = 8),
                    token_hash BLOB NOT NULL CHECK (length(token_hash) = 32),
                    created_at INTEGER NOT NULL,
                    last_used_at INTEGER,
                    revoked_at INTEGER
                )",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "CREATE TABLE audit_log (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    actor_user_id INTEGER REFERENCES users(id),
                    action TEXT NOT NULL,
                    target_type TEXT NOT NULL,
                    target_id TEXT,
                    metadata_json TEXT,
                    created_at INTEGER NOT NULL
                )",
            )
            .execute(&pool)
            .await
            .unwrap();
            Self { _file: file, pool }
        }

        async fn insert_user(&self, email: &str) -> i64 {
            let now = 1000i64;
            sqlx::query_scalar(
                "INSERT INTO users (normalized_email, display_name, status, created_at)
                 VALUES (?, 'Test', 'active', ?) RETURNING id",
            )
            .bind(email)
            .bind(now)
            .fetch_one(&self.pool)
            .await
            .unwrap()
        }
    }

    fn service(db: &TestDb, key: SecretKey) -> CaldavAccountService {
        CaldavAccountService::new_at(
            db.pool.clone(),
            key,
            Url::parse("http://127.0.0.1:3000").unwrap(),
            1000,
        )
    }

    fn basic_header(username: &str, password: &str) -> HeaderValue {
        let encoded = B64.encode(format!("{username}:{password}"));
        HeaderValue::from_str(&format!("Basic {encoded}")).unwrap()
    }

    #[tokio::test]
    async fn issue_credential_returns_secret_once_and_hashes_at_rest() {
        let db = TestDb::new().await;
        let key = SecretKey::generate();
        let svc = service(&db, key.clone());
        let user_id = db.insert_user("alice@example.test").await;

        let issued = svc
            .issue_credential(user_id, "My iPhone".into())
            .await
            .unwrap();
        let clear = issued.password.expose().to_owned();
        assert!(!clear.is_empty());
        assert_eq!(issued.username, "alice@example.test");
        assert_eq!(issued.server_url, "http://127.0.0.1:3000/dav/");

        let row: (String, Vec<u8>) =
            sqlx::query_as("SELECT token_prefix, token_hash FROM caldav_credentials WHERE id = ?")
                .bind(issued.metadata.id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(row.0, clear.chars().take(8).collect::<String>());
        assert_eq!(row.1.len(), 32);
        assert_ne!(String::from_utf8_lossy(&row.1), clear);

        let stored: Vec<String> = sqlx::query_scalar(
            "SELECT label || '|' || token_prefix || '|' || hex(token_hash) FROM caldav_credentials",
        )
        .fetch_all(&db.pool)
        .await
        .unwrap();
        for value in stored {
            assert!(
                !value.contains(&clear),
                "clear password must not be stored at rest"
            );
        }
    }

    #[tokio::test]
    async fn credential_survives_new_instance_but_not_deliberate_key_rotation() {
        let db = TestDb::new().await;
        let user_id = db.insert_user("restart@example.test").await;
        let first = service(&db, SecretKey::derive(b"stable-session-secret"));
        let issued = first
            .issue_credential(user_id, "Restart".into())
            .await
            .unwrap();
        let auth = basic_header(&issued.username, issued.password.expose());
        let next = service(&db, SecretKey::derive(b"stable-session-secret"));
        assert!(next.authenticate(&auth).await.is_ok());
        let rotated = service(&db, SecretKey::derive(b"deliberately-rotated-secret"));
        assert!(matches!(
            rotated.authenticate(&auth).await,
            Err(CaldavAuthError::InvalidCredentials)
        ));
        // A failed authentication does not revoke or rewrite the credential.
        assert!(next.authenticate(&auth).await.is_ok());
    }

    #[tokio::test]
    async fn authenticate_accepts_email_and_active_connection_password() {
        let db = TestDb::new().await;
        let key = SecretKey::generate();
        let svc = service(&db, key.clone());
        let user_id = db.insert_user("bob@example.test").await;

        let issued = svc.issue_credential(user_id, "Mac".into()).await.unwrap();
        let header = basic_header("bob@example.test", issued.password.expose());
        let session = svc.authenticate(&header).await.unwrap();
        assert_eq!(session.user_id, user_id);
        assert_eq!(session.credential_id, issued.metadata.id);
        assert!(!session.principal_id.is_empty());

        let last_used: Option<i64> =
            sqlx::query_scalar("SELECT last_used_at FROM caldav_credentials WHERE id = ?")
                .bind(issued.metadata.id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(last_used, Some(1000));
    }

    #[tokio::test]
    async fn authenticate_returns_same_unauthorized_shape_for_all_failures() {
        let db = TestDb::new().await;
        let key = SecretKey::generate();
        let svc = service(&db, key.clone());
        let user_id = db.insert_user("carol@example.test").await;
        let issued = svc.issue_credential(user_id, "Phone".into()).await.unwrap();

        let wrong_password = svc
            .authenticate(&basic_header("carol@example.test", &"A".repeat(43)))
            .await
            .unwrap_err();
        assert!(matches!(
            wrong_password,
            CaldavAuthError::InvalidCredentials
        ));

        let unknown_user = svc
            .authenticate(&basic_header(
                "nobody@example.test",
                issued.password.expose(),
            ))
            .await
            .unwrap_err();
        assert!(matches!(unknown_user, CaldavAuthError::InvalidCredentials));

        svc.revoke_credential(user_id, issued.metadata.id)
            .await
            .unwrap();
        let revoked = svc
            .authenticate(&basic_header(
                "carol@example.test",
                issued.password.expose(),
            ))
            .await
            .unwrap_err();
        assert!(matches!(revoked, CaldavAuthError::InvalidCredentials));

        let missing_header = svc
            .authenticate(&HeaderValue::from_static("Bearer abc"))
            .await
            .unwrap_err();
        assert!(matches!(
            missing_header,
            CaldavAuthError::InvalidCredentials
        ));
    }

    #[tokio::test]
    async fn revoking_one_credential_does_not_revoke_other_devices() {
        let db = TestDb::new().await;
        let key = SecretKey::generate();
        let svc = service(&db, key.clone());
        let user_id = db.insert_user("dave@example.test").await;
        let first = svc.issue_credential(user_id, "Phone".into()).await.unwrap();
        let second = svc
            .issue_credential(user_id, "Laptop".into())
            .await
            .unwrap();

        svc.revoke_credential(user_id, first.metadata.id)
            .await
            .unwrap();

        let first_rejected = svc
            .authenticate(&basic_header("dave@example.test", first.password.expose()))
            .await
            .unwrap_err();
        assert!(matches!(
            first_rejected,
            CaldavAuthError::InvalidCredentials
        ));

        let second_ok = svc
            .authenticate(&basic_header("dave@example.test", second.password.expose()))
            .await
            .unwrap();
        assert_eq!(second_ok.credential_id, second.metadata.id);
    }

    #[tokio::test]
    async fn revoke_all_invalidates_every_connection_password_immediately() {
        let db = TestDb::new().await;
        let key = SecretKey::generate();
        let svc = service(&db, key.clone());
        let user_id = db.insert_user("erin@example.test").await;
        let first = svc.issue_credential(user_id, "Phone".into()).await.unwrap();
        let second = svc
            .issue_credential(user_id, "Laptop".into())
            .await
            .unwrap();

        let revoked = svc.revoke_all(user_id).await.unwrap();
        assert_eq!(revoked, 2);

        for password in [first.password.expose(), second.password.expose()] {
            let rejected = svc
                .authenticate(&basic_header("erin@example.test", password))
                .await
                .unwrap_err();
            assert!(matches!(rejected, CaldavAuthError::InvalidCredentials));
        }
    }
}
