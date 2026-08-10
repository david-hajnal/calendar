use commoncal_backend::{
    admin::AdminService,
    backup::{Aes256GcmEncryptor, BackupCommand, BackupService, RestoreCommand, RestoreService},
    bootstrap::{BootstrapCommand, InitialSuperadminBootstrap},
    calendar::CalendarService,
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    email::DevelopmentEmailSender,
    event::EventService,
    external_feed::ExternalFeedService,
    http::{
        Readiness, ResponseSecurityConfig,
        build_router_with_auth_flows_sessions_admin_calendars_views_and_external_feeds,
        secure_responses, serve_frontend,
    },
    invitations::InvitationConsumer,
    login::{FixedWindowLoginRateLimiter, LoginService},
    notification::NotificationWorker,
    rate_limiter::FixedWindowRateLimiter,
    write_rate_limit::WriteRateLimiterState,
    public_rate_limit::PublicRateLimiterState,
    admin_invitation_rate_limit::AdminInvitationRateLimiterState,
    security::SecretKey,
    sessions::{SessionManager, SessionSecurityConfig},
    shared_view::SharedViewService,
};
use std::{
    net::SocketAddr,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::net::TcpListener;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();
    let config = AppConfig::from_env()?;
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.first().is_some_and(|value| value == "restore") {
        return run_restore(&config, &arguments[1..]).await;
    }

    let readiness = Readiness::new();
    let database = connect_and_migrate(&config, readiness.clone()).await?;

    if arguments
        .first()
        .is_some_and(|value| value == "bootstrap-superadmin")
    {
        return run_bootstrap(&config, database, &arguments[1..]).await;
    }
    if arguments.first().is_some_and(|value| value == "backup") {
        return run_backup(database, &arguments[1..]).await;
    }
    if arguments.first().is_some_and(|value| value == "seed") {
        return run_seed(&config, database).await;
    }
    if !arguments.is_empty() {
        return Err(format!("unknown command: {}", arguments[0]).into());
    }

    let listener = TcpListener::bind(config.bind_address).await?;

    tracing::info!(
        environment = ?config.environment,
        address = %config.bind_address,
        "server started"
    );

    let secret_key = config
        .session_secret()
        .map(|secret| SecretKey::derive(secret.as_bytes()))
        .unwrap_or_else(SecretKey::generate);
    let invitation_consumer =
        InvitationConsumer::new(database.clone(), secret_key.clone(), 30 * 24 * 60 * 60);
    let email_sender = Arc::new(DevelopmentEmailSender::new());
    let is_secure = config.app_origin().starts_with("https://");
    let login_service = LoginService::new(
        database.clone(),
        secret_key.clone(),
        15 * 60,
        30 * 24 * 60 * 60,
        "/login",
        email_sender.clone(),
        Arc::new(FixedWindowLoginRateLimiter::new(5, 15 * 60)),
        is_secure,
    );
    let session_manager = SessionManager::new(
        database.clone(),
        secret_key.clone(),
        SessionSecurityConfig::new(7 * 24 * 60 * 60, 5 * 60, config.app_origin())?,
    );
    let admin_service = AdminService::with_email_sender(
        database.clone(),
        secret_key.clone(),
        24 * 60 * 60,
        format!("{}/invitations/accept", config.app_origin()),
        email_sender.clone(),
    );
    let calendar_service = CalendarService::new(database.clone());
    let notification_service =
        commoncal_backend::notification::NotificationService::new(database.clone());
    let notification_worker_database = database.clone();
    let notification_worker_sender = email_sender.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            tick.tick().await;
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock is before Unix epoch")
                .as_secs() as i64;
            let worker = NotificationWorker::new_at_with_email_sender(
                notification_worker_database.clone(),
                now,
                5 * 60,
                100,
                notification_worker_sender.clone(),
            );
            if worker.process_due().await.is_err() {
                tracing::error!(error_code = "notification_worker_failed");
            }
        }
    });
    let notification_replanner = notification_service.clone();
    let event_service = EventService::new_with_notification_replanner(
        database.clone(),
        Arc::new(move |event_id| {
            let notifications = notification_replanner.clone();
            tokio::spawn(async move {
                if notifications.replan_event(event_id).await.is_err() {
                    tracing::error!(error_code = "notification_replan_failed");
                }
            });
        }),
    );
    let external_feed_service = ExternalFeedService::new(database.clone(), secret_key.clone());
    let shared_view_service = SharedViewService::new_with_key(database, secret_key);

    let write_rate_limiter = if std::env::var("APP_ENV").ok().as_deref() == Some("development") {
        None
    } else {
        Some(WriteRateLimiterState {
            limiter: Arc::new(FixedWindowRateLimiter::new(30, 60)),
        })
    };

    let public_rate_limiter = if std::env::var("APP_ENV").ok().as_deref() == Some("development") {
        None
    } else {
        Some(PublicRateLimiterState {
            limiter: Arc::new(FixedWindowRateLimiter::new(100, 60)),
        })
    };

    let admin_rate_limiter = if std::env::var("APP_ENV").ok().as_deref() == Some("production") {
        let limiter = FixedWindowRateLimiter::new(5, 60);
        Some(AdminInvitationRateLimiterState {
            limiter: Arc::new(limiter),
        })
    } else {
        None
    };

    let router = build_router_with_auth_flows_sessions_admin_calendars_views_and_external_feeds(
        readiness,
        invitation_consumer,
        login_service,
        session_manager,
        admin_service,
        calendar_service,
        event_service,
        shared_view_service,
        external_feed_service,
        notification_service,
        config.access_log_level(),
        is_secure,
        config.password_login_enabled(),
        write_rate_limiter,
        public_rate_limiter,
        admin_rate_limiter,
    );
    let frontend_directory =
        std::env::var("FRONTEND_DIR").unwrap_or_else(|_| "/app/frontend".into());

    let response_security = if config.environment == Environment::Production
        && config.app_origin().starts_with("https://")
    {
        ResponseSecurityConfig::production_https()
    } else {
        ResponseSecurityConfig::local_http()
    };

    axum::serve(
        listener,
        secure_responses(
            serve_frontend(router, frontend_directory),
            response_security,
        )
        .into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    Ok(())
}

