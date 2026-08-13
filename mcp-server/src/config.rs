use std::{
    env,
    path::PathBuf,
    time::SystemTime,
};

use serde::Serialize;

#[derive(Clone, Debug)]
pub struct Config {
    pub oauth_issuer: String,
    pub internal_api_base: String,
    pub internal_api_key: String,
    pub session_secret: String,
    pub database_path: PathBuf,
    pub dpop_key_path: Option<PathBuf>,
    pub rate_limit_enabled: bool,
    pub tracing_level: String,
    pub bind_address: String,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let oauth_issuer =
            env::var("MCP_OAUTH_ISSUER").unwrap_or_else(|_| "https://auth.commoncal.tld".into());

        let internal_api_base = env::var("MCP_INTERNAL_API_BASE").unwrap_or_else(|_| {
            "https://commoncal-core.internal".into()
        });

        let internal_api_key = env::var("MCP_INTERNAL_API_KEY").unwrap_or_else(|_| {
            "mcp-internal-dev-key".into()
        });

        let session_secret = env::var("MCP_SESSION_SECRET").unwrap_or_else(|_| {
            "mcp-session-dev-secret-change-in-production".into()
        });

        let database_path = env::var("MCP_DATABASE_PATH")
            .unwrap_or_else(|_| "mcp-server.db".into())
            .into();

        let dpop_key_path = env::var("DPOP_KEY_PATH").ok().map(PathBuf::from);

        let rate_limit_enabled =
            env::var("MCP_RATE_LIMIT_ENABLED").ok().as_deref() == Some("1");

        let tracing_level =
            env::var("TRACING_LEVEL").unwrap_or_else(|_| "info".into());

        let bind_address =
            env::var("BIND_ADDRESS").unwrap_or_else(|_| "127.0.0.1:3001".into());

        Ok(Self {
            oauth_issuer,
            internal_api_base,
            internal_api_key,
            session_secret,
            database_path,
            dpop_key_path,
            rate_limit_enabled,
            tracing_level,
            bind_address,
        })
    }
}

#[derive(Debug)]
pub struct ConfigError {
    pub message: String,
}

impl ConfigError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ConfigError {}

#[derive(Serialize)]
pub struct OauthProtectedResourceMetadata {
    pub resource_metadata: Option<String>,
    pub resource: String,
    pub authorization_servers: Vec<String>,
    pub scopes_supported: Vec<String>,
    pub dpop_bound_access_tokens: bool,
}

impl OauthProtectedResourceMetadata {
    pub fn new(resource_url: &str, auth_issuer: &str) -> Self {
        Self {
            resource_metadata: None,
            resource: resource_url.to_string(),
            authorization_servers: vec![auth_issuer.to_string()],
            scopes_supported: vec![
                "commoncal.calendar.metadata.read".to_string(),
                "commoncal.availability.read".to_string(),
                "commoncal.event.read.basic".to_string(),
                "commoncal.event.read.details".to_string(),
                "commoncal.event.create".to_string(),
                "commoncal.event.update".to_string(),
                "commoncal.event.delete".to_string(),
                "commoncal.reminder.read".to_string(),
                "commoncal.reminder.write".to_string(),
            ],
            dpop_bound_access_tokens: true,
        }
    }
}

pub fn current_time_secs() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
