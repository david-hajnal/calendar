//! Slice 2 CommonCal service — real session-based consent, grant storage,
//! and internal API for the MCP server.
//!
//! Replaces the Slice 1 fake auto-approver. This service:
//! - Has a login endpoint that creates a real session (cookie-based)
//! - Renders a consent page (HTML) that requires an authenticated session
//! - Creates a unique mcp_grant transactionally on approve (grant-first)
//! - Provides an internal API for the MCP server to fetch calendars and grants
//! - Supports grant revocation (immediate, otherwise-valid JWT becomes 403)
//!
//! All state is in-memory (lab-only). The real system uses SQLite.
//! No real secrets are stored; the bridge key and session tokens are lab values.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, error, info};

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct User {
    id: i64,
    email: String,
    password: String,
}

#[derive(Debug, Clone)]
struct Session {
    user_id: i64,
    expires_at: i64,
}

#[derive(Debug, Clone, Serialize)]
struct Calendar {
    id: i64,
    name: String,
    owner_id: i64,
}

#[derive(Debug, Clone, Serialize)]
struct McpGrant {
    id: String,
    user_id: i64,
    oauth_client_id: String,
    allowed_calendar_ids: Vec<i64>,
    scopes: Vec<String>,
    created_at: i64,
    revoked_at: Option<i64>,
}

#[derive(Debug, Clone)]
struct CsrfEntry {
    session_id: String,
    expires_at: i64,
}

/// A single-use magic-link login token, bound to the recipient email and the
/// same-origin continuation path it must return to after login.
#[derive(Debug, Clone)]
struct MagicLink {
    email: String,
    continue_url: String,
    expires_at: i64,
    used: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InteractionView {
    client_id: String,
    client_name: String,
    redirect_uri: String,
    resource: String,
    requested_scopes: Vec<String>,
    /// The granted scopes (intersection of requested and catalog), computed by
    /// the auth server. Recorded on the grant so it agrees with the JWT.
    #[serde(default)]
    granted_scopes: Vec<String>,
    prompt: String,
    expires_at: i64,
}

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct AppState {
    inner: Arc<RwLock<InnerState>>,
    auth_private_base: String,
    bridge_key: String,
    http: reqwest::Client,
}

struct InnerState {
    users: HashMap<String, User>,
    sessions: HashMap<String, Session>,
    calendars: HashMap<i64, Calendar>,
    grants: HashMap<String, McpGrant>,
    csrf: HashMap<String, CsrfEntry>,
    magic_links: HashMap<String, MagicLink>,
    next_calendar_id: i64,
    next_user_id: i64,
    /// Lab test hook: when > 0, the next `decide_interaction` call(s) fail and
    /// the counter decrements. Simulates a bridge timeout so the harness can
    /// prove grant-first / bridge-second idempotent retry.
    decide_fail_remaining: u32,
}

impl AppState {
    fn new(auth_private_base: String, bridge_key: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("http client");

        let mut inner = InnerState {
            users: HashMap::new(),
            sessions: HashMap::new(),
            calendars: HashMap::new(),
            grants: HashMap::new(),
            csrf: HashMap::new(),
            magic_links: HashMap::new(),
            next_calendar_id: 1,
            next_user_id: 1,
            decide_fail_remaining: 0,
        };

        // Seed the fixed lab user (matches FIXED_SUBJECT = "1" in the auth server).
        let lab_user = User {
            id: 1,
            email: "lab@commoncal.test".to_string(),
            password: "lab-password-123".to_string(),
        };
        inner.users.insert(lab_user.email.clone(), lab_user);
        inner.next_user_id = 2;

        // Seed two real calendars owned by the lab user.
        let cal1 = Calendar {
            id: 1,
            name: "Work Calendar".to_string(),
            owner_id: 1,
        };
        let cal2 = Calendar {
            id: 2,
            name: "Personal Calendar".to_string(),
            owner_id: 1,
        };
        inner.calendars.insert(1, cal1);
        inner.calendars.insert(2, cal2);
        inner.next_calendar_id = 3;

        Self {
            inner: Arc::new(RwLock::new(inner)),
            auth_private_base,
            bridge_key,
            http,
        }
    }

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }

    // -- session helpers ----------------------------------------------------

    async fn create_session(&self, user_id: i64) -> String {
        let token = format!("sess_{}", uuid::Uuid::new_v4().simple());
        let expires = Self::now() + 3600;
        let mut st = self.inner.write().await;
        st.sessions.insert(
            token.clone(),
            Session {
                user_id,
                expires_at: expires,
            },
        );
        token
    }

    async fn get_session(&self, token: &str) -> Option<i64> {
        let st = self.inner.read().await;
        let sess = st.sessions.get(token)?;
        if sess.expires_at < Self::now() {
            return None;
        }
        Some(sess.user_id)
    }

