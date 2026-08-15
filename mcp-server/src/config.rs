use std::{
    env,
    net::SocketAddr,
    path::PathBuf,
    time::SystemTime,
};

use serde::Serialize;

/// Application environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppEnv {
    Development,
    Production,
}

impl AppEnv {
    pub fn from_env() -> Self {
        match env::var("APP_ENV").ok().as_deref() {
            Some("production") => Self::Production,
            _ => Self::Development,
        }
    }
}

/// Raw configuration parsed from environment before validation.
#[derive(Debug)]
pub struct RawConfig {
    pub app_env: AppEnv,
    pub oauth_issuer: String,
    pub internal_api_base: String,
    pub internal_api_key: String,
    pub session_secret: String,
    pub database_path: PathBuf,
    pub mcp_domain: Option<String>,
    pub public_resource_url: Option<String>,
    pub bind_address: String,
    pub dpop_key_path: Option<PathBuf>,
    pub rate_limit_enabled: bool,
    pub tracing_level: String,
}

/// Validated application configuration.
#[derive(Clone)]
pub struct Config {
    pub app_env: AppEnv,
    pub oauth_issuer: String,
    pub internal_api_base: String,
    pub internal_api_key: String,
    pub session_secret: String,
    pub database_path: PathBuf,
    pub mcp_domain: String,
    pub public_resource_url: String,
    pub bind_address: SocketAddr,
    pub dpop_key_path: Option<PathBuf>,
    pub rate_limit_enabled: bool,
    pub tracing_level: String,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("app_env", &self.app_env)
            .field("oauth_issuer", &"[redacted]")
            .field("internal_api_base", &self.internal_api_base)
            .field("internal_api_key", &"[redacted]")
            .field("session_secret", &"[redacted]")
            .field("database_path", &self.database_path)
            .field("mcp_domain", &self.mcp_domain)
            .field("public_resource_url", &self.public_resource_url)
            .field("bind_address", &self.bind_address)
            .field("dpop_key_path", &self.dpop_key_path)
            .field("rate_limit_enabled", &self.rate_limit_enabled)
            .field("tracing_level", &self.tracing_level)
            .finish()
    }
}

impl Config {
    /// Parse all configuration from environment variables.
    /// Returns raw config without validation.
    pub fn parse_env() -> RawConfig {
        let app_env = AppEnv::from_env();

        let oauth_issuer = env::var("MCP_OAUTH_ISSUER").unwrap_or_else(|_| {
            if app_env == AppEnv::Production {
                String::new()
            } else {
                "https://auth.commoncal.tld".into()
            }
        });

        let internal_api_base = env::var("MCP_INTERNAL_API_BASE").unwrap_or_else(|_| {
            if app_env == AppEnv::Production {
                String::new()
            } else {
                "https://commoncal-core.internal".into()
            }
        });

        let internal_api_key = env::var("MCP_INTERNAL_API_KEY").unwrap_or_else(|_| {
            if app_env == AppEnv::Production {
                String::new()
            } else {
                "mcp-internal-dev-key".into()
            }
        });

        let session_secret = env::var("MCP_SESSION_SECRET").unwrap_or_else(|_| {
            if app_env == AppEnv::Production {
                String::new()
            } else {
                "mcp-session-dev-secret-change-in-production".into()
            }
        });

        let database_path = env::var("MCP_DATABASE_PATH")
            .unwrap_or_else(|_| "mcp-server.db".into())
            .into();

        let mcp_domain = env::var("MCP_DOMAIN").ok();

        let public_resource_url = env::var("MCP_PUBLIC_RESOURCE_URL").ok();

        let bind_address = env::var("BIND_ADDRESS").unwrap_or_else(|_| {
            if app_env == AppEnv::Production {
                "0.0.0.0:3001".into()
            } else {
                "127.0.0.1:3001".into()
            }
        });

        let dpop_key_path = env::var("DPOP_KEY_PATH").ok().map(PathBuf::from);

        let rate_limit_enabled =
            env::var("MCP_RATE_LIMIT_ENABLED").ok().as_deref() == Some("1");

        let tracing_level =
            env::var("TRACING_LEVEL").unwrap_or_else(|_| "info".into());

        RawConfig {
            app_env,
            oauth_issuer,
            internal_api_base,
            internal_api_key,
            session_secret,
            database_path,
            mcp_domain,
            public_resource_url,
            bind_address,
            dpop_key_path,
            rate_limit_enabled,
            tracing_level,
        }
    }

