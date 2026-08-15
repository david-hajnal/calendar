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
            .unwrap_or(tracing::level_filters::LevelFilter::INFO);
        let database_path =
            env::var("DATABASE_PATH").unwrap_or_else(|_| DEFAULT_DATABASE_PATH.into());
        let session_secret = env::var("SESSION_SECRET").ok();
        let app_origin = env::var("APP_ORIGIN").unwrap_or_else(|_| DEFAULT_APP_ORIGIN.into());
        let _password_login_enabled = env::var("PASSWORD_LOGIN_ENABLED")
            .ok()
            .map(|s| s == "1" || s == "true" || s == "TRUE")
            .unwrap_or(false);

        Self::with_database_path_and_origin_and_access_log_level(
            environment,
            &bind_address,
            session_secret,
            database_path,
            app_origin,
            access_log_level,
            _password_login_enabled,
        )
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
            tracing::level_filters::LevelFilter::INFO,
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

        Ok(Self {
            environment,
            bind_address,
            access_log_level,
            database_path,
            session_secret,
            app_origin,
            password_login_enabled,
        })
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

    pub fn access_log_level(&self) -> tracing::level_filters::LevelFilter {
        self.access_log_level
    }

    pub fn password_login_enabled(&self) -> bool {
        self.password_login_enabled
    }
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