    async fn destroy_session(&self, token: &str) {
        let mut st = self.inner.write().await;
        st.sessions.remove(token);
    }

    // -- csrf helpers -------------------------------------------------------

    async fn create_csrf(&self, session_id: &str) -> String {
        let token = format!("csrf_{}", uuid::Uuid::new_v4().simple());
        let expires = Self::now() + 600;
        let mut st = self.inner.write().await;
        st.csrf.insert(
            token.clone(),
            CsrfEntry {
                session_id: session_id.to_string(),
                expires_at: expires,
            },
        );
        token
    }

    async fn validate_csrf(&self, token: &str, session_id: &str) -> bool {
        let st = self.inner.read().await;
        let Some(entry) = st.csrf.get(token) else {
            return false;
        };
        if entry.expires_at < Self::now() {
            return false;
        }
        entry.session_id == session_id
    }

    async fn consume_csrf(&self, token: &str) {
        let mut st = self.inner.write().await;
        st.csrf.remove(token);
    }

    // -- magic-link helpers -------------------------------------------------

    /// Create a single-use magic-link token bound to (email, continue_url).
    /// Returns the opaque token. The `continue_url` is already validated to be
    /// a same-origin relative path by the caller.
    async fn create_magic_link(&self, email: &str, continue_url: &str) -> String {
        let token = format!("magic_{}", uuid::Uuid::new_v4().simple());
        let expires = Self::now() + 600;
        let mut st = self.inner.write().await;
        st.magic_links.insert(
            token.clone(),
            MagicLink {
                email: email.to_string(),
                continue_url: continue_url.to_string(),
                expires_at: expires,
                used: false,
            },
        );
        token
    }

    /// Consume a magic-link token. Returns (email, continue_url) if valid and
    /// unused; None if unknown, expired, or already used (single-use).
    async fn verify_magic_link(&self, token: &str) -> Option<(String, String)> {
        let mut st = self.inner.write().await;
        let link = st.magic_links.get_mut(token)?;
        if link.used || link.expires_at < Self::now() {
            return None;
        }
        link.used = true;
        Some((link.email.clone(), link.continue_url.clone()))
    }

    // -- test hooks (lab-only, bridge-keyed) --------------------------------

    /// Add a user. Returns the new user id.
    async fn test_add_user(&self, email: &str, password: &str) -> i64 {
        let mut st = self.inner.write().await;
        if st.users.contains_key(email) {
            return st.users[email].id;
        }
        let id = st.next_user_id;
        st.next_user_id += 1;
        st.users.insert(
            email.to_string(),
            User {
                id,
                email: email.to_string(),
                password: password.to_string(),
            },
        );
        id
    }

    /// Add a calendar owned by `user_id`. Returns the new calendar id.
    async fn test_add_calendar(&self, user_id: i64, name: &str) -> i64 {
        let mut st = self.inner.write().await;
        let id = st.next_calendar_id;
        st.next_calendar_id += 1;
        st.calendars.insert(
            id,
            Calendar {
                id,
                name: name.to_string(),
                owner_id: user_id,
            },
        );
        id
    }

    /// Remove a calendar if it belongs to `user_id`. Returns true if removed.
    async fn test_remove_calendar(&self, user_id: i64, calendar_id: i64) -> bool {
        let mut st = self.inner.write().await;
        match st.calendars.get(&calendar_id) {
            Some(c) if c.owner_id == user_id => {
                st.calendars.remove(&calendar_id);
                true
            }
            _ => false,
        }
    }

    /// Arm the bridge-failure test hook: the next `n` decide calls fail.
    async fn test_arm_decide_failure(&self, n: u32) {
        let mut st = self.inner.write().await;
        st.decide_fail_remaining = n;
    }

    /// List ALL grants (active and revoked) for a user — for harness assertions.
    async fn test_list_grants_for_user(&self, user_id: i64) -> Vec<McpGrant> {
        let st = self.inner.read().await;
        st.grants
            .values()
            .filter(|g| g.user_id == user_id)
            .cloned()
            .collect()
    }

    // -- grant helpers ------------------------------------------------------

    /// Create a unique mcp_grant for (user_id, oauth_client_id).
    /// Replaces any existing active grant (no union — replace semantics).
    /// Returns the grant ID.
    async fn upsert_grant(
        &self,
        user_id: i64,
        oauth_client_id: &str,
        allowed_calendar_ids: Vec<i64>,
        scopes: Vec<String>,
    ) -> String {
        let now = Self::now();
        let mut st = self.inner.write().await;

        // Revoke any existing active grant for this (user, client) pair.
        // This is "replace rather than union" semantics.
        for grant in st.grants.values_mut() {
            if grant.user_id == user_id
                && grant.oauth_client_id == oauth_client_id
                && grant.revoked_at.is_none()
            {
                grant.revoked_at = Some(now);
            }
        }

        let grant_id = format!("grant_{}", uuid::Uuid::new_v4().simple());
        let grant = McpGrant {
            id: grant_id.clone(),
            user_id,
            oauth_client_id: oauth_client_id.to_string(),
            allowed_calendar_ids,
            scopes,
            created_at: now,
            revoked_at: None,
        };
        st.grants.insert(grant_id.clone(), grant);
        debug!(grant_id, user_id, oauth_client_id, "mcp_grant created");
        grant_id
    }

