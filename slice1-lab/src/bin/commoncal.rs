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
    routing::{delete, get, patch, post},
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

#[derive(Debug, Clone, Serialize)]
struct Event {
    id: i64,
    calendar_id: i64,
    title: Option<String>,
    description: Option<String>,
    location: Option<String>,
    status: String,
    event_kind: String,
    start_utc: Option<i64>,
    end_utc: Option<i64>,
    version: i64,
}

#[derive(Debug, Clone, Serialize)]
struct Reminder {
    id: i64,
    event_id: i64,
    offset_minutes: i64,
    created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
struct DeleteIntent {
    intent_id: String,
    user_id: i64,
    oauth_client_id: String,
    event_id: i64,
    calendar_id: i64,
    event_version: i64,
    expires_at: i64,
    confirmation_state: String, // "pending" or "committed"
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
    events: HashMap<i64, Event>,
    reminders: HashMap<i64, Reminder>,
    idempotency_keys: HashMap<String, i64>,
    next_calendar_id: i64,
    next_user_id: i64,
    next_event_id: i64,
    next_reminder_id: i64,
    delete_intents: HashMap<String, DeleteIntent>,
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
            events: HashMap::new(),
            reminders: HashMap::new(),
            idempotency_keys: HashMap::new(),
            next_calendar_id: 1,
            next_user_id: 1,
            next_event_id: 1,
            next_reminder_id: 1,
            delete_intents: HashMap::new(),
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

        // Seed test events for the lab user (calendar 1 = Work, calendar 2 = Personal).
        // Times are relative to "now" so availability/search ranges always cover them.
        let now = Self::now();
        let ev1 = Event {
            id: 1,
            calendar_id: 1,
            title: Some("Team Standup".to_string()),
            description: Some("Daily sync with the platform team".to_string()),
            location: Some("Room 5".to_string()),
            status: "confirmed".to_string(),
            event_kind: "default".to_string(),
            start_utc: Some(now + 3600),
            end_utc: Some(now + 4200),
            version: 1,
        };
        let ev2 = Event {
            id: 2,
            calendar_id: 1,
            title: Some("Design Review".to_string()),
            description: Some("Review Q3 mockups".to_string()),
            location: None,
            status: "confirmed".to_string(),
            event_kind: "default".to_string(),
            start_utc: Some(now + 7200),
            end_utc: Some(now + 8100),
            version: 1,
        };
        let ev3 = Event {
            id: 3,
            calendar_id: 2,
            title: Some("Gym".to_string()),
            description: None,
            location: Some("FitLab".to_string()),
            status: "confirmed".to_string(),
            event_kind: "default".to_string(),
            start_utc: Some(now + 10800),
            end_utc: Some(now + 11400),
            version: 1,
        };
        inner.events.insert(1, ev1);
        inner.events.insert(2, ev2);
        inner.events.insert(3, ev3);
        inner.next_event_id = 4;

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

    // -- Slice 6: event + availability helpers ------------------------------

    /// Parse a UTC timestamp (ISO 8601 or Unix epoch) to i64 seconds.
    fn parse_ts(s: &str) -> Option<i64> {
        if let Ok(ts) = s.parse::<i64>() {
            return Some(ts);
        }
        chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.timestamp())
    }

    /// Compute availability slots for a calendar in [from, to].
    /// A slot is "busy" if an event overlaps it, otherwise "free".
    /// Slots are 1-hour blocks aligned to the hour.
    async fn compute_availability(
        &self,
        calendar_id: i64,
        from: &str,
        to: &str,
    ) -> Vec<serde_json::Value> {
        let from_ts = Self::parse_ts(from).unwrap_or(0);
        let to_ts = Self::parse_ts(to).unwrap_or(0);
        let events: Vec<Event> = {
            let st = self.inner.read().await;
            st.events
                .values()
                .filter(|e| e.calendar_id == calendar_id)
                .cloned()
                .collect()
        };

        let mut slots = Vec::new();
        let mut t = from_ts;
        while t < to_ts {
            let slot_end = (t + 3600).min(to_ts);
            let busy = events.iter().any(|e| {
                e.start_utc.is_some_and(|s| s < slot_end)
                    && e.end_utc.is_some_and(|en| en > t)
            });
            slots.push(serde_json::json!({
                "start": t,
                "end": slot_end,
                "status": if busy { "busy" } else { "free" },
            }));
            t = slot_end;
        }
        slots
    }

