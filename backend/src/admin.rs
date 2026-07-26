use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    future::Future,
    pin::Pin,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
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
pub struct AdminService {
    pool: SqlitePool,
    secret_key: SecretKey,
    invitation_lifetime_seconds: i64,
    delivery: Option<Arc<dyn InvitationDelivery>>,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl AdminService {
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

    pub async fn list_users(
        &self,
        status: Option<&str>,
        page: u32,
        per_page: u32,
    ) -> Result<UserPage, AdminError> {
        if let Some(status) = status
            && !matches!(status, "invited" | "active" | "suspended" | "deleted")
        {
            return Err(AdminError::InvalidInput);
        }
        if page == 0 || per_page == 0 || per_page > 100 {
            return Err(AdminError::InvalidInput);
        }
        let offset = i64::from(page - 1) * i64::from(per_page);
        let (users, total) = match status {
            Some(status) => {
                let users = sqlx::query_as::<_, UserSummary>(
                    "SELECT id, normalized_email AS email, display_name, status, is_superadmin,
                            created_at
                     FROM users WHERE status = ?
                     ORDER BY id LIMIT ? OFFSET ?",
                )
                .bind(status)
                .bind(per_page)
                .bind(offset)
                .fetch_all(&self.pool)
                .await?;
                let total = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE status = ?")
                    .bind(status)
                    .fetch_one(&self.pool)
                    .await?;
                (users, total)
            }
            None => {
                let users = sqlx::query_as::<_, UserSummary>(
                    "SELECT id, normalized_email AS email, display_name, status, is_superadmin,
                            created_at
                     FROM users ORDER BY id LIMIT ? OFFSET ?",
                )
                .bind(per_page)
                .bind(offset)
                .fetch_all(&self.pool)
                .await?;
                let total = sqlx::query_scalar("SELECT COUNT(*) FROM users")
                    .fetch_one(&self.pool)
                    .await?;
                (users, total)
            }
        };
        Ok(UserPage {
            users,
            page,
            per_page,
            total,
        })
    }

    pub async fn invite(
        &self,
        actor_user_id: i64,
        command: InviteUser,
    ) -> Result<CreatedInvitation, AdminError> {
        let now = (self.clock)();
        let email = normalize_email(&command.email);
        if email.is_empty() {
            return Err(AdminError::InvalidInput);
        }
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let existing_user: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE normalized_email = ?")
                .bind(&email)
                .fetch_one(&mut *transaction)
                .await?;
        let pending: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM invitations
             WHERE normalized_email = ? AND revoked_at IS NULL AND consumed_at IS NULL",
        )
        .bind(&email)
        .fetch_one(&mut *transaction)
        .await?;
        if existing_user != 0 || pending != 0 {
            transaction.rollback().await?;
            return Err(AdminError::Conflict);
        }
        let created = self
            .insert_invitation(
                &mut transaction,
                &email,
                command.display_name,
                actor_user_id,
                now,
            )
            .await?;
        audit(
            &mut transaction,
            actor_user_id,
            "admin.invitation.create",
            "invitation",
            created.invitation_id,
            now,
        )
        .await?;
        transaction.commit().await?;
        self.deliver_or_revoke(&created, email, actor_user_id, now)
            .await?;
        Ok(created)
    }