    /// Get the active grant for (user_id, oauth_client_id), if any.
    async fn get_active_grant(&self, user_id: i64, oauth_client_id: &str) -> Option<McpGrant> {
        let st = self.inner.read().await;
        st.grants
            .values()
            .find(|g| {
                g.user_id == user_id
                    && g.oauth_client_id == oauth_client_id
                    && g.revoked_at.is_none()
                    && g.scopes.iter().any(|s| {
                        s == "commoncal.calendar.metadata.read"
                            || s == "commoncal.availability.read"
                    })
            })
            .cloned()
    }

    /// Get the active grant for (user_id, oauth_client_id) regardless of scope.
    async fn get_grant_by_pair(&self, user_id: i64, oauth_client_id: &str) -> Option<McpGrant> {
        let st = self.inner.read().await;
        st.grants
            .values()
            .find(|g| {
                g.user_id == user_id
                    && g.oauth_client_id == oauth_client_id
                    && g.revoked_at.is_none()
            })
            .cloned()
    }

    /// Revoke the active grant for (user_id, oauth_client_id).
    async fn revoke_grant(&self, user_id: i64, oauth_client_id: &str) -> bool {
        let now = Self::now();
        let mut st = self.inner.write().await;
        let mut revoked = false;
        for grant in st.grants.values_mut() {
            if grant.user_id == user_id
                && grant.oauth_client_id == oauth_client_id
                && grant.revoked_at.is_none()
            {
                grant.revoked_at = Some(now);
                revoked = true;
            }
        }
        if revoked {
            debug!(user_id, oauth_client_id, "mcp_grant revoked");
        }
        revoked
    }

    /// List the active grants for a user (authenticated management view).
    async fn list_active_grants(&self, user_id: i64) -> Vec<McpGrant> {
        let st = self.inner.read().await;
        st.grants
            .values()
            .filter(|g| g.user_id == user_id && g.revoked_at.is_none())
            .cloned()
            .collect()
    }

    /// Find a grant by id, enforcing that it belongs to `user_id` (ownership
    /// check). Returns None if absent or owned by another user (cross-user).
    async fn find_grant_owned(&self, grant_id: &str, user_id: i64) -> Option<McpGrant> {
        let st = self.inner.read().await;
        st.grants
            .get(grant_id)
            .filter(|g| g.user_id == user_id)
            .cloned()
    }

    /// Revoke a grant by id, enforcing ownership. Returns true if revoked.
    async fn revoke_grant_by_id(&self, grant_id: &str, user_id: i64) -> bool {
        let now = Self::now();
        let mut st = self.inner.write().await;
        let Some(grant) = st.grants.get_mut(grant_id) else {
            return false;
        };
        if grant.user_id != user_id {
            return false;
        }
        if grant.revoked_at.is_none() {
            grant.revoked_at = Some(now);
            debug!(grant_id, user_id, "mcp_grant revoked (authenticated)");
            true
        } else {
            false
        }
    }

    // -- calendar helpers ---------------------------------------------------

    async fn list_calendars_for_user(&self, user_id: i64) -> Vec<Calendar> {
        let st = self.inner.read().await;
        st.calendars
            .values()
            .filter(|c| c.owner_id == user_id)
            .cloned()
            .collect()
    }

    // -- auth server bridge -------------------------------------------------

