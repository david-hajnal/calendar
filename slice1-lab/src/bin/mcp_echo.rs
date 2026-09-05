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
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    tool, tool_handler, tool_router,
    transport::{
        StreamableHttpServerConfig,
        streamable_http_server::{
            session::local::LocalSessionManager, tower::StreamableHttpService,
        },
    },
};
use schemars::JsonSchema;
use serde::Deserialize;
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

    // -- Slice 6: typed read tools ------------------------------------------

    #[tool(description = "Find availability slots for the specified calendars within a time range.")]
    async fn availability_find(
        &self,
        params: Parameters<AvailabilityFindParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let claims = CURRENT_CLAIMS
            .read()
            .await
            .clone()
            .ok_or_else(|| err("unauthenticated"))?;

        let (allowed_ids, scopes) = load_grant(&claims).await?;
        require_scope(&scopes, "commoncal.availability.read")?;
        validate_range(&params.0.from, &params.0.to)?;

        let base = commoncal_base();
        let key = commoncal_bridge_key();
        let mut all_slots = Vec::new();

        for cal_id in &params.0.calendar_ids {
            require_calendar(&allowed_ids, *cal_id)?;
            let url = format!(
                "{}/internal/availability?calendar_id={}&from={}&to={}",
                base,
                cal_id,
                urlencoding::encode(&params.0.from),
                urlencoding::encode(&params.0.to)
            );
            let resp = ECHO_HTTP
                .get(&url)
                .header("Authorization", format!("Bearer {}", key))
                .send()
                .await
                .map_err(|e| err(format!("availability transport: {e}")))?;
            if !resp.status().is_success() {
                return Err(err(format!("availability failed: {}", resp.status())));
            }
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| err(format!("availability parse: {e}")))?;
            if let Some(slots) = body.get("slots").and_then(|v| v.as_array()) {
                all_slots.extend(slots.iter().cloned());
            }
        }

        let output = json!({ "slots": all_slots });
        Ok(CallToolResult::success(vec![ContentBlock::text(
            serde_json::to_string_pretty(&output).unwrap_or_else(|_| "[]".to_string()),
        )]))
    }

    #[tool(description = "Get the details of a specific event by calendar and event ID.")]
    async fn event_get(
        &self,
        params: Parameters<EventGetParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let claims = CURRENT_CLAIMS
            .read()
            .await
            .clone()
            .ok_or_else(|| err("unauthenticated"))?;

        let (allowed_ids, scopes) = load_grant(&claims).await?;
        require_calendar(&allowed_ids, params.0.calendar_id)?;

        let has_details = scopes.iter().any(|s| s == "commoncal.event.read.details");
        let has_basic = scopes.iter().any(|s| s == "commoncal.event.read.basic");
        if !has_details && !has_basic {
            return Err(err(
                "event_get requires commoncal.event.read.basic or commoncal.event.read.details",
            ));
        }

        let base = commoncal_base();
        let key = commoncal_bridge_key();
        let url = format!(
            "{}/internal/event?calendar_id={}&event_id={}",
            base,
            params.0.calendar_id,
            params.0.event_id
        );
        let resp = ECHO_HTTP
            .get(&url)
            .header("Authorization", format!("Bearer {}", key))
            .send()
            .await
            .map_err(|e| err(format!("event_get transport: {e}")))?;
        if resp.status().as_u16() == 404 {
            return Err(err("event not found"));
        }
        if !resp.status().is_success() {
            return Err(err(format!("event_get failed: {}", resp.status())));
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| err(format!("event_get parse: {e}")))?;
        let event = body.get("event").cloned().unwrap_or(json!({}));

        let access = if has_details { "full" } else { "basic" };
        let output = if has_details {
            json!({ "event": event, "access": access })
        } else {
            let mut ev = event.clone();
            if let Some(obj) = ev.as_object_mut() {
                obj.remove("description");
                obj.remove("location");
            }
            json!({ "event": ev, "access": access })
        };

        Ok(CallToolResult::success(vec![ContentBlock::text(
            serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{}".to_string()),
        )]))
    }

    #[tool(description = "Search events in a calendar within a time range, optionally filtered by query.")]
    async fn event_search(
        &self,
        params: Parameters<EventSearchParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let claims = CURRENT_CLAIMS
            .read()
            .await
            .clone()
            .ok_or_else(|| err("unauthenticated"))?;

        let (allowed_ids, scopes) = load_grant(&claims).await?;
        require_calendar(&allowed_ids, params.0.calendar_id)?;
        require_scope(&scopes, "commoncal.event.read.basic")?;
        validate_range(&params.0.from, &params.0.to)?;

        let base = commoncal_base();
        let key = commoncal_bridge_key();
        let mut url = format!(
            "{}/internal/events?calendar_id={}&from={}&to={}",
            base,
            params.0.calendar_id,
            urlencoding::encode(&params.0.from),
            urlencoding::encode(&params.0.to)
        );
        if let Some(q) = &params.0.query {
            url.push_str(&format!("&query={}", urlencoding::encode(q)));
        }
        let resp = ECHO_HTTP
            .get(&url)
            .header("Authorization", format!("Bearer {}", key))
            .send()
            .await
            .map_err(|e| err(format!("event_search transport: {e}")))?;
        if !resp.status().is_success() {
            return Err(err(format!("event_search failed: {}", resp.status())));
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| err(format!("event_search parse: {e}")))?;
        let events = body.get("events").cloned().unwrap_or(json!([]));

        let has_details = scopes.iter().any(|s| s == "commoncal.event.read.details");
        let access = if has_details { "full" } else { "basic" };

        let output_events: Vec<serde_json::Value> = if let Some(arr) = events.as_array() {
            if has_details {
                arr.iter().cloned().collect()
            } else {
                arr.iter()
                    .map(|e| {
                        let mut ev = e.clone();
                        if let Some(obj) = ev.as_object_mut() {
                            obj.remove("description");
                            obj.remove("location");
                        }
                        ev
                    })
                    .collect()
            }
        } else {
            Vec::new()
        };

        let output = json!({ "events": output_events, "access": access });
        Ok(CallToolResult::success(vec![ContentBlock::text(
            serde_json::to_string_pretty(&output).unwrap_or_else(|_| "[]".to_string()),
        )]))
    }

    // -- Slice 7: mutation tools ---------------------------------------------

    #[tool(description = "Create a new event in the specified calendar.")]
    async fn event_create(
        &self,
        params: Parameters<EventCreateParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let claims = CURRENT_CLAIMS
            .read()
            .await
            .clone()
            .ok_or_else(|| err("unauthenticated"))?;

        let (allowed_ids, scopes) = load_grant(&claims).await?;
        require_calendar(&allowed_ids, params.0.calendar_id)?;
        require_scope(&scopes, "commoncal.event.create")?;

        let base = commoncal_base();
        let key = commoncal_bridge_key();
        let body = json!({
            "calendar_id": params.0.calendar_id,
            "title": params.0.title,
            "description": params.0.description,
            "location": params.0.location,
            "start_utc": parse_ts(&params.0.start_utc).ok_or_else(|| err(format!("invalid start_utc: {}", params.0.start_utc)))?,
            "end_utc": parse_ts(&params.0.end_utc).ok_or_else(|| err(format!("invalid end_utc: {}", params.0.end_utc)))?,
            "idempotency_key": params.0.idempotency_key,
        });
        let resp = ECHO_HTTP
            .post(format!("{}/internal/event", base))
            .header("Authorization", format!("Bearer {}", key))
            .json(&body)
            .send()
            .await
            .map_err(|e| err(format!("event_create transport: {e}")))?;
        let status = resp.status();
        let resp_body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| err(format!("event_create parse: {e}")))?;
        if !status.is_success() {
            return Err(err(format!("event_create failed: {status}")));
        }
        let output = json!({
            "event": resp_body.get("event").cloned().unwrap_or(json!({})),
            "replayed": resp_body.get("replayed").cloned().unwrap_or(json!(false)),
        });
        Ok(CallToolResult::success(vec![ContentBlock::text(
            serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{}".to_string()),
        )]))
    }

    #[tool(description = "Update an existing event. Requires the expected_version for optimistic concurrency.")]
    async fn event_update(
        &self,
        params: Parameters<EventUpdateParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let claims = CURRENT_CLAIMS
            .read()
            .await
            .clone()
            .ok_or_else(|| err("unauthenticated"))?;

        let (allowed_ids, scopes) = load_grant(&claims).await?;
        require_calendar(&allowed_ids, params.0.calendar_id)?;
        require_scope(&scopes, "commoncal.event.update")?;

        let base = commoncal_base();
        let key = commoncal_bridge_key();
        let body = json!({
            "calendar_id": params.0.calendar_id,
            "expected_version": params.0.expected_version,
            "title": params.0.title,
            "description": params.0.description,
            "location": params.0.location,
            "start_utc": params.0.start_utc.as_deref().and_then(parse_ts),
            "end_utc": params.0.end_utc.as_deref().and_then(parse_ts),
        });
        let resp = ECHO_HTTP
            .patch(format!("{}/internal/event/{}", base, params.0.event_id))
            .header("Authorization", format!("Bearer {}", key))
            .json(&body)
            .send()
            .await
            .map_err(|e| err(format!("event_update transport: {e}")))?;
        let status = resp.status();
        let resp_body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| err(format!("event_update parse: {e}")))?;
        if status.as_u16() == 409 {
            return Err(err("version_conflict: the event was modified by another client"));
        }
        if status.as_u16() == 404 {
            return Err(err("event not found"));
        }
        if !status.is_success() {
            return Err(err(format!("event_update failed: {status}")));
        }
        let output = json!({
            "event": resp_body.get("event").cloned().unwrap_or(json!({})),
        });
        Ok(CallToolResult::success(vec![ContentBlock::text(
            serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{}".to_string()),
        )]))
    }

    #[tool(description = "Set a reminder on an event.")]
    async fn reminder_set(
        &self,
        params: Parameters<ReminderSetParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let claims = CURRENT_CLAIMS
            .read()
            .await
            .clone()
            .ok_or_else(|| err("unauthenticated"))?;

        let (allowed_ids, scopes) = load_grant(&claims).await?;
        require_calendar(&allowed_ids, params.0.calendar_id)?;
        require_scope(&scopes, "commoncal.reminder.write")?;

        let base = commoncal_base();
        let key = commoncal_bridge_key();
        let body = json!({
            "calendar_id": params.0.calendar_id,
            "event_id": params.0.event_id,
            "offset_minutes": params.0.offset_minutes,
        });
        let resp = ECHO_HTTP
            .post(format!("{}/internal/reminder", base))
            .header("Authorization", format!("Bearer {}", key))
            .json(&body)
            .send()
            .await
            .map_err(|e| err(format!("reminder_set transport: {e}")))?;
        let status = resp.status();
        let resp_body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| err(format!("reminder_set parse: {e}")))?;
        if !status.is_success() {
            return Err(err(format!("reminder_set failed: {status}")));
        }
        let output = json!({
            "reminder": resp_body.get("reminder").cloned().unwrap_or(json!({})),
        });
        Ok(CallToolResult::success(vec![ContentBlock::text(
            serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{}".to_string()),
        )]))
    }
}

