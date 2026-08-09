use std::{
    error::Error,
    fmt::{self, Display, Formatter},
};

use sqlx::{FromRow, SqlitePool};

#[derive(Clone)]
pub struct IdentityRepository {
    pool: SqlitePool,
}

impl IdentityRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn create_user(&self, user: NewUser) -> Result<User, RepositoryError> {
        let normalized_email = normalize_email(&user.normalized_email);
        let result = sqlx::query(
            "INSERT INTO users (normalized_email, display_name, status, created_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(&normalized_email)
        .bind(&user.display_name)
        .bind(user.status.as_str())
        .bind(user.created_at)
        .execute(&self.pool)
        .await?;

        Ok(User {
            id: result.last_insert_rowid(),
            normalized_email,
            display_name: user.display_name,
            status: user.status,
            created_at: user.created_at,
            password_hash: None,
        })
    }

    pub async fn user_by_normalized_email(
        &self,
        normalized_email: &str,
    ) -> Result<Option<User>, RepositoryError> {
        let record = sqlx::query_as::<_, UserRecord>(
            "SELECT id, normalized_email, display_name, status, created_at, password_hash
             FROM users WHERE normalized_email = ?",
        )
        .bind(normalize_email(normalized_email))
        .fetch_optional(&self.pool)
        .await?;

        record.map(User::try_from).transpose()
    }

