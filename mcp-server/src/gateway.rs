use std::sync::Arc;

use axum::http::{header, Response, StatusCode};
use sqlx::SqlitePool;

use crate::config::Config;
use crate::error::ToolError;
use crate::internal_client::InternalClient;
use crate::mcp_grant::get_grant;
use crate::oauth::{self, TokenValidationResult};
use crate::rate_limiter::RateLimiter;
use crate::tools;

#[derive(Clone)]
pub struct Gateway {
    pub config: Config,
    pub db_pool: SqlitePool,
    pub internal_client: InternalClient,
    pub rate_limiter: Arc<RateLimiter>,
}

impl Gateway {
    pub fn new(
        config: Config,
        db_pool: SqlitePool,
    ) -> Result<Self, Vec<crate::config::ConfigError>> {
        let internal_client = InternalClient::new(
            config.internal_api_base.clone(),
            config.internal_api_key.clone(),
        )
        .map_err(|e| vec![e])?;

        let rate_limiter = if config.rate_limit_enabled {
            Arc::new(RateLimiter::new())
        } else {
            Arc::new(RateLimiter::disabled())
        };

        Ok(Self {
            config,
            db_pool,
            internal_client,
            rate_limiter,
        })
    }

    /// Extract the OAuth bearer token from the request Authorization header.
    fn extract_bearer_token(
        &self,
        request: &axum::http::Request<axum::body::Body>,
    ) -> Result<String, axum::http::Response<axum::body::Body>> {
        let auth_header = request
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .ok_or_else(|| unauthorized_response(None, "missing authorization header"))?;

        let auth_str = auth_header
            .to_str()
            .map_err(|_| unauthorized_response(None, "invalid authorization header encoding"))?;

        if !auth_str.starts_with("Bearer ") {
            return Err(unauthorized_response(
                None,
                "authorization must use Bearer scheme",
            ));
        }

        let token = &auth_str[7..];
        if token.is_empty() {
            return Err(unauthorized_response(
                None,
                "empty bearer token",
            ));
        }

        Ok(token.to_string())
    }

    /// Validate the OAuth token and return the TokenValidationResult.
    async fn validate_token(
        &self,
        token: &str,
    ) -> Result<TokenValidationResult, axum::http::Response<axum::body::Body>> {
        let resource = &self.config.public_resource_url;
        let result = oauth::validate_access_token(token, &self.config.oauth_issuer, resource)
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, "token validation failed");
                unauthorized_response_from_token_error(&e)
            })?;

        tracing::info!(
            user_id = result.user_id,
            client_id = result.oauth_client_id,
            "token validated"
        );

        Ok(result)
    }

    /// Check Origin header against allowed origins.
    fn check_origin(&self, _request: &axum::http::Request<axum::body::Body>) -> bool {
        // No origin configuration — allow all (loopback/dev mode)
        true
    }

    /// Handle an incoming MCP request.
    ///
    /// This is the entry point for all MCP protocol communication.
    /// It validates the OAuth token, dispatches to the appropriate tool,
    /// and returns a structured MCP response.
    pub async fn handle_mcp_request(
        &self,
        request: axum::http::Request<axum::body::Body>,
    ) -> axum::http::Response<axum::body::Body> {
        // Check Origin/CORS
        if !self.check_origin(&request) {
            return axum::http::Response::builder()
                .status(StatusCode::FORBIDDEN)
                .body(axum::body::Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "error": {
                            "code": -2001,
                            "message": "origin not allowed",
                        },
                    }))
                    .unwrap(),
                ))
                .unwrap();
        }

        // Parse the MCP protocol message from the request body.
        // Limit body size to 1MB to prevent memory exhaustion DoS.
        let body_bytes = match axum::body::to_bytes(request.into_body(), 1024 * 1024).await {
            Ok(b) => b,
            Err(e) => {
                tracing::error!(error = %e, "failed to read request body");
                return axum::http::Response::builder()
                    .status(axum::http::StatusCode::BAD_REQUEST)
                    .body(axum::body::Body::from(r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"parse error"}}"#).into())
                    .unwrap();
            }
        };

        let message: serde_json::Value = match serde_json::from_slice(&body_bytes) {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(error = %e, "failed to parse MCP message");
                return axum::http::Response::builder()
                    .status(axum::http::StatusCode::BAD_REQUEST)
                    .body(axum::body::Body::from(r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"parse error"}}"#).into())
                    .unwrap();
            }
        };

        let method = message.get("method").and_then(|v| v.as_str()).unwrap_or("");

        match method {
            "tools/list" => self.handle_tools_list(&message).await,
            "tools/call" => self.handle_tools_call(&message).await,
            _ => {
                tracing::warn!(method = %method, "unknown MCP method");
                axum::http::Response::builder()
                    .status(axum::http::StatusCode::BAD_REQUEST)
                    .body(
                        axum::body::Body::from(
                            serde_json::to_string(&serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": message.get("id"),
                                "error": {
                                    "code": -32601,
                                    "message": format!("Method not found: {}", method),
                                },
                            }))
                            .unwrap(),
                        )
                        .into(),
                    )
                    .unwrap()
            }
        }
    }

    /// Handle tools/list — return the full tool catalog with all nine tools.
    async fn handle_tools_list(
        &self,
        message: &serde_json::Value,
    ) -> axum::http::Response<axum::body::Body> {
        let tools = tools::list_tools();
        let tools_array: Vec<serde_json::Value> = tools
            .iter()
            .map(|name| serde_json::json!({ "name": name }))
            .collect();

        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "result": {
                "tools": tools_array
            }
        });

        axum::http::Response::builder()
            .status(axum::http::StatusCode::OK)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                serde_json::to_string_pretty(&response).unwrap(),
            ))
            .unwrap()
    }

    /// Handle tools/call — validate token, dispatch to tool, return result.
    async fn handle_tools_call(
        &self,
        message: &serde_json::Value,
    ) -> axum::http::Response<axum::body::Body> {
        // Extract tool name and params
        let tool_name = message
            .get("params")
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("");

        let params = message
            .get("params")
            .and_then(|p| p.get("arguments"))
            .cloned()
            .unwrap_or(serde_json::json!({}));

        // Rate limiting check
        if !self.rate_limiter.check("mcp_tool", 100, 60) {
            return axum::http::Response::builder()
                .status(StatusCode::TOO_MANY_REQUESTS)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::to_string(&serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": message.get("id"),
                        "error": {
                            "code": -2004,
                            "message": "rate limit exceeded",
                        },
                    }))
                    .unwrap(),
                ))
                .unwrap();
        }

        // Extract and validate bearer token
        let token = match self.extract_bearer_token(&axum::http::Request::new(axum::body::Body::empty())) {
            Ok(t) => t,
            Err(resp) => return resp,
        };

        // Validate token
        let token_result = match self.validate_token(&token).await {
            Ok(r) => r,
            Err(resp) => return resp,
        };

        // Dispatch to tool
        match tools::dispatch(
            &token_result,
            &self.db_pool,
            &self.internal_client,
            tool_name,
            params,
        )
        .await
        {
            Ok(response) => response,
            Err(e) => {
                let mcp_error = e.to_mcp_error(message.get("id"));
                axum::http::Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_string(&serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": message.get("id"),
                            "error": {
                            "code": mcp_error["error"]["code"],
                            "message": mcp_error["error"]["message"],
                            },
                        }))
                        .unwrap(),
                    ))
                    .unwrap()
            }
        }
    }

    async fn handle_calendar_list(
        &self,
        message: &serde_json::Value,
    ) -> axum::http::Response<axum::body::Body> {
        // Delegated to tools/call handler
        self.handle_tools_list(message).await
    }
}