    /// Validate raw configuration. Returns errors for production; returns Config for dev.
    pub fn validate(raw: RawConfig) -> Result<Self, Vec<ConfigError>> {
        let mut errors = Vec::new();

        // Validate required production fields.
        if raw.app_env == AppEnv::Production {
            if raw.oauth_issuer.is_empty() {
                errors.push(ConfigError::new("MCP_OAUTH_ISSUER is required in production"));
            } else if raw.oauth_issuer.contains("commoncal.tld") {
                errors.push(ConfigError::new(
                    "MCP_OAUTH_ISSUER must not contain placeholder domain 'commoncal.tld'",
                ));
            } else if raw.oauth_issuer.starts_with("http://") {
                errors.push(ConfigError::new(
                    "MCP_OAUTH_ISSUER must use HTTPS in production",
                ));
            }

            if raw.internal_api_base.is_empty() {
                errors.push(ConfigError::new(
                    "MCP_INTERNAL_API_BASE is required in production",
                ));
            } else             if raw.internal_api_base.contains("commoncal-core.internal") {
                errors.push(ConfigError::new(
                    "MCP_INTERNAL_API_BASE must not contain placeholder domain 'commoncal-core.internal'",
                ));
            } else if raw.internal_api_base.starts_with("http://") {
                errors.push(ConfigError::new(
                    "MCP_INTERNAL_API_BASE must use HTTPS in production",
                ));
            }

            if raw.internal_api_key.is_empty() {
                errors.push(ConfigError::new(
                    "MCP_INTERNAL_API_KEY is required in production",
                ));
            } else if raw.internal_api_key == "mcp-internal-dev-key" {
                errors.push(ConfigError::new(
                    "MCP_INTERNAL_API_KEY must not equal development placeholder",
                ));
            }

            if raw.session_secret.is_empty() {
                errors.push(ConfigError::new(
                    "MCP_SESSION_SECRET is required in production",
                ));
            } else if raw.session_secret.contains("dev-secret") {
                errors.push(ConfigError::new(
                    "MCP_SESSION_SECRET must not contain development placeholder",
                ));
            }

            if raw.mcp_domain.as_ref().map_or(true, |d| d.is_empty()) {
                errors.push(ConfigError::new(
                    "MCP_DOMAIN is required in production",
                ));
            }

            if raw.public_resource_url.as_ref().map_or(true, |u| u.is_empty()) {
                errors.push(ConfigError::new(
                    "MCP_PUBLIC_RESOURCE_URL is required in production",
                ));
            }

            if raw.database_path.as_os_str().is_empty() {
                errors.push(ConfigError::new(
                    "MCP_DATABASE_PATH is required in production",
                ));
            }

            if !raw.rate_limit_enabled {
                errors.push(ConfigError::new(
                    "MCP_RATE_LIMIT_ENABLED must be '1' in production",
                ));
            }
        }

        // Validate bind address in all environments.
        let bind_addr = match raw.bind_address.parse::<SocketAddr>() {
            Ok(addr) => addr,
            Err(e) => {
                errors.push(ConfigError::new(format!(
                    "BIND_ADDRESS '{}' is not a valid socket address: {}",
                    raw.bind_address, e
                )));
                return Err(errors);
            }
        };

        if errors.is_empty() {
            Ok(Self {
                app_env: raw.app_env,
                oauth_issuer: raw.oauth_issuer,
                internal_api_base: raw.internal_api_base,
                internal_api_key: raw.internal_api_key,
                session_secret: raw.session_secret,
                database_path: raw.database_path,
                mcp_domain: raw.mcp_domain.unwrap_or_default(),
                public_resource_url: raw.public_resource_url.unwrap_or_default(),
                bind_address: bind_addr,
                dpop_key_path: raw.dpop_key_path,
                rate_limit_enabled: raw.rate_limit_enabled,
                tracing_level: raw.tracing_level,
            })
        } else {
            Err(errors)
        }
    }

