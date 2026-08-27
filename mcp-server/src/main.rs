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
    extract::Request,
    http::{StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use tower_http::trace::TraceLayer;

use config::Config;
use db::connect_and_migrate;
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
    let db_pool = connect_and_migrate(&config.database_path.to_string_lossy()).await?;

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

async fn health_ready() -> impl IntoResponse {
    (StatusCode::OK, "ready")
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