    /// Look up a handoff from the auth server's private API.
    async fn lookup_interaction(&self, handoff: &str) -> Result<InteractionView, String> {
        let url = format!(
            "{}/internal/interactions/{}",
            self.auth_private_base,
            urlencoding::encode(handoff)
        );
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.bridge_key))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            tracing::error!(status = %status, body = %body, "lookup_interaction failed");
            return Err(format!("interaction lookup failed: {status} {body}"));
        }
        let body = resp.text().await.map_err(|e| e.to_string())?;
        let view: InteractionView =
            serde_json::from_str(&body).map_err(|e| format!("parse interaction view: {e}"))?;
        Ok(view)
    }

    /// Decide a handoff (login/consent/deny) via the auth server's private API.
    /// `subject` is the authenticated CommonCal user id (the identity authority
    /// is CommonCal; the auth server binds the provider grant to this subject
    /// rather than a hardcoded value). Returns the resume URL.
    ///
    /// Honors the lab bridge-failure test hook: if armed, this call fails
    /// (simulating a bridge timeout) and the hook decrements, so a retry
    /// succeeds — proving grant-first / bridge-second idempotent retry.
    async fn decide_interaction(
        &self,
        handoff: &str,
        kind: &str,
        subject: Option<i64>,
    ) -> Result<String, String> {
        // Lab test hook: simulate a bridge failure on the next CONSENT decide
        // call(s). Only consent decisions are affected — the grant-first /
        // bridge-second ordering is a consent-path property, and login decisions
        // must not be disrupted (they precede the grant write).
        if kind == "consent" {
            let mut st = self.inner.write().await;
            if st.decide_fail_remaining > 0 {
                st.decide_fail_remaining -= 1;
                tracing::warn!("simulating bridge decide failure (test hook)");
                return Err("simulated bridge timeout (test hook)".to_string());
            }
        }

        let url = format!(
            "{}/internal/interactions/{}",
            self.auth_private_base,
            urlencoding::encode(handoff)
        );
        let mut body = serde_json::json!({ "kind": kind });
        if let Some(sub) = subject {
            body["subject"] = serde_json::json!(sub);
        }
        let resp = self
            .http
            .put(&url)
            .header("Authorization", format!("Bearer {}", self.bridge_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            tracing::error!(status = %status, text = %text, "decide_interaction failed");
            return Err(format!("interaction decide failed: {status} {text}"));
        }
        let result: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        result
            .get("resumeUrl")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "decide response missing resumeUrl".to_string())
    }
}

// ---------------------------------------------------------------------------
// HTTP handlers
// ---------------------------------------------------------------------------

async fn health() -> &'static str {
    "ok"
}

#[derive(Deserialize)]
struct ConsentQuery {
    handoff: Option<String>,
}

