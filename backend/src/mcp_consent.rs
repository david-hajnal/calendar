// Session-owned OAuth consent. Grants precede bridge approval and retries retain
// the same grant, including any subsequent narrowing or revocation.
use crate::mcp_bridge::{InteractionView, McpBridgeClient};
use crate::sessions::{AuthenticatedSession, SessionManager};
use axum::extract::{Extension, Query, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::{Sqlite, SqlitePool, Transaction};
use std::time::SystemTime;

#[derive(Clone)]
pub struct ConsentState {
    pub pool: SqlitePool,
    pub session_manager: SessionManager,
    pub bridge: McpBridgeClient,
}
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
fn scopes_to_allow_flags(scopes: &[String]) -> (bool, bool, bool, bool, bool, bool) {
    let has = |s: &str| scopes.iter().any(|x| x == s);
    (
        has("commoncal.availability.read") || has("commoncal.calendar.metadata.read"),
        has("commoncal.event.read.basic"),
        has("commoncal.event.read.details"),
        has("commoncal.event.create") || has("commoncal.reminder.write"),
        has("commoncal.event.update"),
        has("commoncal.event.delete"),
    )
}
type ExistingGrant = (String, String, bool, bool, bool, bool, bool, bool);

// Read membership and existing permissions inside the same transaction as the
// write. Reauthorization may narrow a live grant, but cannot silently widen it.
async fn upsert_in_transaction(
    tx: &mut Transaction<'_, Sqlite>,
    user_id: i64,
    client: &str,
    selected: Vec<i64>,
    scopes: &[String],
) -> Result<String, sqlx::Error> {
    let now = now_secs();
    let live: Vec<i64> = sqlx::query_scalar("SELECT DISTINCT c.id FROM calendars c JOIN calendar_acl ca ON c.id = ca.calendar_id WHERE ca.user_id = ? AND c.archived = 0 ORDER BY c.id").bind(user_id).fetch_all(&mut **tx).await?;
    let mut calendars: Vec<i64> = live
        .into_iter()
        .filter(|id| selected.contains(id))
        .collect();
    let flags = scopes_to_allow_flags(scopes);
    let mut flags = [flags.0, flags.1, flags.2, flags.3, flags.4, flags.5];
    let existing: Option<ExistingGrant> = sqlx::query_as("SELECT id, allowed_calendar_ids, allow_availability, allow_event_titles, allow_event_details, allow_create, allow_update, allow_delete FROM mcp_grant WHERE user_id = ? AND oauth_client_id = ? AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > ?) ORDER BY created_at DESC, id LIMIT 1")
        .bind(user_id).bind(client).bind(now).fetch_optional(&mut **tx).await?;
    let id = if let Some((id, stored, a, b, c, d, e, f)) = existing {
        let previous: Vec<i64> =
            serde_json::from_str(&stored).map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
        calendars.retain(|id| previous.contains(id));
        for (flag, previous) in flags.iter_mut().zip([a, b, c, d, e, f]) {
            *flag &= previous;
        }
        // Retire legacy duplicates while retaining the authoritative identity.
        sqlx::query("UPDATE mcp_grant SET revoked_at = ? WHERE user_id = ? AND oauth_client_id = ? AND revoked_at IS NULL AND id <> ?").bind(now).bind(user_id).bind(client).bind(&id).execute(&mut **tx).await?;
        id
    } else {
        sqlx::query("UPDATE mcp_grant SET revoked_at = ? WHERE user_id = ? AND oauth_client_id = ? AND revoked_at IS NULL").bind(now).bind(user_id).bind(client).execute(&mut **tx).await?;
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO mcp_grant (id,user_id,oauth_client_id,created_at) VALUES (?,?,?,?)",
        )
        .bind(&id)
        .bind(user_id)
        .bind(client)
        .bind(now)
        .execute(&mut **tx)
        .await?;
        id
    };
    sqlx::query("UPDATE mcp_grant SET allowed_calendar_ids = ?, allow_availability = ?, allow_event_titles = ?, allow_event_details = ?, allow_create = ?, allow_update = ?, allow_delete = ? WHERE id = ?")
        .bind(serde_json::to_string(&calendars).unwrap()).bind(flags[0]).bind(flags[1]).bind(flags[2]).bind(flags[3]).bind(flags[4]).bind(flags[5]).bind(&id).execute(&mut **tx).await?;
    Ok(id)
}
#[cfg(test)]
async fn upsert_grant(
    pool: &SqlitePool,
    user_id: i64,
    client: &str,
    calendars: Vec<i64>,
    scopes: &[String],
) -> Result<String, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let id = upsert_in_transaction(&mut tx, user_id, client, calendars, scopes).await?;
    tx.commit().await?;
    Ok(id)
}
fn error(status: StatusCode, code: &str) -> Response {
    (status, Json(serde_json::json!({"error":code}))).into_response()
}
fn handoff_hash(handoff: &str) -> String {
    format!("{:x}", Sha256::digest(handoff.as_bytes()))
}
fn valid_view(view: &InteractionView, user_id: i64, prompt: &str) -> bool {
    view.prompt == prompt
        && view.expires_at > now_secs()
        && !view.client_id.is_empty()
        && (prompt == "login"
            || view.subject.as_deref().and_then(|v| v.parse::<i64>().ok()) == Some(user_id))
}
fn approved_scopes(view: &InteractionView) -> Vec<String> {
    view.granted_scopes
        .iter()
        .filter(|scope| view.requested_scopes.contains(scope))
        .cloned()
        .collect()
}
#[derive(Deserialize)]
struct ConsentQuery {
    handoff: Option<String>,
}
async fn consent_page(
    State(state): State<ConsentState>,
    Query(q): Query<ConsentQuery>,
    Extension(session): Extension<AuthenticatedSession>,
) -> Response {
    let Some(handoff) = q.handoff.filter(|h| !h.is_empty()) else {
        return error(StatusCode::BAD_REQUEST, "missing_handoff");
    };
    let view = match state.bridge.lookup_interaction(&handoff).await {
        Ok(v) => v,
        Err(_) => return error(StatusCode::BAD_GATEWAY, "interaction_unavailable"),
    };
    if view.prompt == "login" && valid_view(&view, session.user.id, "login") {
        return match state
            .bridge
            .decide_interaction(&handoff, "login", Some(session.user.id))
            .await
        {
            Ok(url) => (StatusCode::SEE_OTHER, [(header::LOCATION, url)]).into_response(),
            Err(_) => error(StatusCode::BAD_GATEWAY, "interaction_unavailable"),
        };
    }
    if !valid_view(&view, session.user.id, "consent") {
        return error(StatusCode::FORBIDDEN, "invalid_interaction");
    }
    let calendars: Vec<(i64,String)> = match sqlx::query_as("SELECT DISTINCT c.id,c.name FROM calendars c JOIN calendar_acl ca ON c.id=ca.calendar_id WHERE ca.user_id=? AND c.archived=0 ORDER BY c.id").bind(session.user.id).fetch_all(&state.pool).await {
        Ok(c) => c, Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR,"internal_error")
    };
    let scopes_html = view
        .requested_scopes
        .iter()
        .map(|s| format!("<li>{}</li>", escape_html(s)))
        .collect::<Vec<_>>()
        .join("");
    let calendars_html = calendars.iter().map(|(id,name)|format!("<label><input type=\"checkbox\" name=\"calendar_ids\" value=\"{id}\">{}</label><br>",escape_html(name))).collect::<Vec<_>>().join("");
    let html = format!(
        r#"<!DOCTYPE html><html lang="en"><head><meta charset="utf-8"><title>CommonCal — Authorize</title></head><body>
<h1>Authorize access</h1><p><strong>{}</strong> is requesting access to your CommonCal account.</p>
<p>Client ID: {}</p><p>Requested permissions:</p><ul>{}</ul><p>Select calendars to share:</p>
<form id="consent" data-handoff="{}" data-csrf="{}">{}
<button type="submit" value="approve">Approve</button><button type="submit" value="deny">Deny</button></form><p id="error" role="alert"></p>
<script>
document.getElementById('consent').addEventListener('submit', async (event) => {{
 event.preventDefault(); const form = event.currentTarget; const decision = event.submitter.value;
 const calendar_ids = Array.from(form.querySelectorAll('input:checked'), input => Number(input.value));
 const buttons = form.querySelectorAll('button'); buttons.forEach(button => button.disabled = true);
 try {{
  const response = await fetch('/consent/decision', {{method:'POST',credentials:'same-origin',headers:{{'content-type':'application/json','x-csrf-token':form.dataset.csrf}},body:JSON.stringify({{handoff:form.dataset.handoff,decision,calendar_ids}})}});
  const result = await response.json(); if (!response.ok) throw new Error('Authorization failed. Please restart authorization.');
  window.location.assign(result.resume_url);
 }} catch (error) {{ document.getElementById('error').textContent = error.message; buttons.forEach(button => button.disabled = false); }}
}});
</script></body></html>"#,
        escape_html(&view.client_name),
        escape_html(&view.client_id),
        scopes_html,
        escape_html(&handoff),
        escape_html(&session.csrf_token),
        calendars_html
    );
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        html,
    )
        .into_response()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionForm {
    handoff: String,
    decision: String,
    #[serde(default)]
    calendar_ids: Vec<i64>,
}
#[derive(sqlx::FromRow)]
struct Receipt {
    user_id: i64,
    decision: String,
    grant_id: Option<String>,
    resume_url: Option<String>,
    expires_at: i64,
}
async fn consent_decision(
    State(state): State<ConsentState>,
    Extension(session): Extension<AuthenticatedSession>,
    Json(form): Json<DecisionForm>,
) -> Response {
    if !matches!(form.decision.as_str(), "approve" | "deny") || form.handoff.is_empty() {
        return error(StatusCode::BAD_REQUEST, "invalid_decision");
    }
    let hash = handoff_hash(&form.handoff);
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
    };
    // Acquire the SQLite write lock before reading a receipt, serializing races
    // between duplicate requests without a process-local mutex.
    if sqlx::query("UPDATE mcp_consent_receipt SET user_id=user_id WHERE handoff_hash=?")
        .bind(&hash)
        .execute(&mut *tx)
        .await
        .is_err()
    {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error");
    }
    let receipt: Option<Receipt> = match sqlx::query_as("SELECT user_id,decision,grant_id,resume_url,expires_at FROM mcp_consent_receipt WHERE handoff_hash=?").bind(&hash).fetch_optional(&mut *tx).await {
        Ok(r) => r, Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR,"internal_error")
    };
    if let Some(receipt) = &receipt {
        if receipt.user_id != session.user.id
            || receipt.decision != form.decision
            || receipt.expires_at <= now_secs()
        {
            return error(StatusCode::FORBIDDEN, "invalid_interaction");
        }
        if let Some(id) = &receipt.grant_id {
            let active: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mcp_grant WHERE id=? AND user_id=? AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > ?))").bind(id).bind(session.user.id).bind(now_secs()).fetch_one(&mut *tx).await.unwrap_or(false);
            if !active {
                return error(StatusCode::FORBIDDEN, "grant_inactive");
            }
        }
        if let Some(url) = &receipt.resume_url {
            return Json(serde_json::json!({"resume_url":url})).into_response();
        }
    }
    let view = match state.bridge.lookup_interaction(&form.handoff).await {
        Ok(v) => v,
        Err(_) => return error(StatusCode::BAD_GATEWAY, "interaction_unavailable"),
    };
    if !valid_view(&view, session.user.id, "consent") {
        return error(StatusCode::FORBIDDEN, "invalid_interaction");
    }
    if receipt.is_none() {
        let grant = if form.decision == "approve" {
            let scopes = approved_scopes(&view);
            if scopes.is_empty() || form.calendar_ids.is_empty() {
                return error(StatusCode::BAD_REQUEST, "empty_selection");
            }
            match upsert_in_transaction(
                &mut tx,
                session.user.id,
                &view.client_id,
                form.calendar_ids,
                &scopes,
            )
            .await
            {
                Ok(id) => Some(id),
                Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
            }
        } else {
            None
        };
        if sqlx::query("INSERT INTO mcp_consent_receipt (handoff_hash,user_id,decision,grant_id,expires_at) VALUES (?,?,?,?,?)").bind(&hash).bind(session.user.id).bind(&form.decision).bind(grant).bind(view.expires_at).execute(&mut *tx).await.is_err() { return error(StatusCode::INTERNAL_SERVER_ERROR,"internal_error"); }
    }
    if tx.commit().await.is_err() {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error");
    }
    let kind = if form.decision == "approve" {
        "consent"
    } else {
        "deny"
    };
    match state
        .bridge
        .decide_interaction(&form.handoff, kind, Some(session.user.id))
        .await
    {
        Ok(url) => {
            if sqlx::query("UPDATE mcp_consent_receipt SET resume_url=? WHERE handoff_hash=?")
                .bind(&url)
                .bind(hash)
                .execute(&state.pool)
                .await
                .is_err()
            {
                return error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error");
            }
            (
                [(header::CACHE_CONTROL, "no-store")],
                Json(serde_json::json!({"resume_url":url})),
            )
                .into_response()
        }
        Err(_) => error(StatusCode::BAD_GATEWAY, "interaction_unavailable"),
    }
}
pub(crate) fn validate_continuation(value: &str) -> Option<String> {
    let v = value.trim();
    if !v.starts_with('/')
        || v.starts_with("//")
        || v.contains("://")
        || v.contains('\\')
        || v.chars().any(|c| c.is_control())
    {
        return None;
    }
    Some(v.to_string())
}
#[derive(Deserialize)]
struct LoginContinueQuery {
    #[serde(alias = "continue")]
    continue_url: Option<String>,
}
async fn login_continue(
    Query(q): Query<LoginContinueQuery>,
    Extension(_session): Extension<AuthenticatedSession>,
) -> Response {
    let url = q
        .continue_url
        .as_deref()
        .and_then(validate_continuation)
        .unwrap_or_else(|| "/".into());
    (StatusCode::SEE_OTHER, [(header::LOCATION, url)]).into_response()
}
// GET navigation can reach the existing login UI. Unsafe requests still use
// the same session/Origin/fetch-site/CSRF checks as all authenticated APIs.
async fn consent_session(
    State(manager): State<SessionManager>,
    mut request: Request,
    next: Next,
) -> Response {
    let session = manager
        .authenticate(crate::http::session_cookie(
            request.headers(),
            manager.uses_secure_cookies(),
        ))
        .await;
    match session {
        Ok(session) => {
            if manager
                .enforce_csrf(request.method(), request.headers(), &session)
                .is_err()
            {
                return error(StatusCode::FORBIDDEN, "csrf_failed");
            }
            request.extensions_mut().insert(session);
            let mut response = next.run(request).await;
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("no-store"),
            );
            response.headers_mut().insert(
                header::REFERRER_POLICY,
                axum::http::HeaderValue::from_static("no-referrer"),
            );
            response
        }
        Err(crate::sessions::SessionError::Unauthorized)
            if request.method() == axum::http::Method::GET =>
        {
            let destination = request
                .uri()
                .path_and_query()
                .map(|v| v.as_str())
                .unwrap_or("/consent");
            let encoded: String =
                url::form_urlencoded::byte_serialize(destination.as_bytes()).collect();
            (
                StatusCode::SEE_OTHER,
                [(header::LOCATION, format!("/login?redirect={encoded}"))],
            )
                .into_response()
        }
        Err(crate::sessions::SessionError::Unauthorized) => {
            error(StatusCode::UNAUTHORIZED, "unauthorized")
        }
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
    }
}
pub fn build_consent_router(state: ConsentState) -> Router {
    Router::new()
        .route("/consent", get(consent_page))
        .route("/consent/decision", post(consent_decision))
        .route("/consent/login-continue", get(login_continue))
        .layer(axum::middleware::from_fn_with_state(
            state.session_manager.clone(),
            consent_session,
        ))
        .with_state(state)
}
fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\"', "&quot;")
        .replace('\'', "&#39;")
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_to_allow_flags_maps_correctly() {
        let scopes = vec![
            "commoncal.availability.read".to_string(),
            "commoncal.event.read.basic".to_string(),
            "commoncal.event.create".to_string(),
        ];
        let (avail, titles, details, create, update, delete) = scopes_to_allow_flags(&scopes);
        assert!(avail);
        assert!(titles);
        assert!(!details);
        assert!(create);
        assert!(!update);
        assert!(!delete);
    }

    #[test]
    fn scopes_to_allow_flags_empty() {
        let scopes: Vec<String> = vec![];
        let (avail, titles, details, create, update, delete) = scopes_to_allow_flags(&scopes);
        assert!(!avail);
        assert!(!titles);
        assert!(!details);
        assert!(!create);
        assert!(!update);
        assert!(!delete);
    }

    #[test]
    fn scopes_to_allow_flags_calendar_metadata() {
        let scopes = vec!["commoncal.calendar.metadata.read".to_string()];
        let (avail, _, _, _, _, _) = scopes_to_allow_flags(&scopes);
        assert!(avail);
    }

    #[test]
    fn escape_html_escapes_special_chars() {
        assert_eq!(escape_html("<script>"), "&lt;script&gt;");
        assert_eq!(escape_html("a&b"), "a&amp;b");
        assert_eq!(escape_html("a\"b"), "a&quot;b");
    }

    #[test]
    fn validate_continuation_accepts_relative_path() {
        assert_eq!(
            validate_continuation("/consent?handoff=abc"),
            Some("/consent?handoff=abc".to_string())
        );
        assert_eq!(validate_continuation("/"), Some("/".to_string()));
    }

    #[test]
    fn validate_continuation_rejects_external_url() {
        assert_eq!(validate_continuation("https://evil.com"), None);
        assert_eq!(validate_continuation("http://evil.com"), None);
    }

    #[test]
    fn validate_continuation_rejects_protocol_relative() {
        assert_eq!(validate_continuation("//evil.com/path"), None);
    }

    #[test]
    fn validate_continuation_rejects_backslash_and_control() {
        assert_eq!(validate_continuation("/\\evil"), None);
        assert_eq!(validate_continuation("/\n/evil"), None);
    }

    #[test]
    fn validate_continuation_rejects_empty() {
        assert_eq!(validate_continuation(""), None);
        assert_eq!(validate_continuation("   "), None);
    }

    async fn test_pool() -> SqlitePool {
        use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
        let options = SqliteConnectOptions::new()
            .filename(":memory:")
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn upsert_grant_creates_exactly_one_active_grant() {
        let pool = test_pool().await;
        let now = now_secs();

        // Create a user and a calendar.
        let user_id: i64 = sqlx::query_scalar(
            "INSERT INTO users (normalized_email, display_name, status, created_at, is_superadmin) VALUES ('test@example.com', 'Test', 'registered', ?, 0) RETURNING id",
        )
        .bind(now)
        .fetch_one(&pool)
        .await
        .unwrap();
        let cal_id: i64 = sqlx::query_scalar(
            "INSERT INTO calendars (owner_user_id, name, color, default_timezone, default_event_visibility, created_at, updated_at) VALUES (?, 'Cal', '#fff', 'UTC', 'default', ?, ?) RETURNING id",
        )
        .bind(user_id)
        .bind(now)
        .bind(now)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO calendar_acl (calendar_id, user_id, role, created_at, updated_at) VALUES (?, ?, 'owner', ?, ?)",
        )
        .bind(cal_id)
        .bind(user_id)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();

        let scopes = vec![
            "commoncal.availability.read".to_string(),
            "commoncal.event.read.basic".to_string(),
        ];

        // First upsert.
        let grant_id1 = upsert_grant(&pool, user_id, "client-1", vec![cal_id], &scopes)
            .await
            .unwrap();
        assert!(!grant_id1.is_empty());

        // Second upsert (retry) — must replace, not accumulate.
        let grant_id2 = upsert_grant(&pool, user_id, "client-1", vec![cal_id], &scopes)
            .await
            .unwrap();
        assert_eq!(
            grant_id1, grant_id2,
            "retry must preserve the grant identity"
        );

        // Exactly one active grant.
        let active_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM mcp_grant WHERE user_id = ? AND oauth_client_id = ? AND revoked_at IS NULL",
        )
        .bind(user_id)
        .bind("client-1")
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(active_count, 1, "exactly one active grant after retry");

        // The active grant is the second one.
        let active_id: String = sqlx::query_scalar(
            "SELECT id FROM mcp_grant WHERE user_id = ? AND oauth_client_id = ? AND revoked_at IS NULL",
        )
        .bind(user_id)
        .bind("client-1")
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(active_id, grant_id2);

        // The first grant is revoked.
        let first_revoked: Option<i64> =
            sqlx::query_scalar("SELECT revoked_at FROM mcp_grant WHERE id = ?")
                .bind(&grant_id1)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            first_revoked.is_none(),
            "retry must preserve the active grant"
        );
    }

    #[tokio::test]
    async fn upsert_grant_intersects_with_live_calendars() {
        let pool = test_pool().await;
        let now = now_secs();

        // Create a user and two calendars.
        let user_id: i64 = sqlx::query_scalar(
            "INSERT INTO users (normalized_email, display_name, status, created_at, is_superadmin) VALUES ('test2@example.com', 'Test2', 'registered', ?, 0) RETURNING id",
        )
        .bind(now)
        .fetch_one(&pool)
        .await
        .unwrap();
        let cal1: i64 = sqlx::query_scalar(
            "INSERT INTO calendars (owner_user_id, name, color, default_timezone, default_event_visibility, created_at, updated_at) VALUES (?, 'Cal1', '#fff', 'UTC', 'default', ?, ?) RETURNING id",
        )
        .bind(user_id)
        .bind(now)
        .bind(now)
        .fetch_one(&pool)
        .await
        .unwrap();
        let cal2: i64 = sqlx::query_scalar(
            "INSERT INTO calendars (owner_user_id, name, color, default_timezone, default_event_visibility, created_at, updated_at) VALUES (?, 'Cal2', '#fff', 'UTC', 'default', ?, ?) RETURNING id",
        )
        .bind(user_id)
        .bind(now)
        .bind(now)
        .fetch_one(&pool)
        .await
        .unwrap();
        for cal in [cal1, cal2] {
            sqlx::query(
                "INSERT INTO calendar_acl (calendar_id, user_id, role, created_at, updated_at) VALUES (?, ?, 'owner', ?, ?)",
            )
            .bind(cal)
            .bind(user_id)
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        }

        let scopes = vec!["commoncal.availability.read".to_string()];

        // Upsert with both calendars.
        upsert_grant(&pool, user_id, "client-1", vec![cal1, cal2], &scopes)
            .await
            .unwrap();

        // Verify the grant has both calendars.
        let cal_ids: String = sqlx::query_scalar(
            "SELECT allowed_calendar_ids FROM mcp_grant WHERE user_id = ? AND oauth_client_id = ? AND revoked_at IS NULL",
        )
        .bind(user_id)
        .bind("client-1")
        .fetch_one(&pool)
        .await
        .unwrap();
        let ids: Vec<i64> = serde_json::from_str(&cal_ids).unwrap();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&cal1));
        assert!(ids.contains(&cal2));
        sqlx::query("DELETE FROM calendar_acl WHERE calendar_id = ?")
            .bind(cal2)
            .execute(&pool)
            .await
            .unwrap();
        upsert_grant(&pool, user_id, "client-1", vec![cal1, cal2, 9999], &scopes)
            .await
            .unwrap();
        let calendars: String = sqlx::query_scalar(
            "SELECT allowed_calendar_ids FROM mcp_grant WHERE user_id = ? AND revoked_at IS NULL",
        )
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<i64>>(&calendars).unwrap(),
            vec![cal1],
            "retry intersects live membership and never adds unknown calendars"
        );
    }
    use axum::body::{Body, to_bytes};
    use axum::http::Request as HttpRequest;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tower::ServiceExt;

    struct BrowserFixture {
        pool: SqlitePool,
        router: Router,
        cookie: String,
        other_cookie: String,
        csrf: String,
        other_csrf: String,
        user: i64,
        calendars: Vec<i64>,
        bridge_calls: Arc<AtomicUsize>,
        server: tokio::task::JoinHandle<()>,
    }
    impl Drop for BrowserFixture {
        fn drop(&mut self) {
            self.server.abort();
        }
    }
    async fn browser_fixture() -> BrowserFixture {
        let pool = test_pool().await;
        let key = crate::security::SecretKey::generate();
        let now = now_secs();
        let mut users = Vec::new();
        let mut cookies = Vec::new();
        let mut csrf = Vec::new();
        for email in ["owner@example.test", "other@example.test"] {
            let user: i64 = sqlx::query_scalar("INSERT INTO users (normalized_email,display_name,status,created_at,is_superadmin) VALUES (?,'Owner','registered',?,0) RETURNING id").bind(email).bind(now).fetch_one(&pool).await.unwrap();
            let token = key.generate_token();
            let hash = key.hash_token(crate::security::TokenDomain::Session, &token);
            sqlx::query("INSERT INTO sessions (user_id,session_hash,expires_at,created_at) VALUES (?,?,?,?)").bind(user).bind(hash.as_bytes().as_slice()).bind(now+3600).bind(now).execute(&pool).await.unwrap();
            cookies.push(format!("commoncal_session={}", token.expose()));
            csrf.push(key.generate_csrf_token(&token).expose().to_owned());
            users.push(user);
        }
        let mut calendars = Vec::new();
        for (user, name) in [
            (users[0], "Personal"),
            (users[0], "Work"),
            (users[1], "Other's private calendar"),
        ] {
            let id: i64 = sqlx::query_scalar("INSERT INTO calendars (owner_user_id,name,color,default_timezone,default_event_visibility,created_at,updated_at) VALUES (?,?,'#fff','UTC','default',?,?) RETURNING id").bind(user).bind(name).bind(now).bind(now).fetch_one(&pool).await.unwrap();
            sqlx::query("INSERT INTO calendar_acl (calendar_id,user_id,role,created_at,updated_at) VALUES (?,?,'owner',?,?)").bind(id).bind(user).bind(now).bind(now).execute(&pool).await.unwrap();
            calendars.push(id);
        }
        let bridge_calls = Arc::new(AtomicUsize::new(0));
        let calls = bridge_calls.clone();
        let owner = users[0];
        let bridge_router=Router::new().route("/internal/interactions/:handoff",get(move |axum::extract::Path(handoff): axum::extract::Path<String>, headers: axum::http::HeaderMap| async move {
            assert_eq!(headers.get(header::AUTHORIZATION).unwrap(),"Bearer bridge-secret");
            Json(InteractionView {
                client_id:"client-browser".into(),client_name:"Browser Client <script>".into(),redirect_uri:"http://localhost/callback".into(),resource:"https://mcal.hajnal.space/mcp".into(),
                requested_scopes:vec!["commoncal.calendar.metadata.read".into(),"commoncal.event.delete".into()],
                granted_scopes:if handoff=="empty-scopes" { vec![] } else { vec!["commoncal.calendar.metadata.read".into(),"commoncal.event.create".into()] },
                subject:Some(if handoff=="wrong-subject" {(owner+1).to_string()} else {owner.to_string()}),
                prompt:if handoff=="wrong-prompt" {"login".into()} else {"consent".into()},
                expires_at:if handoff=="expired" {now-1} else {now+3600},
            })
        }).put(move |axum::extract::Path(handoff): axum::extract::Path<String>, Json(body): Json<serde_json::Value>| { let calls=calls.clone();async move {
            let count=calls.fetch_add(1,Ordering::SeqCst);
            assert_eq!(body["subject"],owner);
            if handoff=="bridge-retry" && count==0 { return error(StatusCode::BAD_GATEWAY,"response_lost"); }
            Json(serde_json::json!({"resumeUrl":"https://auth.hajnal.space/interaction/resume"})).into_response()
        }}));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, bridge_router).await.unwrap();
        });
        let manager = SessionManager::new(
            pool.clone(),
            key,
            crate::sessions::SessionSecurityConfig::new(3600, 60, "http://localhost").unwrap(),
        );
        let router = build_consent_router(ConsentState {
            pool: pool.clone(),
            session_manager: manager,
            bridge: McpBridgeClient::new(
                format!("http://{addr}"),
                std::time::Duration::from_secs(2),
                "bridge-secret".into(),
            ),
        });
        BrowserFixture {
            pool,
            router,
            cookie: cookies[0].clone(),
            other_cookie: cookies[1].clone(),
            csrf: csrf[0].clone(),
            other_csrf: csrf[1].clone(),
            user: users[0],
            calendars,
            bridge_calls,
            server,
        }
    }
    async fn decide(
        f: &BrowserFixture,
        handoff: &str,
        decision: &str,
        calendars: &[i64],
        other: bool,
    ) -> Response {
        let request = HttpRequest::builder()
            .method("POST")
            .uri("/consent/decision")
            .header(
                header::COOKIE,
                if other { &f.other_cookie } else { &f.cookie },
            )
            .header(header::ORIGIN, "http://localhost")
            .header("sec-fetch-site", "same-origin")
            .header("x-csrf-token", if other { &f.other_csrf } else { &f.csrf })
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({"handoff":handoff,"decision":decision,"calendar_ids":calendars})
                    .to_string(),
            ))
            .unwrap();
        f.router.clone().oneshot(request).await.unwrap()
    }
    #[tokio::test]
    async fn browser_navigation_requires_login_and_page_submits_csrf_with_selected_calendars() {
        let f = browser_fixture().await;
        let response = f
            .router
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .uri("/consent?handoff=abc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers()[header::LOCATION],
            "/login?redirect=%2Fconsent%3Fhandoff%3Dabc"
        );
        let response = f
            .router
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .uri("/consent?handoff=abc")
                    .header(header::COOKIE, &f.cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let html = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(html.contains("'x-csrf-token':form.dataset.csrf"));
        assert!(html.contains("data-csrf=\""));
        assert!(html.contains("Personal"));
        assert!(html.contains("Work"));
        assert!(!html.contains("Other's private"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("value=\"1\" checked"));
    }
    #[tokio::test]
    async fn browser_approve_intersects_selection_scopes_and_live_membership() {
        let f = browser_fixture().await;
        sqlx::query("DELETE FROM calendar_acl WHERE calendar_id=?")
            .bind(f.calendars[1])
            .execute(&f.pool)
            .await
            .unwrap();
        let response = decide(
            &f,
            "approve",
            "approve",
            &[f.calendars[0], f.calendars[1], f.calendars[2], 9999],
            false,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let grant:(i64,String,bool,bool,bool)=sqlx::query_as("SELECT user_id,allowed_calendar_ids,allow_availability,allow_create,allow_delete FROM mcp_grant WHERE revoked_at IS NULL").fetch_one(&f.pool).await.unwrap();
        assert_eq!(grant.0, f.user);
        assert_eq!(
            serde_json::from_str::<Vec<i64>>(&grant.1).unwrap(),
            vec![f.calendars[0]]
        );
        assert!(grant.2);
        assert!(!grant.3);
        assert!(!grant.4);
    }
    #[tokio::test]
    async fn browser_retries_do_not_widen_recreate_or_cross_users() {
        let f = browser_fixture().await;
        assert_eq!(
            decide(&f, "retry", "approve", &f.calendars[..2], false)
                .await
                .status(),
            StatusCode::OK
        );
        let id: String = sqlx::query_scalar("SELECT id FROM mcp_grant WHERE revoked_at IS NULL")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE mcp_grant SET allowed_calendar_ids='[]',allow_availability=0 WHERE id=?",
        )
        .bind(&id)
        .execute(&f.pool)
        .await
        .unwrap();
        let response = decide(&f, "retry", "approve", &f.calendars, false).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let unchanged: (String, bool) = sqlx::query_as(
            "SELECT allowed_calendar_ids,allow_availability FROM mcp_grant WHERE id=?",
        )
        .bind(&id)
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(unchanged, ("[]".into(), false));
        assert_eq!(f.bridge_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            decide(&f, "retry", "approve", &f.calendars, true)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            decide(&f, "retry", "deny", &[], false).await.status(),
            StatusCode::FORBIDDEN
        );
        sqlx::query("UPDATE mcp_grant SET revoked_at=? WHERE id=?")
            .bind(now_secs())
            .bind(id)
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            decide(&f, "retry", "approve", &f.calendars, false)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mcp_grant")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }
    #[tokio::test]
    async fn browser_deny_and_invalid_interactions_create_no_grants() {
        let f = browser_fixture().await;
        assert_eq!(
            decide(&f, "deny", "deny", &[], false).await.status(),
            StatusCode::OK
        );
        assert_eq!(
            decide(&f, "deny", "deny", &[], false).await.status(),
            StatusCode::OK
        );
        for handoff in ["wrong-subject", "expired", "wrong-prompt", "empty-scopes"] {
            assert!(
                !decide(&f, handoff, "approve", &f.calendars, false)
                    .await
                    .status()
                    .is_success(),
                "{handoff} must fail closed"
            );
        }
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mcp_grant")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
    #[tokio::test]
    async fn browser_bridge_failure_retry_keeps_narrowed_grant() {
        let f = browser_fixture().await;
        assert_eq!(
            decide(&f, "bridge-retry", "approve", &f.calendars[..2], false)
                .await
                .status(),
            StatusCode::BAD_GATEWAY
        );
        sqlx::query("UPDATE mcp_grant SET allowed_calendar_ids='[]',allow_availability=0")
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            decide(&f, "bridge-retry", "approve", &f.calendars, false)
                .await
                .status(),
            StatusCode::OK
        );
        let grant: (String, bool) =
            sqlx::query_as("SELECT allowed_calendar_ids,allow_availability FROM mcp_grant")
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(grant, ("[]".into(), false));
    }
    #[tokio::test]
    async fn browser_decision_enforces_session_origin_fetch_site_and_csrf() {
        let f = browser_fixture().await;
        for omitted in ["cookie", "origin", "sec-fetch-site", "x-csrf-token"] {
            let mut request = HttpRequest::builder()
                .method("POST")
                .uri("/consent/decision")
                .header(header::CONTENT_TYPE, "application/json");
            for (header, value) in [
                ("cookie", f.cookie.as_str()),
                ("origin", "http://localhost"),
                ("sec-fetch-site", "same-origin"),
                ("x-csrf-token", f.csrf.as_str()),
            ] {
                if omitted != header {
                    request = request.header(header, value);
                }
            }
            let response=f.router.clone().oneshot(request.body(Body::from(serde_json::json!({"handoff":"csrf","decision":"approve","calendar_ids":f.calendars}).to_string())).unwrap()).await.unwrap();
            assert_eq!(
                response.status(),
                if omitted == "cookie" {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::FORBIDDEN
                },
                "missing {omitted}"
            );
        }
        assert_eq!(f.bridge_calls.load(Ordering::SeqCst), 0);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mcp_grant")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
}
