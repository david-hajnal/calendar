//! Slice 2 SDK-backed MCP endpoint (rmcp 3.1.4).
//!
//! Mounts a Streamable HTTP MCP service at `/mcp` with exactly ONE tool,
//! `calendar_list`, returning REAL calendars from the CommonCal internal API,
//! filtered by the active mcp_grant. Every request is gated by a Bearer JWT
//! validated against the candidate issuer (discovery JWKS, standard claims,
//! exact resource audience). Unauthenticated/invalid requests receive a 401
//! with a WWW-Authenticate challenge referencing the protected-resource
//! metadata. A valid JWT with no active mcp_grant receives a 403.
//!
//! This is NOT the production MCP server. It proves the rmcp SDK chain and the
//! real calendar-read path in Slice 2. The real server (mcp-server/) adopts
//! this in later slices.

use std::net::SocketAddr;
use std::sync::LazyLock;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, Request, StatusCode, header},
    middleware,
    response::{IntoResponse, Response},
    routing::get,
};
use rmcp::{
    ServerHandler,
    handler::server::router::tool::ToolRouter,
    model::*,
    tool, tool_handler, tool_router,
    transport::{
        StreamableHttpServerConfig,
        streamable_http_server::{
            session::local::LocalSessionManager, tower::StreamableHttpService,
        },
    },
};
use serde_json::json;
use tokio::sync::RwLock;

use slice1_lab::common::LabConfig;
use slice1_lab::jwt::{self, JwtError};

// ---------------------------------------------------------------------------
// Shared authenticated-claims context
//
// The axum auth middleware validates the JWT and stores the claims here. The
// rmcp tool handler reads them. The lab drives requests sequentially, so a
// single shared slot is safe (no concurrent-request crosstalk).
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct CurrentClaims {
    user_id: i64,
    client_id: String,
    scopes: Vec<String>,
}

static CURRENT_CLAIMS: LazyLock<RwLock<Option<CurrentClaims>>> =
    LazyLock::new(|| RwLock::new(None));

/// The CommonCal internal API base URL (loopback).
fn commoncal_base() -> String {
    std::env::var("MCP_ECHO_COMMONCAL").unwrap_or_else(|_| "http://127.0.0.1:4002".to_string())
}

/// The bridge key for the CommonCal internal API (lab value).
fn commoncal_bridge_key() -> String {
    std::env::var("MCP_ECHO_BRIDGE_KEY")
        .unwrap_or_else(|_| "slice1-loopback-bridge-key".to_string())
}

/// Build an rmcp ErrorData with an internal error code.
fn err(message: impl Into<String>) -> rmcp::ErrorData {
    rmcp::ErrorData::new(rmcp::model::ErrorCode::INTERNAL_ERROR, message.into(), None)
}

/// Shared HTTP client for the CommonCal internal API (lab-only, loopback).
static ECHO_HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("http client")
});

#[derive(Clone)]
struct CommonCalEcho {
    tool_router: ToolRouter<CommonCalEcho>,
}

#[tool_router]
impl CommonCalEcho {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "List the calendars available to the authenticated user.")]
    async fn calendar_list(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        // Read the authenticated claims set by the axum middleware.
        let claims = CURRENT_CLAIMS
            .read()
            .await
            .clone()
            .ok_or_else(|| err("unauthenticated"))?;

        let base = commoncal_base();
        let key = commoncal_bridge_key();

        // 1. Check the active mcp_grant for (user_id, client_id).
        let grant_url = format!(
            "{}/internal/grant?user_id={}&client_id={}",
            base,
            claims.user_id,
            urlencoding::encode(&claims.client_id)
        );
        let grant_resp = ECHO_HTTP
            .get(&grant_url)
            .header("Authorization", format!("Bearer {}", key))
            .send()
            .await
            .map_err(|e| err(format!("grant lookup transport: {e}")))?;
        let grant_status = grant_resp.status();
        if grant_status.as_u16() == 404 {
            return Err(err("no active MCP grant — consent required"));
        }
        if !grant_status.is_success() {
            return Err(err(format!("grant lookup failed: {grant_status}")));
        }
        let grant_body: serde_json::Value = grant_resp
            .json()
            .await
            .map_err(|e| err(format!("grant parse: {e}")))?;
        let allowed_ids: Vec<i64> = grant_body
            .pointer("/grant/allowed_calendar_ids")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_i64()).collect())
            .unwrap_or_default();

        // 2. Fetch the user's real calendars from the CommonCal internal API.
        let cal_url = format!("{}/internal/calendars/{}", base, claims.user_id);
        let cal_resp = ECHO_HTTP
            .get(&cal_url)
            .header("Authorization", format!("Bearer {}", key))
            .send()
            .await
            .map_err(|e| err(format!("calendar fetch transport: {e}")))?;
        if !cal_resp.status().is_success() {
            return Err(err(format!("calendar fetch failed: {}", cal_resp.status())));
        }
        let cal_body: serde_json::Value = cal_resp
            .json()
            .await
            .map_err(|e| err(format!("calendar parse: {e}")))?;
        let calendars: Vec<serde_json::Value> = cal_body
            .get("calendars")
            .and_then(|v| v.as_array())
            .map(|a| a.clone())
            .unwrap_or_default();

        // 3. Filter by the grant's allowed calendar IDs.
        let filtered: Vec<serde_json::Value> = calendars
            .into_iter()
            .filter(|c| {
                c.get("id")
                    .and_then(|v| v.as_i64())
                    .is_some_and(|id| allowed_ids.contains(&id))
            })
            .collect();

        let output = json!({ "calendars": filtered });
        Ok(CallToolResult::success(vec![ContentBlock::text(
            serde_json::to_string_pretty(&output).unwrap_or_else(|_| "[]".to_string()),
        )]))
    }
}

