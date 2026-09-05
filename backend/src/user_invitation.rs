use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    future::Future,
    pin::Pin,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use sqlx::{FromRow, Sqlite, SqlitePool, Transaction};

use crate::{
    email::{AuthenticationLink, EmailSender, InvitationEmail},
    security::{SecretKey, SecretToken, TokenDomain},
};

trait InvitationDelivery: Send + Sync {
    fn send<'a>(
        &'a self,
        recipient: String,
        token: &'a SecretToken,
    ) -> Pin<Box<dyn Future<Output = Result<(), ()>> + Send + 'a>>;
}

struct EmailInvitationDelivery<E> {
    invitation_url: Arc<str>,
    sender: Arc<E>,
}

impl<E> InvitationDelivery for EmailInvitationDelivery<E>
where
    E: EmailSender + Send + Sync,
{
    fn send<'a>(
        &'a self,
        recipient: String,
        token: &'a SecretToken,
    ) -> Pin<Box<dyn Future<Output = Result<(), ()>> + Send + 'a>> {
        Box::pin(async move {
            let link = AuthenticationLink::new(format!(
                "{}?token={}",
                self.invitation_url,
                token.expose()
            ));
            self.sender
                .send_invitation(InvitationEmail::new(recipient, link))
                .await
                .map_err(|_| ())
        })
    }
}

