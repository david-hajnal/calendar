use crate::{
    config::Environment,
    email::{ProductionEmailProvider, ProviderEmail, ProviderError},
};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, header::ContentType},
    transport::smtp::{
        authentication::Credentials,
        client::{Tls, TlsParameters},
    },
};
use std::{fmt, time::Duration};

#[derive(Clone)]
pub struct MailConfig {
    host: String,
    port: u16,
    username: String,
    password: String,
    from: Mailbox,
}
impl fmt::Debug for MailConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MailConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("credentials", &"[REDACTED]")
            .field("from", &self.from)
            .finish()
    }
}
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct MailConfigurationError(&'static str);
impl fmt::Display for MailConfigurationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for MailConfigurationError {}
impl MailConfig {
    pub fn new(
        host: String,
        port: u16,
        username: String,
        password: String,
        from: String,
    ) -> Result<Self, MailConfigurationError> {
        if host.trim() != host
            || host.is_empty()
            || host.contains(['/', '@', '\\', '\r', '\n'])
            || url::Host::parse(&host).is_err()
        {
            return Err(MailConfigurationError(
                "SMTP_HOST must be a hostname or IP address",
            ));
        }
        if port == 0 {
            return Err(MailConfigurationError(
                "SMTP_PORT must be between 1 and 65535",
            ));
        }
        if username.trim().is_empty() || password.is_empty() {
            return Err(MailConfigurationError(
                "SMTP_USERNAME and SMTP_PASSWORD are required",
            ));
        }
        let from = from
            .parse::<Mailbox>()
            .map_err(|_| MailConfigurationError("SMTP_FROM must be a valid sender mailbox"))?;
        Ok(Self {
            host,
            port,
            username,
            password,
            from,
        })
    }
    pub fn from_env(environment: Environment) -> Result<Option<Self>, MailConfigurationError> {
        Self::from_values(environment, |name| std::env::var(name).ok())
    }
    pub fn from_values(
        environment: Environment,
        value: impl Fn(&str) -> Option<String>,
    ) -> Result<Option<Self>, MailConfigurationError> {
        if environment == Environment::Development {
            return Ok(None);
        }
        let required = |name| {
            value(name).filter(|v| !v.is_empty()).ok_or(MailConfigurationError("Production requires SMTP_HOST, SMTP_PORT, SMTP_USERNAME, SMTP_PASSWORD, and SMTP_FROM"))
        };
        let host = required("SMTP_HOST")?;
        let port = required("SMTP_PORT")?
            .parse::<u16>()
            .map_err(|_| MailConfigurationError("SMTP_PORT must be between 1 and 65535"))?;
        Ok(Some(Self::new(
            host,
            port,
            required("SMTP_USERNAME")?,
            required("SMTP_PASSWORD")?,
            required("SMTP_FROM")?,
        )?))
    }
}
pub struct SmtpProvider {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}
impl fmt::Debug for SmtpProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SmtpProvider([REDACTED])")
    }
}
impl SmtpProvider {
    pub fn new(config: MailConfig) -> Result<Self, MailConfigurationError> {
        let tls = TlsParameters::new(config.host.clone())
            .map_err(|_| MailConfigurationError("Invalid SMTP TLS configuration"))?;
        Self::with_tls(config, tls)
    }
    fn with_tls(config: MailConfig, tls: TlsParameters) -> Result<Self, MailConfigurationError> {
        let implicit_tls = config.port == 465;
        Self::with_tls_mode(config, tls, implicit_tls)
    }
    fn with_tls_mode(
        config: MailConfig,
        tls: TlsParameters,
        implicit_tls: bool,
    ) -> Result<Self, MailConfigurationError> {
        // STARTTLS is mandatory except on the standard implicit-TLS port.
        // The builder is private and never exposes plaintext/opportunistic TLS.
        let mode = if implicit_tls {
            Tls::Wrapper(tls)
        } else {
            Tls::Required(tls)
        };
        let transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&config.host)
            .port(config.port)
            .tls(mode)
            .credentials(Credentials::new(config.username, config.password))
            .timeout(Some(Duration::from_secs(10)))
            .build();
        Ok(Self {
            transport,
            from: config.from,
        })
    }
}
impl ProductionEmailProvider for SmtpProvider {
    async fn send(&self, email: ProviderEmail) -> Result<(), ProviderError> {
        let to = email
            .recipient()
            .parse::<Mailbox>()
            .map_err(|_| ProviderError::new())?;
        let message = Message::builder()
            .from(self.from.clone())
            .to(to)
            .subject(email.subject())
            .header(ContentType::TEXT_PLAIN)
            .body(email.body().to_owned())
            .map_err(|_| ProviderError::new())?;
        // Bound total DNS/connect/handshake/auth/send time as well as commands.
        tokio::time::timeout(Duration::from_secs(30), self.transport.send(message))
            .await
            .map_err(|_| ProviderError::new())?
            .map_err(|_| ProviderError::new())?;
        Ok(())
    }
}
#[cfg(test)]
#[path = "smtp/tests.rs"]
mod tests;
