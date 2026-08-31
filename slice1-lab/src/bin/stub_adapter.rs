//! Disposable Slice 1 login/consent stub adapter.
//!
//! Accepts exactly ONE fixed test subject (`common::FIXED_SUBJECT`) and grants
//! only the intersection of requested scopes, the CommonCal scope catalog, and
//! the fixed test approval, for the exact MCP resource audience. Fails closed
//! on unknown audiences, unknown challenges, or malformed requests.
//!
//! This is NOT the production adapter. It exists only to prove the Hydra
//! DCR/PKCE/JWT chain in Slice 1. It must never be used with a real account.

use axum::{
    Router,
    extract::Query,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};

use slice1_lab::common::{LabConfig, RESOURCE_URL, FIXED_SUBJECT, granted_scopes};

#[derive(Debug, Deserialize)]
struct ChallengeQuery {
    login_challenge: Option<String>,
    consent_challenge: Option<String>,
    // `request` is present but not used by the stub.
    #[allow(dead_code)]
    request: Option<String>,
}

#[derive(Clone)]
struct Admin {
    http: reqwest::Client,
    base: String,
}

impl Admin {
    fn new(base: String) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("http client"),
            base,
        }
    }

    async fn get(&self, path: &str) -> Result<Value, String> {
        let url = format!("{}{}", self.base, path);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("admin GET {url} transport: {e}"))?;
        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| format!("admin GET {url} bad json: {e}"))?;
        if !status.is_success() {
            return Err(format!("admin GET {url} -> {status}: {}", err_text(&body)));
        }
        Ok(body)
    }

    /// Hydra's admin accept endpoints require PUT (POST -> 405 Method Not Allowed).
    async fn put(&self, path: &str, payload: &Value) -> Result<Value, String> {
        let url = format!("{}{}", self.base, path);
        let resp = self
            .http
            .put(&url)
            .json(payload)
            .send()
            .await
            .map_err(|e| format!("admin PUT {url} transport: {e}"))?;
        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| format!("admin PUT {url} bad json: {e}"))?;
        if !status.is_success() {
            return Err(format!("admin PUT {url} -> {status}: {}", err_text(&body)));
        }
        Ok(body)
    }
}

fn err_text(body: &Value) -> String {
    body.get("error_description")
        .or_else(|| body.get("error"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown error")
        .to_string()
}

/// Extract the `redirect_to` URL from a Hydra accept response.
fn redirect_to(body: &Value) -> Result<String, String> {
    body.get("redirect_to")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("accept response missing redirect_to: {}", truncate(body)))
}

fn truncate(v: &Value) -> String {
    let s = v.to_string();
    if s.len() > 200 {
        format!("{}…", &s[..200])
    } else {
        s
    }
}

async fn handle_login(
    admin: &Admin,
    q: &ChallengeQuery,
) -> Result<Response, (StatusCode, String)> {
    let challenge = q
        .login_challenge
        .as_ref()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing login_challenge".to_string()))?;

    // 1. Verify the challenge through the admin API (fail closed if unknown).
    let get_path = format!(
        "/admin/oauth2/auth/requests/login?challenge={}",
        urlencoding(challenge)
    );
    let _req = admin.get(&get_path).await.map_err(|e| {
        (StatusCode::BAD_GATEWAY, format!("login challenge invalid: {e}"))
    })?;

    // 2. Accept login with the fixed test subject.
    let accept_path = format!(
        "/admin/oauth2/auth/requests/login/accept?challenge={}",
        urlencoding(challenge)
    );
    let payload = json!({
        "subject": FIXED_SUBJECT,
        "remember": true,
        "remember_for": 0,
        "amr": ["password"]
    });
    let resp = admin
        .put(&accept_path, &payload)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("login accept failed: {e}")))?;
    let to = redirect_to(&resp).map_err(|e| (StatusCode::BAD_GATEWAY, e))?;

    Ok(redirect_response(&to))
}

async fn handle_consent(
    admin: &Admin,
    q: &ChallengeQuery,
) -> Result<Response, (StatusCode, String)> {
    let challenge = q
        .consent_challenge
        .as_ref()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing consent_challenge".to_string()))?;

    // 1. Read the consent request (fail closed if unknown).
    let get_path = format!(
        "/admin/oauth2/auth/requests/consent?challenge={}",
        urlencoding(challenge)
    );
    let req = admin.get(&get_path).await.map_err(|e| {
        (StatusCode::BAD_GATEWAY, format!("consent challenge invalid: {e}"))
    })?;

    let requested_scope: Vec<String> = req
        .get("requested_scope")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let requested_audience: Vec<String> = req
        .get("requested_access_token_audience")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    // 2. Grant only the intersection of requested ∩ catalog ∩ fixed-approval.
    let granted = granted_scopes(&requested_scope);
    if granted.is_empty() {
        return Err((
            StatusCode::FORBIDDEN,
            "no grantable scopes in the CommonCal catalog".to_string(),
        ));
    }

    // 3. Grant only the exact MCP resource audience. Fail closed otherwise.
    let granted_audience: Vec<String> = requested_audience
        .into_iter()
        .filter(|a| a == RESOURCE_URL)
        .collect();
    if granted_audience.is_empty() {
        return Err((
            StatusCode::FORBIDDEN,
            format!(
                "requested audience does not include the exact MCP resource {RESOURCE_URL}; refusing"
            ),
        ));
    }

    // 4. Accept consent with the intersection only.
    let accept_path = format!(
        "/admin/oauth2/auth/requests/consent/accept?challenge={}",
        urlencoding(challenge)
    );
    let payload = json!({
        "grant_scope": granted,
        "grant_access_token_audience": granted_audience,
        "remember": true,
        "remember_for": 0
    });
    let resp = admin
        .put(&accept_path, &payload)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("consent accept failed: {e}")))?;
    let to = redirect_to(&resp).map_err(|e| (StatusCode::BAD_GATEWAY, e))?;

    tracing::info!(
        granted_scopes = ?granted,
        granted_audience = ?granted_audience,
        "consent accepted (intersection only)"
    );

    Ok(redirect_response(&to))
}

fn redirect_response(to: &str) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::LOCATION,
        to.parse().expect("valid redirect_to"),
    );
    (StatusCode::FOUND, headers).into_response()
}

fn urlencoding(s: &str) -> String {
    // Challenges are URL-safe base64url; encode conservatively.
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

async fn login_handler(
    axum::Extension(admin): axum::Extension<Admin>,
    Query(q): Query<ChallengeQuery>,
) -> Result<Response, (StatusCode, String)> {
    handle_login(&admin, &q).await
}

async fn consent_handler(
    axum::Extension(admin): axum::Extension<Admin>,
    Query(q): Query<ChallengeQuery>,
) -> Result<Response, (StatusCode, String)> {
    handle_consent(&admin, &q).await
}

async fn health() -> &'static str {
    "ok"
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cfg = LabConfig::from_env();
    let admin = Admin::new(cfg.hydra_admin.clone());

    let app = Router::new()
        .route("/health", axum::routing::get(health))
        .route("/oauth/hydra/login", axum::routing::get(login_handler))
        .route("/oauth/hydra/consent", axum::routing::get(consent_handler))
        .layer(axum::Extension(admin));

    // Bind to the IPv6 loopback so `localhost` (which resolves to `::1` first)
    // reaches us and Hydra's `localhost`-scoped session cookie is accepted.
    let bind = slice1_lab::common::bind_addr("STUB_BIND", "[::1]:8080".parse().unwrap());
    tracing::info!(%bind, "stub adapter listening");
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
