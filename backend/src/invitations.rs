use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use sqlx::{FromRow, Sqlite, SqlitePool, Transaction};

use crate::security::{CsrfToken, SecretKey, SecretToken, TokenDomain};

const SUCCEEDED_ACTION: &str = "auth.invitation.consume.succeeded";
const FAILED_ACTION: &str = "auth.invitation.consume.failed";

#[derive(Clone)]
pub struct InvitationConsumer {
    pool: SqlitePool,
    secret_key: SecretKey,
    session_lifetime_seconds: i64,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl InvitationConsumer {
    pub fn new(pool: SqlitePool, secret_key: SecretKey, session_lifetime_seconds: i64) -> Self {
        Self {
            pool,
            secret_key,
            session_lifetime_seconds,
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
        session_lifetime_seconds: i64,
        now: i64,
    ) -> Self {
        Self {
            pool,
            secret_key,
            session_lifetime_seconds,
            clock: Arc::new(move || now),
        }
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
            Some(user) if user.status == "suspended" || user.status == "deleted" => {
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
                     SET status = 'active',
                         display_name = COALESCE(display_name, ?),
                         is_superadmin = ?
                     WHERE id = ?",
                )
                .bind(&invitation.display_name)
                .bind(is_superadmin)
                .bind(user.id)
                .execute(&mut *transaction)
                .await?;
                ActiveUser {
                    id: user.id,
                    email: user.normalized_email,
                    display_name: user.display_name.or(invitation.display_name.clone()),
                    status: "active",
                    is_superadmin,
                }
            }
            None => {
                let is_superadmin = invitation.platform_role == "superadmin";
                let inserted = sqlx::query(
                    "INSERT INTO users (
                        normalized_email, display_name, status, is_superadmin, created_at
                     ) VALUES (?, ?, 'active', ?, ?)",
                )
                .bind(&invitation.normalized_email)
                .bind(&invitation.display_name)
                .bind(is_superadmin)
                .bind(now)
                .execute(&mut *transaction)
                .await?;
                ActiveUser {
                    id: inserted.last_insert_rowid(),
                    email: invitation.normalized_email.clone(),
                    display_name: invitation.display_name.clone(),
                    status: "active",
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

        if let Some(prior_session_token) = command.prior_session_token.and_then(SecretToken::parse)
        {
            let prior_hash = self
                .secret_key
                .hash_token(TokenDomain::Session, &prior_session_token);
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
        let session_result = sqlx::query(
            "INSERT INTO sessions (
                user_id, session_hash, expires_at, revoked_at, created_at, last_seen_at
             ) VALUES (?, ?, ?, NULL, ?, ?)",
        )
        .bind(user.id)
        .bind(session_hash.as_bytes().as_slice())
        .bind(now + self.session_lifetime_seconds)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await;
        if let Err(error) = session_result {
            transaction.rollback().await?;
            audit_failure(
                &self.pool,
                Some(invitation.id),
                "session_creation_failed",
                now,
            )
            .await?;
            return Err(ConsumeInvitationError::Database(error));
        }

        audit_success(&mut transaction, invitation.id, user.id, now).await?;
        transaction.commit().await?;
        let csrf_token = self.secret_key.generate_csrf_token(&session_token);

        Ok(ConsumedInvitation {
            user,
            session_token,
            csrf_token,
        })
    }
}

pub struct ConsumeInvitation {
    pub token: String,
    pub prior_session_token: Option<String>,
}

pub struct ConsumedInvitation {
    pub user: ActiveUser,
    pub session_token: SecretToken,
    pub csrf_token: CsrfToken,
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
    Database(sqlx::Error),
}

impl Display for ConsumeInvitationError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid => formatter.write_str("invitation is invalid or expired"),
            Self::Database(_) => formatter.write_str("invitation consumption failed"),
        }
    }
}

impl Error for ConsumeInvitationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Invalid => None,
            Self::Database(error) => Some(error),
        }
    }
}

impl From<sqlx::Error> for ConsumeInvitationError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}