async fn run_backup(
    database: sqlx::SqlitePool,
    arguments: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let command = BackupCommand::from_arguments(arguments)?;
    let created_at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
    let encryption_key = std::env::var("BACKUP_ENCRYPTION_KEY_HEX")
        .map_err(|_| "BACKUP_ENCRYPTION_KEY_HEX is required for backup")?;
    let encryptor = Aes256GcmEncryptor::from_hex_key(&encryption_key)?;
    let metadata = BackupService::new(database)
        .create_encrypted_and_upload(command.destination_directory, created_at, &encryptor, None)
        .await?;
    println!("backup_id={}", metadata.id);
    println!("artifact_path={}", metadata.artifact_path.display());
    println!("snapshot_sha256={}", metadata.snapshot_sha256);
    println!("compressed_sha256={}", metadata.compressed_sha256);
    Ok(())
}

async fn run_restore(
    config: &AppConfig,
    arguments: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let command = RestoreCommand::from_arguments(arguments)?;
    if config.environment == Environment::Production {
        command.refuse_production_target(config.database_path())?;
    }
    let encryption_key = std::env::var("BACKUP_ENCRYPTION_KEY_HEX")
        .map_err(|_| "BACKUP_ENCRYPTION_KEY_HEX is required for restore")?;
    let encryptor = Aes256GcmEncryptor::from_hex_key(&encryption_key)?;
    RestoreService::restore_encrypted(
        command.artifact_path,
        command.destination_database.clone(),
        &encryptor,
    )
    .await?;
    println!(
        "restored_database={}",
        command.destination_database.display()
    );
    Ok(())
}

async fn run_bootstrap(
    config: &AppConfig,
    database: sqlx::SqlitePool,
    arguments: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    if !(1..=2).contains(&arguments.len()) {
        return Err("usage: commoncal-backend bootstrap-superadmin <email> [display-name]".into());
    }
    let secret = config
        .session_secret()
        .filter(|value| !value.is_empty())
        .ok_or("SESSION_SECRET is required for bootstrap-superadmin")?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
    let result = InitialSuperadminBootstrap::new(database, SecretKey::derive(secret.as_bytes()))
        .execute(BootstrapCommand {
            normalized_email: arguments[0].clone(),
            display_name: arguments.get(1).cloned(),
            created_at: now,
            expires_at: now + 24 * 60 * 60,
        })
        .await?;

    println!("invitation_id={}", result.invitation_id);
    println!("token={}", result.token.expose());
    Ok(())
}

async fn run_seed(
    config: &AppConfig,
    database: sqlx::SqlitePool,
) -> Result<(), Box<dyn std::error::Error>> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;

    // Check if user already exists
    let mut tx = database.begin().await?;
    let user_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&mut *tx)
        .await?;

    if user_count > 0 {
        println!("database already has {} user(s), skipping seed", user_count);
        return Ok(());
    }

    // Allow seed in production only when DEFAULT_ADMIN_PASSWORD is set.
    if config.environment == Environment::Production
        && std::env::var("DEFAULT_ADMIN_PASSWORD").ok().filter(|v| !v.is_empty()).is_none()
    {
        return Err(
            "seed command is not allowed in production without DEFAULT_ADMIN_PASSWORD"
                .into(),
        );
    }

    // Create user
    let user_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (normalized_email, display_name, status, is_superadmin, created_at)
         VALUES ('dev@example.com', 'Dev User', 'active', 1, ?)
         RETURNING id",
    )
    .bind(now)
    .fetch_one(&database)
    .await?;

    // Optionally set password from DEFAULT_ADMIN_PASSWORD env var.
    if let Ok(password) = std::env::var("DEFAULT_ADMIN_PASSWORD") {
        if !password.is_empty() {
            let hash = bcrypt::hash(&password, bcrypt::DEFAULT_COST)
                .map_err(|e| format!("bcrypt hash failed: {e}"))?;
            sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
                .bind(&hash)
                .bind(user_id)
                .execute(&database)
                .await?;
            println!("  password:  set (from DEFAULT_ADMIN_PASSWORD)");
        }
    }

    // Create calendar
    let calendar_id: i64 = sqlx::query_scalar(
        "INSERT INTO calendars (owner_user_id, name, color, default_timezone, default_event_visibility, created_at, updated_at)
         VALUES (?, 'My Calendar', '#3b82f6', 'UTC', 'default', ?, ?)
         RETURNING id",
    )
    .bind(user_id)
    .bind(now)
    .bind(now)
    .fetch_one(&database)
    .await?;

    // Create calendar_acl
    sqlx::query(
        "INSERT INTO calendar_acl (calendar_id, user_id, role, created_at, updated_at)
         VALUES (?, ?, 'owner', ?, ?)",
    )
    .bind(calendar_id)
    .bind(user_id)
    .bind(now)
    .bind(now)
    .execute(&database)
    .await?;

    println!("seeded:");
    println!("  email:     dev@example.com");
    println!("  display:   Dev User");
    println!("  superadmin: true");
    println!("  calendar:  My Calendar");

    Ok(())
}

fn init_tracing() {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer().json())
        .init();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }

    tracing::info!("shutdown signal received");
}
