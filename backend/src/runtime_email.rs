use crate::{
    config::Environment,
    email::*,
    smtp::{MailConfig, MailConfigurationError, SmtpProvider},
};

pub enum RuntimeEmailSender {
    Development(DevelopmentEmailSender),
    Production(Box<ProductionEmailSender<SmtpProvider>>),
}
impl std::fmt::Debug for RuntimeEmailSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Development(_) => "RuntimeEmailSender(Development)",
            Self::Production(_) => "RuntimeEmailSender(Production [REDACTED])",
        })
    }
}
impl RuntimeEmailSender {
    pub fn from_env(environment: Environment) -> Result<Self, MailConfigurationError> {
        match MailConfig::from_env(environment)? {
            None => Ok(Self::Development(DevelopmentEmailSender::new())),
            Some(config) => Ok(Self::Production(Box::new(ProductionEmailSender::new(
                SmtpProvider::new(config)?,
            )))),
        }
    }
}
impl EmailSender for RuntimeEmailSender {
    async fn send_invitation(&self, command: InvitationEmail) -> Result<(), EmailError> {
        match self {
            Self::Development(sender) => sender.send_invitation(command).await,
            Self::Production(sender) => sender.send_invitation(command).await,
        }
    }
    async fn send_login_link(&self, command: LoginLinkEmail) -> Result<(), EmailError> {
        match self {
            Self::Development(sender) => sender.send_login_link(command).await,
            Self::Production(sender) => sender.send_login_link(command).await,
        }
    }
    async fn send_password_reset(&self, command: PasswordResetEmail) -> Result<(), EmailError> {
        match self {
            Self::Development(sender) => sender.send_password_reset(command).await,
            Self::Production(sender) => sender.send_password_reset(command).await,
        }
    }
    async fn send_email_confirmation(
        &self,
        command: EmailConfirmationEmail,
    ) -> Result<(), EmailError> {
        match self {
            Self::Development(sender) => sender.send_email_confirmation(command).await,
            Self::Production(sender) => sender.send_email_confirmation(command).await,
        }
    }
    async fn send_email_changed(&self, command: EmailChangedEmail) -> Result<(), EmailError> {
        match self {
            Self::Development(sender) => sender.send_email_changed(command).await,
            Self::Production(sender) => sender.send_email_changed(command).await,
        }
    }
    async fn send_notification(&self, command: NotificationEmail) -> Result<(), EmailError> {
        match self {
            Self::Development(sender) => sender.send_notification(command).await,
            Self::Production(sender) => sender.send_notification(command).await,
        }
    }
}