/// GET /consent?handoff=...
///
/// If the interaction prompt is "login" and the user has a session, decide
/// login immediately (redirect to resume URL). If the prompt is "consent",
/// render the consent page. If no session, redirect to /login.
async fn consent_page(
    State(state): State<AppState>,
    Query(q): Query<ConsentQuery>,
    headers: HeaderMap,
) -> Response {
    let handoff = match &q.handoff {
        Some(h) if !h.is_empty() => h.clone(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "missing_handoff"})),
            )
                .into_response();
        }
    };

    // Look up the interaction from the auth server.
    let view = match state.lookup_interaction(&handoff).await {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({"error": e})),
            )
                .into_response();
        }
    };

    // Check if the user has a session.
    let session_token = extract_session_cookie(&headers);
    let user_id = match &session_token {
        Some(t) => state.get_session(t).await,
        None => None,
    };

    // If no session, redirect to login with a continue param.
    let continue_url = format!("/consent?handoff={}", urlencoding::encode(&handoff));
    let (session_token, user_id) = match (&session_token, user_id) {
        (Some(t), Some(uid)) => (t.clone(), uid),
        _ => {
            let location = format!("/login?continue={}", urlencoding::encode(&continue_url));
            return (StatusCode::SEE_OTHER, [(header::LOCATION, location)]).into_response();
        }
    };

    // If the prompt is "login", decide login immediately (user is authenticated).
    if view.prompt == "login" {
        match state
            .decide_interaction(&handoff, "login", Some(user_id))
            .await
        {
            Ok(resume_url) => {
                return (StatusCode::SEE_OTHER, [(header::LOCATION, resume_url)]).into_response();
            }
            Err(e) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({"error": e})),
                )
                    .into_response();
            }
        }
    }

    // If the prompt is "consent", render the consent page.
    if view.prompt == "consent" {
        let csrf_token = state.create_csrf(&session_token).await;
        let scopes_html: Vec<String> = view
            .requested_scopes
            .iter()
            .map(|s| format!("<li>{}</li>", escape_html(s)))
            .collect();
        let html = format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"><title>CommonCal — Authorize</title>
<style>body{{font-family:system-ui,sans-serif;max-width:480px;margin:2rem auto;padding:0 1rem}}
.card{{border:1px solid #ddd;border-radius:8px;padding:1.5rem;margin-top:1rem}}
button{{padding:.5rem 1rem;margin-right:.5rem;cursor:pointer;border-radius:4px;border:1px solid #ccc}}
.approve{{background:#22c55e;color:#fff;border-color:#16a34a}}
.deny{{background:#fff;color:#dc2626;border-color:#dc2626}}
ul{{padding-left:1.2rem}}</style></head>
<body>
<h1>Authorize access</h1>
<div class="card">
<p><strong>{}</strong> is requesting access to your CommonCal account.</p>
<p>Requested permissions:</p>
<ul>{}</ul>
<form method="POST" action="/consent/decision">
<input type="hidden" name="handoff" value="{}">
<input type="hidden" name="csrf" value="{}">
<button type="submit" name="decision" value="approve" class="approve">Approve</button>
<button type="submit" name="decision" value="deny" class="deny">Deny</button>
</form>
</div>
</body></html>"#,
            escape_html(&view.client_name),
            scopes_html.join(""),
            escape_html(&handoff),
            escape_html(&csrf_token),
        );
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            html,
        )
            .into_response();
    }

    // Unknown prompt — deny.
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({"error": "unknown_prompt", "prompt": view.prompt})),
    )
        .into_response()
}

/// POST /login?continue=...
/// Form-encoded login (from the consent page redirect).
async fn login_form(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    let params = parse_form(&body);
    let email = params.get("email").cloned().unwrap_or_default();
    let password = params.get("password").cloned().unwrap_or_default();
    // Open-redirect guard: an unsafe `continue` falls back to the safe root.
    let continue_url = params
        .get("continue")
        .and_then(|v| validate_continuation(v))
        .unwrap_or_else(|| "/".to_string());

    let user = {
        let st = state.inner.read().await;
        st.users.get(&email).cloned()
    };

    let Some(user) = user else {
        return (StatusCode::UNAUTHORIZED, "Invalid credentials").into_response();
    };
    if user.password != password {
        return (StatusCode::UNAUTHORIZED, "Invalid credentials").into_response();
    }

    let session_token = state.create_session(user.id).await;
    let set_cookie = format!(
        "commoncal_session={}; Path=/; HttpOnly; SameSite=Lax",
        session_token
    );
    (
        StatusCode::SEE_OTHER,
        [
            (header::SET_COOKIE, set_cookie),
            (header::LOCATION, continue_url),
        ],
    )
        .into_response()
}

/// GET /login?continue=... — render a minimal login form.
async fn login_page(Query(q): Query<LoginQuery>) -> Response {
    let continue_val = q.continue_url.unwrap_or_else(|| "/".to_string());
    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"><title>CommonCal — Sign in</title>
<style>body{{font-family:system-ui,sans-serif;max-width:400px;margin:2rem auto;padding:0 1rem}}
.card{{border:1px solid #ddd;border-radius:8px;padding:1.5rem}}
input{{display:block;width:100%;padding:.5rem;margin:.5rem 0;box-sizing:border-box}}
button{{padding:.5rem 1rem;cursor:pointer;border-radius:4px;background:#3b82f6;color:#fff;border:none}}</style></head>
<body>
<h1>Sign in to CommonCal</h1>
<div class="card">
<form method="POST" action="/login">
<input type="hidden" name="continue" value="{}">
<label>Email</label><input type="email" name="email" required>
<label>Password</label><input type="password" name="password" required>
<button type="submit">Sign in</button>
</form>
<p><small>Lab credentials: lab@commoncal.test / lab-password-123</small></p>
</div>
</body></html>"#,
        escape_html(&continue_val),
    );
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

#[derive(Deserialize)]
struct LoginQuery {
    continue_url: Option<String>,
}

/// POST /login/magic-link — request a magic-link login.
/// Form: email, continue. Creates a single-use token bound to (email,
/// same-origin continue). In production this would be emailed; the lab returns
/// the link so the harness can follow it.
async fn magic_link_request(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    let params = parse_form(&body);
    let email = params.get("email").cloned().unwrap_or_default();
    let continue_url = params
        .get("continue")
        .and_then(|v| validate_continuation(v))
        .unwrap_or_else(|| "/".to_string());

    let exists = {
        let st = state.inner.read().await;
        st.users.contains_key(&email)
    };
    if !exists {
        // Do not reveal whether the account exists (anti-enumeration).
        return (
            StatusCode::OK,
            Json(serde_json::json!({ "sent": true, "magic_link": null })),
        )
            .into_response();
    }

    let token = state.create_magic_link(&email, &continue_url).await;
    // Lab-only: surface the link so the harness can follow it. A real service
    // would deliver it out-of-band (email) and never return it to the caller.
    let link = format!("/login/magic-link/verify?token={token}");
    (
        StatusCode::OK,
        Json(serde_json::json!({ "sent": true, "magic_link": link })),
    )
        .into_response()
}

/// GET /login/magic-link/verify?token=... — consume a magic link, create a
/// session, and return to the bound same-origin continuation.
async fn magic_link_verify(
    State(state): State<AppState>,
    Query(q): Query<MagicLinkQuery>,
) -> Response {
    let Some(token) = q.token.filter(|t| !t.is_empty()) else {
        return (StatusCode::BAD_REQUEST, "missing token").into_response();
    };

    let Some((email, continue_url)) = state.verify_magic_link(&token).await else {
        // Unknown, expired, or already-used token (single-use).
        return (StatusCode::FORBIDDEN, "invalid or expired magic link").into_response();
    };

    let user_id = {
        let st = state.inner.read().await;
        st.users.get(&email).map(|u| u.id)
    };
    let Some(user_id) = user_id else {
        return (StatusCode::FORBIDDEN, "unknown user").into_response();
    };

    let session_token = state.create_session(user_id).await;
    let set_cookie = format!(
        "commoncal_session={}; Path=/; HttpOnly; SameSite=Lax",
        session_token
    );
    (
        StatusCode::SEE_OTHER,
        [
            (header::SET_COOKIE, set_cookie),
            (header::LOCATION, continue_url),
        ],
    )
        .into_response()
}

#[derive(Deserialize)]
struct MagicLinkQuery {
    token: Option<String>,
}

/// POST /consent/decision — form-encoded approve/deny.
async fn consent_decision(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let params = parse_form(&body);
    let handoff = match params.get("handoff").cloned() {
        Some(h) => h,
        None => return (StatusCode::BAD_REQUEST, "missing handoff").into_response(),
    };
    let csrf = match params.get("csrf").cloned() {
        Some(c) => c,
        None => return (StatusCode::BAD_REQUEST, "missing csrf").into_response(),
    };
    let decision = match params.get("decision").cloned() {
        Some(d) if d == "approve" || d == "deny" => d,
        _ => return (StatusCode::BAD_REQUEST, "invalid decision").into_response(),
    };

    // Validate session.
    let session_token = match extract_session_cookie(&headers) {
        Some(t) => t,
        None => return (StatusCode::UNAUTHORIZED, "no session").into_response(),
    };
    let user_id = match state.get_session(&session_token).await {
        Some(id) => id,
        None => return (StatusCode::UNAUTHORIZED, "invalid session").into_response(),
    };

    // Validate CSRF.
    if !state.validate_csrf(&csrf, &session_token).await {
        return (StatusCode::FORBIDDEN, "invalid csrf token").into_response();
    }
    state.consume_csrf(&csrf).await;

    // Look up the interaction to get the requested scopes and client ID.
    let view = match state.lookup_interaction(&handoff).await {
        Ok(v) => v,
        Err(e) => return (StatusCode::BAD_GATEWAY, e).into_response(),
    };

    if decision == "deny" {
        match state
            .decide_interaction(&handoff, "deny", Some(user_id))
            .await
        {
            Ok(resume_url) => {
                return (StatusCode::SEE_OTHER, [(header::LOCATION, resume_url)]).into_response();
            }
            Err(e) => return (StatusCode::BAD_GATEWAY, e).into_response(),
        }
    }

    // Approve: grant-first ordering.
    // 1. Create the mcp_grant (in CommonCal's store).
    // 2. Then decide consent via the auth server.
    // If step 2 fails, the grant still exists (safe — user can revoke).

    // Determine which calendars the user owns (eligible calendars).
    let calendars = state.list_calendars_for_user(user_id).await;
    let allowed_ids: Vec<i64> = calendars.iter().map(|c| c.id).collect();

    // The granted scopes are the intersection of requested and catalog, computed
    // by the auth server and returned in the handoff view. Record these (not the
    // raw requested set) so the grant agrees with the JWT. Fall back to the
    // requested set only if the view predates the grantedScopes field.
    let granted_scopes = if view.granted_scopes.is_empty() {
        view.requested_scopes.clone()
    } else {
        view.granted_scopes.clone()
    };

    // Grant created. Now decide consent (grant-first: grant precedes consent).
    state
        .upsert_grant(user_id, &view.client_id, allowed_ids, granted_scopes)
        .await;
    match state
        .decide_interaction(&handoff, "consent", Some(user_id))
        .await
    {
        Ok(resume_url) => {
            info!(user_id, client_id = %view.client_id, "consent approved, grant created");
            (StatusCode::SEE_OTHER, [(header::LOCATION, resume_url)]).into_response()
        }
        Err(e) => {
            // Grant exists but consent failed. This is safe (grant-first).
            error!(error = %e, "consent decision failed after grant creation");
            (StatusCode::BAD_GATEWAY, e).into_response()
        }
    }
}

// -- Internal API (for the MCP server) --------------------------------------

fn check_internal_key(headers: &HeaderMap, expected: &str) -> bool {
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|a| a.strip_prefix("Bearer "))
        .unwrap_or("");
    // Lab-only: constant-time comparison not required for loopback test infra.
    presented.len() == expected.len() && presented.as_bytes().eq(expected.as_bytes())
}

/// GET /internal/calendars/:user_id
async fn internal_list_calendars(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(user_id): axum::extract::Path<i64>,
) -> Response {
    if !check_internal_key(&headers, &state.bridge_key) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let calendars = state.list_calendars_for_user(user_id).await;
    (
        StatusCode::OK,
        Json(serde_json::json!({ "calendars": calendars })),
    )
        .into_response()
}

/// GET /internal/grant?user_id=&client_id=
#[derive(Deserialize)]
struct GrantQuery {
    user_id: i64,
    client_id: String,
}

async fn internal_get_grant(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<GrantQuery>,
) -> Response {
    if !check_internal_key(&headers, &state.bridge_key) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    match state.get_grant_by_pair(q.user_id, &q.client_id).await {
        Some(grant) => {
            (StatusCode::OK, Json(serde_json::json!({ "grant": grant }))).into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "no_active_grant"})),
        )
            .into_response(),
    }
}

/// POST /internal/grant/revoke  body: {user_id, client_id}
#[derive(Deserialize)]
struct RevokeRequest {
    user_id: i64,
    client_id: String,
}

async fn internal_revoke_grant(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<RevokeRequest>,
) -> Response {
    if !check_internal_key(&headers, &state.bridge_key) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    match state.revoke_grant(req.user_id, &req.client_id).await {
        true => (StatusCode::OK, Json(serde_json::json!({"revoked": true}))).into_response(),
        false => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "no_active_grant"})),
        )
            .into_response(),
    }
}

