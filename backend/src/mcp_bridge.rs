// Private interaction bridge client for the MCP consent flow.
//
// Talks to the authorization server's private API (bearer-keyed) to look up
// and decide OAuth interactions. CommonCal is the identity authority: it
// approves the subject through this trusted bridge rather than trusting a
// browser-supplied user id.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The interaction view returned by the authorization server's private API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractionView {
    pub client_id: String,
    pub client_name: String,
    pub redirect_uri: String,
    pub resource: String,
    pub requested_scopes: Vec<String>,
    /// The granted scopes (intersection of requested and catalog), computed by
    /// the auth server. Recorded on the grant so it agrees with the JWT.
    #[serde(default)]
    pub granted_scopes: Vec<String>,
    pub prompt: String,
    /// OAuth login subject. Consent must be approved by this same user.
    #[serde(default)]
    pub subject: Option<String>,
    pub expires_at: i64,
}

/// Errors from the private bridge.
#[derive(Debug)]
pub enum BridgeError {
    /// The bridge is not configured (no URL or secret).
    NotConfigured,
    /// The HTTP request failed (network, timeout, etc.).
    Transport(String),
    /// The authorization server returned a non-success status.
    Http(u16, String),
    /// The response body could not be parsed.
    Parse(String),
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConfigured => write!(f, "bridge not configured"),
            Self::Transport(e) => write!(f, "bridge transport error: {e}"),
            Self::Http(status, body) => write!(f, "bridge returned {status}: {body}"),
            Self::Parse(e) => write!(f, "bridge parse error: {e}"),
        }
    }
}

impl std::error::Error for BridgeError {}

/// Client for the authorization server's private interaction bridge.
#[derive(Clone)]
pub struct McpBridgeClient {
    base_url: String,
    secret: String,
    http: reqwest::Client,
}

impl McpBridgeClient {
    /// Create a new bridge client.
    ///
    /// `base_url` is the private API base (e.g. `http://auth:4001`).
    /// `timeout` bounds each request. `secret` is the bearer bridge key.
    pub fn new(base_url: String, timeout: Duration, secret: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("http client");
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            secret,
            http,
        }
    }

    /// Whether the bridge is configured (has a non-empty URL and secret).
    pub fn is_configured(&self) -> bool {
        !self.base_url.is_empty() && !self.secret.is_empty()
    }

    /// Look up a handoff from the authorization server's private API.
    pub async fn lookup_interaction(&self, handoff: &str) -> Result<InteractionView, BridgeError> {
        if !self.is_configured() {
            return Err(BridgeError::NotConfigured);
        }
        let url = format!(
            "{}/internal/interactions/{}",
            self.base_url,
            urlencoding(handoff)
        );
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.secret))
            .send()
            .await
            .map_err(|e| BridgeError::Transport(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(BridgeError::Http(status.as_u16(), body));
        }
        let body = resp
            .text()
            .await
            .map_err(|e| BridgeError::Transport(e.to_string()))?;
        let view: InteractionView =
            serde_json::from_str(&body).map_err(|e| BridgeError::Parse(e.to_string()))?;
        Ok(view)
    }

    /// Decide a handoff (login/consent/deny) via the authorization server's
    /// private API. `subject` is the authenticated CommonCal user id (the
    /// identity authority). Returns the resume URL.
    pub async fn decide_interaction(
        &self,
        handoff: &str,
        kind: &str,
        subject: Option<i64>,
    ) -> Result<String, BridgeError> {
        if !self.is_configured() {
            return Err(BridgeError::NotConfigured);
        }
        let url = format!(
            "{}/internal/interactions/{}",
            self.base_url,
            urlencoding(handoff)
        );
        let mut body = serde_json::json!({ "kind": kind });
        if let Some(sub) = subject {
            body["subject"] = serde_json::json!(sub);
        }
        let resp = self
            .http
            .put(&url)
            .header("Authorization", format!("Bearer {}", self.secret))
            .json(&body)
            .send()
            .await
            .map_err(|e| BridgeError::Transport(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(BridgeError::Http(status.as_u16(), text));
        }
        let result: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| BridgeError::Parse(e.to_string()))?;
        result
            .get("resumeUrl")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| BridgeError::Parse("decide response missing resumeUrl".to_string()))
    }
}

/// Percent-encode a path segment (handoff token).
fn urlencoding(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            _ => {
                out.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencoding_encodes_special_chars() {
        assert_eq!(urlencoding("abc"), "abc");
        assert_eq!(urlencoding("a/b"), "a%2Fb");
        assert_eq!(urlencoding("a b"), "a%20b");
        assert_eq!(urlencoding("a+b=c"), "a%2Bb%3Dc");
        assert_eq!(urlencoding("a-b_c.d~e"), "a-b_c.d~e");
    }

    #[test]
    fn not_configured_when_empty() {
        let client = McpBridgeClient::new(String::new(), Duration::from_secs(5), String::new());
        assert!(!client.is_configured());
    }

    #[test]
    fn configured_when_url_and_secret_present() {
        let client = McpBridgeClient::new(
            "http://auth:4001".into(),
            Duration::from_secs(5),
            "key".into(),
        );
        assert!(client.is_configured());
    }

    #[test]
    fn base_url_trims_trailing_slash() {
        let client = McpBridgeClient::new(
            "http://auth:4001/".into(),
            Duration::from_secs(5),
            "key".into(),
        );
        assert_eq!(client.base_url, "http://auth:4001");
    }

    #[tokio::test]
    async fn lookup_fails_when_not_configured() {
        let client = McpBridgeClient::new(String::new(), Duration::from_secs(5), String::new());
        let result = client.lookup_interaction("handoff").await;
        assert!(matches!(result, Err(BridgeError::NotConfigured)));
    }

    #[tokio::test]
    async fn decide_fails_when_not_configured() {
        let client = McpBridgeClient::new(String::new(), Duration::from_secs(5), String::new());
        let result = client
            .decide_interaction("handoff", "consent", Some(1))
            .await;
        assert!(matches!(result, Err(BridgeError::NotConfigured)));
    }
}