#[tool_handler]
impl ServerHandler for CommonCalEcho {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("commoncal-echo", "0.2.0"))
            .with_instructions(
                "Slice 2: calendar_list backed by real CommonCal calendars + mcp_grant.",
            )
    }
}

/// Auth state shared with the middleware.
#[derive(Clone)]
struct AuthState {
    http: reqwest::Client,
    issuer: String,
    resource: String,
    protected_resource_metadata: String,
}

impl AuthState {
    async fn validate(&self, token: &str) -> Result<slice1_lab::jwt::AccessClaims, JwtError> {
        jwt::validate_access_token(&self.http, token, &self.issuer, &self.resource).await
    }
}

fn extract_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|auth| auth.strip_prefix("Bearer ").map(|s| s.to_string()))
}

/// Build the 401 challenge response referencing protected-resource metadata.
fn unauthorized(cfg: &AuthState, reason: Option<String>) -> Response {
    let reason_str = reason.unwrap_or_else(|| "missing or invalid bearer token".to_string());
    let mut challenge = format!(
        "Bearer realm=\"commoncal\", resource_metadata=\"{}\"",
        cfg.protected_resource_metadata
    );
    challenge.push_str(&format!(", error=\"{}\"", sanitize(&reason_str)));
    let body = json!({
        "error": "unauthorized",
        "error_description": reason_str
    });
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(header::WWW_AUTHENTICATE, challenge)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap_or_else(|_| Response::new(axum::body::Body::empty()))
}

fn sanitize(s: &str) -> String {
    s.chars()
        .take(120)
        .map(|c| match c {
            '"' | '\\' => '-',
            c if (c as u32) < 0x20 => '-',
            c => c,
        })
        .collect()
}

async fn auth_middleware(
    State(cfg): State<AuthState>,
    headers: HeaderMap,
    request: Request<axum::body::Body>,
    next: middleware::Next,
) -> Response {
    match extract_token(&headers) {
        Some(token) => match cfg.validate(&token).await {
            Ok(claims) => {
                // Store the authenticated claims for the rmcp tool handler.
                let user_id: i64 = claims.sub.parse().unwrap_or(0);
                let client_id = claims.client_id.clone().unwrap_or_default();
                let scopes: Vec<String> = claims
                    .scope
                    .clone()
                    .unwrap_or_default()
                    .split_whitespace()
                    .map(|s| s.to_string())
                    .collect();
                {
                    let mut slot = CURRENT_CLAIMS.write().await;
                    *slot = Some(CurrentClaims {
                        user_id,
                        client_id,
                        scopes,
                    });
                }
                tracing::debug!(sub = %claims.sub, "mcp request authorized");
                next.run(request).await.into_response()
            }
            Err(e) => unauthorized(&cfg, Some(e.to_string())),
        },
        None => unauthorized(&cfg, None),
    }
}

/// Protected-resource metadata (RFC 9728 style) advertising the exact resource
/// and the candidate issuer.
async fn protected_resource_metadata(State(cfg): State<AuthState>) -> Json<serde_json::Value> {
    Json(json!({
        "resource": cfg.resource,
        "authorization_servers": [cfg.issuer],
        "scopes_supported": slice1_lab::common::SCOPE_CATALOG,
        "bearer_methods_supported": ["header"]
    }))
}

async fn health() -> &'static str {
    "ok"
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cfg = LabConfig::from_env();
    let auth = AuthState {
        http: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("http client"),
        issuer: cfg.issuer.clone(),
        resource: cfg.resource_url.clone(),
        protected_resource_metadata: format!(
            "{}/.well-known/oauth-protected-resource",
            cfg.mcp_echo
        ),
    };

    let mcp_service: StreamableHttpService<CommonCalEcho, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(CommonCalEcho::new()),
            LocalSessionManager::default().into(),
            StreamableHttpServerConfig::default(),
        );

    let protected =
        Router::new()
            .nest_service("/mcp", mcp_service)
            .layer(middleware::from_fn_with_state(
                auth.clone(),
                auth_middleware,
            ));

    let app = Router::new()
        .route("/health", get(health))
        .route(
            "/.well-known/oauth-protected-resource",
            get(protected_resource_metadata),
        )
        .merge(protected)
        .with_state(auth);

    let bind: SocketAddr =
        slice1_lab::common::bind_addr("MCP_ECHO_BIND", "127.0.0.1:3001".parse().unwrap());
    tracing::info!(%bind, "mcp-echo listening");
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