    pub async fn revoke_invitation(
        &self,
        actor_user_id: i64,
        invitation_id: i64,
    ) -> Result<(), AdminError> {
        let now = (self.clock)();
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let result = sqlx::query(
            "UPDATE invitations SET revoked_at = ?
             WHERE id = ? AND revoked_at IS NULL AND consumed_at IS NULL",
        )
        .bind(now)
        .bind(invitation_id)
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() != 1 {
            transaction.rollback().await?;
            return Err(AdminError::NotFound);
        }
        audit(
            &mut transaction,
            actor_user_id,
            "admin.invitation.revoke",
            "invitation",
            invitation_id,
            now,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn resend_invitation(
        &self,
        actor_user_id: i64,
        invitation_id: i64,
    ) -> Result<CreatedInvitation, AdminError> {
        let now = (self.clock)();
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let invitation = sqlx::query_as::<_, PendingInvitation>(
            "SELECT normalized_email, display_name FROM invitations
             WHERE id = ? AND revoked_at IS NULL AND consumed_at IS NULL",
        )
        .bind(invitation_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(AdminError::NotFound)?;
        sqlx::query("UPDATE invitations SET revoked_at = ? WHERE id = ?")
            .bind(now)
            .bind(invitation_id)
            .execute(&mut *transaction)
            .await?;
        let created = self
            .insert_invitation(
                &mut transaction,
                &invitation.normalized_email,
                invitation.display_name,
                actor_user_id,
                now,
            )
            .await?;
        audit(
            &mut transaction,
            actor_user_id,
            "admin.invitation.resend",
            "invitation",
            created.invitation_id,
            now,
        )
        .await?;
        transaction.commit().await?;
        self.deliver_or_revoke(&created, invitation.normalized_email, actor_user_id, now)
            .await?;
        Ok(created)
    }

    pub async fn suspend_user(&self, actor_user_id: i64, user_id: i64) -> Result<(), AdminError> {
        let now = (self.clock)();
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        protect_final_active_superadmin(&mut transaction, user_id).await?;
        let result =
            sqlx::query("UPDATE users SET status = 'suspended' WHERE id = ? AND status = 'active'")
                .bind(user_id)
                .execute(&mut *transaction)
                .await?;
        if result.rows_affected() != 1 {
            transaction.rollback().await?;
            return Err(AdminError::NotFound);
        }
        sqlx::query(
            "UPDATE sessions SET revoked_at = ?
             WHERE user_id = ? AND revoked_at IS NULL",
        )
        .bind(now)
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
        audit(
            &mut transaction,
            actor_user_id,
            "admin.user.suspend",
            "user",
            user_id,
            now,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn reactivate_user(
        &self,
        actor_user_id: i64,
        user_id: i64,
    ) -> Result<(), AdminError> {
        self.update_user(
            actor_user_id,
            user_id,
            "UPDATE users SET status = 'active' WHERE id = ? AND status = 'suspended'",
            "admin.user.reactivate",
        )
        .await
    }

    pub async fn promote_user(&self, actor_user_id: i64, user_id: i64) -> Result<(), AdminError> {
        self.update_user(
            actor_user_id,
            user_id,
            "UPDATE users SET is_superadmin = 1
             WHERE id = ? AND status = 'active' AND is_superadmin = 0",
            "admin.user.promote",
        )
        .await
    }

    pub async fn demote_user(&self, actor_user_id: i64, user_id: i64) -> Result<(), AdminError> {
        let now = (self.clock)();
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        protect_final_active_superadmin(&mut transaction, user_id).await?;
        let result =
            sqlx::query("UPDATE users SET is_superadmin = 0 WHERE id = ? AND is_superadmin = 1")
                .bind(user_id)
                .execute(&mut *transaction)
                .await?;
        if result.rows_affected() != 1 {
            transaction.rollback().await?;
            return Err(AdminError::NotFound);
        }
        audit(
            &mut transaction,
            actor_user_id,
            "admin.user.demote",
            "user",
            user_id,
            now,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn revoke_sessions(
        &self,
        actor_user_id: i64,
        user_id: i64,
    ) -> Result<(), AdminError> {
        let now = (self.clock)();
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_one(&mut *transaction)
            .await?;
        if exists == 0 {
            transaction.rollback().await?;
            return Err(AdminError::NotFound);
        }
        sqlx::query(
            "UPDATE sessions SET revoked_at = ?
             WHERE user_id = ? AND revoked_at IS NULL",
        )
        .bind(now)
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
        audit(
            &mut transaction,
            actor_user_id,
            "admin.user.revoke_sessions",
            "user",
            user_id,
            now,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    async fn update_user(
        &self,
        actor_user_id: i64,
        user_id: i64,
        statement: &'static str,
        action: &'static str,
    ) -> Result<(), AdminError> {
        let now = (self.clock)();
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let result = sqlx::query(statement)
            .bind(user_id)
            .execute(&mut *transaction)
            .await?;
        if result.rows_affected() != 1 {
            transaction.rollback().await?;
            return Err(AdminError::NotFound);
        }
        audit(
            &mut transaction,
            actor_user_id,
            action,
            "user",
            user_id,
            now,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    async fn insert_invitation(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        email: &str,
        display_name: Option<String>,
        actor_user_id: i64,
        now: i64,
    ) -> Result<CreatedInvitation, AdminError> {
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

    async fn deliver_or_revoke(
        &self,
        invitation: &CreatedInvitation,
        recipient: String,
        actor_user_id: i64,
        now: i64,
    ) -> Result<(), AdminError> {
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
        audit(
            &mut transaction,
            actor_user_id,
            "admin.invitation.delivery_failed",
            "invitation",
            invitation.invitation_id,
            now,
        )
        .await?;
        transaction.commit().await?;
        tracing::error!(error_code = "admin_invitation_delivery_failed");
        Err(AdminError::DeliveryFailed)
    }
}

async fn protect_final_active_superadmin(
    transaction: &mut Transaction<'_, Sqlite>,
    user_id: i64,
) -> Result<(), AdminError> {
    let target: Option<(String, bool)> =
        sqlx::query_as("SELECT status, is_superadmin FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(&mut **transaction)
            .await?;
    let Some((status, is_superadmin)) = target else {
        return Err(AdminError::NotFound);
    };
    if status == "active" && is_superadmin {
        let active_superadmins: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM users WHERE status = 'active' AND is_superadmin = 1",
        )
        .fetch_one(&mut **transaction)
        .await?;
        if active_superadmins <= 1 {
            return Err(AdminError::FinalActiveSuperadmin);
        }
    }
    Ok(())
}

async fn audit(
    transaction: &mut Transaction<'_, Sqlite>,
    actor_user_id: i64,
    action: &'static str,
    target_type: &'static str,
    target_id: i64,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO audit_log (
            actor_user_id, action, target_type, target_id, metadata_json, created_at
         ) VALUES (?, ?, ?, ?, NULL, ?)",
    )
    .bind(actor_user_id)
    .bind(action)
    .bind(target_type)
    .bind(target_id.to_string())
    .bind(now)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

#[derive(Clone)]
pub struct InviteUser {
    pub email: String,
    pub display_name: Option<String>,
}

pub struct CreatedInvitation {
    pub invitation_id: i64,
    pub token: SecretToken,
}

#[derive(Debug, Serialize, FromRow)]
pub struct UserSummary {
    pub id: i64,
    pub email: String,
    pub display_name: Option<String>,
    pub status: String,
    pub is_superadmin: bool,
    pub created_at: i64,
}

#[derive(Debug, Serialize)]
pub struct UserPage {
    pub users: Vec<UserSummary>,
    pub page: u32,
    pub per_page: u32,
    pub total: i64,
}

#[derive(FromRow)]
struct PendingInvitation {
    normalized_email: String,
    display_name: Option<String>,
}

#[derive(Debug)]
pub enum AdminError {
    InvalidInput,
    NotFound,
    Conflict,
    FinalActiveSuperadmin,
    DeliveryFailed,
    Database(sqlx::Error),
}

impl Display for AdminError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("user administration operation failed")
    }
}

impl Error for AdminError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            _ => None,
        }
    }
}

impl From<sqlx::Error> for AdminError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}