    /// Get a single event by (calendar_id, event_id).
    async fn get_event(&self, calendar_id: i64, event_id: i64) -> Option<Event> {
        let st = self.inner.read().await;
        st.events
            .get(&event_id)
            .filter(|e| e.calendar_id == calendar_id)
            .cloned()
    }

    /// Search events in a calendar within [from, to], optionally filtered by query.
    async fn search_events(
        &self,
        calendar_id: i64,
        from: &str,
        to: &str,
        query: Option<&str>,
    ) -> Vec<Event> {
        let from_ts = Self::parse_ts(from).unwrap_or(0);
        let to_ts = Self::parse_ts(to).unwrap_or(0);
        let st = self.inner.read().await;
        let mut results: Vec<Event> = st
            .events
            .values()
            .filter(|e| e.calendar_id == calendar_id)
            .filter(|e| {
                e.start_utc.is_some_and(|s| s >= from_ts && s <= to_ts)
                    || e.end_utc.is_some_and(|en| en >= from_ts && en <= to_ts)
            })
            .filter(|e| {
                query.is_none_or(|q| {
                    let q_lower = q.to_lowercase();
                    e.title
                        .as_deref()
                        .is_some_and(|t| t.to_lowercase().contains(&q_lower))
                        || e.description
                            .as_deref()
                            .is_some_and(|d| d.to_lowercase().contains(&q_lower))
                })
            })
            .cloned()
            .collect();
        drop(st);
        results.sort_by_key(|e| e.start_utc.unwrap_or(0));
        results
    }

    /// Test hook: add an event. Returns the new event id.
    async fn test_add_event(
        &self,
        calendar_id: i64,
        title: Option<String>,
        description: Option<String>,
        location: Option<String>,
        status: String,
        event_kind: String,
        start_utc: Option<i64>,
        end_utc: Option<i64>,
    ) -> i64 {
        let mut st = self.inner.write().await;
        let id = st.next_event_id;
        st.next_event_id += 1;
        st.events.insert(
            id,
            Event {
                id,
                calendar_id,
                title,
                description,
                location,
                status,
                event_kind,
                start_utc,
                end_utc,
                version: 1,
            },
        );
        id
    }

    // -- Slice 7: mutation methods ------------------------------------------

    /// Create an event. Returns (event, was_idempotent_replay).
    async fn create_event(
        &self,
        calendar_id: i64,
        title: Option<String>,
        description: Option<String>,
        location: Option<String>,
        start_utc: Option<i64>,
        end_utc: Option<i64>,
        idempotency_key: Option<&str>,
    ) -> Result<(Event, bool), String> {
        let mut st = self.inner.write().await;
        if let Some(key) = idempotency_key {
            if let Some(&existing_id) = st.idempotency_keys.get(key) {
                if let Some(ev) = st.events.get(&existing_id) {
                    return Ok((ev.clone(), true));
                }
            }
        }
        let id = st.next_event_id;
        st.next_event_id += 1;
        let event = Event {
            id,
            calendar_id,
            title,
            description,
            location,
            status: "confirmed".to_string(),
            event_kind: "default".to_string(),
            start_utc,
            end_utc,
            version: 1,
        };
        st.events.insert(id, event.clone());
        if let Some(key) = idempotency_key {
            st.idempotency_keys.insert(key.to_string(), id);
        }
        Ok((event, false))
    }

    /// Update an event with optimistic concurrency. Returns the updated event.
    async fn update_event(
        &self,
        calendar_id: i64,
        event_id: i64,
        expected_version: i64,
        title: Option<String>,
        description: Option<String>,
        location: Option<String>,
        start_utc: Option<i64>,
        end_utc: Option<i64>,
    ) -> Result<Event, String> {
        let mut st = self.inner.write().await;
        let ev = st
            .events
            .get_mut(&event_id)
            .filter(|e| e.calendar_id == calendar_id)
            .ok_or_else(|| "event_not_found".to_string())?;
        if ev.version != expected_version {
            return Err("version_conflict".to_string());
        }
        if let Some(t) = title {
            ev.title = Some(t);
        }
        if let Some(d) = description {
            ev.description = Some(d);
        }
        if let Some(l) = location {
            ev.location = Some(l);
        }
        if let Some(s) = start_utc {
            ev.start_utc = Some(s);
        }
        if let Some(e) = end_utc {
            ev.end_utc = Some(e);
        }
        ev.version += 1;
        Ok(ev.clone())
    }