// -- Lab test hooks (bridge-keyed, harness-only) ----------------------------

/// POST /internal/test/add-user  body: {email, password}
async fn test_add_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<serde_json::Value>,
) -> Response {
    if !check_internal_key(&headers, &state.bridge_key) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let email = req
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let password = req
        .get("password")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let id = state.test_add_user(&email, &password).await;
    (StatusCode::OK, Json(serde_json::json!({ "user_id": id }))).into_response()
}

/// POST /internal/test/add-calendar  body: {user_id, name}
async fn test_add_calendar(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<serde_json::Value>,
) -> Response {
    if !check_internal_key(&headers, &state.bridge_key) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let user_id = req.get("user_id").and_then(|v| v.as_i64()).unwrap_or(0);
    let name = req
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let id = state.test_add_calendar(user_id, &name).await;
    (
        StatusCode::OK,
        Json(serde_json::json!({ "calendar_id": id })),
    )
        .into_response()
}

/// POST /internal/test/remove-calendar  body: {user_id, calendar_id}
async fn test_remove_calendar(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<serde_json::Value>,
) -> Response {
    if !check_internal_key(&headers, &state.bridge_key) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let user_id = req.get("user_id").and_then(|v| v.as_i64()).unwrap_or(0);
    let calendar_id = req.get("calendar_id").and_then(|v| v.as_i64()).unwrap_or(0);
    let removed = state.test_remove_calendar(user_id, calendar_id).await;
    (
        StatusCode::OK,
        Json(serde_json::json!({ "removed": removed })),
    )
        .into_response()
}

