use std::{
    error::Error,
    fmt::{self, Display, Formatter},
};

use serde::Serialize;

use crate::security::SecretToken;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaldavAccount {
    pub user_id: i64,
    pub principal_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CredentialMetadata {
    pub id: i64,
    pub label: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct IssuedCredential {
    pub metadata: CredentialMetadata,
    pub username: String,
    pub password: SecretToken,
    pub server_url: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ConnectionStatus {
    pub server_url: String,
    pub principal_id: Option<String>,
    pub credentials: Vec<CredentialMetadata>,
    pub last_successful_access_at: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DavSession {
    pub user_id: i64,
    pub credential_id: i64,
    pub principal_id: String,
}

#[derive(Debug)]
pub enum CaldavAuthError {
    InvalidCredentials,
    Revoked,
    RateLimited,
    Persistence,
}

impl Display for CaldavAuthError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCredentials => formatter.write_str("invalid credentials"),
            Self::Revoked => formatter.write_str("credential revoked"),
            Self::RateLimited => formatter.write_str("too many requests"),
            Self::Persistence => formatter.write_str("caldav account operation failed"),
        }
    }
}

impl Error for CaldavAuthError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}
