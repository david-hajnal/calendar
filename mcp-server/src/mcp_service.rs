// rmcp-backed MCP service.
//
// Implements the MCP `ServerHandler` with all nine CommonCal tools as typed
// SDK handlers. Each handler:
//   1. Reads the validated identity from the request's task-local (set by the
//      auth middleware). Fails closed when absent.
//   2. Resolves the authoritative grant from CommonCal core (live lookup).
//   3. Delegates to the existing domain tool handler (authorization, internal
//      API call, structured output).
//   4. Records the invocation in the local audit log (best-effort).
//   5. Wraps the result in an MCP content block, or maps the domain error to a
//      protocol error.
//
// This replaces the custom JSON-RPC dispatcher in `gateway.rs`.

use std::sync::Arc;

use axum::http::Response;
use http_body_util::BodyExt;
use rmcp::{
    ErrorData, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    tool, tool_handler, tool_router,
};
use sqlx::SqlitePool;

use crate::audit::{self, AuditRecord};
use crate::error::ToolError;
use crate::identity;
use crate::internal_client::InternalClient;
use crate::mcp_grant::McpGrant;
use crate::oauth::TokenValidationResult;
use crate::rate_limiter::RateLimiter;
use crate::tools::{
    self, AuthorizedToolContext, availability_find::AvailabilityFindParams,
    calendar_list::CalendarListParams, event_create::EventCreateParams,
    event_delete_commit::EventDeleteCommitParams, event_delete_prepare::EventDeletePrepareParams,
    event_get::EventGetParams, event_search::EventSearchParams, event_update::EventUpdateParams,
    reminder_set::ReminderSetParams,
};

/// The rmcp-backed MCP server. Holds the shared dependencies used by the tool
/// handlers. The per-request identity is NOT stored here; it is read from the
/// request's task-local at call time, which keeps concurrent sessions isolated.
#[derive(Clone)]
pub struct CommonCalServer {
    pub internal_client: InternalClient,
    pub db_pool: SqlitePool,
    pub rate_limiter: Arc<RateLimiter>,
    _tool_router: ToolRouter<CommonCalServer>,
}

impl CommonCalServer {
    pub fn new(
        internal_client: InternalClient,
        db_pool: SqlitePool,
        rate_limiter: Arc<RateLimiter>,
    ) -> Self {
        Self {
            internal_client,
            db_pool,
            rate_limiter,
            _tool_router: Self::tool_router(),
        }
    }

    /// Read the validated identity for the current request, failing closed when
    /// the auth middleware did not establish one.
    fn identity(&self) -> Result<identity::Identity, ErrorData> {
        identity::current().ok_or_else(|| {
            ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                "unauthenticated: no validated identity for this request".to_string(),
                None,
            )
        })
    }

    /// Resolve the authoritative grant for the current identity, failing closed
    /// when no grant exists or the lookup fails.
    async fn resolve_grant(&self, identity: &identity::Identity) -> Result<McpGrant, ErrorData> {
        match self
            .internal_client
            .get_mcp_grant(identity.user_id, &identity.client_id)
            .await
        {
            Ok(Some(resp)) => Ok(McpGrant::from(resp)),
            Ok(None) => Err(ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                "no active MCP grant — consent required".to_string(),
                None,
            )),
            Err(e) => Err(ErrorData::new(
                ErrorCode::INTERNAL_ERROR,
                format!("grant resolution failed: {e}"),
                None,
            )),
        }
    }

    /// Best-effort audit insert. Failures are logged, never propagated.
    async fn record_audit(&self, record: &AuditRecord<'_>) {
        if let Err(e) = audit::log_invocation(&self.db_pool, record).await {
            tracing::error!(
                error = %e,
                tool = record.tool,
                "failed to record MCP audit"
            );
        }
    }

    /// Convert a successful domain response into an MCP `CallToolResult`.
    async fn to_call_tool_result(
        response: Response<axum::body::Body>,
    ) -> Result<CallToolResult, ErrorData> {
        let body_bytes = response
            .into_body()
            .collect()
            .await
            .map_err(|e| {
                ErrorData::new(
                    ErrorCode::INTERNAL_ERROR,
                    format!("failed to read tool response body: {e}"),
                    None,
                )
            })?
            .to_bytes();

        let tool_output: crate::output_schema::ToolOutput = serde_json::from_slice(&body_bytes)
            .map_err(|e| {
                ErrorData::new(
                    ErrorCode::INTERNAL_ERROR,
                    format!("failed to parse tool response: {e}"),
                    None,
                )
            })?;

        let content: Vec<ContentBlock> = tool_output
            .content
            .into_iter()
            .map(|block| match block {
                crate::output_schema::ContentBlock::Text { text } => ContentBlock::text(text),
                crate::output_schema::ContentBlock::Image { data, mime_type } => {
                    ContentBlock::image(data, mime_type)
                }
            })
            .collect();

        Ok(CallToolResult::success(content))
    }

    /// Map a domain `ToolError` to an MCP protocol error.
    fn to_error_data(e: &ToolError) -> ErrorData {
        let (code, message) = match e {
            ToolError::Unauthorized(msg) => {
                (ErrorCode::INTERNAL_ERROR, format!("unauthorized: {msg}"))
            }
            ToolError::Forbidden(msg) => (ErrorCode::INTERNAL_ERROR, format!("forbidden: {msg}")),
            ToolError::BadRequest(msg) => {
                (ErrorCode::INVALID_PARAMS, format!("bad request: {msg}"))
            }
            ToolError::NotFound => (ErrorCode::INTERNAL_ERROR, "not found".to_string()),
            ToolError::Conflict(msg) => (ErrorCode::INTERNAL_ERROR, format!("conflict: {msg}")),
            ToolError::RateLimited => {
                (ErrorCode::INTERNAL_ERROR, "rate limit exceeded".to_string())
            }
            ToolError::Internal(msg) => {
                (ErrorCode::INTERNAL_ERROR, format!("internal error: {msg}"))
            }
        };
        ErrorData::new(code, message, None)
    }
}

