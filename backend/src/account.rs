use crate::{
    account_credentials::{RevocationScope, revoke_account_credentials},
    email::{
        AuthenticationLink, EmailChangedEmail, EmailConfirmationEmail, EmailSender,
        PasswordResetEmail,
    },
    password::{PasswordError, hash_new_password, verify_password_async},
    security::{SecretKey, SecretToken, TokenDomain},
};
use sqlx::{Sqlite, SqlitePool, Transaction};
use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    future::Future,
    pin::Pin,
    sync::Arc,
};

trait RecoveryDelivery: Send + Sync {
    fn send<'a>(
        &'a self,
        email: String,
        token: &'a SecretToken,
    ) -> Pin<Box<dyn Future<Output = Result<(), ()>> + Send + 'a>>;
    fn confirm<'a>(
        &'a self,
        email: String,
        token: &'a SecretToken,
    ) -> Pin<Box<dyn Future<Output = Result<(), ()>> + Send + 'a>>;
    fn changed<'a>(
        &'a self,
        old_email: String,
        new_email: String,
    ) -> Pin<Box<dyn Future<Output = Result<(), ()>> + Send + 'a>>;
}
struct EmailRecoveryDelivery<E> {
    origin: Arc<str>,
    sender: Arc<E>,
}
impl<E: EmailSender + Send + Sync> RecoveryDelivery for EmailRecoveryDelivery<E> {
    fn send<'a>(
        &'a self,
        email: String,
        token: &'a SecretToken,
    ) -> Pin<Box<dyn Future<Output = Result<(), ()>> + Send + 'a>> {
        Box::pin(async move {
            self.sender
                .send_password_reset(PasswordResetEmail::new(
                    email,
                    AuthenticationLink::new(format!(
                        "{}/password-reset?token={}",
                        self.origin.trim_end_matches('/'),
                        token.expose()
                    )),
                ))
                .await
                .map_err(|_| ())
        })
    }
    fn confirm<'a>(
        &'a self,
        email: String,
        token: &'a SecretToken,
    ) -> Pin<Box<dyn Future<Output = Result<(), ()>> + Send + 'a>> {
        Box::pin(async move {
            self.sender
                .send_email_confirmation(EmailConfirmationEmail::new(
                    email,
                    AuthenticationLink::new(format!(
                        "{}/email/confirm?token={}",
                        self.origin.trim_end_matches('/'),
                        token.expose()
                    )),
                ))
                .await
                .map_err(|_| ())
        })
    }
    fn changed<'a>(
        &'a self,
        old_email: String,
        new_email: String,
    ) -> Pin<Box<dyn Future<Output = Result<(), ()>> + Send + 'a>> {
        Box::pin(async move {
            self.sender
                .send_email_changed(EmailChangedEmail::new(old_email, new_email))
                .await
                .map_err(|_| ())
        })
    }
}
#[derive(Clone)]
pub struct AccountService {
    pool: SqlitePool,
    key: SecretKey,
    delivery: Arc<dyn RecoveryDelivery>,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}