    /// Create a delete intent for two-phase deletion. Returns the intent.
    async fn create_delete_intent(
        &self,
        user_id: i64,
        oauth_client_id: String,
        event_id: i64,
        calendar_id: i64,
        event_version: i64,
        expires_at: i64,
    ) -> DeleteIntent {
        let mut st = self.inner.write().await;
        let intent_id = format!("del_{}", uuid::Uuid::new_v4().simple());
        let intent = DeleteIntent {
            intent_id: intent_id.clone(),
            user_id,
            oauth_client_id,
            event_id,
            calendar_id,
            event_version,
            expires_at,
            confirmation_state: "pending".to_string(),
        };
        st.delete_intents.insert(intent_id, intent.clone());
        intent
    }

    /// Get a delete intent by ID. Returns None if not found or expired.
    /// Returns intents in any state (pending or committed) so callers can
    /// inspect the intent data (user_id, oauth_client_id, event_version).
    async fn get_delete_intent(&self, intent_id: &str) -> Option<DeleteIntent> {
        let st = self.inner.read().await;
        st.delete_intents.get(intent_id).cloned()
    }

    /// Commit a delete intent, changing its state to "committed" and
    /// deleting the associated event from the events store.
    async fn commit_delete_intent(&self, intent_id: &str) -> Result<(), String> {
        let mut st = self.inner.write().await;
        let intent = st
            .delete_intents
            .get_mut(intent_id)
            .ok_or("delete_intent_not_found".to_string())?;
        if intent.confirmation_state != "pending" {
            return Err("delete_intent_already_committed".to_string());
        }
        intent.confirmation_state = "committed".to_string();
        // Actually delete the event.
        st.events.remove(&intent.event_id);
        Ok(())
    }

    /// Set a reminder on an event. Returns the reminder.
    async fn set_reminder(
        &self,
        calendar_id: i64,
        event_id: i64,
        offset_minutes: i64,
    ) -> Result<Reminder, String> {
        let mut st = self.inner.write().await;
        let ev = st
            .events
            .get(&event_id)
            .filter(|e| e.calendar_id == calendar_id)
            .ok_or_else(|| "event_not_found".to_string())?;
        let _ = ev;
        let id = st.next_reminder_id;
        st.next_reminder_id += 1;
        let reminder = Reminder {
            id,
            event_id,
            offset_minutes,
            created_at: Self::now(),
        };
        st.reminders.insert(id, reminder.clone());
        Ok(reminder)
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

// -- Slice 6 internal API: availability, event_get, event_search ------------

/// GET /internal/availability?calendar_id=&from=&to=
#[derive(Deserialize)]
struct AvailabilityQuery {
    calendar_id: i64,
    from: String,
    to: String,
}

async fn internal_availability(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<AvailabilityQuery>,
) -> Response {
    if !check_internal_key(&headers, &state.bridge_key) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let slots = state.compute_availability(q.calendar_id, &q.from, &q.to).await;
    (StatusCode::OK, Json(serde_json::json!({ "slots": slots }))).into_response()
}

/// GET /internal/event?calendar_id=&event_id=
#[derive(Deserialize)]
struct EventGetQuery {
    calendar_id: i64,
    event_id: i64,
}

async fn internal_event_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<EventGetQuery>,
) -> Response {
    if !check_internal_key(&headers, &state.bridge_key) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    match state.get_event(q.calendar_id, q.event_id).await {
        Some(ev) => (StatusCode::OK, Json(serde_json::json!({ "event": ev }))).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "event_not_found"})),
        )
            .into_response(),
    }
}

/// GET /internal/events?calendar_id=&from=&to=&query=
#[derive(Deserialize)]
struct EventSearchQuery {
    calendar_id: i64,
    from: String,
    to: String,
    query: Option<String>,
}

async fn internal_event_search(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<EventSearchQuery>,
) -> Response {
    if !check_internal_key(&headers, &state.bridge_key) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let events = state.search_events(q.calendar_id, &q.from, &q.to, q.query.as_deref()).await;
    (StatusCode::OK, Json(serde_json::json!({ "events": events }))).into_response()
}

// -- Slice 7 internal API: event_create, event_update, reminder_set ---------

/// POST /internal/event  body: {calendar_id, title?, description?, location?, start_utc?, end_utc?, idempotency_key?}
#[derive(Deserialize)]
struct EventCreateRequest {
    calendar_id: i64,
    title: Option<String>,
    description: Option<String>,
    location: Option<String>,
    start_utc: Option<i64>,
    end_utc: Option<i64>,
    idempotency_key: Option<String>,
}

