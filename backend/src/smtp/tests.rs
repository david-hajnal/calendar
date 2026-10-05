use super::*;
use crate::email::{
    AuthenticationLink, EmailChangedEmail, EmailConfirmationEmail, EmailSender, InvitationEmail,
    LoginLinkEmail, NotificationEmail, PasswordResetEmail, ProductionEmailSender,
};
use std::sync::{Arc, Mutex};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    net::TcpListener,
};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
    },
};

const CA: &[u8] = include_bytes!("../../tests/fixtures/smtp/ca.pem");
const CERT: &[u8] = include_bytes!("../../tests/fixtures/smtp/server.pem");
const KEY: &[u8] = include_bytes!("../../tests/fixtures/smtp/server-key.pem");
#[derive(Default)]
struct Captured {
    auth: Vec<String>,
    bodies: Vec<String>,
    commands: Vec<String>,
}
trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}
async fn server(
    starttls: bool,
    reject_auth: bool,
    reject_mail: bool,
    count: usize,
    implicit_tls: bool,
) -> (u16, Arc<Mutex<Captured>>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let captured = Arc::new(Mutex::new(Captured::default()));
    let output = captured.clone();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![CertificateDer::from_pem_slice(CERT).unwrap()],
        PrivateKeyDer::from_pem_slice(KEY).unwrap(),
    )
    .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let job = tokio::spawn(async move {
        for _ in 0..count {
            let (socket, _) = listener.accept().await.unwrap();
            let socket: Box<dyn Stream> = if implicit_tls {
                Box::new(acceptor.accept(socket).await.unwrap())
            } else {
                Box::new(socket)
            };
            let mut stream = BufReader::new(socket);
            stream
                .get_mut()
                .write_all(b"220 localhost controlled SMTP\r\n")
                .await
                .unwrap();
            let mut encrypted = implicit_tls;
            loop {
                let mut line = String::new();
                match stream.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    _ => {}
                }
                let command = line.trim_end().to_owned();
                output.lock().unwrap().commands.push(command.clone());
                let reply: &[u8];
                if command.starts_with("EHLO") {
                    reply = if encrypted {
                        b"250-localhost\r\n250 AUTH PLAIN\r\n"
                    } else if starttls {
                        b"250-localhost\r\n250 STARTTLS\r\n"
                    } else {
                        b"250-localhost\r\n250 AUTH PLAIN\r\n"
                    };
                } else if command == "STARTTLS" {
                    stream
                        .get_mut()
                        .write_all(b"220 upgrade\r\n")
                        .await
                        .unwrap();
                    match acceptor.accept(stream.into_inner()).await {
                        Ok(tls) => {
                            stream = BufReader::new(Box::new(tls) as Box<dyn Stream>);
                            encrypted = true;
                            continue;
                        }
                        Err(_) => break,
                    }
                } else if command.starts_with("AUTH PLAIN ") {
                    assert!(encrypted, "credentials must only arrive over TLS");
                    output.lock().unwrap().auth.push(command.clone());
                    reply = if reject_auth {
                        b"535 test provider secret rejection\r\n"
                    } else {
                        b"235 authenticated\r\n"
                    };
                } else if command.starts_with("MAIL FROM") {
                    reply = if reject_mail {
                        b"550 test provider secret rejection\r\n"
                    } else {
                        b"250 sender\r\n"
                    };
                } else if command.starts_with("RCPT TO") {
                    reply = b"250 recipient\r\n";
                } else if command == "DATA" {
                    stream.get_mut().write_all(b"354 data\r\n").await.unwrap();
                    let mut body = String::new();
                    loop {
                        let mut data = String::new();
                        if stream.read_line(&mut data).await.unwrap() == 0 {
                            break;
                        }
                        if data == ".\r\n" {
                            break;
                        }
                        body.push_str(&data);
                    }
                    output.lock().unwrap().bodies.push(body);
                    reply = b"250 accepted\r\n";
                } else if command == "QUIT" {
                    let _ = stream.get_mut().write_all(b"221 bye\r\n").await;
                    break;
                } else {
                    reply = b"250 ok\r\n";
                }
                if stream.get_mut().write_all(reply).await.is_err() {
                    break;
                }
            }
        }
    });
    (port, captured, job)
}
fn config(port: u16) -> MailConfig {
    MailConfig::new(
        "localhost".into(),
        port,
        "controlled-user".into(),
        "controlled-password".into(),
        "CommonCal <no-reply@example.test>".into(),
    )
    .unwrap()
}
fn trusted(port: u16) -> ProductionEmailSender<SmtpProvider> {
    let tls = TlsParameters::builder("localhost".into())
        .add_root_certificate(lettre::transport::smtp::client::Certificate::from_pem(CA).unwrap())
        .build()
        .unwrap();
    ProductionEmailSender::new(SmtpProvider::with_tls(config(port), tls).unwrap())
}

