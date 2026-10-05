use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use sqlx::{FromRow, Sqlite, SqlitePool, Transaction};

use crate::{
    password::{PasswordError, hash_new_password},
    security::{SecretKey, SecretToken, TokenDomain},
};

const SUCCEEDED_ACTION: &str = "auth.invitation.consume.succeeded";
const FAILED_ACTION: &str = "auth.invitation.consume.failed";

#[derive(Clone)]
pub struct InvitationConsumer {
    pool: SqlitePool,
    secret_key: SecretKey,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl InvitationConsumer {
    pub fn new(pool: SqlitePool, secret_key: SecretKey) -> Self {
        Self {
            pool,
            secret_key,
            clock: Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system clock is before Unix epoch")
                    .as_secs() as i64
            }),
        }
    }

    pub fn new_at(pool: SqlitePool, secret_key: SecretKey, now: i64) -> Self {
        Self {
            pool,
            secret_key,
            clock: Arc::new(move || now),
        }
    }

    pub async fn preview(
        &self,
        token: String,
    ) -> Result<InvitationPreview, ConsumeInvitationError> {
        let token = SecretToken::parse(token).ok_or(ConsumeInvitationError::Invalid)?;
        let hash = self.secret_key.hash_token(TokenDomain::Invitation, &token);
        let email: Option<String> = sqlx::query_scalar(
            "SELECT i.normalized_email FROM invitations i
             LEFT JOIN users u ON u.normalized_email = i.normalized_email
             WHERE i.token_hash = ? AND i.revoked_at IS NULL AND i.consumed_at IS NULL
               AND i.expires_at > ? AND (u.id IS NULL OR u.status = 'invited')",
        )
        .bind(hash.as_bytes().as_slice())
        .bind((self.clock)())
        .fetch_optional(&self.pool)
        .await?;
        Ok(InvitationPreview {
            email: email.ok_or(ConsumeInvitationError::Invalid)?,
        })
    }

    pub async fn consume(
        &self,
        command: ConsumeInvitation,
    ) -> Result<ConsumedInvitation, ConsumeInvitationError> {
        let now = (self.clock)();
        let Some(invitation_token) = SecretToken::parse(command.token) else {
            audit_failure(&self.pool, None, "malformed_token", now).await?;
            return Err(ConsumeInvitationError::Invalid);
        };
        let invitation_hash = self
            .secret_key
            .hash_token(TokenDomain::Invitation, &invitation_token);
        // Reject unavailable tokens before doing expensive password work.
        // The transaction below rechecks the record after hashing to prevent races.
        let available = sqlx::query_as::<_, InvitationRecord>(
            "SELECT id, normalized_email, display_name, expires_at, revoked_at,
                    consumed_at, platform_role FROM invitations WHERE token_hash = ?",
        )
        .bind(invitation_hash.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await?;
        let Some(available) = available else {
            audit_failure(&self.pool, None, "token_not_found", now).await?;
            return Err(ConsumeInvitationError::Invalid);
        };
        if let Some(reason) = available.rejection_reason(now) {
            audit_failure(&self.pool, Some(available.id), reason, now).await?;
            return Err(ConsumeInvitationError::Invalid);
        }
        let password_hash = hash_new_password(command.password, command.password_confirmation)
            .await
            .map_err(ConsumeInvitationError::Password)?;
        let now = (self.clock)();
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let invitation = sqlx::query_as::<_, InvitationRecord>(
            "SELECT id, normalized_email, display_name, expires_at, revoked_at,
                    consumed_at, platform_role
             FROM invitations WHERE token_hash = ?",
        )
        .bind(invitation_hash.as_bytes().as_slice())
        .fetch_optional(&mut *transaction)
        .await?;

        let Some(invitation) = invitation else {
            audit_failure_in_transaction(&mut transaction, None, "token_not_found", now).await?;
            transaction.commit().await?;
            return Err(ConsumeInvitationError::Invalid);
        };

        if let Some(reason) = invitation.rejection_reason(now) {
            audit_failure_in_transaction(&mut transaction, Some(invitation.id), reason, now)
                .await?;
            transaction.commit().await?;
            return Err(ConsumeInvitationError::Invalid);
        }

        let existing_user = sqlx::query_as::<_, UserRecord>(
            "SELECT id, normalized_email, display_name, status, is_superadmin
             FROM users WHERE normalized_email = ?",
        )
        .bind(&invitation.normalized_email)
        .fetch_optional(&mut *transaction)
        .await?;

        let user = match existing_user {
            Some(user) if user.status != "invited" => {
                audit_failure_in_transaction(
                    &mut transaction,
                    Some(invitation.id),
                    "account_ineligible",
                    now,
                )
                .await?;
                transaction.commit().await?;
                return Err(ConsumeInvitationError::Invalid);
            }
            Some(user) => {
                let is_superadmin = user.is_superadmin || invitation.platform_role == "superadmin";
                sqlx::query(
                    "UPDATE users
                     SET status = 'registered',
                         display_name = COALESCE(display_name, ?),
                         is_superadmin = ?, password_hash = ?
                     WHERE id = ?",
                )
                .bind(&invitation.display_name)
                .bind(is_superadmin)
                .bind(&password_hash)
                .bind(user.id)
                .execute(&mut *transaction)
                .await?;
                ActiveUser {
                    id: user.id,
                    email: user.normalized_email,
                    display_name: user.display_name.or(invitation.display_name.clone()),
                    status: "registered",
                    is_superadmin,
                }
            }
            None => {
                let is_superadmin = invitation.platform_role == "superadmin";
                let inserted = sqlx::query(
                    "INSERT INTO users (
                        normalized_email, display_name, status, is_superadmin, created_at, password_hash
                     ) VALUES (?, ?, 'registered', ?, ?, ?)",
                )
                .bind(&invitation.normalized_email)
                .bind(&invitation.display_name)
                .bind(is_superadmin)
                .bind(now)
                .bind(&password_hash)
                .execute(&mut *transaction)
                .await?;
                ActiveUser {
                    id: inserted.last_insert_rowid(),
                    email: invitation.normalized_email.clone(),
                    display_name: invitation.display_name.clone(),
                    status: "registered",
                    is_superadmin,
                }
            }
        };

        sqlx::query("UPDATE invitations SET consumed_at = ? WHERE id = ? AND consumed_at IS NULL")
            .bind(now)
            .bind(invitation.id)
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            "UPDATE initial_superadmin_bootstrap
             SET consumed_at = ?
             WHERE invitation_id = ? AND consumed_at IS NULL",
        )
        .bind(now)
        .bind(invitation.id)
        .execute(&mut *transaction)
        .await?;

        audit_success(&mut transaction, invitation.id, user.id, now).await?;
        transaction.commit().await?;
        Ok(ConsumedInvitation { user })
    }
}