/// Run a domain tool handler with audit recording and result conversion.
///
/// This is a free function (not a method) so it can be called from the
/// `#[tool]` methods without lifetime complications.
async fn run_domain_tool(
    server: &CommonCalServer,
    tool_name: &str,
    identity: &identity::Identity,
    grant: &McpGrant,
    result: Result<Response<axum::body::Body>, ToolError>,
) -> Result<CallToolResult, ErrorData> {
    match result {
        Ok(response) => {
            server
                .record_audit(&AuditRecord {
                    request_id: &uuid::Uuid::new_v4().to_string(),
                    user_id: identity.user_id,
                    client_id: &identity.client_id,
                    grant_id: Some(&grant.grant_id),
                    tool: tool_name,
                    resource_ids: None,
                    auth_result: "allowed",
                    scope: None,
                    auth_strength: &identity.auth_strength.to_string(),
                    latency_ms: 0,
                    result_type: "success",
                    operation_id: None,
                })
                .await;
            CommonCalServer::to_call_tool_result(response).await
        }
        Err(e) => {
            let (auth_result, result_type) = match e {
                ToolError::Unauthorized(_) | ToolError::Forbidden(_) => ("denied", "denied"),
                _ => ("allowed", "error"),
            };
            server
                .record_audit(&AuditRecord {
                    request_id: &uuid::Uuid::new_v4().to_string(),
                    user_id: identity.user_id,
                    client_id: &identity.client_id,
                    grant_id: Some(&grant.grant_id),
                    tool: tool_name,
                    resource_ids: None,
                    auth_result,
                    scope: None,
                    auth_strength: &identity.auth_strength.to_string(),
                    latency_ms: 0,
                    result_type,
                    operation_id: None,
                })
                .await;
            Err(CommonCalServer::to_error_data(&e))
        }
    }
}