async fn internal_event_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<EventCreateRequest>,
) -> Response {
    if !check_internal_key(&headers, &state.bridge_key) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    match state
        .create_event(
            req.calendar_id,
            req.title,
            req.description,
            req.location,
            req.start_utc,
            req.end_utc,
            req.idempotency_key.as_deref(),
        )
        .await
    {
        Ok((event, replay)) => (
            StatusCode::OK,
            Json(serde_json::json!({ "event": event, "replayed": replay })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

/// PATCH /internal/event/:id  body: {calendar_id, expected_version, title?, description?, location?, start_utc?, end_utc?}
#[derive(Deserialize)]
struct EventUpdateRequest {
    calendar_id: i64,
    expected_version: i64,
    title: Option<String>,
    description: Option<String>,
    location: Option<String>,
    start_utc: Option<i64>,
    end_utc: Option<i64>,
}

async fn internal_event_update(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(event_id): axum::extract::Path<i64>,
    Json(req): Json<EventUpdateRequest>,
) -> Response {
    if !check_internal_key(&headers, &state.bridge_key) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    match state
        .update_event(
            req.calendar_id,
            event_id,
            req.expected_version,
            req.title,
            req.description,
            req.location,
            req.start_utc,
            req.end_utc,
        )
        .await
    {
        Ok(event) => (
            StatusCode::OK,
            Json(serde_json::json!({ "event": event })),
        )
            .into_response(),
        Err(e) => {
            let status = if e == "version_conflict" {
                StatusCode::CONFLICT
            } else if e == "event_not_found" {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (status, Json(serde_json::json!({ "error": e }))).into_response()
        }
    }
}

/// POST /internal/reminder  body: {calendar_id, event_id, offset_minutes}
#[derive(Deserialize)]
struct ReminderSetRequest {
    calendar_id: i64,
    event_id: i64,
    offset_minutes: i64,
}

async fn internal_reminder_set(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ReminderSetRequest>,
) -> Response {
    if !check_internal_key(&headers, &state.bridge_key) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    match state
        .set_reminder(req.calendar_id, req.event_id, req.offset_minutes)
        .await
    {
        Ok(reminder) => (
            StatusCode::OK,
            Json(serde_json::json!({ "reminder": reminder })),
        )
            .into_response(),
        Err(e) => {
            let status = if e == "event_not_found" {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (status, Json(serde_json::json!({ "error": e }))).into_response()
        }
    }
}

// -- Slice 8 internal API: delete intent ------------------------------------

/// POST /internal/delete-intent  body: {user_id, oauth_client_id, event_id, calendar_id, event_version, expires_at}
#[derive(Deserialize)]
struct DeleteIntentCreateRequest {
    user_id: i64,
    oauth_client_id: String,
    event_id: i64,
    calendar_id: i64,
    event_version: i64,
    expires_at: i64,
}

async fn internal_delete_intent_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<DeleteIntentCreateRequest>,
) -> Response {
    if !check_internal_key(&headers, &state.bridge_key) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let intent = state
        .create_delete_intent(
            req.user_id,
            req.oauth_client_id,
            req.event_id,
            req.calendar_id,
            req.event_version,
            req.expires_at,
        )
        .await;
    (StatusCode::OK, Json(serde_json::json!({ "delete_intent": intent }))).into_response()
}

/// GET /internal/delete-intent/:intent_id
async fn internal_delete_intent_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(intent_id): axum::extract::Path<String>,
) -> Response {
    if !check_internal_key(&headers, &state.bridge_key) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    match state.get_delete_intent(&intent_id).await {
        Some(intent) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;
            if intent.expires_at <= now {
                (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({"error": "delete_intent_expired"})),
                )
                    .into_response()
            } else {
                (StatusCode::OK, Json(serde_json::json!({ "delete_intent": intent }))).into_response()
            }
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "delete_intent_not_found"})),
        )
            .into_response(),
    }
}

