use std::sync::Arc;

use axum::http::StatusCode;
use sqlx::SqlitePool;

use crate::audit::{self, AuditRecord};
use crate::config::Config;
use crate::error::ToolError;
use crate::internal_client::InternalClient;
use crate::mcp_grant::McpGrant;
use crate::oauth::TokenValidationResult;
use crate::rate_limiter::RateLimiter;
use crate::tools::{self, AuthorizedToolContext};

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

    /// Run an already-authenticated tools/call: resolve the authoritative
    /// grant, dispatch to the tool, and record the invocation in the local
    /// audit log.
    ///
    /// Audit recording is best-effort: a failed audit insert is logged as an
    /// operational error and never replaces an already-completed tool response.
    /// Returning failure would encourage a client to repeat a mutation that
    /// already succeeded.
    pub async fn handle_authorized_tool_call(
        &self,
        request_id: &str,
        token_result: &TokenValidationResult,
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

        let auth_strength = token_result.auth_strength.to_string();

        // Resolve the authoritative grant from CommonCal core (source of truth).
        let grant = match self
            .internal_client
            .get_mcp_grant(token_result.user_id, &token_result.oauth_client_id)
            .await
        {
            Ok(Some(resp)) => McpGrant::from(resp),
            Ok(None) => {
                self.record_audit(&AuditRecord {
                    request_id,
                    user_id: token_result.user_id,
                    client_id: &token_result.oauth_client_id,
                    grant_id: None,
                    tool: tool_name,
                    resource_ids: None,
                    auth_result: "denied",
                    scope: None,
                    auth_strength: &auth_strength,
                    latency_ms: 0,
                    result_type: "denied",
                    operation_id: None,
                })
                .await;
                return axum::http::Response::builder()
                    .status(StatusCode::FORBIDDEN)
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_string(&serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": message.get("id"),
                            "error": {
                                "code": -2003,
                                "message": "no MCP grant found",
                            },
                        }))
                        .unwrap(),
                    ))
                    .unwrap();
            }
            Err(e) => {
                self.record_audit(&AuditRecord {
                    request_id,
                    user_id: token_result.user_id,
                    client_id: &token_result.oauth_client_id,
                    grant_id: None,
                    tool: tool_name,
                    resource_ids: None,
                    auth_result: "denied",
                    scope: None,
                    auth_strength: &auth_strength,
                    latency_ms: 0,
                    result_type: "error",
                    operation_id: None,
                })
                .await;
                return axum::http::Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_string(&serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": message.get("id"),
                            "error": {
                                "code": -2005,
                                "message": format!("grant resolution failed: {}", e),
                            },
                        }))
                        .unwrap(),
                    ))
                    .unwrap();
            }
        };

        // Build the authorized context and dispatch to the tool.
        let context = AuthorizedToolContext {
            token: token_result,
            grant: &grant,
            internal_client: &self.internal_client,
        };

        let started = std::time::Instant::now();
        match tools::dispatch(&context, tool_name, params).await {
            Ok(response) => {
                self.record_audit(&AuditRecord {
                    request_id,
                    user_id: token_result.user_id,
                    client_id: &token_result.oauth_client_id,
                    grant_id: Some(&grant.grant_id),
                    tool: tool_name,
                    resource_ids: None,
                    auth_result: "allowed",
                    scope: None,
                    auth_strength: &auth_strength,
                    latency_ms: started.elapsed().as_millis() as i64,
                    result_type: "success",
                    operation_id: None,
                })
                .await;
                response
            }
            Err(e) => {
                let (auth_result, result_type) = match e {
                    ToolError::Unauthorized(_) | ToolError::Forbidden(_) => ("denied", "denied"),
                    _ => ("allowed", "error"),
                };
                self.record_audit(&AuditRecord {
                    request_id,
                    user_id: token_result.user_id,
                    client_id: &token_result.oauth_client_id,
                    grant_id: Some(&grant.grant_id),
                    tool: tool_name,
                    resource_ids: None,
                    auth_result,
                    scope: None,
                    auth_strength: &auth_strength,
                    latency_ms: started.elapsed().as_millis() as i64,
                    result_type,
                    operation_id: None,
                })
                .await;
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

    /// Best-effort audit insert. Failures are logged, never propagated.
    async fn record_audit(&self, record: &AuditRecord<'_>) {
        if let Err(e) = audit::log_invocation(&self.db_pool, record).await {
            tracing::error!(
                error = %e,
                tool = record.tool,
                request_id = record.request_id,
                "failed to record MCP audit"
            );
        }
    }
}
