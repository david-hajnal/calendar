use commoncal_backend::config::{AppConfig, Environment};
use commoncal_backend::{runtime_email::RuntimeEmailSender, smtp::MailConfig};

fn values(name: &str) -> Option<String> {
    Some(
        match name {
            "SMTP_HOST" => "smtp.example.test",
            "SMTP_PORT" => "587",
            "SMTP_USERNAME" => "private-username",
            "SMTP_PASSWORD" => "private-password",
            "SMTP_FROM" => "CommonCal <no-reply@example.test>",
            _ => return None,
        }
        .into(),
    )
}
#[test]
fn production_requires_password_login_for_invitation_accounts() {
    let result = AppConfig::with_database_path_and_origin_and_access_log_level(
        Environment::Production,
        "127.0.0.1:3000",
        Some("test-session-secret".into()),
        "test.sqlite",
        "https://app.example",
        tracing::level_filters::LevelFilter::OFF,
        false,
    );
    assert!(result.is_err(), "production password login must be enabled");
}

#[test]
fn production_requires_every_mail_setting_without_exposing_values() {
    for missing in [
        "SMTP_HOST",
        "SMTP_PORT",
        "SMTP_USERNAME",
        "SMTP_PASSWORD",
        "SMTP_FROM",
    ] {
        let error = MailConfig::from_values(Environment::Production, |name| {
            if name == missing { None } else { values(name) }
        })
        .unwrap_err();
        assert!(error.to_string().contains("Production requires SMTP_HOST"));
        assert!(!format!("{error:?}").contains("private-password"));
    }
    let config = MailConfig::from_values(Environment::Production, values)
        .unwrap()
        .unwrap();
    let debug = format!("{config:?}");
    assert!(!debug.contains("private-password"));
    assert!(!debug.contains("private-username"));
}

#[test]
fn malformed_mail_configuration_is_rejected_without_echoing_input() {
    for (setting, invalid) in [
        ("SMTP_HOST", "https://private-password@example.test"),
        ("SMTP_HOST", "smtp.example.test:587"),
        ("SMTP_HOST", "smtp.example.test\r\nprivate-password"),
        ("SMTP_PORT", "0"),
        ("SMTP_PORT", "65536"),
        ("SMTP_PORT", "private-password"),
        ("SMTP_USERNAME", "   "),
        ("SMTP_PASSWORD", ""),
        ("SMTP_FROM", "private-password"),
        (
            "SMTP_FROM",
            "from@example.test\r\nBcc: attacker@example.test",
        ),
    ] {
        let error = MailConfig::from_values(Environment::Production, |name| {
            if name == setting {
                Some(invalid.into())
            } else {
                values(name)
            }
        })
        .unwrap_err();
        assert!(!format!("{error:?}").contains("private-password"));
        assert!(!error.to_string().contains("attacker"));
    }
}

#[test]
fn development_does_not_read_smtp_settings_and_uses_capture() {
    assert!(
        MailConfig::from_values(Environment::Development, |_| panic!(
            "development must not read SMTP settings"
        ))
        .unwrap()
        .is_none()
    );
    assert!(matches!(
        RuntimeEmailSender::from_env(Environment::Development).unwrap(),
        RuntimeEmailSender::Development(_)
    ));
}

#[test]
fn production_startup_without_mail_fails_before_creating_database() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("must-not-exist.sqlite");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_commoncal-backend"))
        .env_clear()
        .env("APP_ENV", "production")
        .env("APP_ORIGIN", "https://app.example.test")
        .env("CALDAV_PUBLIC_ORIGIN", "https://app.example.test")
        .env("PASSWORD_LOGIN_ENABLED", "true")
        .env("SESSION_SECRET", "isolated-test-session-secret")
        .env("DATABASE_PATH", &database)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("Production requires SMTP_HOST")
    );
    assert!(
        !database.exists(),
        "mail configuration must fail before migrations"
    );
}

#[tokio::test]
async fn production_bootstrap_does_not_require_mail_credentials() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("bootstrap.sqlite");
    let config =
        AppConfig::with_database_path(Environment::Development, "127.0.0.1:3000", None, &database)
            .unwrap();
    let pool = commoncal_backend::database::connect_and_migrate(
        &config,
        commoncal_backend::http::Readiness::new(),
    )
    .await
    .unwrap();
    // Existing migration 0019 seeds admin@localhost. Match the established
    // bootstrap fixture by removing only that seeded user in this isolated DB.
    sqlx::query("DELETE FROM users WHERE normalized_email = 'admin@localhost'")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_commoncal-backend"))
        .args(["bootstrap-superadmin", "admin@example.test"])
        .env_clear()
        .env("APP_ENV", "production")
        .env("APP_ORIGIN", "https://app.example.test")
        .env("CALDAV_PUBLIC_ORIGIN", "https://app.example.test")
        .env("PASSWORD_LOGIN_ENABLED", "true")
        .env("SESSION_SECRET", "isolated-test-session-secret")
        .env("DATABASE_PATH", &database)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "bootstrap must not initialize the mail transport: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(database.exists());
}