#[tool_router]
impl CommonCalServer {
    #[tool(
        description = "List the calendars available to the authenticated user, filtered by the active MCP grant."
    )]
    async fn calendar_list(&self) -> Result<CallToolResult, ErrorData> {
        let identity = self.identity()?;
        let grant = self.resolve_grant(&identity).await?;
        let token = token_from_identity(&identity);
        let context = AuthorizedToolContext {
            token: &token,
            grant: &grant,
            internal_client: &self.internal_client,
        };
        let result = tools::calendar_list::handle(
            &context,
            CalendarListParams {
                include_access: false,
            },
        )
        .await;
        run_domain_tool(self, "calendar_list", &identity, &grant, result).await
    }

    #[tool(
        description = "Find availability slots for the specified calendars within a time range."
    )]
    async fn availability_find(
        &self,
        params: Parameters<AvailabilityFindParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let identity = self.identity()?;
        let grant = self.resolve_grant(&identity).await?;
        let token = token_from_identity(&identity);
        let context = AuthorizedToolContext {
            token: &token,
            grant: &grant,
            internal_client: &self.internal_client,
        };
        let result = tools::availability_find::handle(&context, params.0).await;
        run_domain_tool(self, "availability_find", &identity, &grant, result).await
    }

    #[tool(description = "Get the details of a specific event by calendar and event ID.")]
    async fn event_get(
        &self,
        params: Parameters<EventGetParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let identity = self.identity()?;
        let grant = self.resolve_grant(&identity).await?;
        let token = token_from_identity(&identity);
        let context = AuthorizedToolContext {
            token: &token,
            grant: &grant,
            internal_client: &self.internal_client,
        };
        let result = tools::event_get::handle(&context, params.0).await;
        run_domain_tool(self, "event_get", &identity, &grant, result).await
    }

    #[tool(
        description = "Search events in a calendar within a time range, optionally filtered by query."
    )]
    async fn event_search(
        &self,
        params: Parameters<EventSearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let identity = self.identity()?;
        let grant = self.resolve_grant(&identity).await?;
        let token = token_from_identity(&identity);
        let context = AuthorizedToolContext {
            token: &token,
            grant: &grant,
            internal_client: &self.internal_client,
        };
        let result = tools::event_search::handle(&context, params.0).await;
        run_domain_tool(self, "event_search", &identity, &grant, result).await
    }

    #[tool(description = "Create a new event in the specified calendar.")]
    async fn event_create(
        &self,
        params: Parameters<EventCreateParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let identity = self.identity()?;
        let grant = self.resolve_grant(&identity).await?;
        let token = token_from_identity(&identity);
        let context = AuthorizedToolContext {
            token: &token,
            grant: &grant,
            internal_client: &self.internal_client,
        };
        let result = tools::event_create::handle(&context, params.0).await;
        run_domain_tool(self, "event_create", &identity, &grant, result).await
    }

    #[tool(
        description = "Update an existing event. Requires the expected version for optimistic concurrency."
    )]
    async fn event_update(
        &self,
        params: Parameters<EventUpdateParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let identity = self.identity()?;
        let grant = self.resolve_grant(&identity).await?;
        let token = token_from_identity(&identity);
        let context = AuthorizedToolContext {
            token: &token,
            grant: &grant,
            internal_client: &self.internal_client,
        };
        let result = tools::event_update::handle(&context, params.0).await;
        run_domain_tool(self, "event_update", &identity, &grant, result).await
    }

    #[tool(description = "Set a reminder on an event.")]
    async fn reminder_set(
        &self,
        params: Parameters<ReminderSetParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let identity = self.identity()?;
        let grant = self.resolve_grant(&identity).await?;
        let token = token_from_identity(&identity);
        let context = AuthorizedToolContext {
            token: &token,
            grant: &grant,
            internal_client: &self.internal_client,
        };
        let result = tools::reminder_set::handle(&context, params.0).await;
        run_domain_tool(self, "reminder_set", &identity, &grant, result).await
    }

    #[tool(
        description = "Begin the two-phase deletion of an event; returns a confirmation intent."
    )]
    async fn event_delete_prepare(
        &self,
        params: Parameters<EventDeletePrepareParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let identity = self.identity()?;
        let grant = self.resolve_grant(&identity).await?;
        let token = token_from_identity(&identity);
        let context = AuthorizedToolContext {
            token: &token,
            grant: &grant,
            internal_client: &self.internal_client,
        };
        let result = tools::event_delete_prepare::handle(&context, params.0).await;
        run_domain_tool(self, "event_delete_prepare", &identity, &grant, result).await
    }

    #[tool(description = "Commit a pending event deletion after the user has confirmed.")]
    async fn event_delete_commit(
        &self,
        params: Parameters<EventDeleteCommitParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let identity = self.identity()?;
        let grant = self.resolve_grant(&identity).await?;
        let token = token_from_identity(&identity);
        let context = AuthorizedToolContext {
            token: &token,
            grant: &grant,
            internal_client: &self.internal_client,
        };
        let result = tools::event_delete_commit::handle(&context, params.0).await;
        run_domain_tool(self, "event_delete_commit", &identity, &grant, result).await
    }
}

/// Build a `TokenValidationResult` from the validated identity.
fn token_from_identity(identity: &identity::Identity) -> TokenValidationResult {
    TokenValidationResult {
        user_id: identity.user_id,
        oauth_client_id: identity.client_id.clone(),
        scopes: identity.scopes.clone(),
        auth_strength: identity.auth_strength.clone(),
        auth_time: identity.auth_time,
        token_id: identity.token_id.clone(),
        expires_at: i64::MAX,
    }
}

#[tool_handler]
impl ServerHandler for CommonCalServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "commoncal-mcp",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "CommonCal calendar tools: calendar_list, availability_find, event_get, \
                 event_search, event_create, event_update, reminder_set, event_delete_prepare, \
                 event_delete_commit. All calls are gated by the active MCP grant.",
            )
    }
}
