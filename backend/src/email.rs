use std::{
    fmt::{self, Debug, Display, Formatter},
    future::Future,
    sync::Mutex,
};

const INVITATION_SUBJECT: &str = "You are invited to CommonCal";
const LOGIN_LINK_SUBJECT: &str = "Your CommonCal login link";

pub trait EmailSender {
    fn send_invitation(
        &self,
        command: InvitationEmail,
    ) -> impl Future<Output = Result<(), EmailError>> + Send;

    fn send_login_link(
        &self,
        command: LoginLinkEmail,
    ) -> impl Future<Output = Result<(), EmailError>> + Send;
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
}

impl EmailMessageType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Invitation => "invitation",
            Self::LoginLink => "login_link",
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
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DevelopmentEmailSender;

impl DevelopmentEmailSender {
    pub fn new() -> Self {
        Self
    }

    fn log(message_type: EmailMessageType, subject: &str) {
        tracing::info!(
            target: "commoncal::email",
            message_type = message_type.as_str(),
            subject,
            authentication_link = "[REDACTED]",
            "development email"
        );
    }
}

impl EmailSender for DevelopmentEmailSender {
    async fn send_invitation(&self, _command: InvitationEmail) -> Result<(), EmailError> {
        Self::log(EmailMessageType::Invitation, INVITATION_SUBJECT);
        Ok(())
    }

    async fn send_login_link(&self, _command: LoginLinkEmail) -> Result<(), EmailError> {
        Self::log(EmailMessageType::LoginLink, LOGIN_LINK_SUBJECT);
        Ok(())
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmailErrorCode {
    ProviderFailure,
}

impl EmailErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProviderFailure => "email_provider_failure",
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

    pub fn code(&self) -> EmailErrorCode {
        self.code
    }
}

impl Display for EmailError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("email delivery failed")
    }
}

impl std::error::Error for EmailError {}