/// Build a 401 Unauthorized MCP error response with WWW-Authenticate header.
fn unauthorized_response(
    reason: Option<String>,
    msg: &str,
) -> axum::http::Response<axum::body::Body> {
    let mut challenge = format!(
        "Bearer realm=\"mcp\", resource_metadata=\"{}/.well-known/oauth-protected-resource\"",
        "http://127.0.0.1:3001"
    );
    if let Some(ref r) = reason {
        let sanitized = sanitize_error(r);
        challenge.push_str(&format!(", error=\"{}\"", sanitized));
    }
    axum::http::Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(header::WWW_AUTHENTICATE, challenge)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_string(&serde_json::json!({
                "error": {
                    "code": -2000,
                    "message": msg,
                },
            }))
            .unwrap(),
        ))
        .unwrap()
}

/// Build a 401 response from a token validation error with appropriate WWW-Authenticate.
fn unauthorized_response_from_token_error(
    error: &crate::error::TokenError,
) -> axum::http::Response<axum::body::Body> {
    let (http_code, error_param, msg) = match error {
        crate::error::TokenError::Expired => (
            StatusCode::UNAUTHORIZED,
            "token_expired",
            "access token has expired",
        ),
        crate::error::TokenError::InvalidAudience => (
            StatusCode::UNAUTHORIZED,
            "invalid_resource",
            "token audience does not match this resource",
        ),
        crate::error::TokenError::InvalidIssuer => (
            StatusCode::UNAUTHORIZED,
            "invalid_issuer",
            "token issuer is not trusted",
        ),
        crate::error::TokenError::Revoked => (
            StatusCode::UNAUTHORIZED,
            "invalid_token",
            "token has been revoked",
        ),
        crate::error::TokenError::MissingToken => (
            StatusCode::UNAUTHORIZED,
            "missing_token",
            "authorization token is required",
        ),
        _ => (
            StatusCode::UNAUTHORIZED,
            "invalid_token",
            "invalid authorization token",
        ),
    };

    let mut challenge = format!(
        "Bearer realm=\"mcp\", resource_metadata=\"{}/.well-known/oauth-protected-resource\"",
        "http://127.0.0.1:3001"
    );
    challenge.push_str(&format!(", error=\"{}\"", error_param));

    axum::http::Response::builder()
        .status(http_code)
        .header(header::WWW_AUTHENTICATE, challenge)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_string(&serde_json::json!({
                "error": {
                    "code": -2000,
                    "message": msg,
                },
            }))
            .unwrap(),
        ))
        .unwrap()
}

/// Sanitize a string for use in WWW-Authenticate header value.
/// Escapes quotes and removes control characters.
fn sanitize_error(s: &str) -> String {
    s.chars()
        .filter(|c| !(*c as u32) < 0x20)
        .map(|c| if c == '"' { '\\' } else { c })
        .collect()
}