/// POST /internal/test/fail-next-decide  body: {n}
/// Arms the bridge-failure hook so the next `n` consent decisions fail.
async fn test_fail_next_decide(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<serde_json::Value>,
) -> Response {
    if !check_internal_key(&headers, &state.bridge_key) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let n = req.get("n").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
    state.test_arm_decide_failure(n).await;
    (StatusCode::OK, Json(serde_json::json!({ "armed": n }))).into_response()
}

#[derive(Deserialize)]
struct UserIdQuery {
    user_id: i64,
}

/// GET /internal/test/grants?user_id= — list ALL grants (active + revoked)
/// for a user, for harness assertions.
async fn test_list_grants(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<UserIdQuery>,
) -> Response {
    if !check_internal_key(&headers, &state.bridge_key) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let grants = state.test_list_grants_for_user(q.user_id).await;
    (
        StatusCode::OK,
        Json(serde_json::json!({ "grants": grants })),
    )
        .into_response()
}

// -- Authenticated grant management (session-based) -------------------------

/// Resolve the authenticated user id from the session cookie, or None.
async fn session_user_id(state: &AppState, headers: &HeaderMap) -> Option<i64> {
    let token = extract_session_cookie(headers)?;
    state.get_session(&token).await
}

/// GET /grants — list the authenticated user's active grants.
async fn mgmt_list_grants(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(user_id) = session_user_id(&state, &headers).await else {
        return (StatusCode::UNAUTHORIZED, "no session").into_response();
    };
    let grants = state.list_active_grants(user_id).await;
    (
        StatusCode::OK,
        Json(serde_json::json!({ "grants": grants })),
    )
        .into_response()
}

/// DELETE /grants/:id — revoke one of the authenticated user's grants.
/// Ownership is enforced: a grant belonging to another user is not found.
async fn mgmt_revoke_grant(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(grant_id): axum::extract::Path<String>,
) -> Response {
    let Some(user_id) = session_user_id(&state, &headers).await else {
        return (StatusCode::UNAUTHORIZED, "no session").into_response();
    };
    match state.revoke_grant_by_id(&grant_id, user_id).await {
        true => (StatusCode::OK, Json(serde_json::json!({ "revoked": true }))).into_response(),
        false => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "grant_not_found_or_not_owned" })),
        )
            .into_response(),
    }
}

