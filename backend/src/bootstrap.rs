use std::{
    error::Error,
    fmt::{self, Display, Formatter},
};

use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::security::{SecretKey, SecretToken, TokenDomain};

const CREATED_ACTION: &str = "bootstrap.initial_superadmin.created";
const REJECTED_ACTION: &str = "bootstrap.initial_superadmin.rejected";

#[derive(Clone)]
pub struct InitialSuperadminBootstrap {
    pool: SqlitePool,
    secret_key: SecretKey,
}

impl InitialSuperadminBootstrap {
    pub fn new(pool: SqlitePool, secret_key: SecretKey) -> Self {
        Self { pool, secret_key }
    }

    pub async fn execute(
        &self,
        command: BootstrapCommand,
    ) -> Result<BootstrapResult, BootstrapError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;

        let user_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&mut *transaction)
            .await?;
        if user_count != 0 {
            audit_rejection(&mut transaction, "users_exist", command.created_at).await?;
            transaction.commit().await?;
            return Err(BootstrapError::UsersExist);
        }

        let bootstrap_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM initial_superadmin_bootstrap")
                .fetch_one(&mut *transaction)
                .await?;
        if bootstrap_count != 0 {
            audit_rejection(&mut transaction, "already_initialized", command.created_at).await?;
            transaction.commit().await?;
            return Err(BootstrapError::AlreadyInitialized);
        }

        let token = self.secret_key.generate_token();
        let token_hash = self.secret_key.hash_token(TokenDomain::Invitation, &token);
        let normalized_email = normalize_email(&command.normalized_email);
        let invitation = sqlx::query(
            "INSERT INTO invitations (
                normalized_email, display_name, token_hash, expires_at, revoked_at,
                consumed_at, created_by_user_id, platform_role, created_at
             ) VALUES (?, ?, ?, ?, NULL, NULL, NULL, 'superadmin', ?)",
        )
        .bind(normalized_email)
        .bind(command.display_name)
        .bind(token_hash.as_bytes().as_slice())
        .bind(command.expires_at)
        .bind(command.created_at)
        .execute(&mut *transaction)
        .await?;
        let invitation_id = invitation.last_insert_rowid();

        sqlx::query(
            "INSERT INTO initial_superadmin_bootstrap (
                singleton, invitation_id, consumed_at, created_at
             ) VALUES (1, ?, NULL, ?)",
        )
        .bind(invitation_id)
        .bind(command.created_at)
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            "INSERT INTO audit_log (
                actor_user_id, action, target_type, target_id, metadata_json, created_at
             ) VALUES (NULL, ?, 'invitation', ?, NULL, ?)",
        )
        .bind(CREATED_ACTION)
        .bind(invitation_id.to_string())
        .bind(command.created_at)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;

        Ok(BootstrapResult {
            invitation_id,
            token,
        })
    }
}

async fn audit_rejection(
    transaction: &mut Transaction<'_, Sqlite>,
    reason: &str,
    created_at: i64,
) -> Result<(), sqlx::Error> {
    let metadata = format!(r#"{{"reason":"{reason}"}}"#);
    sqlx::query(
        "INSERT INTO audit_log (
            actor_user_id, action, target_type, target_id, metadata_json, created_at
         ) VALUES (NULL, ?, 'bootstrap', NULL, ?, ?)",
    )
    .bind(REJECTED_ACTION)
    .bind(metadata)
    .bind(created_at)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BootstrapCommand {
    pub normalized_email: String,
    pub display_name: Option<String>,
    pub created_at: i64,
    pub expires_at: i64,
}

pub struct BootstrapResult {
    pub invitation_id: i64,
    pub token: SecretToken,
}

#[derive(Debug)]
pub enum BootstrapError {
    AlreadyInitialized,
    UsersExist,
    Database(sqlx::Error),
}

impl Display for BootstrapError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyInitialized => {
                formatter.write_str("initial superadmin bootstrap is already initialized")
            }
            Self::UsersExist => {
                formatter.write_str("initial superadmin bootstrap requires an empty user table")
            }
            Self::Database(_) => formatter.write_str("initial superadmin bootstrap failed"),
        }
    }
}

impl Error for BootstrapError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            Self::AlreadyInitialized | Self::UsersExist => None,
        }
    }
}

impl From<sqlx::Error> for BootstrapError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}