#[derive(Clone)]
pub struct UserInvitationService {
    pool: SqlitePool,
    secret_key: SecretKey,
    invitation_lifetime_seconds: i64,
    delivery: Option<Arc<dyn InvitationDelivery>>,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl UserInvitationService {
    pub fn new(pool: SqlitePool, secret_key: SecretKey, invitation_lifetime_seconds: i64) -> Self {
        Self {
            pool,
            secret_key,
            invitation_lifetime_seconds,
            delivery: None,
            clock: Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system clock is before Unix epoch")
                    .as_secs() as i64
            }),
        }
    }

    pub fn with_email_sender<E>(
        pool: SqlitePool,
        secret_key: SecretKey,
        invitation_lifetime_seconds: i64,
        invitation_url: impl Into<Arc<str>>,
        email_sender: Arc<E>,
    ) -> Self
    where
        E: EmailSender + Send + Sync + 'static,
    {
        let mut service = Self::new(pool, secret_key, invitation_lifetime_seconds);
        service.delivery = Some(Arc::new(EmailInvitationDelivery {
            invitation_url: invitation_url.into(),
            sender: email_sender,
        }));
        service
    }

    pub fn new_at(
        pool: SqlitePool,
        secret_key: SecretKey,
        invitation_lifetime_seconds: i64,
        now: i64,
    ) -> Self {
        let mut service = Self::new(pool, secret_key, invitation_lifetime_seconds);
        service.clock = Arc::new(move || now);
        service
    }

    pub fn new_for_test() -> Self {
        let pool = SqlitePool::connect_lazy(":memory:").unwrap();
        Self::new(pool, SecretKey::new([42; 32]), 3_600)
    }

    /// Create a new invitation for the given email.
    ///
    /// Returns Conflict if the user already has a pending invitation or an active account.
    pub async fn create_invitation(
        &self,
        actor_user_id: i64,
        email: String,
        display_name: Option<String>,
    ) -> Result<CreatedInvitation, UserInvitationError> {
        let now = (self.clock)();
        let normalized_email = normalize_email(&email);
        if normalized_email.is_empty() {
            return Err(UserInvitationError::InvalidInput);
        }
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;

        // Check for existing user
        let existing_user: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE normalized_email = ?")
                .bind(&normalized_email)
                .fetch_one(&mut *transaction)
                .await?;

        // Check for pending invitation
        let pending: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM invitations
             WHERE normalized_email = ? AND revoked_at IS NULL AND consumed_at IS NULL",
        )
        .bind(&normalized_email)
        .fetch_one(&mut *transaction)
        .await?;

        if existing_user != 0 || pending != 0 {
            transaction.rollback().await?;
            return Err(UserInvitationError::Conflict);
        }

        let created = self
            .insert_invitation(
                &mut transaction,
                &normalized_email,
                display_name,
                actor_user_id,
                now,
            )
            .await?;

        transaction.commit().await?;
        self.deliver_or_revoke(&created, normalized_email, actor_user_id, now)
            .await?;
        Ok(created)
    }

    /// Resend an invitation by revoking the old one and creating a new one.
    ///
    /// Looks for a pending (unconsumed, unrevoke) invitation for the given email.
    /// Returns NotFound if no pending invitation exists.
    pub async fn resend_invitation_by_email(
        &self,
        email: String,
    ) -> Result<CreatedInvitation, UserInvitationError> {
        let now = (self.clock)();
        let normalized_email = normalize_email(&email);
        if normalized_email.is_empty() {
            return Err(UserInvitationError::InvalidInput);
        }

        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;

        // Find pending invitation for this email
        let invitation = sqlx::query_as::<_, PendingInvitation>(
            "SELECT normalized_email, display_name FROM invitations
             WHERE normalized_email = ? AND revoked_at IS NULL AND consumed_at IS NULL",
        )
        .bind(&normalized_email)
        .fetch_optional(&mut *transaction)
        .await?;

        let invitation = invitation.ok_or(UserInvitationError::NotFound)?;

        // Revoke old invitation
        sqlx::query(
            "UPDATE invitations SET revoked_at = ?
             WHERE normalized_email = ? AND revoked_at IS NULL AND consumed_at IS NULL",
        )
        .bind(now)
        .bind(&normalized_email)
        .execute(&mut *transaction)
        .await?;

        // Create new invitation (no actor for email-based resend)
        let created = self
            .insert_invitation_no_actor(
                &mut transaction,
                &invitation.normalized_email,
                invitation.display_name,
                now,
            )
            .await?;

        transaction.commit().await?;
        self.deliver_or_revoke(&created, invitation.normalized_email, 0, now)
            .await?;
        Ok(created)
    }

    async fn insert_invitation(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        email: &str,
        display_name: Option<String>,
        actor_user_id: i64,
        now: i64,
    ) -> Result<CreatedInvitation, UserInvitationError> {
        let token = self.secret_key.generate_token();
        let token_hash = self.secret_key.hash_token(TokenDomain::Invitation, &token);
        let result = sqlx::query(
            "INSERT INTO invitations (
                normalized_email, display_name, token_hash, expires_at, revoked_at,
                consumed_at, created_by_user_id, platform_role, created_at
             ) VALUES (?, ?, ?, ?, NULL, NULL, ?, 'user', ?)",
        )
        .bind(email)
        .bind(display_name)
        .bind(token_hash.as_bytes().as_slice())
        .bind(now + self.invitation_lifetime_seconds)
        .bind(actor_user_id)
        .bind(now)
        .execute(&mut **transaction)
        .await?;
        Ok(CreatedInvitation {
            invitation_id: result.last_insert_rowid(),
            token,
        })
    }

    async fn insert_invitation_no_actor(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        email: &str,
        display_name: Option<String>,
        now: i64,
    ) -> Result<CreatedInvitation, UserInvitationError> {
        let token = self.secret_key.generate_token();
        let token_hash = self.secret_key.hash_token(TokenDomain::Invitation, &token);
        let result = sqlx::query(
            "INSERT INTO invitations (
                normalized_email, display_name, token_hash, expires_at, revoked_at,
                consumed_at, created_by_user_id, platform_role, created_at
             ) VALUES (?, ?, ?, ?, NULL, NULL, NULL, 'user', ?)",
        )
        .bind(email)
        .bind(display_name)
        .bind(token_hash.as_bytes().as_slice())
        .bind(now + self.invitation_lifetime_seconds)
        .bind(now)
        .execute(&mut **transaction)
        .await?;
        Ok(CreatedInvitation {
            invitation_id: result.last_insert_rowid(),
            token,
        })
    }

    async fn deliver_or_revoke(
        &self,
        invitation: &CreatedInvitation,
        recipient: String,
        _actor_user_id: i64,
        now: i64,
    ) -> Result<(), UserInvitationError> {
        let Some(delivery) = &self.delivery else {
            return Ok(());
        };
        if delivery.send(recipient, &invitation.token).await.is_ok() {
            return Ok(());
        }

        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "UPDATE invitations SET revoked_at = ?
             WHERE id = ? AND revoked_at IS NULL AND consumed_at IS NULL",
        )
        .bind(now)
        .bind(invitation.invitation_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        tracing::error!(error_code = "user_invitation_delivery_failed");
        Err(UserInvitationError::DeliveryFailed)
    }
}

fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

#[derive(FromRow)]
struct PendingInvitation {
    normalized_email: String,
    display_name: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatedInvitation {
    pub invitation_id: i64,
    pub token: SecretToken,
}

#[derive(Debug, Clone)]
pub enum UserInvitationError {
    InvalidInput,
    NotFound,
    Conflict,
    DeliveryFailed,
    Database(Arc<sqlx::Error>),
}

impl PartialEq for UserInvitationError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::InvalidInput, Self::InvalidInput) => true,
            (Self::NotFound, Self::NotFound) => true,
            (Self::Conflict, Self::Conflict) => true,
            (Self::DeliveryFailed, Self::DeliveryFailed) => true,
            (Self::Database(a), Self::Database(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl Eq for UserInvitationError {}

impl Display for UserInvitationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("user invitation operation failed")
    }
}

impl Error for UserInvitationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Database(error) => Some(&**error),
            _ => None,
        }
    }
}

impl From<sqlx::Error> for UserInvitationError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(Arc::new(error))
    }
}
