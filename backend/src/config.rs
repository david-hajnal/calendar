use std::{
    env,
    error::Error,
    fmt::{self, Display, Formatter},
    net::{AddrParseError, SocketAddr},
    path::{Path, PathBuf},
};

const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1:3000";
const DEFAULT_DATABASE_PATH: &str = "commoncal.sqlite";
const DEFAULT_APP_ORIGIN: &str = "http://127.0.0.1:3000";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Environment {
    Development,
    Production,
}

impl Environment {
    fn parse(value: &str) -> Result<Self, ConfigError> {
        match value {
            "development" => Ok(Self::Development),
            "production" => Ok(Self::Production),
            _ => Err(ConfigError::new(
                "APP_ENV must be either development or production",
            )),
        }
    }
}

#[derive(Clone)]
pub struct AppConfig {
    pub environment: Environment,
    pub bind_address: SocketAddr,
    pub access_log_level: tracing::level_filters::LevelFilter,
    database_path: PathBuf,
    session_secret: Option<String>,
    app_origin: String,
    caldav_public_origin: String,
    password_login_enabled: bool,
}

impl fmt::Debug for AppConfig {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AppConfig")
            .field("environment", &self.environment)
            .field("bind_address", &self.bind_address)
            .field("access_log_level", &self.access_log_level)
            .field("database_path", &self.database_path)
            .field(
                "session_secret",
                &self.session_secret.as_ref().map(|_| "[REDACTED]"),
            )
            .field("app_origin", &self.app_origin)
            .field("caldav_public_origin", &self.caldav_public_origin)
            .field("password_login_enabled", &self.password_login_enabled)
            .finish()
    }
}

impl AppConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let environment =
            Environment::parse(&env::var("APP_ENV").unwrap_or_else(|_| "development".into()))?;
        let bind_address = env::var("BIND_ADDRESS").unwrap_or_else(|_| DEFAULT_BIND_ADDRESS.into());
        let access_log_level = env::var("ACCESS_LOG_LEVEL")
            .ok()
            .map(|s| s.parse())
            .transpose()
            .map_err(|_| ConfigError::new("invalid ACCESS_LOG_LEVEL"))?
            .unwrap_or(tracing::level_filters::LevelFilter::DEBUG);
        let database_path =
            env::var("DATABASE_PATH").unwrap_or_else(|_| DEFAULT_DATABASE_PATH.into());
        let session_secret = env::var("SESSION_SECRET").ok();
        let app_origin = env::var("APP_ORIGIN").unwrap_or_else(|_| DEFAULT_APP_ORIGIN.into());
        let caldav_public_origin = env::var("CALDAV_PUBLIC_ORIGIN")
            .ok()
            .filter(|value| !value.is_empty());
        let _password_login_enabled = env::var("PASSWORD_LOGIN_ENABLED")
            .ok()
            .map(|s| s == "1" || s == "true" || s == "TRUE")
            .unwrap_or(false);

        let config = Self::with_database_path_and_origin_and_access_log_level(
            environment,
            &bind_address,
            session_secret,
            database_path,
            app_origin,
            access_log_level,
            _password_login_enabled,
        )?;
        config.with_caldav_public_origin(caldav_public_origin)
    }

    pub fn new(
        environment: Environment,
        bind_address: &str,
        session_secret: Option<String>,
    ) -> Result<Self, ConfigError> {
        Self::with_database_path(
            environment,
            bind_address,
            session_secret,
            DEFAULT_DATABASE_PATH,
        )
    }

    pub fn with_database_path(
        environment: Environment,
        bind_address: &str,
        session_secret: Option<String>,
        database_path: impl Into<PathBuf>,
    ) -> Result<Self, ConfigError> {
        Self::with_database_path_and_origin(
            environment,
            bind_address,
            session_secret,
            database_path,
            DEFAULT_APP_ORIGIN,
        )
    }

    pub fn with_database_path_and_origin(
        environment: Environment,
        bind_address: &str,
        session_secret: Option<String>,
        database_path: impl Into<PathBuf>,
        app_origin: impl Into<String>,
    ) -> Result<Self, ConfigError> {
        Self::with_database_path_and_origin_and_access_log_level(
            environment,
            bind_address,
            session_secret,
            database_path,
            app_origin,
            tracing::level_filters::LevelFilter::DEBUG,
            false,
        )
    }

    pub fn with_database_path_and_origin_and_access_log_level(
        environment: Environment,
        bind_address: &str,
        session_secret: Option<String>,
        database_path: impl Into<PathBuf>,
        app_origin: impl Into<String>,
        access_log_level: tracing::level_filters::LevelFilter,
        password_login_enabled: bool,
    ) -> Result<Self, ConfigError> {
        let bind_address = bind_address.parse().map_err(|error: AddrParseError| {
            ConfigError::new(format!("invalid BIND_ADDRESS: {error}"))
        })?;
        let database_path = database_path.into();
        let app_origin = app_origin.into();

        if environment == Environment::Production
            && session_secret.as_deref().is_none_or(str::is_empty)
        {
            return Err(ConfigError::new("SESSION_SECRET is required in production"));
        }
        if database_path.as_os_str().is_empty() {
            return Err(ConfigError::new("DATABASE_PATH must not be empty"));
        }
        if app_origin.is_empty()
            || app_origin.ends_with('/')
            || !(app_origin.starts_with("https://") || app_origin.starts_with("http://"))
        {
            return Err(ConfigError::new(
                "APP_ORIGIN must be an http(s) origin without a trailing slash",
            ));
        }

        let password_login_enabled = if environment == Environment::Production {
            password_login_enabled
        } else {
            true
        };

        Ok(Self {
            environment,
            bind_address,
            access_log_level,
            database_path,
            session_secret,
            caldav_public_origin: app_origin.clone(),
            app_origin,
            password_login_enabled,
        })
    }

    pub fn with_caldav_public_origin(
        mut self,
        caldav_public_origin: Option<String>,
    ) -> Result<Self, ConfigError> {
        let origin = match caldav_public_origin {
            Some(value) => value,
            None => {
                if self.environment == Environment::Production {
                    return Err(ConfigError::new(
                        "CALDAV_PUBLIC_ORIGIN is required in production",
                    ));
                }
                self.app_origin.clone()
            }
        };
        validate_caldav_public_origin(&origin, self.environment)?;
        self.caldav_public_origin = origin;
        Ok(self)
    }

    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    pub fn session_secret(&self) -> Option<&str> {
        self.session_secret.as_deref()
    }

    pub fn app_origin(&self) -> &str {
        &self.app_origin
    }

    pub fn caldav_public_origin(&self) -> &str {
        &self.caldav_public_origin
    }

    pub fn access_log_level(&self) -> tracing::level_filters::LevelFilter {
        self.access_log_level
    }

    pub fn password_login_enabled(&self) -> bool {
        self.password_login_enabled
    }
}