    /// Backwards-compatible entry point: parse and validate in one call.
    pub fn from_env() -> Result<Self, Vec<ConfigError>> {
        let raw = Self::parse_env();
        Self::validate(raw)
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

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    fn set_env(name: &str, val: &str) {
        unsafe { env::set_var(name, val) };
    }

    fn remove_env(name: &str) {
        unsafe { env::remove_var(name) };
    }

    fn remove_env_safe(name: &str) -> Option<String> {
        let orig = env::var(name).ok();
        if orig.is_some() {
            unsafe { env::remove_var(name) };
        }
        orig
    }

    #[test]
    #[serial]
    fn test_parse_env_default_app_env() {
        let orig = env::var("APP_ENV").ok();
        remove_env("APP_ENV");

        let raw = RawConfig {
            app_env: AppEnv::from_env(),
            ..Config::parse_env()
        };

        assert_eq!(raw.app_env, AppEnv::Development);

        if let Some(v) = orig {
            set_env("APP_ENV", &v);
        }
    }

    #[test]
    #[serial]
    fn test_parse_env_production() {
        set_env("APP_ENV", "production");
        let raw = Config::parse_env();
        assert_eq!(raw.app_env, AppEnv::Production);
        remove_env("APP_ENV");
    }

    #[test]
    #[serial]
    fn test_validate_production_missing_oauth_issuer() {
        remove_env("MCP_OAUTH_ISSUER");
        remove_env("MCP_INTERNAL_API_BASE");
        remove_env("MCP_INTERNAL_API_KEY");
        remove_env("MCP_SESSION_SECRET");
        remove_env("MCP_DOMAIN");
        remove_env("MCP_PUBLIC_RESOURCE_URL");
        set_env("APP_ENV", "production");
        set_env("MCP_DATABASE_PATH", "/tmp/test.db");
        let raw = Config::parse_env();
        let errors = Config::validate(raw).unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("MCP_OAUTH_ISSUER")));
        remove_env("APP_ENV");
        remove_env("MCP_DATABASE_PATH");
    }

    #[test]
    #[serial]
    fn test_validate_production_placeholder_oauth_issuer() {
        remove_env("MCP_OAUTH_ISSUER");
        set_env("APP_ENV", "production");
        set_env("MCP_OAUTH_ISSUER", "https://auth.commoncal.tld");
        let raw = Config::parse_env();
        let errors = Config::validate(raw).unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("placeholder")));
        remove_env("APP_ENV");
        remove_env("MCP_OAUTH_ISSUER");
    }

    #[test]
    #[serial]
    fn test_validate_production_oauth_issuer_http_rejected() {
        remove_env("MCP_OAUTH_ISSUER");
        set_env("APP_ENV", "production");
        set_env("MCP_OAUTH_ISSUER", "http://auth.example.com");
        let raw = Config::parse_env();
        let errors = Config::validate(raw).unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("HTTPS")));
        remove_env("APP_ENV");
        remove_env("MCP_OAUTH_ISSUER");
    }

    #[test]
    #[serial]
    fn test_validate_production_missing_api_key() {
        remove_env("MCP_OAUTH_ISSUER");
        remove_env("MCP_INTERNAL_API_BASE");
        remove_env("MCP_INTERNAL_API_KEY");
        remove_env("MCP_SESSION_SECRET");
        remove_env("MCP_DOMAIN");
        remove_env("MCP_PUBLIC_RESOURCE_URL");
        set_env("APP_ENV", "production");
        set_env("MCP_DATABASE_PATH", "/tmp/test.db");
        let raw = Config::parse_env();
        let errors = Config::validate(raw).unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("MCP_INTERNAL_API_KEY")));
        remove_env("APP_ENV");
        remove_env("MCP_DATABASE_PATH");
    }

    #[test]
    #[serial]
    fn test_validate_production_placeholder_api_key() {
        remove_env("MCP_OAUTH_ISSUER");
        remove_env("MCP_INTERNAL_API_BASE");
        remove_env("MCP_INTERNAL_API_KEY");
        remove_env("MCP_SESSION_SECRET");
        remove_env("MCP_DOMAIN");
        remove_env("MCP_PUBLIC_RESOURCE_URL");
        remove_env("MCP_DATABASE_PATH");
        set_env("APP_ENV", "production");
        set_env("MCP_INTERNAL_API_KEY", "mcp-internal-dev-key");
        let raw = Config::parse_env();
        let errors = Config::validate(raw).unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("placeholder")));
        remove_env("APP_ENV");
        remove_env("MCP_INTERNAL_API_KEY");
    }

    #[test]
    #[serial]
    fn test_validate_production_placeholder_secret() {
        remove_env("MCP_OAUTH_ISSUER");
        remove_env("MCP_INTERNAL_API_BASE");
        remove_env("MCP_INTERNAL_API_KEY");
        remove_env("MCP_SESSION_SECRET");
        remove_env("MCP_DOMAIN");
        remove_env("MCP_PUBLIC_RESOURCE_URL");
        remove_env("MCP_DATABASE_PATH");
        set_env("APP_ENV", "production");
        set_env("MCP_SESSION_SECRET", "mcp-session-dev-secret-change-in-production");
        let raw = Config::parse_env();
        let errors = Config::validate(raw).unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("development placeholder")));
        remove_env("APP_ENV");
        remove_env("MCP_SESSION_SECRET");
    }

    #[test]
    #[serial]
    fn test_validate_production_placeholder_api_base() {
        set_env("APP_ENV", "production");
        set_env("MCP_INTERNAL_API_BASE", "https://commoncal-core.internal");
        let raw = Config::parse_env();
        let errors = Config::validate(raw).unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("placeholder")));
        remove_env("APP_ENV");
        remove_env("MCP_INTERNAL_API_BASE");
    }

    #[test]
    #[serial]
    fn test_validate_production_api_base_http_rejected() {
        remove_env("MCP_OAUTH_ISSUER");
        remove_env("MCP_INTERNAL_API_BASE");
        remove_env("MCP_INTERNAL_API_KEY");
        remove_env("MCP_SESSION_SECRET");
        remove_env("MCP_DOMAIN");
        remove_env("MCP_PUBLIC_RESOURCE_URL");
        set_env("APP_ENV", "production");
        set_env("MCP_OAUTH_ISSUER", "https://auth.example.com");
        set_env("MCP_INTERNAL_API_BASE", "http://commoncal-core:3000");
        set_env("MCP_INTERNAL_API_KEY", "real-key-12345");
        set_env("MCP_SESSION_SECRET", "real-secret-12345");
        set_env("MCP_DOMAIN", "mcal.example.com");
        set_env("MCP_PUBLIC_RESOURCE_URL", "https://mcal.example.com/mcp");
        set_env("MCP_DATABASE_PATH", "/tmp/test.db");
        let raw = Config::parse_env();
        let errors = Config::validate(raw).unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("HTTPS")));
        remove_env("APP_ENV");
        remove_env("MCP_OAUTH_ISSUER");
        remove_env("MCP_INTERNAL_API_BASE");
        remove_env("MCP_INTERNAL_API_KEY");
        remove_env("MCP_SESSION_SECRET");
        remove_env("MCP_DOMAIN");
        remove_env("MCP_PUBLIC_RESOURCE_URL");
        remove_env("MCP_DATABASE_PATH");
    }

    #[test]
    #[serial]
    fn test_validate_development_accepts_defaults() {
        let orig = remove_env_safe("APP_ENV");
        let raw = Config::parse_env();
        let result = Config::validate(raw);
        assert!(result.is_ok());
        if let Some(v) = orig {
            set_env("APP_ENV", &v);
        }
    }

    #[test]
    #[serial]
    fn test_validate_production_missing_bind_address() {
        set_env("APP_ENV", "production");
        set_env("MCP_OAUTH_ISSUER", "https://auth.example.com");
        set_env("MCP_INTERNAL_API_BASE", "https://api.example.com");
        set_env("MCP_INTERNAL_API_KEY", "real-key-12345");
        set_env("MCP_SESSION_SECRET", "real-secret-12345");
        set_env("MCP_DOMAIN", "mcal.example.com");
        set_env("MCP_PUBLIC_RESOURCE_URL", "https://mcal.example.com/mcp");
        set_env("MCP_DATABASE_PATH", "/tmp/test.db");
        set_env("MCP_RATE_LIMIT_ENABLED", "1");
        remove_env("BIND_ADDRESS");
        let raw = Config::parse_env();
        let result = Config::validate(raw);
        assert!(result.is_ok());
        remove_env("APP_ENV");
        remove_env("MCP_OAUTH_ISSUER");
        remove_env("MCP_INTERNAL_API_BASE");
        remove_env("MCP_INTERNAL_API_KEY");
        remove_env("MCP_SESSION_SECRET");
        remove_env("MCP_DOMAIN");
        remove_env("MCP_PUBLIC_RESOURCE_URL");
        remove_env("MCP_DATABASE_PATH");
        remove_env("MCP_RATE_LIMIT_ENABLED");
    }

    #[test]
    #[serial]
    fn test_validate_production_bind_address_malformed() {
        remove_env("MCP_OAUTH_ISSUER");
        remove_env("MCP_INTERNAL_API_BASE");
        remove_env("MCP_INTERNAL_API_KEY");
        remove_env("MCP_SESSION_SECRET");
        remove_env("MCP_DOMAIN");
        remove_env("MCP_PUBLIC_RESOURCE_URL");
        remove_env("BIND_ADDRESS");
        set_env("APP_ENV", "production");
        set_env("MCP_OAUTH_ISSUER", "https://auth.example.com");
        set_env("MCP_INTERNAL_API_BASE", "https://api.example.com");
        set_env("MCP_INTERNAL_API_KEY", "real-key-12345");
        set_env("MCP_SESSION_SECRET", "real-secret-12345");
        set_env("MCP_DOMAIN", "mcal.example.com");
        set_env("MCP_PUBLIC_RESOURCE_URL", "https://mcal.example.com/mcp");
        set_env("BIND_ADDRESS", "not-a-port");
        let raw = Config::parse_env();
        let errors = Config::validate(raw).unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("BIND_ADDRESS")));
        remove_env("APP_ENV");
        remove_env("MCP_OAUTH_ISSUER");
        remove_env("MCP_INTERNAL_API_BASE");
        remove_env("MCP_INTERNAL_API_KEY");
        remove_env("MCP_SESSION_SECRET");
        remove_env("MCP_DOMAIN");
        remove_env("MCP_PUBLIC_RESOURCE_URL");
        remove_env("BIND_ADDRESS");
    }

    #[test]
    #[serial]
    fn test_validate_production_missing_mcp_domain() {
        remove_env("MCP_OAUTH_ISSUER");
        remove_env("MCP_INTERNAL_API_BASE");
        remove_env("MCP_INTERNAL_API_KEY");
        remove_env("MCP_SESSION_SECRET");
        remove_env("MCP_DOMAIN");
        remove_env("MCP_PUBLIC_RESOURCE_URL");
        set_env("APP_ENV", "production");
        set_env("MCP_OAUTH_ISSUER", "https://auth.example.com");
        set_env("MCP_INTERNAL_API_BASE", "https://api.example.com");
        set_env("MCP_INTERNAL_API_KEY", "real-key-12345");
        set_env("MCP_SESSION_SECRET", "real-secret-12345");
        let raw = Config::parse_env();
        let errors = Config::validate(raw).unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("MCP_DOMAIN")));
        remove_env("APP_ENV");
        remove_env("MCP_OAUTH_ISSUER");
        remove_env("MCP_INTERNAL_API_BASE");
        remove_env("MCP_INTERNAL_API_KEY");
        remove_env("MCP_SESSION_SECRET");
        remove_env("MCP_DOMAIN");
        remove_env("MCP_PUBLIC_RESOURCE_URL");
    }

    #[test]
    #[serial]
    fn test_validate_production_missing_public_resource_url() {
        set_env("APP_ENV", "production");
        set_env("MCP_OAUTH_ISSUER", "https://auth.example.com");
        set_env("MCP_INTERNAL_API_BASE", "https://api.example.com");
        set_env("MCP_INTERNAL_API_KEY", "real-key-12345");
        set_env("MCP_SESSION_SECRET", "real-secret-12345");
        set_env("MCP_DOMAIN", "mcal.example.com");
        remove_env("MCP_PUBLIC_RESOURCE_URL");
        let raw = Config::parse_env();
        let errors = Config::validate(raw).unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("MCP_PUBLIC_RESOURCE_URL")));
        remove_env("APP_ENV");
        remove_env("MCP_OAUTH_ISSUER");
        remove_env("MCP_INTERNAL_API_BASE");
        remove_env("MCP_INTERNAL_API_KEY");
        remove_env("MCP_SESSION_SECRET");
        remove_env("MCP_DOMAIN");
    }

    #[test]
    #[serial]
    fn test_parse_bind_address_valid() {
        let addr: SocketAddr = "0.0.0.0:3001".parse().unwrap();
        assert_eq!(addr.ip().to_string(), "0.0.0.0");
        assert_eq!(addr.port(), 3001);
    }

    #[test]
    #[serial]
    fn test_validate_production_rate_limit_required() {
        remove_env("MCP_OAUTH_ISSUER");
        remove_env("MCP_INTERNAL_API_BASE");
        remove_env("MCP_INTERNAL_API_KEY");
        remove_env("MCP_SESSION_SECRET");
        remove_env("MCP_DOMAIN");
        remove_env("MCP_PUBLIC_RESOURCE_URL");
        remove_env("MCP_DATABASE_PATH");
        remove_env("MCP_RATE_LIMIT_ENABLED");
        set_env("APP_ENV", "production");
        set_env("MCP_OAUTH_ISSUER", "https://auth.example.com");
        set_env("MCP_INTERNAL_API_BASE", "https://api.example.com");
        set_env("MCP_INTERNAL_API_KEY", "real-key-12345");
        set_env("MCP_SESSION_SECRET", "real-secret-12345");
        set_env("MCP_DOMAIN", "mcal.example.com");
        set_env("MCP_PUBLIC_RESOURCE_URL", "https://mcal.example.com/mcp");
        set_env("MCP_DATABASE_PATH", "/tmp/test.db");
        let raw = Config::parse_env();
        let errors = Config::validate(raw).unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("MCP_RATE_LIMIT_ENABLED")));
        remove_env("APP_ENV");
        remove_env("MCP_OAUTH_ISSUER");
        remove_env("MCP_INTERNAL_API_BASE");
        remove_env("MCP_INTERNAL_API_KEY");
        remove_env("MCP_SESSION_SECRET");
        remove_env("MCP_DOMAIN");
        remove_env("MCP_PUBLIC_RESOURCE_URL");
        remove_env("MCP_DATABASE_PATH");
    }

    #[test]
    #[serial]
    fn test_validate_production_rate_limit_enabled_accepted() {
        remove_env("MCP_OAUTH_ISSUER");
        remove_env("MCP_INTERNAL_API_BASE");
        remove_env("MCP_INTERNAL_API_KEY");
        remove_env("MCP_SESSION_SECRET");
        remove_env("MCP_DOMAIN");
        remove_env("MCP_PUBLIC_RESOURCE_URL");
        remove_env("MCP_DATABASE_PATH");
        remove_env("MCP_RATE_LIMIT_ENABLED");
        set_env("APP_ENV", "production");
        set_env("MCP_OAUTH_ISSUER", "https://auth.example.com");
        set_env("MCP_INTERNAL_API_BASE", "https://api.example.com");
        set_env("MCP_INTERNAL_API_KEY", "real-key-12345");
        set_env("MCP_SESSION_SECRET", "real-secret-12345");
        set_env("MCP_DOMAIN", "mcal.example.com");
        set_env("MCP_PUBLIC_RESOURCE_URL", "https://mcal.example.com/mcp");
        set_env("MCP_DATABASE_PATH", "/tmp/test.db");
        set_env("MCP_RATE_LIMIT_ENABLED", "1");
        let raw = Config::parse_env();
        let result = Config::validate(raw);
        assert!(result.is_ok());
        remove_env("APP_ENV");
        remove_env("MCP_OAUTH_ISSUER");
        remove_env("MCP_INTERNAL_API_BASE");
        remove_env("MCP_INTERNAL_API_KEY");
        remove_env("MCP_SESSION_SECRET");
        remove_env("MCP_DOMAIN");
        remove_env("MCP_PUBLIC_RESOURCE_URL");
        remove_env("MCP_DATABASE_PATH");
        remove_env("MCP_RATE_LIMIT_ENABLED");
    }

    #[test]
    #[serial]
    fn test_validate_production_missing_database_path() {
        remove_env("MCP_OAUTH_ISSUER");
        remove_env("MCP_INTERNAL_API_BASE");
        remove_env("MCP_INTERNAL_API_KEY");
        remove_env("MCP_SESSION_SECRET");
        remove_env("MCP_DOMAIN");
        remove_env("MCP_PUBLIC_RESOURCE_URL");
        remove_env("MCP_DATABASE_PATH");
        set_env("APP_ENV", "production");
        set_env("MCP_OAUTH_ISSUER", "https://auth.example.com");
        set_env("MCP_INTERNAL_API_BASE", "https://api.example.com");
        set_env("MCP_INTERNAL_API_KEY", "real-key-12345");
        set_env("MCP_SESSION_SECRET", "real-secret-12345");
        set_env("MCP_DOMAIN", "mcal.example.com");
        set_env("MCP_PUBLIC_RESOURCE_URL", "https://mcal.example.com/mcp");
        set_env("MCP_DATABASE_PATH", "");
        let raw = Config::parse_env();
        let errors = Config::validate(raw).unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("MCP_DATABASE_PATH")));
        remove_env("APP_ENV");
        remove_env("MCP_OAUTH_ISSUER");
        remove_env("MCP_INTERNAL_API_BASE");
        remove_env("MCP_INTERNAL_API_KEY");
        remove_env("MCP_SESSION_SECRET");
        remove_env("MCP_DOMAIN");
        remove_env("MCP_PUBLIC_RESOURCE_URL");
        remove_env("MCP_DATABASE_PATH");
    }

    #[test]
    #[serial]
    fn test_redacted_debug_excludes_secrets() {
        let orig = remove_env_safe("APP_ENV");
        remove_env_safe("MCP_DATABASE_PATH");
        let raw = Config::parse_env();
        let config = Config::validate(raw).unwrap();
        let debug_str = format!("{:?}", config);
        assert!(!debug_str.contains("mcp-internal-dev-key"));
        assert!(!debug_str.contains("dev-secret"));
        assert!(!debug_str.contains("auth.commoncal.tld"));
        if let Some(v) = orig {
            set_env("APP_ENV", &v);
        }
    }
}
