use mcp_server::{auth, config, db, gateway, mcp_service};

use axum::{
    Router,
    extract::State,
    http::{StatusCode, header::CONTENT_TYPE},
    middleware,
    response::{IntoResponse, Response},
    routing::get,
};
use rmcp::transport::{
    StreamableHttpServerConfig,
    streamable_http_server::{session::local::LocalSessionManager, tower::StreamableHttpService},
};
use tower_http::trace::TraceLayer;

use config::Config;
use db::{connect_and_migrate, is_ready};
use gateway::Gateway;
use mcp_service::CommonCalServer;

use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer().json())
        .init();

    let config = match Config::from_env() {
        Ok(config) => config,
        Err(errors) => {
            for e in &errors {
                eprintln!("CONFIG ERROR: {}", e);
            }
            std::process::exit(1);
        }
    };
    run(config).await
}

async fn run(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let db_pool = connect_and_migrate(&config.database_path).await?;

    let gateway = Gateway::new(config.clone(), db_pool.clone())
        .map_err(|e| format!("Gateway initialization failed: {:?}", e))?;

    // Build the rmcp-backed MCP service. The per-request identity is published
    // by the auth middleware into the request's task-local; the tool handlers
    // read it at call time, keeping concurrent sessions isolated.
    let server = CommonCalServer::new(
        gateway.internal_client.clone(),
        db_pool.clone(),
        gateway.rate_limiter.clone(),
    );

    let mcp_service: StreamableHttpService<CommonCalServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(server.clone()),
            LocalSessionManager::default().into(),
            StreamableHttpServerConfig::default(),
        );

    // Auth state for the bearer-token middleware. The protected-resource
    // metadata URL is derived from MCP_PUBLIC_RESOURCE_URL (no loopback).
    let auth_state = auth::AuthState::new(
        config.oauth_issuer.clone(),
        config.public_resource_url.clone(),
    );

    // The MCP endpoint is fully wrapped by bearer-token auth so the first
    // unauthenticated `initialize` receives a standards-compliant 401 challenge.
    let mcp_router =
        Router::new()
            .nest_service("/mcp", mcp_service)
            .layer(middleware::from_fn_with_state(
                auth_state.clone(),
                auth::auth_middleware,
            ));

    let router = Router::new()
        .route("/health/live", get(health_live))
        .route("/health/ready", get(health_ready))
        .route(
            "/.well-known/oauth-protected-resource",
            get(move || {
                let issuer = config.oauth_issuer.clone();
                let dpop_supported = config.dpop_key_path.is_some();
                async move {
                    let meta = crate::config::OauthProtectedResourceMetadata::new(
                        &config.public_resource_url,
                        &issuer,
                        dpop_supported,
                    );
                    (
                        StatusCode::OK,
                        [(CONTENT_TYPE, "application/json")],
                        serde_json::to_string(&meta).unwrap(),
                    )
                        .into_response()
                }
            }),
        )
        .merge(mcp_router)
        .with_state(gateway)
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &axum::http::Request<_>| {
                    if request.uri().path().starts_with("/health/") {
                        tracing::debug_span!(
                            "http_request",
                            method = %request.method(),
                            path = %request.uri().path()
                        )
                    } else {
                        tracing::info_span!(
                            "http_request",
                            method = %request.method(),
                            path = %request.uri().path()
                        )
                    }
                })
                .on_response(
                    |response: &Response, latency: std::time::Duration, span: &tracing::Span| {
                        let status = response.status().as_u16();
                        let latency_ms = latency.as_millis();
                        if span
                            .metadata()
                            .is_some_and(|m| m.level() == &tracing::Level::DEBUG)
                        {
                            tracing::debug!(
                                status = status,
                                latency_ms = latency_ms,
                                "finished processing request"
                            );
                        } else {
                            tracing::info!(
                                status = status,
                                latency_ms = latency_ms,
                                "finished processing request"
                            );
                        }
                    },
                ),
        );

    let listener = tokio::net::TcpListener::bind(&config.bind_address).await?;

    tracing::info!(
        address = %config.bind_address,
        "mcp-server started"
    );

    axum::serve(listener, router).await?;

    Ok(())
}

async fn health_live() -> impl IntoResponse {
    (StatusCode::OK, "live")
}

async fn health_ready(State(gateway): State<Gateway>) -> impl IntoResponse {
    if is_ready(&gateway.db_pool).await {
        (StatusCode::OK, "ready")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not ready")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;
    use config::AppEnv;
    use tower::ServiceExt;

    fn test_config(database_path: std::path::PathBuf) -> Config {
        Config {
            app_env: AppEnv::Development,
            oauth_issuer: "https://auth.example.com".to_string(),
            internal_api_base: "https://api.example.com".to_string(),
            internal_api_key: "test-key".to_string(),
            session_secret: "test-secret".to_string(),
            database_path,
            mcp_domain: "mcp.example.com".to_string(),
            public_resource_url: "https://mcp.example.com/mcp".to_string(),
            bind_address: "127.0.0.1:3001".parse().unwrap(),
            dpop_key_path: None,
            rate_limit_enabled: false,
            tracing_level: "info".to_string(),
        }
    }

    fn unique_database_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("commoncal-mcp-{}.sqlite", uuid::Uuid::new_v4()))
    }

    #[tokio::test]
    async fn health_ready_returns_503_when_database_is_unavailable() {
        let database_path = unique_database_path();
        let pool = connect_and_migrate(&database_path)
            .await
            .expect("a fresh database should be created and migrated");
        pool.close().await;
        let _ = std::fs::remove_file(&database_path);

        let gateway = Gateway::new(test_config(database_path), pool).expect("gateway should build");
        let response = health_ready(State(gateway)).await.into_response();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn health_ready_returns_200_when_database_is_queryable() {
        let database_path = unique_database_path();
        let pool = connect_and_migrate(&database_path)
            .await
            .expect("a fresh database should be created and migrated");

        let gateway = Gateway::new(test_config(database_path.clone()), pool.clone())
            .expect("gateway should build");
        let response = health_ready(State(gateway)).await.into_response();

        assert_eq!(response.status(), StatusCode::OK);

        pool.close().await;
        let _ = std::fs::remove_file(&database_path);
    }

    #[tokio::test]
    async fn readiness_route_reports_ready_for_temporary_sqlite_database() {
        let database_path = unique_database_path();
        let pool = connect_and_migrate(&database_path)
            .await
            .expect("a fresh database should be created and migrated");
        let gateway = Gateway::new(test_config(database_path.clone()), pool.clone())
            .expect("gateway should build");
        let router = Router::new()
            .route("/health/ready", get(health_ready))
            .with_state(gateway);

        let response = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/health/ready")
                    .body(axum::body::Body::empty())
                    .expect("test request should build"),
            )
            .await
            .expect("router should serve readiness request");

        assert_eq!(response.status(), StatusCode::OK);

        pool.close().await;
        let _ = std::fs::remove_file(&database_path);
    }

    #[tokio::test]
    async fn health_live_does_not_depend_on_storage() {
        let response = health_live().await.into_response();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
