use commoncal_backend::{
    admin::AdminService,
    bootstrap::{BootstrapCommand, InitialSuperadminBootstrap},
    calendar::CalendarService,
    config::AppConfig,
    database::connect_and_migrate,
    email::DevelopmentEmailSender,
    event::EventService,
    http::{Readiness, build_router_with_auth_flows_sessions_admin_calendars_and_views},
    invitations::InvitationConsumer,
    login::{FixedWindowLoginRateLimiter, LoginService},
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
    let readiness = Readiness::new();
    let database = connect_and_migrate(&config, readiness.clone()).await?;

    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments
        .first()
        .is_some_and(|value| value == "bootstrap-superadmin")
    {
        return run_bootstrap(&config, database, &arguments[1..]).await;
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
    let login_service = LoginService::new(
        database.clone(),
        secret_key.clone(),
        15 * 60,
        30 * 24 * 60 * 60,
        "/login",
        email_sender.clone(),
        Arc::new(FixedWindowLoginRateLimiter::new(5, 15 * 60)),
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
        email_sender,
    );
    let calendar_service = CalendarService::new(database.clone());
    let event_service = EventService::new(database.clone());
    let shared_view_service = SharedViewService::new_with_key(database, secret_key);

    axum::serve(
        listener,
        build_router_with_auth_flows_sessions_admin_calendars_and_views(
            readiness,
            invitation_consumer,
            login_service,
            session_manager,
            admin_service,
            calendar_service,
            event_service,
            shared_view_service,
        )
        .into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

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