impl AccountService {
    pub fn new<E: EmailSender + Send + Sync + 'static>(
        pool: SqlitePool,
        key: SecretKey,
        origin: impl Into<Arc<str>>,
        sender: Arc<E>,
    ) -> Self {
        Self {
            pool,
            key,
            delivery: Arc::new(EmailRecoveryDelivery {
                origin: origin.into(),
                sender,
            }),
            clock: Arc::new(|| chrono::Utc::now().timestamp()),
        }
    }
    pub fn new_at<E: EmailSender + Send + Sync + 'static>(
        pool: SqlitePool,
        key: SecretKey,
        origin: impl Into<Arc<str>>,
        sender: Arc<E>,
        now: i64,
    ) -> Self {
        let mut service = Self::new(pool, key, origin, sender);
        service.clock = Arc::new(move || now);
        service
    }
    pub async fn request_password_reset(
        &self,
        command: RequestPasswordReset,
    ) -> Result<(), AccountError> {
        let email = command.email.trim().to_lowercase();
        if !crate::admin::valid_invitation_email(&email) {
            return Ok(());
        }
        let now = (self.clock)();
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let user_id: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM users WHERE normalized_email = ? AND status = 'registered'",
        )
        .bind(&email)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(user_id) = user_id else {
            return Ok(());
        };
        sqlx::query("UPDATE password_reset_tokens SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL AND consumed_at IS NULL").bind(now).bind(user_id).execute(&mut *tx).await?;
        let token = self.key.generate_token();
        let hash = self.key.hash_token(TokenDomain::PasswordReset, &token);
        let id = sqlx::query("INSERT INTO password_reset_tokens(user_id, token_hash, expires_at, created_at) VALUES (?, ?, ?, ?)").bind(user_id).bind(hash.as_bytes().as_slice()).bind(now + 900).bind(now).execute(&mut *tx).await?.last_insert_rowid();
        account_audit(&mut tx, user_id, "account.password_reset.request", id, now).await?;
        tx.commit().await?;
        if self.delivery.send(email, &token).await.is_err() {
            let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
            sqlx::query("UPDATE password_reset_tokens SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL AND consumed_at IS NULL").bind((self.clock)()).bind(id).execute(&mut *tx).await?;
            account_audit(
                &mut tx,
                user_id,
                "account.password_reset.delivery_failed",
                id,
                (self.clock)(),
            )
            .await?;
            tx.commit().await?;
            tracing::error!(error_code = "password_reset_delivery_failed");
        }
        Ok(())
    }
    pub async fn consume_password_reset(
        &self,
        command: ConsumePasswordReset,
    ) -> Result<(), AccountError> {
        let token = SecretToken::parse(command.token).ok_or(AccountError::InvalidToken)?;
        let hash = self.key.hash_token(TokenDomain::PasswordReset, &token);
        let available: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM password_reset_tokens t JOIN users u ON u.id = t.user_id WHERE t.token_hash = ? AND t.revoked_at IS NULL AND t.consumed_at IS NULL AND t.expires_at > ? AND u.status = 'registered')").bind(hash.as_bytes().as_slice()).bind((self.clock)()).fetch_one(&self.pool).await?;
        if !available {
            return Err(AccountError::InvalidToken);
        }
        let password_hash = hash_new_password(command.password, command.password_confirmation)
            .await
            .map_err(AccountError::Password)?;
        // Password hashing runs outside the write lock. Recheck expiry and status
        // after hashing so disable, replacement, and competing consumption win safely.
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let now = (self.clock)();
        let record: Option<(i64, i64)> = sqlx::query_as("SELECT t.id, t.user_id FROM password_reset_tokens t JOIN users u ON u.id = t.user_id WHERE t.token_hash = ? AND t.revoked_at IS NULL AND t.consumed_at IS NULL AND t.expires_at > ? AND u.status = 'registered'").bind(hash.as_bytes().as_slice()).bind(now).fetch_optional(&mut *tx).await?;
        let Some((id, user_id)) = record else {
            return Err(AccountError::InvalidToken);
        };
        sqlx::query("UPDATE users SET password_hash = ? WHERE id = ? AND status = 'registered'")
            .bind(password_hash)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE password_reset_tokens SET consumed_at = ? WHERE id = ?")
            .bind(now)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        revoke_account_credentials(&mut tx, user_id, now, RevocationScope::PasswordReset).await?;
        account_audit(&mut tx, user_id, "account.password_reset.consume", id, now).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn summary(&self, user_id: i64) -> Result<AccountSummary, AccountError> {
        let record: Option<(String, Option<String>)> = sqlx::query_as("SELECT normalized_email, password_hash FROM users WHERE id = ? AND status = 'registered'").bind(user_id).fetch_optional(&self.pool).await?;
        let (email, hash) = record.ok_or(AccountError::Ineligible)?;
        let pending: Option<(String, i64)> = sqlx::query_as("SELECT normalized_new_email, expires_at FROM email_change_requests WHERE user_id = ? AND revoked_at IS NULL AND consumed_at IS NULL AND expires_at > ?").bind(user_id).bind((self.clock)()).fetch_optional(&self.pool).await?;
        Ok(AccountSummary {
            email,
            has_password: hash.is_some(),
            pending_email_change: pending
                .map(|(email, expires_at)| PendingEmailChange { email, expires_at }),
        })
    }
    pub async fn request_email_change(
        &self,
        user_id: i64,
        command: RequestEmailChange,
    ) -> Result<(), AccountError> {
        self.request_email_change_inner(None, user_id, command)
            .await
    }
    pub async fn request_admin_email_change(
        &self,
        actor_user_id: i64,
        user_id: i64,
        email: String,
    ) -> Result<(), AccountError> {
        self.request_email_change_inner(
            Some(actor_user_id),
            user_id,
            RequestEmailChange {
                email,
                current_password: String::new(),
            },
        )
        .await
    }
    async fn request_email_change_inner(
        &self,
        admin_actor: Option<i64>,
        user_id: i64,
        command: RequestEmailChange,
    ) -> Result<(), AccountError> {
        if let Some(actor) = admin_actor {
            let permitted: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = ? AND status = 'registered' AND is_superadmin = 1)").bind(actor).fetch_one(&self.pool).await?;
            if !permitted {
                return Err(AccountError::Forbidden);
            }
        }
        let email = command.email.trim().to_lowercase();
        if !crate::admin::valid_invitation_email(&email) {
            return Err(AccountError::InvalidInput);
        }
        let record: Option<(String, Option<String>)> = sqlx::query_as("SELECT normalized_email, password_hash FROM users WHERE id = ? AND status = 'registered'").bind(user_id).fetch_optional(&self.pool).await?;
        let (old_email, expected_hash) = record.ok_or(AccountError::Ineligible)?;
        if old_email == email {
            return Err(AccountError::InvalidInput);
        }
        if admin_actor.is_none() {
            let expected_hash = expected_hash
                .clone()
                .ok_or(AccountError::PasswordRequired)?;
            if !verify_password_async(command.current_password, expected_hash)
                .await
                .map_err(AccountError::Password)?
            {
                return Err(AccountError::WrongPassword);
            }
        }
        let actor_user_id = admin_actor.unwrap_or(user_id);
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let now = (self.clock)();
        if let Some(actor) = admin_actor {
            let permitted: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = ? AND status = 'registered' AND is_superadmin = 1)").bind(actor).fetch_one(&mut *tx).await?;
            if !permitted {
                return Err(AccountError::Forbidden);
            }
        }
        let unchanged: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = ? AND status = 'registered' AND normalized_email = ? AND password_hash IS ?)").bind(user_id).bind(&old_email).bind(&expected_hash).fetch_one(&mut *tx).await?;
        if !unchanged {
            return Err(AccountError::Ineligible);
        }
        sqlx::query("UPDATE email_change_requests SET revoked_at = ? WHERE revoked_at IS NULL AND consumed_at IS NULL AND expires_at <= ?").bind(now).bind(now).execute(&mut *tx).await?;
        let occupied: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE normalized_email = ?) OR EXISTS(SELECT 1 FROM invitations WHERE normalized_email = ? AND consumed_at IS NULL AND revoked_at IS NULL AND expires_at > ?) OR EXISTS(SELECT 1 FROM email_change_requests WHERE normalized_new_email = ? AND user_id <> ? AND consumed_at IS NULL AND revoked_at IS NULL)").bind(&email).bind(&email).bind(now).bind(&email).bind(user_id).fetch_one(&mut *tx).await?;
        if occupied {
            return Err(AccountError::EmailConflict);
        }
        sqlx::query("UPDATE email_change_requests SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL AND consumed_at IS NULL").bind(now).bind(user_id).execute(&mut *tx).await?;
        let token = self.key.generate_token();
        let hash = self.key.hash_token(TokenDomain::EmailChange, &token);
        let id = sqlx::query("INSERT INTO email_change_requests(user_id, normalized_new_email, token_hash, expires_at, actor_user_id, created_at) VALUES (?, ?, ?, ?, ?, ?)").bind(user_id).bind(&email).bind(hash.as_bytes().as_slice()).bind(now + 86400).bind(actor_user_id).bind(now).execute(&mut *tx).await?.last_insert_rowid();
        email_audit(
            &mut tx,
            actor_user_id,
            "account.email_change.request",
            id,
            now,
        )
        .await?;
        tx.commit().await?;
        if self.delivery.confirm(email, &token).await.is_err() {
            let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
            sqlx::query("UPDATE email_change_requests SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL AND consumed_at IS NULL").bind((self.clock)()).bind(id).execute(&mut *tx).await?;
            email_audit(
                &mut tx,
                actor_user_id,
                "account.email_change.delivery_failed",
                id,
                (self.clock)(),
            )
            .await?;
            tx.commit().await?;
            return Err(AccountError::DeliveryFailed);
        }
        Ok(())
    }
    pub async fn confirm_email_change(&self, token: String) -> Result<(), AccountError> {
        let token = SecretToken::parse(token).ok_or(AccountError::InvalidToken)?;
        let hash = self.key.hash_token(TokenDomain::EmailChange, &token);
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let now = (self.clock)();
        let record: Option<(i64, i64, String, String)> = sqlx::query_as("SELECT r.id, r.user_id, r.normalized_new_email, u.normalized_email FROM email_change_requests r JOIN users u ON u.id = r.user_id WHERE r.token_hash = ? AND r.revoked_at IS NULL AND r.consumed_at IS NULL AND r.expires_at > ? AND u.status = 'registered'").bind(hash.as_bytes().as_slice()).bind(now).fetch_optional(&mut *tx).await?;
        let (id, user_id, new_email, old_email) = record.ok_or(AccountError::InvalidToken)?;
        let occupied: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE normalized_email = ? AND id <> ?) OR EXISTS(SELECT 1 FROM invitations WHERE normalized_email = ? AND consumed_at IS NULL AND revoked_at IS NULL AND expires_at > ?)").bind(&new_email).bind(user_id).bind(&new_email).bind(now).fetch_one(&mut *tx).await?;
        if occupied {
            return Err(AccountError::EmailConflict);
        }
        sqlx::query("UPDATE email_change_requests SET consumed_at = ? WHERE id = ?")
            .bind(now)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        // Revoke invitations while the user's old email is still available.
        revoke_account_credentials(&mut tx, user_id, now, RevocationScope::EmailChanged).await?;
        sqlx::query("UPDATE users SET normalized_email = ? WHERE id = ?")
            .bind(&new_email)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        email_audit(&mut tx, user_id, "account.email_change.consume", id, now).await?;
        tx.commit().await?;
        if self.delivery.changed(old_email, new_email).await.is_err() {
            tracing::error!(error_code = "email_changed_notice_failed");
            // The email transition is committed. Notification failure cannot undo it.
            if let Ok(mut tx) = self.pool.begin().await
                && email_audit(
                    &mut tx,
                    user_id,
                    "account.email_change.notice_failed",
                    id,
                    (self.clock)(),
                )
                .await
                .is_ok()
            {
                let _ = tx.commit().await;
            }
        }
        Ok(())
    }
}
async fn account_audit(
    tx: &mut Transaction<'_, Sqlite>,
    user_id: i64,
    action: &'static str,
    token_id: i64,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO audit_log(actor_user_id, action, target_type, target_id, created_at) VALUES (?, ?, 'password_reset', ?, ?)").bind(user_id).bind(action).bind(token_id.to_string()).bind(now).execute(&mut **tx).await?;
    Ok(())
}
async fn email_audit(
    tx: &mut Transaction<'_, Sqlite>,
    user_id: i64,
    action: &'static str,
    request_id: i64,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO audit_log(actor_user_id, action, target_type, target_id, created_at) VALUES (?, ?, 'email_change', ?, ?)").bind(user_id).bind(action).bind(request_id.to_string()).bind(now).execute(&mut **tx).await?;
    Ok(())
}
#[derive(Debug, serde::Serialize)]
pub struct PendingEmailChange {
    pub email: String,
    pub expires_at: i64,
}
#[derive(serde::Serialize)]
pub struct AccountSummary {
    pub email: String,
    pub has_password: bool,
    pub pending_email_change: Option<PendingEmailChange>,
}
pub struct RequestEmailChange {
    pub email: String,
    pub current_password: String,
}
pub struct RequestPasswordReset {
    pub email: String,
}
pub struct ConsumePasswordReset {
    pub token: String,
    pub password: String,
    pub password_confirmation: String,
}
#[derive(Debug)]
pub enum AccountError {
    Forbidden,
    InvalidInput,
    WrongPassword,
    PasswordRequired,
    EmailConflict,
    Ineligible,
    DeliveryFailed,
    InvalidToken,
    Password(PasswordError),
    Storage(sqlx::Error),
}
impl Display for AccountError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("account operation failed")
    }
}
impl Error for AccountError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error),
            Self::Password(error) => Some(error),
            _ => None,
        }
    }
}
impl From<sqlx::Error> for AccountError {
    fn from(error: sqlx::Error) -> Self {
        Self::Storage(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn expiry_is_rechecked_after_password_hashing() {
        let dir = tempfile::TempDir::new().unwrap();
        let config = crate::config::AppConfig::with_database_path(
            crate::config::Environment::Development,
            "127.0.0.1:3000",
            None,
            dir.path().join("expiry.sqlite"),
        )
        .unwrap();
        let pool = crate::database::connect_and_migrate(&config, crate::http::Readiness::new())
            .await
            .unwrap();
        let user = sqlx::query("INSERT INTO users(normalized_email, status, created_at) VALUES ('expiry@example.test', 'registered', 1000)").execute(&pool).await.unwrap().last_insert_rowid();
        let key = SecretKey::new([42; 32]);
        let token = key.generate_token();
        let hash = key.hash_token(TokenDomain::PasswordReset, &token);
        sqlx::query("INSERT INTO password_reset_tokens(user_id, token_hash, expires_at, created_at) VALUES (?, ?, 1900, 1000)").bind(user).bind(hash.as_bytes().as_slice()).execute(&pool).await.unwrap();
        let mut service = AccountService::new_at(
            pool.clone(),
            key,
            "https://commoncal.test",
            Arc::new(crate::email::InMemoryEmailSender::new()),
            1000,
        );
        let calls = AtomicUsize::new(0);
        service.clock = Arc::new(move || {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                1000
            } else {
                1900
            }
        });
        assert!(matches!(
            service
                .consume_password_reset(ConsumePasswordReset {
                    token: token.expose().into(),
                    password: "a-new-password-for-expiry".into(),
                    password_confirmation: "a-new-password-for-expiry".into()
                })
                .await,
            Err(AccountError::InvalidToken)
        ));
        let password: Option<String> =
            sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?")
                .bind(user)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(password, None);
    }
}