pub struct ConsumeInvitation {
    pub token: String,
    pub password: String,
    pub password_confirmation: String,
}

pub struct ConsumedInvitation {
    pub user: ActiveUser,
}

#[derive(Serialize)]
pub struct InvitationPreview {
    pub email: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ActiveUser {
    pub id: i64,
    pub email: String,
    pub display_name: Option<String>,
    pub status: &'static str,
    pub is_superadmin: bool,
}

#[derive(FromRow)]
struct InvitationRecord {
    id: i64,
    normalized_email: String,
    display_name: Option<String>,
    expires_at: i64,
    revoked_at: Option<i64>,
    consumed_at: Option<i64>,
    platform_role: String,
}

impl InvitationRecord {
    fn rejection_reason(&self, now: i64) -> Option<&'static str> {
        if self.revoked_at.is_some() {
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

#[derive(FromRow)]
struct UserRecord {
    id: i64,
    normalized_email: String,
    display_name: Option<String>,
    status: String,
    is_superadmin: bool,
}

async fn audit_success(
    transaction: &mut Transaction<'_, Sqlite>,
    invitation_id: i64,
    user_id: i64,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO audit_log (
            actor_user_id, action, target_type, target_id, metadata_json, created_at
         ) VALUES (?, ?, 'invitation', ?, ?, ?)",
    )
    .bind(user_id)
    .bind(SUCCEEDED_ACTION)
    .bind(invitation_id.to_string())
    .bind(r#"{"result":"activated"}"#)
    .bind(now)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn audit_failure_in_transaction(
    transaction: &mut Transaction<'_, Sqlite>,
    invitation_id: Option<i64>,
    reason: &'static str,
    now: i64,
) -> Result<(), sqlx::Error> {
    insert_failure_audit(&mut **transaction, invitation_id, reason, now).await
}

async fn audit_failure(
    pool: &SqlitePool,
    invitation_id: Option<i64>,
    reason: &'static str,
    now: i64,
) -> Result<(), sqlx::Error> {
    insert_failure_audit(pool, invitation_id, reason, now).await
}

async fn insert_failure_audit<'e, E>(
    executor: E,
    invitation_id: Option<i64>,
    reason: &'static str,
    now: i64,
) -> Result<(), sqlx::Error>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    let metadata = format!(r#"{{"reason":"{reason}"}}"#);
    sqlx::query(
        "INSERT INTO audit_log (
            actor_user_id, action, target_type, target_id, metadata_json, created_at
         ) VALUES (NULL, ?, 'invitation', ?, ?, ?)",
    )
    .bind(FAILED_ACTION)
    .bind(invitation_id.map(|id| id.to_string()))
    .bind(metadata)
    .bind(now)
    .execute(executor)
    .await?;
    Ok(())
}

#[derive(Debug)]
pub enum ConsumeInvitationError {
    Invalid,
    Password(PasswordError),
    Database(sqlx::Error),
}

impl Display for ConsumeInvitationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Password(_) => formatter.write_str("password creation failed"),
            Self::Invalid => formatter.write_str("invitation is invalid or expired"),
            Self::Database(_) => formatter.write_str("invitation consumption failed"),
        }
    }
}

impl Error for ConsumeInvitationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Invalid => None,
            Self::Password(error) => Some(error),
            Self::Database(error) => Some(error),
        }
    }
}

impl From<sqlx::Error> for ConsumeInvitationError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}
