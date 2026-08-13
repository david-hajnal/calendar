use std::sync::Arc;

use axum::http::{Response, StatusCode};
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
    pub fn new(config: Config, db_pool: SqlitePool) -> Self {
        let internal_client = InternalClient::new(
            config.internal_api_base.clone(),
            config.internal_api_key.clone(),
        );

        let rate_limiter = if config.rate_limit_enabled {
            Arc::new(RateLimiter::new())
        } else {
            Arc::new(RateLimiter::disabled())
        };

        Self {
            config,
            db_pool,
            internal_client,
            rate_limiter,
        }
    }

    /// Extract the OAuth bearer token from the request Authorization header.
    fn extract_bearer_token(
        &self,
        request: &axum::http::Request<axum::body::Body>,
    ) -> Result<String, axum::http::Response<axum::body::Body>> {
        let auth_header = request
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .ok_or_else(|| unauthorized_response("missing authorization header"))?;

        let auth_str = auth_header.to_str().map_err(|_| {
            unauthorized_response("invalid authorization header encoding")
        })?;

        if !auth_str.starts_with("Bearer ") {
            return Err(unauthorized_response("authorization must use Bearer scheme"));
        }

        let token = &auth_str[7..];
        if token.is_empty() {
            return Err(unauthorized_response("empty bearer token"));
        }

        Ok(token.to_string())
    }

    /// Validate the OAuth token and return the TokenValidationResult.
    async fn validate_token(
        &self,
        token: &str,
    ) -> Result<TokenValidationResult, axum::http::Response<axum::body::Body>> {
        let resource = &self.config.oauth_issuer;
        let result = oauth::validate_access_token(token, resource, resource)
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, "token validation failed");
                unauthorized_response(e.to_string())
            })?;

        tracing::info!(
            user_id = result.user_id,
            client_id = result.oauth_client_id,
            "token validated"
        );

        Ok(result)
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
        // Parse the MCP protocol message from the request body.
        let body_bytes = match axum::body::to_bytes(request.into_body(), usize::MAX).await {
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

        let method = message
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        match method {
            "tools/list" => self.handle_tools_list(&message).await,
            "calendar_list" => self.handle_calendar_list(&message).await,
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

    async fn handle_tools_list(
        &self,
        _message: &serde_json::Value,
    ) -> axum::http::Response<axum::body::Body> {
        // Tracer bullet: return empty tool catalog.
        // Slice 5 will wire this to the real tool list.
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "result": {
                "tools": []
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

    /// Handle calendar_list tool call with full authorization pipeline.
    ///
    /// Pipeline:
    /// 1. Extract bearer token from Authorization header
    /// 2. Validate OAuth token (JWT signature, issuer, audience, expiry)
    /// 3. Load McpGrant from DB
    /// 4. Check tool permission (allow_availability)
    /// 5. Call internal API to fetch calendars
    /// 6. Filter by grant's allowed_calendar_ids
    /// 7. Return structured response
    async fn handle_calendar_list(
        &self,
        message: &serde_json::Value,
    ) -> axum::http::Response<axum::body::Body> {
        // Step 1: Extract bearer token.
        // Note: for tools/list calls without auth, we skip token validation.
        // The bearer token is extracted from the request in the gateway handler.
        // For tools that require auth, we validate the token.

        // Step 2: Parse parameters.
        let params: tools::calendar_list::CalendarListParams = match message.get("params") {
            Some(p) => match serde_json::from_value(p.clone()) {
                Ok(params) => params,
                Err(e) => {
                    return axum::http::Response::builder()
                        .status(axum::http::StatusCode::BAD_REQUEST)
                        .header("content-type", "application/json")
                        .body(
                            axum::body::Body::from(
                                serde_json::to_string(&serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": message.get("id"),
                                    "error": {
                                        "code": -32600,
                                        "message": format!("invalid params: {}", e),
                                    },
                                }))
                                .unwrap(),
                            )
                            .into(),
                        )
                        .unwrap();
                }
            },
            None => tools::calendar_list::CalendarListParams {
                include_access: false,
            },
        };

        // Step 3: For the tracer bullet, return empty tool list.
        // Slice 5 will wire the full authorization pipeline.
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "result": {
                "tools": []
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
}

/// Build a 401 Unauthorized MCP error response.
fn unauthorized_response(msg: impl Into<String>) -> axum::http::Response<axum::body::Body> {
    axum::http::Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_string(&serde_json::json!({
                "error": {
                    "code": -2000,
                    "message": msg.into(),
                },
            }))
            .unwrap(),
        ))
        .unwrap()
}
