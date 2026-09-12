mod audit;
mod config;
mod db;
mod error;
mod gateway;
mod internal_client;
mod mcp_grant;
mod oauth;
mod output_schema;
mod rate_limiter;
mod security;
mod tools;

use axum::{
    Router,
    extract::{Request, State},
    http::{StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use tower_http::trace::TraceLayer;

use config::Config;
use db::{connect_and_migrate, is_ready};
use gateway::Gateway;

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

    let gateway = Gateway::new(config.clone(), db_pool)
        .map_err(|e| format!("Gateway initialization failed: {:?}", e))?;

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
        .route("/mcp", post(mcp_handler))
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

async fn mcp_handler(
    axum::extract::State(gateway): axum::extract::State<Gateway>,
    request: Request,
) -> Response {
    // Extract request ID from headers or generate one
    let request_id = request
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let start = std::time::Instant::now();

    let response = gateway.handle_mcp_request(request).await;

    let latency = start.elapsed().as_millis() as i64;

    // Log the request
    tracing::info!(
        request_id = %request_id,
        method = %response.status().as_str(),
        latency_ms = latency,
        "mcp_request"
    );

    let mut response = response;
    response
        .headers_mut()
        .insert("x-request-id", request_id.parse().unwrap());

    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;
    use config::AppEnv;

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
    async fn health_live_does_not_depend_on_storage() {
        let response = health_live().await.into_response();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