fn validate_caldav_public_origin(
    origin: &str,
    environment: Environment,
) -> Result<(), ConfigError> {
    if origin.is_empty()
        || origin.ends_with('/')
        || !(origin.starts_with("https://") || origin.starts_with("http://"))
    {
        return Err(ConfigError::new(
            "CALDAV_PUBLIC_ORIGIN must be an http(s) origin without a trailing slash",
        ));
    }
    let host_part = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
        .expect("scheme already validated");
    if host_part.contains('/') {
        return Err(ConfigError::new(
            "CALDAV_PUBLIC_ORIGIN must be an origin without a path",
        ));
    }
    if environment == Environment::Production && !origin.starts_with("https://") {
        return Err(ConfigError::new(
            "CALDAV_PUBLIC_ORIGIN must be an https origin in production",
        ));
    }
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
pub struct ConfigError {
    message: String,
}

impl ConfigError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Display for ConfigError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_config(environment: Environment) -> AppConfig {
        AppConfig::with_database_path_and_origin(
            environment,
            "127.0.0.1:3000",
            Some("secret".into()),
            "test.sqlite",
            "https://app.example",
        )
        .unwrap()
    }

    #[test]
    fn production_rejects_non_https_caldav_public_origin() {
        let config = base_config(Environment::Production);
        let result = config.with_caldav_public_origin(Some("http://dav.example".into()));
        assert!(result.is_err());
    }

    #[test]
    fn production_rejects_caldav_public_origin_with_path() {
        let config = base_config(Environment::Production);
        let result = config.with_caldav_public_origin(Some("https://dav.example/dav".into()));
        assert!(result.is_err());
    }

    #[test]
    fn production_requires_caldav_public_origin() {
        let config = base_config(Environment::Production);
        assert!(config.with_caldav_public_origin(None).is_err());
    }

    #[test]
    fn production_accepts_https_caldav_public_origin() {
        let config = base_config(Environment::Production);
        let config = config
            .with_caldav_public_origin(Some("https://dav.example".into()))
            .unwrap();
        assert_eq!(config.caldav_public_origin(), "https://dav.example");
    }

    #[test]
    fn development_allows_http_caldav_public_origin() {
        let config = base_config(Environment::Development);
        let config = config
            .with_caldav_public_origin(Some("http://127.0.0.1:3000".into()))
            .unwrap();
        assert_eq!(config.caldav_public_origin(), "http://127.0.0.1:3000");
    }

    #[test]
    fn development_defaults_caldav_public_origin_to_app_origin() {
        let config = base_config(Environment::Development);
        let config = config.with_caldav_public_origin(None).unwrap();
        assert_eq!(config.caldav_public_origin(), "https://app.example");
    }
}