#[tokio::test]
async fn implicit_tls_authenticates_before_sending_smtp_commands() {
    // Use an ephemeral port for the same implicit TLS mode selected on 465 in
    // production, avoiding a privileged/fixed listener in the test suite.
    let (port, captured, job) = server(false, false, false, 1, true).await;
    let tls = TlsParameters::builder("localhost".into())
        .add_root_certificate(lettre::transport::smtp::client::Certificate::from_pem(CA).unwrap())
        .build()
        .unwrap();
    let sender =
        ProductionEmailSender::new(SmtpProvider::with_tls_mode(config(port), tls, true).unwrap());
    sender
        .send_invitation(InvitationEmail::new(
            "recipient@example.test",
            AuthenticationLink::new("https://app.example.test/invite?token=secret"),
        ))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), job)
        .await
        .unwrap()
        .unwrap();
    let output = captured.lock().unwrap();
    assert_eq!(output.bodies.len(), 1);
    assert_eq!(output.auth.len(), 1);
    assert!(!output.commands.iter().any(|command| command == "STARTTLS"));
}
#[tokio::test]
async fn authenticated_tls_delivers_every_message_with_expected_links_and_sender() {
    let (port, captured, job) = server(true, false, false, 6, false).await;
    let sender = crate::runtime_email::RuntimeEmailSender::Production(Box::new(trusted(port)));
    let link = || {
        AuthenticationLink::new("https://app.example.test/token?token=controlled-message-secret")
    };
    sender
        .send_invitation(InvitationEmail::new("recipient@example.test", link()))
        .await
        .unwrap();
    sender
        .send_login_link(LoginLinkEmail::new("recipient@example.test", link()))
        .await
        .unwrap();
    sender
        .send_password_reset(PasswordResetEmail::new("recipient@example.test", link()))
        .await
        .unwrap();
    sender
        .send_email_confirmation(EmailConfirmationEmail::new(
            "recipient@example.test",
            link(),
        ))
        .await
        .unwrap();
    sender
        .send_email_changed(EmailChangedEmail::new(
            "old@example.test",
            "new@example.test",
        ))
        .await
        .unwrap();
    sender
        .send_notification(NotificationEmail::new(
            "recipient@example.test",
            "Controlled event",
        ))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), job)
        .await
        .unwrap()
        .unwrap();
    let output = captured.lock().unwrap();
    assert_eq!(output.bodies.len(), 6);
    assert_eq!(output.auth.len(), 6);
    use base64::Engine;
    for auth in &output.auth {
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(auth.strip_prefix("AUTH PLAIN ").unwrap())
                .unwrap(),
            b"\0controlled-user\0controlled-password"
        );
    }
    for body in &output.bodies {
        assert!(body.contains("From: CommonCal <no-reply@example.test>"));
    }
    let bodies: Vec<String> = output
        .bodies
        .iter()
        .map(|message| {
            let (headers, body) = message.split_once("\r\n\r\n").unwrap();
            if headers.contains("Content-Transfer-Encoding: quoted-printable") {
                String::from_utf8(
                    quoted_printable::decode(body, quoted_printable::ParseMode::Strict).unwrap(),
                )
                .unwrap()
            } else {
                body.into()
            }
        })
        .collect();
    for body in &bodies[..4] {
        assert!(body.contains("https://app.example.test/token?token=controlled-message-secret"));
    }
    assert!(bodies[2].contains("15 minutes"));
    assert!(bodies[3].contains("24 hours"));
    assert!(bodies[4].contains("new@example.test"));
    assert!(bodies[5].contains("Controlled event"));
}
#[tokio::test]
async fn missing_starttls_never_sends_credentials_or_message() {
    let (port, captured, job) = server(false, false, false, 1, false).await;
    let error = trusted(port)
        .send_password_reset(PasswordResetEmail::new(
            "recipient@example.test",
            AuthenticationLink::new("https://app.example.test/reset?token=secret"),
        ))
        .await
        .unwrap_err();
    tokio::time::timeout(Duration::from_secs(5), job)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(error.to_string(), "email delivery failed");
    let output = captured.lock().unwrap();
    assert!(output.auth.is_empty());
    assert!(output.bodies.is_empty());
}
#[tokio::test]
async fn untrusted_tls_certificate_never_sends_credentials() {
    let (port, captured, job) = server(true, false, false, 1, false).await;
    let sender = ProductionEmailSender::new(SmtpProvider::new(config(port)).unwrap());
    assert!(
        sender
            .send_password_reset(PasswordResetEmail::new(
                "recipient@example.test",
                AuthenticationLink::new("https://app.example.test/reset?token=secret")
            ))
            .await
            .is_err()
    );
    tokio::time::timeout(Duration::from_secs(5), job)
        .await
        .unwrap()
        .unwrap();
    assert!(captured.lock().unwrap().auth.is_empty());
}
#[tokio::test]
async fn authentication_and_delivery_failures_are_redacted() {
    for (reject_auth, reject_mail) in [(true, false), (false, true)] {
        let (port, captured, job) = server(true, reject_auth, reject_mail, 1, false).await;
        let sender = trusted(port);
        assert!(!format!("{sender:?}").contains("controlled-password"));
        let error = sender
            .send_email_confirmation(EmailConfirmationEmail::new(
                "recipient@example.test",
                AuthenticationLink::new("https://app.example.test/confirm?token=secret"),
            ))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "email delivery failed");
        assert!(!format!("{error:?}").contains("provider secret"));
        tokio::time::timeout(Duration::from_secs(5), job)
            .await
            .unwrap()
            .unwrap();
        assert!(captured.lock().unwrap().bodies.is_empty());
    }
}