    pub async fn user_by_normalized_email_with_password(
        &self,
        normalized_email: &str,
    ) -> Result<Option<UserWithPasswordRecord>, RepositoryError> {
        sqlx::query_as(
            "SELECT id, normalized_email, display_name, status, created_at, password_hash
             FROM users WHERE normalized_email = ?",
        )
        .bind(normalize_email(normalized_email))
        .fetch_optional(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub async fn set_password_hash(
        &self,
        user_id: i64,
        password_hash: &str,
    ) -> Result<(), RepositoryError> {
        sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
            .bind(password_hash)
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn create_invitation(
        &self,
        invitation: NewInvitation,
    ) -> Result<Invitation, RepositoryError> {
        let normalized_email = normalize_email(&invitation.normalized_email);
        let result = sqlx::query(
            "INSERT INTO invitations (
                normalized_email, display_name, token_hash, expires_at, revoked_at,
                consumed_at, created_by_user_id, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&normalized_email)
        .bind(&invitation.display_name)
        .bind(invitation.token_hash.as_bytes())
        .bind(invitation.expires_at)
        .bind(invitation.revoked_at)
        .bind(invitation.consumed_at)
        .bind(invitation.created_by_user_id)
        .bind(invitation.created_at)
        .execute(&self.pool)
        .await?;

        Ok(Invitation {
            id: result.last_insert_rowid(),
            normalized_email,
            display_name: invitation.display_name,
            token_hash: invitation.token_hash,
            expires_at: invitation.expires_at,
            revoked_at: invitation.revoked_at,
            consumed_at: invitation.consumed_at,
            created_by_user_id: Some(invitation.created_by_user_id),
            platform_role: InvitationPlatformRole::User,
            created_at: invitation.created_at,
        })
    }

    pub async fn invitation(&self, id: i64) -> Result<Option<Invitation>, RepositoryError> {
        sqlx::query_as(
            "SELECT id, normalized_email, display_name, token_hash, expires_at, revoked_at,
                    consumed_at, created_by_user_id, platform_role, created_at
             FROM invitations WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub async fn expired_invitations(
        &self,
        as_of: i64,
    ) -> Result<Vec<Invitation>, RepositoryError> {
        sqlx::query_as(
            "SELECT id, normalized_email, display_name, token_hash, expires_at, revoked_at,
                    consumed_at, created_by_user_id, platform_role, created_at
             FROM invitations WHERE expires_at <= ?",
        )
        .bind(as_of)
        .fetch_all(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub async fn revoked_invitations(&self) -> Result<Vec<Invitation>, RepositoryError> {
        sqlx::query_as(
            "SELECT id, normalized_email, display_name, token_hash, expires_at, revoked_at,
                    consumed_at, created_by_user_id, platform_role, created_at
             FROM invitations WHERE revoked_at IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub async fn create_login_token(
        &self,
        token: NewLoginToken,
    ) -> Result<LoginToken, RepositoryError> {
        let result = sqlx::query(
            "INSERT INTO login_tokens (
                user_id, token_hash, expires_at, revoked_at, consumed_at, created_at
             ) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(token.user_id)
        .bind(token.token_hash.as_bytes())
        .bind(token.expires_at)
        .bind(token.revoked_at)
        .bind(token.consumed_at)
        .bind(token.created_at)
        .execute(&self.pool)
        .await?;

        Ok(LoginToken {
            id: result.last_insert_rowid(),
            user_id: token.user_id,
            token_hash: token.token_hash,
            expires_at: token.expires_at,
            revoked_at: token.revoked_at,
            consumed_at: token.consumed_at,
            created_at: token.created_at,
        })
    }

    pub async fn login_token(&self, id: i64) -> Result<Option<LoginToken>, RepositoryError> {
        sqlx::query_as(
            "SELECT id, user_id, token_hash, expires_at, revoked_at, consumed_at, created_at
             FROM login_tokens WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub async fn expired_login_tokens(
        &self,
        as_of: i64,
    ) -> Result<Vec<LoginToken>, RepositoryError> {
        sqlx::query_as(
            "SELECT id, user_id, token_hash, expires_at, revoked_at, consumed_at, created_at
             FROM login_tokens WHERE expires_at <= ?",
        )
        .bind(as_of)
        .fetch_all(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub async fn revoked_login_tokens(&self) -> Result<Vec<LoginToken>, RepositoryError> {
        sqlx::query_as(
            "SELECT id, user_id, token_hash, expires_at, revoked_at, consumed_at, created_at
             FROM login_tokens WHERE revoked_at IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub async fn create_session(&self, session: NewSession) -> Result<Session, RepositoryError> {
        let result = sqlx::query(
            "INSERT INTO sessions (
                user_id, session_hash, expires_at, revoked_at, created_at, last_seen_at
             ) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(session.user_id)
        .bind(session.session_hash.as_bytes())
        .bind(session.expires_at)
        .bind(session.revoked_at)
        .bind(session.created_at)
        .bind(session.created_at)
        .execute(&self.pool)
        .await?;

        Ok(Session {
            id: result.last_insert_rowid(),
            user_id: session.user_id,
            session_hash: session.session_hash,
            expires_at: session.expires_at,
            revoked_at: session.revoked_at,
            created_at: session.created_at,
        })
    }

    pub async fn session(&self, id: i64) -> Result<Option<Session>, RepositoryError> {
        sqlx::query_as(
            "SELECT id, user_id, session_hash, expires_at, revoked_at, created_at
             FROM sessions WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub async fn expired_sessions(&self, as_of: i64) -> Result<Vec<Session>, RepositoryError> {
        sqlx::query_as(
            "SELECT id, user_id, session_hash, expires_at, revoked_at, created_at
             FROM sessions WHERE expires_at <= ?",
        )
        .bind(as_of)
        .fetch_all(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub async fn revoked_sessions(&self) -> Result<Vec<Session>, RepositoryError> {
        sqlx::query_as(
            "SELECT id, user_id, session_hash, expires_at, revoked_at, created_at
             FROM sessions WHERE revoked_at IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub async fn append_audit_entry(
        &self,
        entry: NewAuditEntry,
    ) -> Result<AuditEntry, RepositoryError> {
        let result = sqlx::query(
            "INSERT INTO audit_log (
                actor_user_id, action, target_type, target_id, metadata_json, created_at
             ) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(entry.actor_user_id)
        .bind(&entry.action)
        .bind(&entry.target_type)
        .bind(&entry.target_id)
        .bind(&entry.metadata_json)
        .bind(entry.created_at)
        .execute(&self.pool)
        .await?;

        Ok(AuditEntry {
            id: result.last_insert_rowid(),
            actor_user_id: entry.actor_user_id,
            action: entry.action,
            target_type: entry.target_type,
            target_id: entry.target_id,
            metadata_json: entry.metadata_json,
            created_at: entry.created_at,
        })
    }

    pub async fn audit_entry(&self, id: i64) -> Result<Option<AuditEntry>, RepositoryError> {
        sqlx::query_as(
            "SELECT id, actor_user_id, action, target_type, target_id, metadata_json, created_at
             FROM audit_log WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(Into::into)
    }
}

fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UserStatus {
    Invited,
    Active,
    Suspended,
    Deleted,
}

impl UserStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Invited => "invited",
            Self::Active => "active",
            Self::Suspended => "suspended",
            Self::Deleted => "deleted",
        }
    }
}

impl TryFrom<&str> for UserStatus {
    type Error = RepositoryError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "invited" => Ok(Self::Invited),
            "active" => Ok(Self::Active),
            "suspended" => Ok(Self::Suspended),
            "deleted" => Ok(Self::Deleted),
            value => Err(RepositoryError::InvalidUserStatus(value.to_owned())),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewUser {
    pub normalized_email: String,
    pub display_name: Option<String>,
    pub status: UserStatus,
    pub created_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct User {
    pub id: i64,
    pub normalized_email: String,
    pub display_name: Option<String>,
    pub status: UserStatus,
    pub created_at: i64,
    pub password_hash: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UserWithPassword {
    pub id: i64,
    pub normalized_email: String,
    pub display_name: Option<String>,
    pub status: UserStatus,
    pub created_at: i64,
    pub password_hash: Option<String>,
}

#[derive(FromRow)]
struct UserRecord {
    id: i64,
    normalized_email: String,
    display_name: Option<String>,
    status: String,
    created_at: i64,
    password_hash: Option<String>,
}

#[derive(Clone, Debug, FromRow)]
pub struct UserWithPasswordRecord {
    pub id: i64,
    pub normalized_email: String,
    pub display_name: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub password_hash: Option<String>,
}

impl TryFrom<UserRecord> for User {
    type Error = RepositoryError;

    fn try_from(record: UserRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            id: record.id,
            normalized_email: record.normalized_email,
            display_name: record.display_name,
            status: UserStatus::try_from(record.status.as_str())?,
            created_at: record.created_at,
            password_hash: record.password_hash,
        })
    }
}

impl TryFrom<UserWithPasswordRecord> for UserWithPassword {
    type Error = RepositoryError;

    fn try_from(record: UserWithPasswordRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            id: record.id,
            normalized_email: record.normalized_email,
            display_name: record.display_name,
            status: UserStatus::try_from(record.status.as_str())?,
            created_at: record.created_at,
            password_hash: record.password_hash,
        })
    }
}

macro_rules! hash_type {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, PartialEq, sqlx::Type)]
        #[sqlx(transparent)]
        pub struct $name(Vec<u8>);

        impl $name {
            pub fn new(bytes: Vec<u8>) -> Self {
                Self(bytes)
            }

            pub fn as_bytes(&self) -> &[u8] {
                &self.0
            }
        }
    };
}

hash_type!(InvitationHash);
hash_type!(LoginTokenHash);
hash_type!(SessionHash);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewInvitation {
    pub normalized_email: String,
    pub display_name: Option<String>,
    pub token_hash: InvitationHash,
    pub expires_at: i64,
    pub revoked_at: Option<i64>,
    pub consumed_at: Option<i64>,
    pub created_by_user_id: i64,
    pub created_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, FromRow)]
pub struct Invitation {
    pub id: i64,
    pub normalized_email: String,
    pub display_name: Option<String>,
    pub token_hash: InvitationHash,
    pub expires_at: i64,
    pub revoked_at: Option<i64>,
    pub consumed_at: Option<i64>,
    pub created_by_user_id: Option<i64>,
    pub platform_role: InvitationPlatformRole,
    pub created_at: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
pub enum InvitationPlatformRole {
    User,
    Superadmin,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewLoginToken {
    pub user_id: i64,
    pub token_hash: LoginTokenHash,
    pub expires_at: i64,
    pub revoked_at: Option<i64>,
    pub consumed_at: Option<i64>,
    pub created_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, FromRow)]
pub struct LoginToken {
    pub id: i64,
    pub user_id: i64,
    pub token_hash: LoginTokenHash,
    pub expires_at: i64,
    pub revoked_at: Option<i64>,
    pub consumed_at: Option<i64>,
    pub created_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewSession {
    pub user_id: i64,
    pub session_hash: SessionHash,
    pub expires_at: i64,
    pub revoked_at: Option<i64>,
    pub created_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, FromRow)]
pub struct Session {
    pub id: i64,
    pub user_id: i64,
    pub session_hash: SessionHash,
    pub expires_at: i64,
    pub revoked_at: Option<i64>,
    pub created_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewAuditEntry {
    pub actor_user_id: Option<i64>,
    pub action: String,
    pub target_type: String,
    pub target_id: Option<String>,
    pub metadata_json: Option<String>,
    pub created_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, FromRow)]
pub struct AuditEntry {
    pub id: i64,
    pub actor_user_id: Option<i64>,
    pub action: String,
    pub target_type: String,
    pub target_id: Option<String>,
    pub metadata_json: Option<String>,
    pub created_at: i64,
}

#[derive(Debug)]
pub enum RepositoryError {
    Database(sqlx::Error),
    InvalidUserStatus(String),
}

impl Display for RepositoryError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(error) => {
                write!(formatter, "identity database operation failed: {error}")
            }
            Self::InvalidUserStatus(status) => write!(formatter, "invalid user status: {status}"),
        }
    }
}

impl Error for RepositoryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            Self::InvalidUserStatus(_) => None,
        }
    }
}

impl From<sqlx::Error> for RepositoryError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}