/// PATCH /grants/:id — narrow the calendars an active grant covers.
/// Only a subset of the currently-allowed calendars may be set (no broadening).
#[derive(Deserialize)]
struct UpdateGrantPayload {
    allowed_calendar_ids: Vec<i64>,
}

async fn mgmt_update_grant(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(grant_id): axum::extract::Path<String>,
    Json(payload): Json<UpdateGrantPayload>,
) -> Response {
    let Some(user_id) = session_user_id(&state, &headers).await else {
        return (StatusCode::UNAUTHORIZED, "no session").into_response();
    };
    // Ownership + current allowed set.
    let current = match state.find_grant_owned(&grant_id, user_id).await {
        Some(g) if g.revoked_at.is_none() => g,
        _ => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "grant_not_found_or_not_owned" })),
            )
                .into_response();
        }
    };
    // No broadening: every requested calendar must already be allowed.
    if !payload
        .allowed_calendar_ids
        .iter()
        .all(|id| current.allowed_calendar_ids.contains(id))
    {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "cannot_broaden_grant" })),
        )
            .into_response();
    }
    {
        let mut st = state.inner.write().await;
        if let Some(grant) = st.grants.get_mut(&grant_id) {
            grant.allowed_calendar_ids = payload.allowed_calendar_ids;
        }
    }
    (StatusCode::OK, Json(serde_json::json!({ "updated": true }))).into_response()
}

// -- Logout -----------------------------------------------------------------

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(token) = extract_session_cookie(&headers) {
        state.destroy_session(&token).await;
    }
    (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn extract_session_cookie(headers: &HeaderMap) -> Option<String> {
    let cookie_header = headers.get(header::COOKIE)?.to_str().ok()?;
    for pair in cookie_header.split(';') {
        let (key, value) = pair.trim().split_once('=')?;
        if key == "commoncal_session" {
            return Some(value.to_string());
        }
    }
    None
}

/// Parse a `application/x-www-form-urlencoded` body into a key/value map.
fn parse_form(body: &axum::body::Bytes) -> HashMap<String, String> {
    url::form_urlencoded::parse(body.as_ref())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

/// Validate a login `continue` target against open redirect.
///
/// Only a same-origin relative path is accepted: it must start with a single
/// `/` (not `//`, which would be protocol-relative), must not contain a scheme
/// (`://`), and must not be an absolute URL. Anything else is rejected so the
/// caller falls back to a safe default. This is the lab's open-redirect guard
/// for both password and magic-link continuation.
fn validate_continuation(value: &str) -> Option<String> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    // Must be a relative path.
    if !v.starts_with('/') {
        return None;
    }
    // Reject protocol-relative (`//host/...`) and any scheme (`://`).
    if v.starts_with("//") || v.contains("://") {
        return None;
    }
    // Reject backslash tricks and control characters.
    if v.contains('\\') || v.chars().any(|c| (c as u32) < 0x20) {
        return None;
    }
    Some(v.to_string())
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let auth_private_base = std::env::var("COMMONCAL_AUTH_PRIVATE")
        .unwrap_or_else(|_| "http://127.0.0.1:4001".to_string());
    let bridge_key = std::env::var("COMMONCAL_BRIDGE_KEY")
        .unwrap_or_else(|_| "slice1-loopback-bridge-key".to_string());
    let bind: SocketAddr = std::env::var("COMMONCAL_BIND")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or("127.0.0.1:4002".parse().unwrap());

    let state = AppState::new(auth_private_base, bridge_key);

    let app = Router::new()
        .route("/health", get(health))
        .route("/login", get(login_page).post(login_form))
        .route("/login/magic-link", post(magic_link_request))
        .route("/login/magic-link/verify", get(magic_link_verify))
        .route("/logout", post(logout))
        .route("/consent", get(consent_page))
        .route("/consent/decision", post(consent_decision))
        .route("/grants", get(mgmt_list_grants))
        .route(
            "/grants/:id",
            delete(mgmt_revoke_grant).patch(mgmt_update_grant),
        )
        .route("/internal/calendars/:user_id", get(internal_list_calendars))
        .route("/internal/grant", get(internal_get_grant))
        .route("/internal/grant/revoke", post(internal_revoke_grant))
        .route("/internal/test/add-user", post(test_add_user))
        .route("/internal/test/add-calendar", post(test_add_calendar))
        .route("/internal/test/remove-calendar", post(test_remove_calendar))
        .route(
            "/internal/test/fail-next-decide",
            post(test_fail_next_decide),
        )
        .route("/internal/test/grants", get(test_list_grants))
        .with_state(state);

    info!(%bind, "commoncal lab service listening");
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
