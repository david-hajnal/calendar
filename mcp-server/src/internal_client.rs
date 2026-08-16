// Internal HTTP client for CommonCal backend API.
//
// In v1, authenticates via MCP_INTERNAL_API_KEY header.
// Future: mTLS with workload identity.

use serde::Deserialize;

#[derive(Clone)]
pub struct InternalClient {
    api_base: url::Url,
    api_key: String,
    http_client: reqwest::Client,
}

#[derive(Debug, Deserialize)]
pub struct UserStatusResponse {
    pub user_id: i64,
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub struct CalendarInfo {
    pub id: i64,
    pub name: String,
    pub role: String,
    pub access: String,
}

#[derive(Debug, Deserialize)]
pub struct EventInfo {
    pub id: i64,
    pub calendar_id: i64,
    pub title: Option<String>,
    pub description: Option<String>,
    pub location: Option<String>,
    pub status: String,
    pub event_kind: String,
    pub start_utc: Option<i64>,
    pub end_utc: Option<i64>,
    pub version: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteIntent {
    pub intent_id: String,
    pub event_id: i64,
    pub calendar_id: i64,
    pub event_version: i64,
    pub expires_at: i64,
    pub confirmation_state: String,
}

#[derive(Debug, Deserialize)]
pub struct ReminderResponse {
    pub reminder_id: String,
}

#[derive(Debug, Deserialize)]
pub struct McpGrantResponse {
    pub grant_id: String,
    pub user_id: i64,
    pub oauth_client_id: String,
    pub allowed_calendar_ids: Vec<i64>,
    pub allow_availability: bool,
    pub allow_event_titles: bool,
    pub allow_event_details: bool,
    pub allow_create: bool,
    pub allow_update: bool,
    pub allow_delete: bool,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

/// Result of RFC 8693 token exchange — converts MCP client access_token to internal API token.
#[derive(Debug, Deserialize)]
pub struct TokenExchangeResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: i64,
    pub scope: String,
}

impl InternalClient {
    pub fn new(api_base: String, api_key: String) -> Result<Self, crate::config::ConfigError> {
        let url = url::Url::parse(&api_base).map_err(|e| {
            crate::config::ConfigError::new(format!(
                "MCP_INTERNAL_API_BASE is not a valid URL: {}",
                e
            ))
        })?;

        if !url.username().is_empty() {
            return Err(crate::config::ConfigError::new(
                "MCP_INTERNAL_API_BASE must not contain username",
            ));
        }

        if url.password().is_some() {
            return Err(crate::config::ConfigError::new(
                "MCP_INTERNAL_API_BASE must not contain password",
            ));
        }

        if url.query().is_some_and(|q| !q.is_empty()) {
            return Err(crate::config::ConfigError::new(
                "MCP_INTERNAL_API_BASE must not contain query string",
            ));
        }

        if url.fragment().is_some_and(|f| !f.is_empty()) {
            return Err(crate::config::ConfigError::new(
                "MCP_INTERNAL_API_BASE must not contain fragment",
            ));
        }

        if url.path() != "/" && url.path().ends_with('/') {
            return Err(crate::config::ConfigError::new(
                "MCP_INTERNAL_API_BASE must not end with trailing slash",
            ));
        }

        Ok(Self {
            api_base: url,
            api_key,
            http_client: reqwest::Client::new(),
        })
    }

    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    pub fn api_base(&self) -> &str {
        self.api_base.as_str()
    }

