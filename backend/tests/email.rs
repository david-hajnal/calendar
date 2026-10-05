use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
};

use commoncal_backend::email::{
    AuthenticationLink, DevelopmentEmailSender, EmailErrorCode, EmailMessageType, EmailSender,
    InMemoryEmailSender, InvitationEmail, LoginLinkEmail, ProductionEmailProvider,
    ProductionEmailSender, ProviderEmail, ProviderError,
};
use tracing::{Instrument, instrument::WithSubscriber};
use tracing_subscriber::fmt::MakeWriter;

const INVITATION_TOKEN: &str = "invitation-token-secret";
const LOGIN_TOKEN: &str = "login-token-secret";

#[tokio::test]
async fn in_memory_sender_captures_recipient_and_message_type() {
    let sender = InMemoryEmailSender::new();

    sender
        .send_invitation(invitation_email())
        .await
        .expect("in-memory send should succeed");
    sender
        .send_login_link(login_email())
        .await
        .expect("in-memory send should succeed");

    let messages = sender.messages();
    assert_eq!(messages[0].recipient(), "invitee@example.com");
    assert_eq!(messages[0].message_type(), EmailMessageType::Invitation);
    assert_eq!(messages[1].recipient(), "member@example.com");
    assert_eq!(messages[1].message_type(), EmailMessageType::LoginLink);
    assert!(!messages[0].subject().contains(INVITATION_TOKEN));
    assert!(!messages[1].subject().contains(LOGIN_TOKEN));
}

#[tokio::test]
async fn development_sender_output_is_redacted() {
    let (writer, captured) = CapturedWriter::new();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_writer(writer)
        .finish();
    let sender = DevelopmentEmailSender::new();

    sender
        .send_invitation(invitation_email())
        .instrument(tracing::info_span!(parent: None, "development_email_test"))
        .with_subscriber(subscriber)
        .await
        .expect("development send should succeed");

    let output = captured.output();
    assert!(output.contains("[REDACTED]"));
}

#[tokio::test]
async fn production_provider_errors_use_safe_internal_codes() {
    let sender = ProductionEmailSender::new(RejectingProvider);

    let error = sender
        .send_login_link(login_email())
        .await
        .expect_err("provider rejection should propagate");

    assert_eq!(error.code(), EmailErrorCode::ProviderFailure);
    assert_eq!(error.to_string(), "email delivery failed");
}

#[tokio::test]
async fn token_values_do_not_appear_in_captured_structured_logs() {
    let (writer, captured) = CapturedWriter::new();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_writer(writer)
        .finish();
    let sender = DevelopmentEmailSender::new();

    async {
        sender.send_invitation(invitation_email()).await.unwrap();
        sender.send_login_link(login_email()).await.unwrap();
        sender
            .send_password_reset(commoncal_backend::email::PasswordResetEmail::new(
                "member@example.com",
                AuthenticationLink::new(
                    "https://commoncal.example/password-reset?token=recovery-log-secret",
                ),
            ))
            .await
            .unwrap();
        sender
            .send_email_confirmation(commoncal_backend::email::EmailConfirmationEmail::new(
                "new@example.test",
                AuthenticationLink::new(
                    "https://commoncal.test/email/confirm?token=confirmation-log-secret",
                ),
            ))
            .await
            .unwrap();
    }
    .instrument(tracing::info_span!(parent: None, "development_email_test"))
    .with_subscriber(subscriber)
    .await;

    let output = captured.output();
    assert!(!output.contains(INVITATION_TOKEN));
    assert!(!output.contains(LOGIN_TOKEN));
    assert!(!output.contains("recovery-log-secret"));
    assert!(!output.contains("confirmation-log-secret"));
}

fn invitation_email() -> InvitationEmail {
    InvitationEmail::new(
        "invitee@example.com",
        AuthenticationLink::new(format!(
            "https://commoncal.example/invitations/consume?token={INVITATION_TOKEN}"
        )),
    )
}

fn login_email() -> LoginLinkEmail {
    LoginLinkEmail::new(
        "member@example.com",
        AuthenticationLink::new(format!(
            "https://commoncal.example/login/consume?token={LOGIN_TOKEN}"
        )),
    )
}

struct RejectingProvider;

impl ProductionEmailProvider for RejectingProvider {
    async fn send(&self, _email: ProviderEmail) -> Result<(), ProviderError> {
        Err(ProviderError::new())
    }
}

#[derive(Clone)]
struct CapturedWriter {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl CapturedWriter {
    fn new() -> (Self, CapturedOutput) {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                bytes: Arc::clone(&bytes),
            },
            CapturedOutput { bytes },
        )
    }
}

impl<'a> MakeWriter<'a> for CapturedWriter {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl Write for CapturedWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.bytes.lock().unwrap().extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct CapturedOutput {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl CapturedOutput {
    fn output(&self) -> String {
        String::from_utf8(self.bytes.lock().unwrap().clone()).unwrap()
    }
}

#[tokio::test]
async fn password_recovery_uses_its_own_message_and_redacts_link_debug() {
    use commoncal_backend::email::PasswordResetEmail;
    let command = PasswordResetEmail::new(
        "member@example.com",
        AuthenticationLink::new("https://commoncal.example/password-reset?token=recovery-secret"),
    );
    assert!(!format!("{command:?}").contains("recovery-secret"));
    let sender = InMemoryEmailSender::new();
    sender.send_password_reset(command.clone()).await.unwrap();
    assert_eq!(
        sender.messages()[0].message_type(),
        EmailMessageType::PasswordReset
    );
    assert_eq!(
        sender.messages()[0].subject(),
        "Reset your CommonCal password"
    );
    let provider = ProductionEmailSender::new(RejectingProvider);
    assert_eq!(
        provider
            .send_password_reset(command)
            .await
            .unwrap_err()
            .code(),
        EmailErrorCode::ProviderFailure
    );
}

#[tokio::test]
async fn email_confirmation_and_notice_use_distinct_messages_and_redact_tokens() {
    use commoncal_backend::email::{EmailChangedEmail, EmailConfirmationEmail};
    let confirmation = EmailConfirmationEmail::new(
        "new@example.test",
        AuthenticationLink::new("https://commoncal.test/email/confirm?token=email-secret"),
    );
    assert!(!format!("{confirmation:?}").contains("email-secret"));
    let notice = EmailChangedEmail::new("old@example.test", "new@example.test");
    let sender = InMemoryEmailSender::new();
    sender
        .send_email_confirmation(confirmation.clone())
        .await
        .unwrap();
    sender.send_email_changed(notice.clone()).await.unwrap();
    assert_eq!(
        sender.messages()[0].message_type(),
        EmailMessageType::EmailConfirmation
    );
    assert_eq!(
        sender.messages()[1].message_type(),
        EmailMessageType::EmailChanged
    );
    let provider = ProductionEmailSender::new(RejectingProvider);
    assert!(
        provider
            .send_email_confirmation(confirmation)
            .await
            .is_err()
    );
    assert!(provider.send_email_changed(notice).await.is_err());
}
