use serde::Serialize;
use std::{
    fmt::{self, Debug, Display, Formatter},
    fs::OpenOptions,
    future::Future,
    io::Write,
    path::PathBuf,
    sync::Mutex,
};

const INVITATION_SUBJECT: &str = "You are invited to CommonCal";
const LOGIN_LINK_SUBJECT: &str = "Your CommonCal login link";
const NOTIFICATION_SUBJECT: &str = "CommonCal reminder";

pub trait EmailSender {
    fn send_invitation(
        &self,
        command: InvitationEmail,
    ) -> impl Future<Output = Result<(), EmailError>> + Send;

    fn send_login_link(
        &self,
        command: LoginLinkEmail,
    ) -> impl Future<Output = Result<(), EmailError>> + Send;

    fn send_notification(
        &self,
        command: NotificationEmail,
    ) -> impl Future<Output = Result<(), EmailError>> + Send {
        let _ = command;
        async { Err(EmailError::provider_failure()) }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct AuthenticationLink(String);

impl AuthenticationLink {
    pub fn new(link: impl Into<String>) -> Self {
        Self(link.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl Debug for AuthenticationLink {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("AuthenticationLink([REDACTED])")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvitationEmail {
    recipient: String,
    authentication_link: AuthenticationLink,
}

impl InvitationEmail {
    pub fn new(recipient: impl Into<String>, authentication_link: AuthenticationLink) -> Self {
        Self {
            recipient: recipient.into(),
            authentication_link,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoginLinkEmail {
    recipient: String,
    authentication_link: AuthenticationLink,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotificationEmail {
    recipient: String,
    event_title: String,
}

impl NotificationEmail {
    pub fn new(recipient: impl Into<String>, event_title: impl Into<String>) -> Self {
        Self {
            recipient: recipient.into(),
            event_title: event_title.into(),
        }
    }

    pub fn recipient(&self) -> &str {
        &self.recipient
    }
}

impl LoginLinkEmail {
    pub fn new(recipient: impl Into<String>, authentication_link: AuthenticationLink) -> Self {
        Self {
            recipient: recipient.into(),
            authentication_link,
        }
    }

    pub fn recipient(&self) -> &str {
        &self.recipient
    }

    pub fn authentication_link(&self) -> &AuthenticationLink {
        &self.authentication_link
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmailMessageType {
    Invitation,
    LoginLink,
    Notification,
}

impl EmailMessageType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Invitation => "invitation",
            Self::LoginLink => "login_link",
            Self::Notification => "notification",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapturedEmail {
    recipient: String,
    message_type: EmailMessageType,
    subject: &'static str,
}

impl CapturedEmail {
    pub fn recipient(&self) -> &str {
        &self.recipient
    }

    pub fn message_type(&self) -> EmailMessageType {
        self.message_type
    }

    pub fn subject(&self) -> &str {
        self.subject
    }
}

#[derive(Debug, Default)]
pub struct InMemoryEmailSender {
    messages: Mutex<Vec<CapturedEmail>>,
}

impl InMemoryEmailSender {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn messages(&self) -> Vec<CapturedEmail> {
        self.messages.lock().unwrap().clone()
    }

    fn capture(&self, recipient: String, message_type: EmailMessageType, subject: &'static str) {
        self.messages.lock().unwrap().push(CapturedEmail {
            recipient,
            message_type,
            subject,
        });
    }
}

impl EmailSender for InMemoryEmailSender {
    async fn send_invitation(&self, command: InvitationEmail) -> Result<(), EmailError> {
        self.capture(
            command.recipient,
            EmailMessageType::Invitation,
            INVITATION_SUBJECT,
        );
        Ok(())
    }

    async fn send_login_link(&self, command: LoginLinkEmail) -> Result<(), EmailError> {
        self.capture(
            command.recipient,
            EmailMessageType::LoginLink,
            LOGIN_LINK_SUBJECT,
        );
        Ok(())
    }

    async fn send_notification(&self, command: NotificationEmail) -> Result<(), EmailError> {
        self.capture(
            command.recipient,
            EmailMessageType::Notification,
            NOTIFICATION_SUBJECT,
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub struct DevelopmentEmailSender {
    e2e_outbox: Option<PathBuf>,
}

impl DevelopmentEmailSender {
    pub fn new() -> Self {
        Self {
            e2e_outbox: (std::env::var("APP_ENV").ok().as_deref() == Some("development"))
                .then(|| std::env::var_os("E2E_EMAIL_OUTBOX").map(PathBuf::from))
                .flatten(),
        }
    }

    fn log(
        &self,
        message_type: EmailMessageType,
        subject: &str,
        recipient: &str,
        link: Option<&str>,
    ) -> Result<(), EmailError> {
        tracing::info!(
            target: "commoncal::email",
            message_type = message_type.as_str(),
            subject,
            authentication_link = "[REDACTED]",
            "development email"
        );
        if let Some(path) = &self.e2e_outbox {
            let message = E2eEmail {
                recipient,
                message_type: message_type.as_str(),
                authentication_link: link,
            };
            let line =
                serde_json::to_string(&message).map_err(|_| EmailError::provider_failure())?;
            let mut outbox = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|_| EmailError::provider_failure())?;
            writeln!(outbox, "{line}").map_err(|_| EmailError::provider_failure())?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct E2eEmail<'a> {
    recipient: &'a str,
    message_type: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    authentication_link: Option<&'a str>,
}

impl EmailSender for DevelopmentEmailSender {
    async fn send_invitation(&self, command: InvitationEmail) -> Result<(), EmailError> {
        self.log(
            EmailMessageType::Invitation,
            INVITATION_SUBJECT,
            &command.recipient,
            Some(command.authentication_link.expose()),
        )
    }

    async fn send_login_link(&self, command: LoginLinkEmail) -> Result<(), EmailError> {
        self.log(
            EmailMessageType::LoginLink,
            LOGIN_LINK_SUBJECT,
            &command.recipient,
            Some(command.authentication_link.expose()),
        )
    }

    async fn send_notification(&self, command: NotificationEmail) -> Result<(), EmailError> {
        self.log(
            EmailMessageType::Notification,
            NOTIFICATION_SUBJECT,
            &command.recipient,
            None,
        )
    }
}

pub trait ProductionEmailProvider {
    fn send(&self, email: ProviderEmail) -> impl Future<Output = Result<(), ProviderError>> + Send;
}

#[derive(Clone, Eq, PartialEq)]
pub struct ProviderEmail {
    recipient: String,
    subject: &'static str,
    body: String,
}

impl ProviderEmail {
    pub fn recipient(&self) -> &str {
        &self.recipient
    }

    pub fn subject(&self) -> &str {
        self.subject
    }

    pub fn body(&self) -> &str {
        &self.body
    }

    fn invitation(command: InvitationEmail) -> Self {
        Self {
            recipient: command.recipient,
            subject: INVITATION_SUBJECT,
            body: format!(
                "Accept your CommonCal invitation: {}",
                command.authentication_link.expose()
            ),
        }
    }

    fn login_link(command: LoginLinkEmail) -> Self {
        Self {
            recipient: command.recipient,
            subject: LOGIN_LINK_SUBJECT,
            body: format!(
                "Log in to CommonCal: {}",
                command.authentication_link.expose()
            ),
        }
    }

    fn notification(command: NotificationEmail) -> Self {
        Self {
            recipient: command.recipient,
            subject: NOTIFICATION_SUBJECT,
            body: format!("Reminder: {}", command.event_title),
        }
    }
}

impl Debug for ProviderEmail {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderEmail")
            .field("recipient", &self.recipient)
            .field("subject", &self.subject)
            .field("body", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProviderError;

impl ProviderError {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug)]
pub struct ProductionEmailSender<P> {
    provider: P,
}

impl<P> ProductionEmailSender<P> {
    pub fn new(provider: P) -> Self {
        Self { provider }
    }
}

impl<P> EmailSender for ProductionEmailSender<P>
where
    P: ProductionEmailProvider + Sync,
{
    async fn send_invitation(&self, command: InvitationEmail) -> Result<(), EmailError> {
        self.provider
            .send(ProviderEmail::invitation(command))
            .await
            .map_err(|_| EmailError::provider_failure())
    }

    async fn send_login_link(&self, command: LoginLinkEmail) -> Result<(), EmailError> {
        self.provider
            .send(ProviderEmail::login_link(command))
            .await
            .map_err(|_| EmailError::provider_failure())
    }

    async fn send_notification(&self, command: NotificationEmail) -> Result<(), EmailError> {
        self.provider
            .send(ProviderEmail::notification(command))
            .await
            .map_err(|_| EmailError::provider_failure())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmailErrorCode {
    ProviderFailure,
    PermanentFailure,
}

impl EmailErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProviderFailure => "email_provider_failure",
            Self::PermanentFailure => "email_permanent_failure",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EmailError {
    code: EmailErrorCode,
}

impl EmailError {
    fn provider_failure() -> Self {
        Self {
            code: EmailErrorCode::ProviderFailure,
        }
    }

    pub fn transient() -> Self {
        Self::provider_failure()
    }

    pub fn permanent() -> Self {
        Self {
            code: EmailErrorCode::PermanentFailure,
        }
    }

    pub fn code(&self) -> EmailErrorCode {
        self.code
    }

    pub fn is_transient(&self) -> bool {
        matches!(self.code, EmailErrorCode::ProviderFailure)
    }
}

impl Display for EmailError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("email delivery failed")
    }
}

impl std::error::Error for EmailError {}