    /// Perform RFC 8693 token exchange.
    ///
    /// Converts the MCP client's access_token (issued by the OAuth issuer)
    /// into an internal API token that the backend recognizes.
    ///
    /// Grant type: `urn:ietf:params:oauth:grant-type:token-exchange`
    /// Subject token: the MCP client's access_token
    /// Audience: the internal service name
    pub async fn exchange_token(
        &self,
        subject_token: &str,
        actor_token: Option<&str>,
        resource: &str,
    ) -> Result<TokenExchangeResponse, InternalError> {
        let url = self
            .api_base
            .join("internal/token-exchange")
            .map_err(|e| InternalError::Connection(format!("failed to join URL: {}", e)))?;

        let mut body = serde_json::json!({
            "grant_type": "urn:ietf:params:oauth:grant-type:token-exchange",
            "subject_token": subject_token,
            "subject_token_type": "urn:ietf:params:oauth:token-type:access_token",
            "audience": "commoncal-internal",
            "resource": [resource],
        });

        if let Some(actor) = actor_token {
            body["actor_token"] = serde_json::json!(actor);
            body["actor_token_type"] =
                serde_json::json!("urn:ietf:params:oauth:token-type:access_token");
        }

        let resp = self
            .http_client
            .post(url.as_str())
            .header("x-mcp-api-key", &self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| InternalError::Connection(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body_text = resp.text().await.unwrap_or_else(|_| "no body".to_string());
            tracing::warn!(
                status,
                body = %body_text,
                "token exchange failed"
            );
            return Err(InternalError::Http(
                status,
                "token exchange failed".to_string(),
            ));
        }

        let exchange_resp: TokenExchangeResponse = resp
            .json()
            .await
            .map_err(|e| InternalError::Deserialize(e.to_string()))?;

        Ok(exchange_resp)
    }

    pub async fn get_user_status(&self, user_id: i64) -> Result<UserStatusResponse, InternalError> {
        let url = self
            .api_base
            .join(&format!("internal/mcp/users/{}/status", user_id))
            .map_err(|e| InternalError::Connection(format!("failed to join URL: {}", e)))?;
        let resp = self
            .http_client
            .get(url.as_str())
            .header("x-mcp-api-key", &self.api_key)
            .send()
            .await
            .map_err(|e| InternalError::Connection(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(InternalError::Http(
                resp.status().as_u16(),
                "get_user_status failed".to_string(),
            ));
        }

        let body = resp
            .json::<UserStatusResponse>()
            .await
            .map_err(|e| InternalError::Deserialize(e.to_string()))?;

        Ok(body)
    }

    pub async fn list_calendars(&self, user_id: i64) -> Result<Vec<CalendarInfo>, InternalError> {
        let url = self
            .api_base
            .join(&format!("internal/mcp/users/{}/calendars", user_id))
            .map_err(|e| InternalError::Connection(format!("failed to join URL: {}", e)))?;
        let resp = self
            .http_client
            .get(url.as_str())
            .header("x-mcp-api-key", &self.api_key)
            .send()
            .await
            .map_err(|e| InternalError::Connection(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(InternalError::Http(
                resp.status().as_u16(),
                "list_calendars failed".to_string(),
            ));
        }

        let body = resp
            .json::<Vec<CalendarInfo>>()
            .await
            .map_err(|e| InternalError::Deserialize(e.to_string()))?;

        Ok(body)
    }

    pub async fn get_calendar_role(
        &self,
        user_id: i64,
        calendar_id: i64,
    ) -> Result<String, InternalError> {
        let url = self
            .api_base
            .join(&format!(
                "internal/mcp/calendars/{}/role/{}",
                calendar_id, user_id
            ))
            .map_err(|e| InternalError::Connection(format!("failed to join URL: {}", e)))?;
        let resp = self
            .http_client
            .get(url.as_str())
            .header("x-mcp-api-key", &self.api_key)
            .send()
            .await
            .map_err(|e| InternalError::Connection(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(InternalError::Http(
                resp.status().as_u16(),
                "get_calendar_role failed".to_string(),
            ));
        }

        let body: String = resp
            .text()
            .await
            .map_err(|e| InternalError::Deserialize(e.to_string()))?;

        Ok(body)
    }

    pub async fn get_event(
        &self,
        calendar_id: i64,
        event_id: i64,
    ) -> Result<EventInfo, InternalError> {
        let url = self
            .api_base
            .join(&format!("internal/mcp/events/{}/{}", calendar_id, event_id))
            .map_err(|e| InternalError::Connection(format!("failed to join URL: {}", e)))?;
        let resp = self
            .http_client
            .get(url.as_str())
            .header("x-mcp-api-key", &self.api_key)
            .send()
            .await
            .map_err(|e| InternalError::Connection(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(InternalError::Http(
                resp.status().as_u16(),
                "get_event failed".to_string(),
            ));
        }

        let body = resp
            .json::<EventInfo>()
            .await
            .map_err(|e| InternalError::Deserialize(e.to_string()))?;

        Ok(body)
    }

    pub async fn search_events(
        &self,
        calendar_id: i64,
        from: &str,
        to: &str,
    ) -> Result<Vec<EventInfo>, InternalError> {
        let mut url = self
            .api_base
            .join(&format!("internal/mcp/events/{}/search", calendar_id))
            .map_err(|e| InternalError::Connection(format!("failed to join URL: {}", e)))?;
        url.query_pairs_mut()
            .append_pair("from", from)
            .append_pair("to", to);
        let resp = self
            .http_client
            .get(url.as_str())
            .header("x-mcp-api-key", &self.api_key)
            .send()
            .await
            .map_err(|e| InternalError::Connection(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(InternalError::Http(
                resp.status().as_u16(),
                "search_events failed".to_string(),
            ));
        }

        let body = resp
            .json::<Vec<EventInfo>>()
            .await
            .map_err(|e| InternalError::Deserialize(e.to_string()))?;

        Ok(body)
    }

    pub async fn create_event(
        &self,
        calendar_id: i64,
        payload: &serde_json::Value,
    ) -> Result<EventInfo, InternalError> {
        let url = self
            .api_base
            .join(&format!("internal/mcp/events/{}", calendar_id))
            .map_err(|e| InternalError::Connection(format!("failed to join URL: {}", e)))?;
        let resp = self
            .http_client
            .post(url.as_str())
            .header("x-mcp-api-key", &self.api_key)
            .json(payload)
            .send()
            .await
            .map_err(|e| InternalError::Connection(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(InternalError::Http(
                resp.status().as_u16(),
                "create_event failed".to_string(),
            ));
        }

        let body = resp
            .json::<EventInfo>()
            .await
            .map_err(|e| InternalError::Deserialize(e.to_string()))?;

        Ok(body)
    }

    pub async fn update_event(
        &self,
        calendar_id: i64,
        event_id: i64,
        payload: &serde_json::Value,
    ) -> Result<EventInfo, InternalError> {
        let url = self
            .api_base
            .join(&format!("internal/mcp/events/{}/{}", calendar_id, event_id))
            .map_err(|e| InternalError::Connection(format!("failed to join URL: {}", e)))?;
        let resp = self
            .http_client
            .patch(url.as_str())
            .header("x-mcp-api-key", &self.api_key)
            .json(payload)
            .send()
            .await
            .map_err(|e| InternalError::Connection(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(InternalError::Http(
                resp.status().as_u16(),
                "update_event failed".to_string(),
            ));
        }

        let body = resp
            .json::<EventInfo>()
            .await
            .map_err(|e| InternalError::Deserialize(e.to_string()))?;

        Ok(body)
    }

    pub async fn create_delete_intent(
        &self,
        payload: &serde_json::Value,
    ) -> Result<DeleteIntent, InternalError> {
        let url = self
            .api_base
            .join("internal/mcp/delete-intents")
            .map_err(|e| InternalError::Connection(format!("failed to join URL: {}", e)))?;
        let resp = self
            .http_client
            .post(url.as_str())
            .header("x-mcp-api-key", &self.api_key)
            .json(payload)
            .send()
            .await
            .map_err(|e| InternalError::Connection(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(InternalError::Http(
                resp.status().as_u16(),
                "create_delete_intent failed".to_string(),
            ));
        }

        let body = resp
            .json::<DeleteIntent>()
            .await
            .map_err(|e| InternalError::Deserialize(e.to_string()))?;

        Ok(body)
    }

    pub async fn get_delete_intent(&self, intent_id: &str) -> Result<DeleteIntent, InternalError> {
        let url = self
            .api_base
            .join(&format!("internal/mcp/delete-intents/{}", intent_id))
            .map_err(|e| InternalError::Connection(format!("failed to join URL: {}", e)))?;
        let resp = self
            .http_client
            .get(url.as_str())
            .header("x-mcp-api-key", &self.api_key)
            .send()
            .await
            .map_err(|e| InternalError::Connection(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(InternalError::Http(
                resp.status().as_u16(),
                "get_delete_intent failed".to_string(),
            ));
        }

        let body = resp
            .json::<DeleteIntent>()
            .await
            .map_err(|e| InternalError::Deserialize(e.to_string()))?;

        Ok(body)
    }

    pub async fn commit_delete_intent(&self, intent_id: &str) -> Result<(), InternalError> {
        let url = self
            .api_base
            .join(&format!("internal/mcp/delete-intents/{}/commit", intent_id))
            .map_err(|e| InternalError::Connection(format!("failed to join URL: {}", e)))?;
        let resp = self
            .http_client
            .post(url.as_str())
            .header("x-mcp-api-key", &self.api_key)
            .send()
            .await
            .map_err(|e| InternalError::Connection(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(InternalError::Http(
                resp.status().as_u16(),
                "commit_delete_intent failed".to_string(),
            ));
        }

        Ok(())
    }

    pub async fn create_reminder(
        &self,
        payload: &serde_json::Value,
    ) -> Result<ReminderResponse, InternalError> {
        let mut url = self.api_base.clone();
        url.path_segments_mut()
            .map_err(|_| InternalError::Connection("api_base has impossible host".to_string()))?
            .extend(&["internal", "mcp", "reminders"]);
        url.set_query(Some(&format!("calendar_id={}", payload["calendar_id"])));

        let resp = self
            .http_client
            .post(url.as_str())
            .header("Content-Type", "application/json")
            .header("x-mcp-api-key", &self.api_key)
            .body(payload.to_string())
            .send()
            .await
            .map_err(|e| InternalError::Connection(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(InternalError::Http(
                resp.status().as_u16(),
                "create_reminder failed".to_string(),
            ));
        }

        let body: String = resp
            .text()
            .await
            .map_err(|e| InternalError::Deserialize(e.to_string()))?;

        serde_json::from_str(&body)
            .map_err(|e| InternalError::Deserialize(format!("reminder response: {}", e)))
    }

    pub async fn get_mcp_grants(
        &self,
        user_id: i64,
        client_id: &str,
    ) -> Result<Vec<McpGrantResponse>, InternalError> {
        let mut url = self
            .api_base
            .join("internal/mcp/mcp-grants")
            .map_err(|e| InternalError::Connection(format!("failed to join URL: {}", e)))?;
        url.query_pairs_mut()
            .append_pair("user_id", &user_id.to_string())
            .append_pair("client_id", client_id);
        let resp = self
            .http_client
            .get(url.as_str())
            .header("x-mcp-api-key", &self.api_key)
            .send()
            .await
            .map_err(|e| InternalError::Connection(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(InternalError::Http(
                resp.status().as_u16(),
                "get_mcp_grants failed".to_string(),
            ));
        }

        let body = resp
            .json::<Vec<McpGrantResponse>>()
            .await
            .map_err(|e| InternalError::Deserialize(e.to_string()))?;

        Ok(body)
    }

    pub async fn check_idempotency(
        &self,
        operation_id: &str,
    ) -> Result<Option<serde_json::Value>, InternalError> {
        let url = self
            .api_base
            .join(&format!("internal/mcp/idempotency/{}", operation_id))
            .map_err(|e| InternalError::Connection(format!("failed to join URL: {}", e)))?;
        let resp = self
            .http_client
            .get(url.as_str())
            .header("x-mcp-api-key", &self.api_key)
            .send()
            .await
            .map_err(|e| InternalError::Connection(e.to_string()))?;

        if resp.status() == 404 {
            return Ok(None);
        }

        if !resp.status().is_success() {
            return Err(InternalError::Http(
                resp.status().as_u16(),
                "check_idempotency failed".to_string(),
            ));
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| InternalError::Deserialize(e.to_string()))?;

        Ok(Some(body))
    }

    pub async fn record_idempotency(
        &self,
        operation_id: &str,
        payload: &serde_json::Value,
    ) -> Result<(), InternalError> {
        let url = self
            .api_base
            .join("internal/mcp/idempotency")
            .map_err(|e| InternalError::Connection(format!("failed to join URL: {}", e)))?;
        let resp = self
            .http_client
            .post(url.as_str())
            .header("x-mcp-api-key", &self.api_key)
            .json(&serde_json::json!({
                "operation_id": operation_id,
                "payload": payload,
            }))
            .send()
            .await
            .map_err(|e| InternalError::Connection(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(InternalError::Http(
                resp.status().as_u16(),
                "record_idempotency failed".to_string(),
            ));
        }

        Ok(())
    }
}

#[derive(Debug)]
pub enum InternalError {
    Connection(String),
    Http(u16, String),
    Deserialize(String),
}

impl std::fmt::Display for InternalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connection(msg) => write!(f, "connection error: {}", msg),
            Self::Http(code, msg) => write!(f, "http {}: {}", code, msg),
            Self::Deserialize(msg) => write!(f, "deserialize error: {}", msg),
        }
    }
}

impl std::error::Error for InternalError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_client_has_expected_fields() {
        let client = InternalClient::new(
            "https://api.example.com".to_string(),
            "test-key".to_string(),
        )
        .unwrap();
        assert_eq!(client.api_base(), "https://api.example.com/");
        assert_eq!(client.api_key(), "test-key");
    }

    #[test]
    fn api_base_returns_correct_value() {
        let client = InternalClient::new(
            "https://internal.commoncal.tld".to_string(),
            "key123".to_string(),
        )
        .unwrap();
        assert_eq!(client.api_base(), "https://internal.commoncal.tld/");
    }

    #[test]
    fn api_key_returns_correct_value() {
        let client = InternalClient::new(
            "https://api.example.com".to_string(),
            "super-secret-key".to_string(),
        )
        .unwrap();
        assert_eq!(client.api_key(), "super-secret-key");
    }

    #[test]
    fn clone_does_not_share_http_client() {
        let client1 =
            InternalClient::new("https://api.example.com".to_string(), "key".to_string()).unwrap();
        let client2 = client1.clone();
        // Clone should create independent copies (reqwest::Client is clonable)
        assert_eq!(client1.api_base(), client2.api_base());
        assert_eq!(client1.api_key(), client2.api_key());
    }

    #[test]
    fn user_status_response_deserializes() {
        let json = r#"{"user_id": 42, "status": "active"}"#;
        let resp: UserStatusResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.user_id, 42);
        assert_eq!(resp.status, "active");
    }

    #[test]
    fn calendar_info_deserializes() {
        let json = r#"{"id": 1, "name": "Work", "role": "owner", "access": "full"}"#;
        let resp: CalendarInfo = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, 1);
        assert_eq!(resp.name, "Work");
        assert_eq!(resp.role, "owner");
        assert_eq!(resp.access, "full");
    }

    #[test]
    fn event_info_deserializes_with_all_fields() {
        let json = r#"{"id": 100, "calendar_id": 1, "title": "Meeting", "description": "Discuss Q1", "location": "Room 5", "status": "confirmed", "event_kind": "default", "start_utc": 1700000000, "end_utc": 1700003600, "version": 3}"#;
        let resp: EventInfo = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, 100);
        assert_eq!(resp.calendar_id, 1);
        assert_eq!(resp.title, Some("Meeting".to_string()));
        assert_eq!(resp.description, Some("Discuss Q1".to_string()));
        assert_eq!(resp.location, Some("Room 5".to_string()));
        assert_eq!(resp.status, "confirmed");
        assert_eq!(resp.event_kind, "default");
        assert_eq!(resp.start_utc, Some(1700000000));
        assert_eq!(resp.end_utc, Some(1700003600));
        assert_eq!(resp.version, Some(3));
    }

    #[test]
    fn event_info_deserializes_with_null_fields() {
        let json = r#"{"id": 101, "calendar_id": 1, "title": null, "description": null, "location": null, "status": "cancelled", "event_kind": "default", "start_utc": null, "end_utc": null, "version": null}"#;
        let resp: EventInfo = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, 101);
        assert_eq!(resp.title, None);
        assert_eq!(resp.description, None);
        assert_eq!(resp.location, None);
        assert_eq!(resp.status, "cancelled");
        assert_eq!(resp.start_utc, None);
        assert_eq!(resp.end_utc, None);
        assert_eq!(resp.version, None);
    }

    #[test]
    fn delete_intent_deserializes() {
        let json = r#"{"intent_id": "abc-123", "event_id": 100, "calendar_id": 1, "event_version": 5, "confirmation_state": "pending", "expires_at": 1700100000}"#;
        let resp: DeleteIntent = serde_json::from_str(json).unwrap();
        assert_eq!(resp.intent_id, "abc-123");
        assert_eq!(resp.event_id, 100);
        assert_eq!(resp.calendar_id, 1);
        assert_eq!(resp.event_version, 5);
        assert_eq!(resp.confirmation_state, "pending");
        assert_eq!(resp.expires_at, 1700100000);
    }

    #[test]
    fn mcp_grant_response_deserializes() {
        let json = r#"{"grant_id": "grant-1", "user_id": 42, "oauth_client_id": "client-abc", "allowed_calendar_ids": [1, 2, 3], "allow_availability": true, "allow_event_titles": true, "allow_event_details": false, "allow_create": true, "allow_update": false, "allow_delete": true, "created_at": 1699999999, "last_used_at": 1700000000, "expires_at": 1700600000, "revoked_at": null}"#;
        let resp: McpGrantResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.grant_id, "grant-1");
        assert_eq!(resp.user_id, 42);
        assert_eq!(resp.oauth_client_id, "client-abc");
        assert_eq!(resp.allowed_calendar_ids, vec![1i64, 2, 3]);
        assert!(resp.allow_availability);
        assert!(resp.allow_event_titles);
        assert!(!resp.allow_event_details);
        assert!(resp.allow_create);
        assert!(!resp.allow_update);
        assert!(resp.allow_delete);
        assert_eq!(resp.created_at, 1699999999);
        assert_eq!(resp.last_used_at, Some(1700000000));
        assert_eq!(resp.expires_at, Some(1700600000));
        assert_eq!(resp.revoked_at, None);
    }

    #[test]
    fn token_exchange_response_deserializes() {
        let json = r#"{"access_token": "internal-token-xyz", "token_type": "Bearer", "expires_in": 300, "scope": "commoncal.event.read.commoncal.event.create"}"#;
        let resp: TokenExchangeResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.access_token, "internal-token-xyz");
        assert_eq!(resp.token_type, "Bearer");
        assert_eq!(resp.expires_in, 300);
        assert_eq!(resp.scope, "commoncal.event.read.commoncal.event.create");
    }

    #[test]
    fn internal_error_display_connection() {
        let err = InternalError::Connection("connection refused".to_string());
        assert_eq!(format!("{}", err), "connection error: connection refused");
    }

    #[test]
    fn internal_error_display_http() {
        let err = InternalError::Http(503, "service unavailable".to_string());
        assert_eq!(format!("{}", err), "http 503: service unavailable");
    }

    #[test]
    fn internal_error_display_deserialize() {
        let err = InternalError::Deserialize("invalid json".to_string());
        assert_eq!(format!("{}", err), "deserialize error: invalid json");
    }
}