/// POST /internal/delete-intent/:intent_id/commit
async fn internal_delete_intent_commit(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(intent_id): axum::extract::Path<String>,
) -> Response {
    if !check_internal_key(&headers, &state.bridge_key) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    match state.commit_delete_intent(&intent_id).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({ "committed": true })),
        )
            .into_response(),
        Err(e) => {
            let status = if e == "delete_intent_not_found" {
                StatusCode::NOT_FOUND
            } else if e == "delete_intent_already_committed" {
                StatusCode::CONFLICT
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (status, Json(serde_json::json!({ "error": e }))).into_response()
        }
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

/// POST /internal/test/add-event  body: {calendar_id, title, description, location, status, event_kind, start_utc, end_utc}
async fn test_add_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<serde_json::Value>,
) -> Response {
    if !check_internal_key(&headers, &state.bridge_key) {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let calendar_id = req.get("calendar_id").and_then(|v| v.as_i64()).unwrap_or(0);
    let title = req
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let description = req
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let location = req
        .get("location")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let status = req
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("confirmed")
        .to_string();
    let event_kind = req
        .get("event_kind")
        .and_then(|v| v.as_str())
        .unwrap_or("default")
        .to_string();
    let start_utc = req.get("start_utc").and_then(|v| v.as_i64());
    let end_utc = req.get("end_utc").and_then(|v| v.as_i64());
    let id = state
        .test_add_event(calendar_id, title, description, location, status, event_kind, start_utc, end_utc)
        .await;
    (StatusCode::OK, Json(serde_json::json!({ "event_id": id }))).into_response()
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

/// GET /confirm-delete/:intent_id — renders the confirmation page.
/// Requires an authenticated session. Shows delete intent details and a
/// confirmation button that POSTs to /confirm-delete/:intent_id/confirm.
async fn confirm_delete_page(
    State(state): State<AppState>,
    axum::extract::Path(intent_id): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    let session_token = match extract_session_cookie(&headers) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                "login required".to_string(),
            ).into_response()
        }
    };
    let session = match state.get_session(&session_token).await {
        Some(user_id) => user_id,
        None => {
            return (StatusCode::FORBIDDEN, "invalid session").into_response()
        }
    };
    let intent = match state.get_delete_intent(&intent_id).await {
        Some(i) => i,
        None => {
            return (
                StatusCode::NOT_FOUND,
                "delete intent not found".to_string(),
            ).into_response()
        }
    };
    // Verify the intent belongs to the current user
    if intent.user_id != session {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    let html = format!(
        "<!DOCTYPE html><html><head><title>Confirm Deletion</title></head><body>
<h2>Confirm Deletion</h2>
<p>Are you sure you want to delete this event?</p>
<p><strong>Event ID:</strong> {}</p>
<p><strong>Calendar ID:</strong> {}</p>
<p><strong>Expires at:</strong> {}</p>
<form method='post' action='/confirm-delete/{}/confirm'>
  <button type='submit'>Confirm Delete</button>
</form>
</body></html>",
        intent.event_id, intent.calendar_id, intent.expires_at, intent_id
    );
    (StatusCode::OK, html).into_response()
}

/// POST /confirm-delete/:intent_id/confirm — commits the deletion.
async fn confirm_delete_commit(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(intent_id): axum::extract::Path<String>,
) -> Response {
    let session_token = match extract_session_cookie(&headers) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                "login required".to_string(),
            ).into_response()
        }
    };
    let session = match state.get_session(&session_token).await {
        Some(user_id) => user_id,
        None => {
            return (StatusCode::FORBIDDEN, "invalid session").into_response()
        }
    };
    let intent = match state.get_delete_intent(&intent_id).await {
        Some(i) => i,
        None => {
            return (
                StatusCode::NOT_FOUND,
                "delete intent not found".to_string(),
            ).into_response()
        }
    };
    if intent.user_id != session {
        return (StatusCode::FORBIDDEN, "intent does not belong to you").into_response();
    }
    if let Err(e) = state.commit_delete_intent(&intent_id).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("deletion failed: {}", e),
        )
            .into_response();
    }
    (
        StatusCode::OK,
        "Deletion confirmed".to_string(),
    )
        .into_response()
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
        .route("/confirm-delete/:intent_id", get(confirm_delete_page))
        .route(
            "/confirm-delete/:intent_id/confirm",
            post(confirm_delete_commit),
        )
        .route("/internal/calendars/:user_id", get(internal_list_calendars))
        .route("/internal/grant", get(internal_get_grant))
        .route("/internal/grant/revoke", post(internal_revoke_grant))
        .route("/internal/availability", get(internal_availability))
        .route("/internal/event", get(internal_event_get).post(internal_event_create))
        .route("/internal/events", get(internal_event_search))
        .route("/internal/event/:id", patch(internal_event_update))
        .route("/internal/reminder", post(internal_reminder_set))
        .route("/internal/delete-intent", post(internal_delete_intent_create))
        .route(
            "/internal/delete-intent/:intent_id",
            get(internal_delete_intent_get),
        )
        .route(
            "/internal/delete-intent/:intent_id/commit",
            post(internal_delete_intent_commit),
        )
        .route("/internal/test/add-user", post(test_add_user))
        .route("/internal/test/add-calendar", post(test_add_calendar))
        .route("/internal/test/remove-calendar", post(test_remove_calendar))
        .route("/internal/test/add-event", post(test_add_event))
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
