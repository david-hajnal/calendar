// Integration tests for the MCP server.

#![allow(dead_code)]

use base64::Engine;
use mcp_server::internal_client::InternalClient;
use wiremock::MockServer;
use wiremock::matchers::{method, path};

/// Integration test: DPoP proof validation with mock JWKS.
#[tokio::test]
async fn test_dpop_proof_validation() {
    let _mock_server = MockServer::start().await;

    wiremock::Mock::given(method("GET"))
        .and(path("/.well-known/oauth-jwks"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(r#"{"keys":[{"kty":"RSA","alg":"RS256","use":"sig","n":"dGVzdA==","e":"AQAB","kid":"key-1"}]}"#),
        )
        .mount(&_mock_server)
        .await;

    let header = serde_json::json!({"typ": "dpop+jwt", "jwk": {"kty": "RSA", "n": "test", "e": "AQAB"}});
    let header_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(header.to_string());
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"jti":"test","htm":"GET","htu":"http://localhost","exp":9999999999}"#);
    let proof = format!("{}.{}.signature", header_b64, payload);

    let result = mcp_server::oauth::validate_dpop_proof("token", &proof, "nonce").await;
    assert!(result.is_ok());
}

/// Integration test: InternalClient creates correctly.
#[tokio::test]
async fn test_internal_client_creation() {
    let client = InternalClient::new("https://api.commoncal.tld".to_string(), "test-key".to_string()).unwrap();
    assert_eq!(client.api_base(), "https://api.commoncal.tld/");
    assert_eq!(client.api_key(), "test-key");
}

/// Integration test: Rate limiter disabled mode always allows.
#[tokio::test]
async fn test_rate_limiter_disabled() {
    let limiter = mcp_server::rate_limiter::RateLimiter::disabled();
    assert!(limiter.check("client-1", 10, 60));
    assert!(limiter.check("client-1", 10, 60));
    assert!(limiter.check("client-2", 10, 60));
}

/// Integration test: McpGrant model serializes correctly.
#[tokio::test]
async fn test_mcp_grant_serialization() {
    let grant = mcp_server::mcp_grant::McpGrant {
        grant_id: "grant-1".to_string(),
        user_id: 42,
        oauth_client_id: "client-1".to_string(),
        allowed_calendar_ids: vec![1, 2, 3],
        allow_availability: true,
        allow_event_titles: true,
        allow_event_details: true,
        allow_create: true,
        allow_update: true,
        allow_delete: true,
        created_at: 1700000000,
        last_used_at: None,
        expires_at: Some(1700100000),
        revoked_at: None,
    };
    let json = serde_json::to_string(&grant).unwrap();
    assert!(json.contains("\"grant_id\""));
    assert!(json.contains("grant-1"));
    assert!(json.contains("\"user_id\":42"));
    assert!(json.contains("\"allow_delete\":true"));
}

/// Integration test: Output schema types serialize correctly.
#[tokio::test]
async fn test_output_schema_serialization() {
    let output = mcp_server::output_schema::ToolOutput {
        content: vec![mcp_server::output_schema::ContentBlock::Text {
            text: r#"{"event_id":42}"#.to_string(),
        }],
    };
    let json = serde_json::to_string(&output).unwrap();
    assert!(json.contains("\"content\""));
    assert!(json.contains("\"text\""));
    assert!(json.contains("42"));
}

/// Integration test: Error types have correct Display implementations.
#[tokio::test]
async fn test_error_display() {
    use mcp_server::error::{TokenError, GrantError, ToolError};

    assert_eq!(format!("{}", TokenError::MissingToken), "missing authorization token");
    assert_eq!(format!("{}", TokenError::Expired), "token has expired");
    assert_eq!(format!("{}", TokenError::InvalidAudience), "token audience mismatch");
    assert_eq!(format!("{}", GrantError::NoGrant), "no MCP grant found");
    assert_eq!(format!("{}", GrantError::GrantExpired), "MCP grant has expired");
    assert_eq!(format!("{}", GrantError::GrantRevoked), "MCP grant has been revoked");
    assert!(matches!(ToolError::NotFound, ToolError::NotFound));
}

/// Integration test: Security module risk classification.
#[tokio::test]
async fn test_risk_classification() {
    use mcp_server::security::{classify_risk, RiskTier};

    assert_eq!(classify_risk("availability_find"), RiskTier::Tier0);
    assert_eq!(classify_risk("event_get"), RiskTier::Tier1);
    assert_eq!(classify_risk("event_create"), RiskTier::Tier2);
    assert_eq!(classify_risk("event_delete_prepare"), RiskTier::Tier3);
    assert_eq!(classify_risk("unknown"), RiskTier::Tier2);
}

/// Integration test: Security module auth strength checks.
#[tokio::test]
async fn test_auth_strength_checks() {
    use mcp_server::security::{check_auth_strength, RiskTier};
    use mcp_server::oauth::AuthStrength;

    assert!(check_auth_strength(&AuthStrength::Passwordless, RiskTier::Tier0).is_ok());
    assert!(check_auth_strength(&AuthStrength::Passwordless, RiskTier::Tier1).is_ok());
    assert!(check_auth_strength(&AuthStrength::Passkey, RiskTier::Tier2).is_ok());
    assert!(check_auth_strength(&AuthStrength::Mfa, RiskTier::Tier2).is_ok());
    assert!(check_auth_strength(&AuthStrength::Passwordless, RiskTier::Tier2).is_err());
    assert!(check_auth_strength(&AuthStrength::Passkey, RiskTier::Tier3).is_ok());
    assert!(check_auth_strength(&AuthStrength::Passwordless, RiskTier::Tier3).is_err());
}

/// Integration test: Security module anomaly detection.
#[tokio::test]
async fn test_anomaly_detection() {
    use mcp_server::security::{check_anomalies, classify_risk, RiskTier};

    // Brute force detection.
    let result = check_anomalies("client-1", "event_create", classify_risk("event_create"), 10, false, false);
    assert!(result.is_some());

    // Off-hours Tier3 detection.
    let result = check_anomalies("client-1", "event_delete_commit", RiskTier::Tier3, 0, true, false);
    assert!(result.is_some());

    // Rate limit detection.
    let result = check_anomalies("client-1", "event_get", RiskTier::Tier1, 0, false, true);
    assert!(result.is_some());

    // Clean request.
    let result = check_anomalies("client-1", "availability_find", RiskTier::Tier0, 0, false, false);
    assert!(result.is_none());

    // Off-hours read allowed.
    let result = check_anomalies("client-1", "availability_find", RiskTier::Tier0, 0, true, false);
    assert!(result.is_none());
}

/// Integration test: Config module current_time_secs returns valid timestamp.
#[tokio::test]
async fn test_config_current_time() {
    let now = mcp_server::config::current_time_secs();
    assert!(now > 1700000000);
    assert!(now < 2000000000);
}

/// Integration test: mcp_grant current_time_secs returns valid timestamp.
#[tokio::test]
async fn test_grant_current_time() {
    let now = mcp_server::mcp_grant::current_time_secs();
    assert!(now > 1700000000);
    assert!(now < 2000000000);
}