// -- Slice 6: typed parameter schemas (module-level for rmcp macro) ----------

#[derive(Debug, Deserialize, JsonSchema)]
struct AvailabilityFindParams {
    /// Calendar IDs to check availability for.
    calendar_ids: Vec<i64>,
    /// Start of the time range (ISO 8601 or Unix epoch seconds).
    from: String,
    /// End of the time range (ISO 8601 or Unix epoch seconds).
    to: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct EventGetParams {
    /// The calendar the event belongs to.
    calendar_id: i64,
    /// The event ID to fetch.
    event_id: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct EventSearchParams {
    /// The calendar to search in.
    calendar_id: i64,
    /// Start of the time range (ISO 8601 or Unix epoch seconds).
    from: String,
    /// End of the time range (ISO 8601 or Unix epoch seconds).
    to: String,
    /// Optional text query to filter events by title/description.
    query: Option<String>,
}

// -- Slice 7: typed parameter schemas for mutations --------------------------

#[derive(Debug, Deserialize, JsonSchema)]
struct EventCreateParams {
    /// The calendar to create the event in.
    calendar_id: i64,
    /// Event title.
    title: String,
    /// Optional description.
    description: Option<String>,
    /// Optional location.
    location: Option<String>,
    /// Start time (ISO 8601 or Unix epoch seconds).
    start_utc: String,
    /// End time (ISO 8601 or Unix epoch seconds).
    end_utc: String,
    /// Optional idempotency key for safe retries.
    idempotency_key: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct EventUpdateParams {
    /// The calendar the event belongs to.
    calendar_id: i64,
    /// The event ID to update.
    event_id: i64,
    /// The expected current version (optimistic concurrency).
    expected_version: i64,
    /// New title (omit to keep current).
    title: Option<String>,
    /// New description (omit to keep current).
    description: Option<String>,
    /// New location (omit to keep current).
    location: Option<String>,
    /// New start time (omit to keep current).
    start_utc: Option<String>,
    /// New end time (omit to keep current).
    end_utc: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ReminderSetParams {
    /// The calendar the event belongs to.
    calendar_id: i64,
    /// The event to set a reminder on.
    event_id: i64,
    /// Minutes before the event to fire the reminder.
    offset_minutes: i64,
}

// -- Slice 6: shared helpers -------------------------------------------------

fn require_scope(scopes: &[String], scope: &str) -> Result<(), rmcp::ErrorData> {
    if scopes.iter().any(|s| s == scope) {
        Ok(())
    } else {
        Err(err(format!("missing required scope: {scope}")))
    }
}

fn require_calendar(allowed_ids: &[i64], calendar_id: i64) -> Result<(), rmcp::ErrorData> {
    if allowed_ids.contains(&calendar_id) {
        Ok(())
    } else {
        Err(err(format!("calendar {calendar_id} not in grant")))
    }
}

fn parse_ts(s: &str) -> Option<i64> {
    if let Ok(ts) = s.parse::<i64>() {
        return Some(ts);
    }
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.timestamp())
}

fn validate_range(from: &str, to: &str) -> Result<(i64, i64), rmcp::ErrorData> {
    let from_ts = parse_ts(from).ok_or_else(|| err(format!("invalid 'from': {from}")))?;
    let to_ts = parse_ts(to).ok_or_else(|| err(format!("invalid 'to': {to}")))?;
    if to_ts <= from_ts {
        return Err(err("'to' must be after 'from'"));
    }
    let max_secs = 31 * 24 * 3600;
    if to_ts - from_ts > max_secs {
        return Err(err("time range exceeds maximum of 31 days"));
    }
    Ok((from_ts, to_ts))
}

async fn load_grant(
    claims: &CurrentClaims,
) -> Result<(Vec<i64>, Vec<String>), rmcp::ErrorData> {
    let base = commoncal_base();
    let key = commoncal_bridge_key();
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
    let scopes: Vec<String> = grant_body
        .pointer("/grant/scopes")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    Ok((allowed_ids, scopes))
}

#[tool_handler]
impl ServerHandler for CommonCalEcho {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("commoncal-echo", "0.4.0"))
            .with_instructions(
                "Slice 7: calendar_list, availability_find, event_get, event_search, event_create, event_update, reminder_set backed by real CommonCal + mcp_grant.",
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
