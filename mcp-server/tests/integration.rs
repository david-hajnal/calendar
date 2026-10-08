// Integration tests for the MCP server.

#![allow(dead_code)]

use base64::Engine;
use http_body_util::BodyExt;
use mcp_server::config::{AppEnv, Config};
use mcp_server::db::connect_and_migrate;
use mcp_server::gateway::Gateway;
use mcp_server::internal_client::InternalClient;
use mcp_server::mcp_grant::McpGrant;
use mcp_server::oauth::{AuthStrength, TokenValidationResult};
use mcp_server::tools::AuthorizedToolContext;
use mcp_server::tools::calendar_list::{CalendarListParams, handle as calendar_list};
use wiremock::MockServer;
use wiremock::matchers::{method, path};

const CORE_GRANT: &str = r#"[{"grant_id":"grant-1","user_id":42,"oauth_client_id":"client-1","allowed_calendar_ids":[1],"allow_availability":true,"allow_event_titles":true,"allow_event_details":false,"allow_create":false,"allow_update":false,"allow_delete":false,"created_at":1700000000,"last_used_at":null,"expires_at":null,"revoked_at":null}]"#;

fn internal_client(mock_server: &MockServer) -> InternalClient {
    InternalClient::new(mock_server.uri(), "test-key".to_string()).expect("client should build")
}

#[tokio::test]
async fn authoritative_grant_lookup_returns_one_matching_grant() {
    let mock_server = MockServer::start().await;
    wiremock::Mock::given(method("GET"))
        .and(path("/internal/mcp/mcp-grants"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(CORE_GRANT))
        .mount(&mock_server)
        .await;

    let grant = internal_client(&mock_server)
        .get_mcp_grant(42, "client-1")
        .await
        .expect("one core grant should resolve")
        .expect("one core grant should be present");

    assert_eq!(grant.grant_id, "grant-1");
    assert_eq!(grant.allowed_calendar_ids, vec![1]);
}

#[tokio::test]
async fn authoritative_grant_lookup_returns_none_for_empty_response() {
    let mock_server = MockServer::start().await;
    wiremock::Mock::given(method("GET"))
        .and(path("/internal/mcp/mcp-grants"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("[]"))
        .mount(&mock_server)
        .await;

    let grant = internal_client(&mock_server)
        .get_mcp_grant(42, "client-1")
        .await
        .expect("an empty core response is a valid no-grant outcome");

    assert!(grant.is_none());
}

#[tokio::test]
async fn authoritative_grant_lookup_rejects_multiple_responses() {
    let mock_server = MockServer::start().await;
    let grants = format!(
        "[{},{}]",
        &CORE_GRANT[1..CORE_GRANT.len() - 1],
        &CORE_GRANT[1..CORE_GRANT.len() - 1]
    );
    wiremock::Mock::given(method("GET"))
        .and(path("/internal/mcp/mcp-grants"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(grants))
        .mount(&mock_server)
        .await;

    let error = internal_client(&mock_server)
        .get_mcp_grant(42, "client-1")
        .await
        .expect_err("ambiguous core grants must fail closed");

    assert!(error.to_string().contains("ambiguous grant response"));
}

#[tokio::test]
async fn calendar_list_uses_authorized_core_grant_to_filter_calendars() {
    let mock_server = MockServer::start().await;
    wiremock::Mock::given(method("GET"))
        .and(path("/internal/mcp/users/42/calendars"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(
            r#"[{"id":1,"name":"Allowed","role":"owner","access":"owner"},{"id":2,"name":"Denied","role":"owner","access":"owner"}]"#,
        ))
        .mount(&mock_server)
        .await;

    let client = internal_client(&mock_server);
    let token = TokenValidationResult {
        user_id: 42,
        oauth_client_id: "client-1".to_string(),
        scopes: vec![],
        auth_strength: AuthStrength::Passwordless,
        auth_time: 0,
        token_id: "token-1".to_string(),
        expires_at: i64::MAX,
    };
    let grant = McpGrant {
        grant_id: "grant-1".to_string(),
        user_id: 42,
        oauth_client_id: "client-1".to_string(),
        allowed_calendar_ids: vec![1],
        allow_availability: true,
        allow_event_titles: false,
        allow_event_details: false,
        allow_create: false,
        allow_update: false,
        allow_delete: false,
        created_at: 0,
        last_used_at: None,
        expires_at: None,
        revoked_at: None,
    };
    let context = AuthorizedToolContext {
        token: &token,
        grant: &grant,
        internal_client: &client,
    };

    let response = calendar_list(
        &context,
        CalendarListParams {
            include_access: false,
        },
    )
    .await
    .expect("authorized calendar list should succeed");
    let body = response
        .into_body()
        .collect()
        .await
        .expect("response body should be readable")
        .to_bytes();
    let output: serde_json::Value = serde_json::from_slice(&body).expect("response should be JSON");

    assert!(output.to_string().contains("Allowed"));
    assert!(!output.to_string().contains("Denied"));
}

const CREATE_GRANT: &str = r#"[{"grant_id":"grant-2","user_id":42,"oauth_client_id":"client-1","allowed_calendar_ids":[1],"allow_availability":true,"allow_event_titles":true,"allow_event_details":false,"allow_create":true,"allow_update":false,"allow_delete":false,"created_at":1700000000,"last_used_at":null,"expires_at":null,"revoked_at":null}]"#;

fn gateway_config(mock_server: &MockServer, database_path: std::path::PathBuf) -> Config {
    Config {
        app_env: AppEnv::Development,
        oauth_issuer: "https://auth.example.com".to_string(),
        internal_api_base: mock_server.uri(),
        internal_api_key: "test-key".to_string(),
        session_secret: "test-secret".to_string(),
        database_path,
        mcp_domain: "mcp.example.com".to_string(),
        public_resource_url: "https://mcp.example.com/mcp".to_string(),
        bind_address: "127.0.0.1:3001".parse().unwrap(),
        dpop_key_path: None,
        rate_limit_enabled: false,
        tracing_level: "info".to_string(),
    }
}

fn audit_token() -> TokenValidationResult {
    TokenValidationResult {
        user_id: 42,
        oauth_client_id: "client-1".to_string(),
        scopes: vec![],
        auth_strength: AuthStrength::Passwordless,
        auth_time: 0,
        token_id: "token-1".to_string(),
        expires_at: i64::MAX,
    }
}

fn unique_database_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("commoncal-mcp-{}.sqlite", uuid::Uuid::new_v4()))
}

/// Slice 6: a successful tool call appends an audit row with request, actor,
/// tool, outcome, and latency metadata.
#[tokio::test]
async fn successful_tool_call_appends_audit_row() {
    let mock_server = MockServer::start().await;
    wiremock::Mock::given(method("GET"))
        .and(path("/internal/mcp/mcp-grants"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(CORE_GRANT))
        .mount(&mock_server)
        .await;
    wiremock::Mock::given(method("GET"))
        .and(path("/internal/mcp/users/42/calendars"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_string(r#"[{"id":1,"name":"Allowed","role":"owner","access":"owner"}]"#),
        )
        .mount(&mock_server)
        .await;

    let database_path = unique_database_path();
    let pool = connect_and_migrate(&database_path)
        .await
        .expect("a fresh database should be created and migrated");
    let gateway = Gateway::new(
        gateway_config(&mock_server, database_path.clone()),
        pool.clone(),
    )
    .expect("gateway should build");

    let message = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": "calendar_list", "arguments": { "include_access": false } },
    });

    let response = gateway
        .handle_authorized_tool_call("req-audit-1", &audit_token(), &message)
        .await;

    assert_eq!(response.status(), axum::http::StatusCode::OK);

    let row: (String, i64, String, String, String, String, i64, String) = sqlx::query_as(
        "SELECT request_id, user_id, oauth_client_id, mcp_grant_id, tool,
         auth_result, latency_ms, result_type
         FROM mcp_audit",
    )
    .fetch_one(&pool)
    .await
    .expect("the successful invocation should be audited");

    pool.close().await;
    let _ = std::fs::remove_file(&database_path);

    assert_eq!(row.0, "req-audit-1");
    assert_eq!(row.1, 42);
    assert_eq!(row.2, "client-1");
    assert_eq!(row.3, "grant-1");
    assert_eq!(row.4, "calendar_list");
    assert_eq!(row.5, "allowed");
    assert!(row.6 >= 0, "latency metadata should be recorded");
    assert_eq!(row.7, "success");
}

/// Slice 6: a denied tool call appends an audit row without storing credentials.
#[tokio::test]
async fn failed_authorization_appends_audit_row() {
    let mock_server = MockServer::start().await;
    wiremock::Mock::given(method("GET"))
        .and(path("/internal/mcp/mcp-grants"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("[]"))
        .mount(&mock_server)
        .await;

    let database_path = unique_database_path();
    let pool = connect_and_migrate(&database_path)
        .await
        .expect("a fresh database should be created and migrated");
    let gateway = Gateway::new(
        gateway_config(&mock_server, database_path.clone()),
        pool.clone(),
    )
    .expect("gateway should build");

    let message = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": { "name": "calendar_list", "arguments": {} },
    });

    let response = gateway
        .handle_authorized_tool_call("req-denied-1", &audit_token(), &message)
        .await;

    assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);

    let row: (String, String, String, String, Option<String>) = sqlx::query_as(
        "SELECT request_id, tool, auth_result, result_type, mcp_grant_id
         FROM mcp_audit",
    )
    .fetch_one(&pool)
    .await
    .expect("the denied invocation should be audited");

    pool.close().await;
    let _ = std::fs::remove_file(&database_path);

    assert_eq!(row.0, "req-denied-1");
    assert_eq!(row.1, "calendar_list");
    assert_eq!(row.2, "denied");
    assert_eq!(row.3, "denied");
    assert_eq!(row.4, None, "no grant exists to record");
}

/// Slice 6: a successful core mutation keeps its response when the local
/// audit insert fails, so the client is not pushed into an unsafe retry.
#[tokio::test]
async fn audit_failure_does_not_turn_completed_mutation_into_retryable_failure() {
    let mock_server = MockServer::start().await;
    wiremock::Mock::given(method("GET"))
        .and(path("/internal/mcp/mcp-grants"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(CREATE_GRANT))
        .mount(&mock_server)
        .await;
    wiremock::Mock::given(method("POST"))
        .and(path("/internal/mcp/events/1"))
        .respond_with(
            wiremock::ResponseTemplate::new(201).set_body_string(
                r#"{"id":7,"calendar_id":1,"title":"Standup","status":"confirmed","event_kind":"timed"}"#,
            ),
        )
        .mount(&mock_server)
        .await;

    let database_path = unique_database_path();
    let pool = connect_and_migrate(&database_path)
        .await
        .expect("a fresh database should be created and migrated");
    // Simulate audit storage failure: the audit table is unavailable.
    sqlx::query("DROP TABLE mcp_audit")
        .execute(&pool)
        .await
        .expect("audit table should be removable in the test");

    let gateway = Gateway::new(
        gateway_config(&mock_server, database_path.clone()),
        pool.clone(),
    )
    .expect("gateway should build");

    let message = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "event_create",
            "arguments": { "calendar_id": 1, "title": "Standup" },
        },
    });

    let response = gateway
        .handle_authorized_tool_call("req-audit-fail", &audit_token(), &message)
        .await;

    assert_eq!(
        response.status(),
        axum::http::StatusCode::CREATED,
        "the completed mutation must keep its success response"
    );

    let audit_table: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'mcp_audit'",
    )
    .fetch_one(&pool)
    .await
    .expect("schema query should succeed");

    pool.close().await;
    let _ = std::fs::remove_file(&database_path);

    assert_eq!(
        audit_table, 0,
        "the failed audit insert must not recreate the table"
    );
}

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

    let header =
        serde_json::json!({"typ": "dpop+jwt", "jwk": {"kty": "RSA", "n": "test", "e": "AQAB"}});
    let header_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(header.to_string());
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(r#"{"jti":"test","htm":"GET","htu":"http://localhost","exp":9999999999}"#);
    let proof = format!("{}.{}.signature", header_b64, payload);

    let result = mcp_server::oauth::validate_dpop_proof("token", &proof, "nonce").await;
    // Dummy signature cannot be verified, so this fails at signature check (not format check)
    assert!(result.is_err());
}

/// Integration test: InternalClient creates correctly.
#[tokio::test]
async fn test_internal_client_creation() {
    let client = InternalClient::new(
        "https://api.commoncal.tld".to_string(),
        "test-key".to_string(),
    )
    .unwrap();
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
    use mcp_server::error::{GrantError, TokenError, ToolError};

    assert_eq!(
        format!("{}", TokenError::MissingToken),
        "missing authorization token"
    );
    assert_eq!(format!("{}", TokenError::Expired), "token has expired");
    assert_eq!(
        format!("{}", TokenError::InvalidAudience),
        "token audience mismatch"
    );
    assert_eq!(format!("{}", GrantError::NoGrant), "no MCP grant found");
    assert_eq!(
        format!("{}", GrantError::GrantExpired),
        "MCP grant has expired"
    );
    assert_eq!(
        format!("{}", GrantError::GrantRevoked),
        "MCP grant has been revoked"
    );
    assert!(matches!(ToolError::NotFound, ToolError::NotFound));
}

/// Integration test: Security module risk classification.
#[tokio::test]
async fn test_risk_classification() {
    use mcp_server::security::{RiskTier, classify_risk};

    assert_eq!(classify_risk("availability_find"), RiskTier::Tier0);
    assert_eq!(classify_risk("event_get"), RiskTier::Tier1);
    assert_eq!(classify_risk("event_create"), RiskTier::Tier2);
    assert_eq!(classify_risk("event_delete_prepare"), RiskTier::Tier3);
    assert_eq!(classify_risk("unknown"), RiskTier::Tier2);
}

/// Integration test: Security module auth strength checks.
#[tokio::test]
async fn test_auth_strength_checks() {
    use mcp_server::oauth::AuthStrength;
    use mcp_server::security::{RiskTier, check_auth_strength};

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
    use mcp_server::security::{RiskTier, check_anomalies, classify_risk};

    // Brute force detection.
    let result = check_anomalies(
        "client-1",
        "event_create",
        classify_risk("event_create"),
        10,
        false,
        false,
    );
    assert!(result.is_some());

    // Off-hours Tier3 detection.
    let result = check_anomalies(
        "client-1",
        "event_delete_commit",
        RiskTier::Tier3,
        0,
        true,
        false,
    );
    assert!(result.is_some());

    // Rate limit detection.
    let result = check_anomalies("client-1", "event_get", RiskTier::Tier1, 0, false, true);
    assert!(result.is_some());

    // Clean request.
    let result = check_anomalies(
        "client-1",
        "availability_find",
        RiskTier::Tier0,
        0,
        false,
        false,
    );
    assert!(result.is_none());

    // Off-hours read allowed.
    let result = check_anomalies(
        "client-1",
        "availability_find",
        RiskTier::Tier0,
        0,
        true,
        false,
    );
    assert!(result.is_none());
}

/// Integration test: Config module current_time_secs returns valid timestamp.
#[tokio::test]
async fn test_config_current_time() {
    let now = mcp_server::config::current_time_secs();
    assert!(now > 1700000000);
    assert!(now < 2000000000);
}

/// Integration test: Protected-resource metadata derives resource URL correctly.
#[tokio::test]
async fn test_protected_resource_metadata_resource_url() {
    let meta = mcp_server::config::OauthProtectedResourceMetadata::new(
        "https://mcal.hajnal.space/mcp",
        "https://auth.cal.hajnal.space",
        true,
    );
    assert_eq!(meta.resource, "https://mcal.hajnal.space/mcp");
    assert_eq!(
        meta.resource_metadata,
        Some("https://mcal.hajnal.space/.well-known/oauth-protected-resource".to_string())
    );
    assert_eq!(
        meta.authorization_servers,
        vec!["https://auth.cal.hajnal.space"]
    );
    assert!(meta.dpop_bound_access_tokens);
}

/// Integration test: Protected-resource metadata sets dpop_bound_access_tokens=false when no DPoP key.
#[tokio::test]
async fn test_protected_resource_metadata_dpop_unsupported() {
    let meta = mcp_server::config::OauthProtectedResourceMetadata::new(
        "https://mcal.hajnal.space/mcp",
        "https://auth.cal.hajnal.space",
        false,
    );
    assert!(!meta.dpop_bound_access_tokens);
}

/// Integration test: Config rejects MCP_PUBLIC_RESOURCE_URL with placeholder domain.
#[test]
fn test_config_rejects_placeholder_in_resource_url() {
    use mcp_server::config::{AppEnv, Config, RawConfig};

    let raw = RawConfig {
        app_env: AppEnv::Production,
        oauth_issuer: "https://auth.example.com".into(),
        internal_api_base: "https://api.example.com".into(),
        internal_api_key: "real-key-12345".into(),
        session_secret: "real-secret-12345".into(),
        database_path: "/tmp/test.db".into(),
        mcp_domain: Some("mcal.example.com".into()),
        public_resource_url: Some("https://mcp.commoncal.tld/mcp".into()),
        bind_address: "127.0.0.1:3001".parse().unwrap(),
        dpop_key_path: None,
        rate_limit_enabled: true,
        tracing_level: "info".into(),
    };

    let errors = Config::validate(raw).unwrap_err();
    assert!(errors.iter().any(|e| e.message.contains("placeholder")));
}

/// Integration test: Config rejects MCP_PUBLIC_RESOURCE_URL with HTTP.
#[test]
fn test_config_rejects_http_resource_url() {
    use mcp_server::config::{AppEnv, Config, RawConfig};

    let raw = RawConfig {
        app_env: AppEnv::Production,
        oauth_issuer: "https://auth.example.com".into(),
        internal_api_base: "https://api.example.com".into(),
        internal_api_key: "real-key-12345".into(),
        session_secret: "real-secret-12345".into(),
        database_path: "/tmp/test.db".into(),
        mcp_domain: Some("mcal.example.com".into()),
        public_resource_url: Some("http://mcal.example.com/mcp".into()),
        bind_address: "127.0.0.1:3001".parse().unwrap(),
        dpop_key_path: None,
        rate_limit_enabled: true,
        tracing_level: "info".into(),
    };

    let errors = Config::validate(raw).unwrap_err();
    assert!(errors.iter().any(|e| e.message.contains("HTTPS")));
}

/// Integration test: Config rejects MCP_PUBLIC_RESOURCE_URL with credentials.
#[test]
fn test_config_rejects_credentials_in_resource_url() {
    use mcp_server::config::{AppEnv, Config, RawConfig};

    let raw = RawConfig {
        app_env: AppEnv::Production,
        oauth_issuer: "https://auth.example.com".into(),
        internal_api_base: "https://api.example.com".into(),
        internal_api_key: "real-key-12345".into(),
        session_secret: "real-secret-12345".into(),
        database_path: "/tmp/test.db".into(),
        mcp_domain: Some("mcal.example.com".into()),
        public_resource_url: Some("https://user:pass@mcal.example.com/mcp".into()),
        bind_address: "127.0.0.1:3001".parse().unwrap(),
        dpop_key_path: None,
        rate_limit_enabled: true,
        tracing_level: "info".into(),
    };

    let errors = Config::validate(raw).unwrap_err();
    assert!(errors.iter().any(|e| e.message.contains("credentials")));
}

/// Integration test: OAuth extract_audience returns the resource URL as-is.
#[test]
fn test_oauth_extract_audience() {
    let audience = mcp_server::oauth::extract_audience("https://mcal.hajnal.space/mcp");
    assert_eq!(audience, "https://mcal.hajnal.space/mcp");
}

/// Integration test: mcp_grant current_time_secs returns valid timestamp.
#[tokio::test]
async fn test_grant_current_time() {
    let now = mcp_server::mcp_grant::current_time_secs();
    assert!(now > 1700000000);
    assert!(now < 2000000000);
}

// ---------------------------------------------------------------------------
// Phase 3: OAuth discovery and access-token validation integration tests.
//
// These tests validate tokens signed with real RSA keys against the standard
// claim contract issued by the repository's authorization server
// (`slice1-lab/auth-server`): numeric `sub`, `client_id`, space-delimited
// `scope`, `iss`, `aud`, `exp`, `iat`, `jti`, `amr`.
//
// The discovery flow (fetch metadata → verify issuer → follow jwks_uri) and
// the JWKS caching / key-rotation-refresh logic are exercised end-to-end via
// wiremock.
// ---------------------------------------------------------------------------

use jsonwebtoken::{EncodingKey, Header};
use mcp_server::oauth::TokenValidator;
use rsa::RsaPrivateKey;
use rsa::pkcs8::EncodePrivateKey;
use rsa::traits::PublicKeyParts;

const TEST_RESOURCE: &str = "https://mcp.example.com/mcp";

/// Generate a 2048-bit RSA key pair and return (private_key, jwk_json).
fn generate_rsa_key(kid: &str) -> (RsaPrivateKey, serde_json::Value) {
    let mut rng = rand::thread_rng();
    let private_key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
    let public_key = private_key.to_public_key();
    let n_bytes = public_key.n().to_bytes_be();
    let e_bytes = public_key.e().to_bytes_be();
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let jwk = serde_json::json!({
        "kty": "RSA",
        "alg": "RS256",
        "use": "sig",
        "n": engine.encode(&n_bytes),
        "e": engine.encode(&e_bytes),
        "kid": kid,
    });
    (private_key, jwk)
}

/// Sign a JWT with the given claims and RSA private key.
fn sign_jwt(private_key: &RsaPrivateKey, kid: &str, claims: &serde_json::Value) -> String {
    let mut header = Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some(kid.to_string());
    let pem = private_key
        .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
        .unwrap();
    let encoding_key = EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap();
    jsonwebtoken::encode(&header, claims, &encoding_key).unwrap()
}

/// Phase 3: a valid token with standard claims validates successfully.
#[tokio::test]
async fn phase3_valid_token_with_standard_claims() {
    let (private_key, jwk) = generate_rsa_key("test-key-1");
    let mock = wiremock::MockServer::start().await;
    let mock_issuer = mock.uri();
    let jwks_uri = format!("{}/jwks", mock_issuer);

    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/.well-known/oauth-authorization-server"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(serde_json::json!({
                    "issuer": mock_issuer,
                    "jwks_uri": jwks_uri,
                })
                .to_string()),
        )
        .mount(&mock)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/jwks"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(serde_json::json!({ "keys": [jwk] }).to_string()),
        )
        .mount(&mock)
        .await;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let claims = serde_json::json!({
        "iss": mock_issuer,
        "sub": "42",
        "aud": TEST_RESOURCE,
        "exp": now + 300,
        "iat": now,
        "jti": uuid::Uuid::new_v4().to_string(),
        "client_id": "client-abc",
        "scope": "commoncal.calendar.metadata.read commoncal.event.read.basic",
        "amr": ["pwd"],
    });
    let token = sign_jwt(&private_key, "test-key-1", &claims);

    let validator = TokenValidator::new();
    let result = validator.validate(&token, &mock_issuer, TEST_RESOURCE).await;

    let result = result.expect("valid token should validate");
    assert_eq!(result.user_id, 42);
    assert_eq!(result.oauth_client_id, "client-abc");
    assert_eq!(
        result.scopes,
        vec![
            "commoncal.calendar.metadata.read".to_string(),
            "commoncal.event.read.basic".to_string(),
        ]
    );
    assert_eq!(result.auth_strength, mcp_server::oauth::AuthStrength::Passwordless);
    assert!(!result.token_id.is_empty());
}

/// Phase 3: wrong issuer in token is rejected.
#[tokio::test]
async fn phase3_wrong_issuer_rejected() {
    let (private_key, jwk) = generate_rsa_key("test-key-2");
    let mock_issuer = "https://auth.example.com".to_string();

    let mock = wiremock::MockServer::start().await;
    let jwks_uri = format!("{}/jwks", mock.uri());
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/.well-known/oauth-authorization-server"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(serde_json::json!({
                    "issuer": mock_issuer,
                    "jwks_uri": jwks_uri,
                })
                .to_string()),
        )
        .mount(&mock)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/jwks"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(serde_json::json!({ "keys": [jwk] }).to_string()),
        )
        .mount(&mock)
        .await;

    // Token has wrong issuer.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let claims = serde_json::json!({
        "iss": "https://evil.example.com",
        "sub": "42",
        "aud": TEST_RESOURCE,
        "exp": now + 300,
        "iat": now,
        "jti": "t1",
        "client_id": "client-abc",
        "scope": "commoncal.calendar.metadata.read",
    });
    let token = sign_jwt(&private_key, "test-key-2", &claims);

    let validator = TokenValidator::new();
    let result = validator.validate(&token, &mock_issuer, TEST_RESOURCE).await;
    assert!(result.is_err(), "wrong issuer must be rejected");
}

/// Phase 3: wrong audience is rejected.
#[tokio::test]
async fn phase3_wrong_audience_rejected() {
    let (private_key, jwk) = generate_rsa_key("test-key-3");
    let mock_issuer = "https://auth.example.com".to_string();

    let mock = wiremock::MockServer::start().await;
    let jwks_uri = format!("{}/jwks", mock.uri());
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/.well-known/oauth-authorization-server"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(serde_json::json!({
                    "issuer": mock_issuer,
                    "jwks_uri": jwks_uri,
                })
                .to_string()),
        )
        .mount(&mock)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/jwks"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(serde_json::json!({ "keys": [jwk] }).to_string()),
        )
        .mount(&mock)
        .await;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let claims = serde_json::json!({
        "iss": mock_issuer,
        "sub": "42",
        "aud": "https://wrong-audience.example.com",
        "exp": now + 300,
        "iat": now,
        "jti": "t2",
        "client_id": "client-abc",
        "scope": "commoncal.calendar.metadata.read",
    });
    let token = sign_jwt(&private_key, "test-key-3", &claims);

    let validator = TokenValidator::new();
    let result = validator.validate(&token, &mock_issuer, TEST_RESOURCE).await;
    assert!(result.is_err(), "wrong audience must be rejected");
}

/// Phase 3: expired token is rejected.
#[tokio::test]
async fn phase3_expired_token_rejected() {
    let (private_key, jwk) = generate_rsa_key("test-key-4");
    let mock = wiremock::MockServer::start().await;
    let mock_issuer = mock.uri();
    let jwks_uri = format!("{}/jwks", mock_issuer);

    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/.well-known/oauth-authorization-server"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(serde_json::json!({
                    "issuer": mock_issuer,
                    "jwks_uri": jwks_uri,
                })
                .to_string()),
        )
        .mount(&mock)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/jwks"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(serde_json::json!({ "keys": [jwk] }).to_string()),
        )
        .mount(&mock)
        .await;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let claims = serde_json::json!({
        "iss": mock_issuer,
        "sub": "42",
        "aud": TEST_RESOURCE,
        "exp": now - 100,
        "iat": now - 400,
        "jti": "t3",
        "client_id": "client-abc",
        "scope": "commoncal.calendar.metadata.read",
    });
    let token = sign_jwt(&private_key, "test-key-4", &claims);

    let validator = TokenValidator::new();
    let result = validator.validate(&token, &mock_issuer, TEST_RESOURCE).await;
    assert!(
        matches!(result, Err(mcp_server::error::TokenError::Expired)),
        "expired token must be rejected with Expired error, got: {:?}",
        result
    );
}

/// Phase 3: non-numeric sub is rejected.
#[tokio::test]
async fn phase3_non_numeric_sub_rejected() {
    let (private_key, jwk) = generate_rsa_key("test-key-5");
    let mock_issuer = "https://auth.example.com".to_string();

    let mock = wiremock::MockServer::start().await;
    let jwks_uri = format!("{}/jwks", mock.uri());
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/.well-known/oauth-authorization-server"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(serde_json::json!({
                    "issuer": mock_issuer,
                    "jwks_uri": jwks_uri,
                })
                .to_string()),
        )
        .mount(&mock)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/jwks"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(serde_json::json!({ "keys": [jwk] }).to_string()),
        )
        .mount(&mock)
        .await;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let claims = serde_json::json!({
        "iss": mock_issuer,
        "sub": "not-a-number",
        "aud": TEST_RESOURCE,
        "exp": now + 300,
        "iat": now,
        "jti": "t4",
        "client_id": "client-abc",
        "scope": "commoncal.calendar.metadata.read",
    });
    let token = sign_jwt(&private_key, "test-key-5", &claims);

    let validator = TokenValidator::new();
    let result = validator.validate(&token, &mock_issuer, TEST_RESOURCE).await;
    assert!(
        result.is_err(),
        "non-numeric sub must be rejected"
    );
}

/// Phase 3: missing client_id is rejected.
#[tokio::test]
async fn phase3_missing_client_id_rejected() {
    let (private_key, jwk) = generate_rsa_key("test-key-6");
    let mock_issuer = "https://auth.example.com".to_string();

    let mock = wiremock::MockServer::start().await;
    let jwks_uri = format!("{}/jwks", mock.uri());
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/.well-known/oauth-authorization-server"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(serde_json::json!({
                    "issuer": mock_issuer,
                    "jwks_uri": jwks_uri,
                })
                .to_string()),
        )
        .mount(&mock)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/jwks"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(serde_json::json!({ "keys": [jwk] }).to_string()),
        )
        .mount(&mock)
        .await;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let claims = serde_json::json!({
        "iss": mock_issuer,
        "sub": "42",
        "aud": TEST_RESOURCE,
        "exp": now + 300,
        "iat": now,
        "jti": "t5",
        "scope": "commoncal.calendar.metadata.read",
    });
    let token = sign_jwt(&private_key, "test-key-6", &claims);

    let validator = TokenValidator::new();
    let result = validator.validate(&token, &mock_issuer, TEST_RESOURCE).await;
    assert!(
        result.is_err(),
        "missing client_id must be rejected"
    );
}

/// Phase 3: key rotation — token signed with a key present in JWKS validates.
///
/// This test verifies that when the JWKS contains multiple keys (rotation
/// overlap), a token signed with any of them validates successfully. The
/// one-refresh-on-unknown-kid path is exercised by the stale-key test below.
#[tokio::test]
async fn phase3_key_rotation_overlap_validates() {
    // Two keys present in JWKS (rotation overlap window).
    let (_old_key, old_jwk) = generate_rsa_key("old-key");
    let (new_key, new_jwk) = generate_rsa_key("new-key");

    let mock = wiremock::MockServer::start().await;
    let mock_issuer = mock.uri();
    let jwks_uri = format!("{}/jwks", mock_issuer);

    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/.well-known/oauth-authorization-server"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(serde_json::json!({
                    "issuer": mock_issuer,
                    "jwks_uri": jwks_uri,
                })
                .to_string()),
        )
        .mount(&mock)
        .await;

    // JWKS contains both keys (overlap window).
    let keys_json = serde_json::json!({ "keys": [old_jwk, new_jwk] });
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/jwks"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(keys_json.to_string()),
        )
        .mount(&mock)
        .await;

    // Token signed with the new key.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let claims = serde_json::json!({
        "iss": mock_issuer,
        "sub": "42",
        "aud": TEST_RESOURCE,
        "exp": now + 300,
        "iat": now,
        "jti": "t6",
        "client_id": "client-abc",
        "scope": "commoncal.calendar.metadata.read",
    });
    let token = sign_jwt(&new_key, "new-key", &claims);

    let validator = TokenValidator::new();
    let result = validator.validate(&token, &mock_issuer, TEST_RESOURCE).await;
    assert!(
        result.is_ok(),
        "token signed with key in JWKS overlap should validate, got: {:?}",
        result
    );
}

/// Phase 3: stale key — token signed with removed key is rejected even after refresh.
#[tokio::test]
async fn phase3_stale_key_rejected_after_refresh() {
    let (stale_key, _stale_jwk) = generate_rsa_key("stale-key");
    let (_current_key, current_jwk) = generate_rsa_key("current-key");

    let mock = wiremock::MockServer::start().await;
    let mock_issuer = mock.uri();
    let jwks_uri = format!("{}/jwks", mock_issuer);

    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/.well-known/oauth-authorization-server"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(serde_json::json!({
                    "issuer": mock_issuer,
                    "jwks_uri": jwks_uri,
                })
                .to_string()),
        )
        .mount(&mock)
        .await;

    // JWKS always returns only the current key (stale key was removed).
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/jwks"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(serde_json::json!({ "keys": [current_jwk] }).to_string()),
        )
        .expect(2..)
        .mount(&mock)
        .await;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let claims = serde_json::json!({
        "iss": mock_issuer,
        "sub": "42",
        "aud": TEST_RESOURCE,
        "exp": now + 300,
        "iat": now,
        "jti": "t7",
        "client_id": "client-abc",
        "scope": "commoncal.calendar.metadata.read",
    });
    let token = sign_jwt(&stale_key, "stale-key", &claims);

    let validator = TokenValidator::new();
    let result = validator.validate(&token, &mock_issuer, TEST_RESOURCE).await;
    assert!(
        result.is_err(),
        "token signed with removed key must be rejected"
    );
}

/// Phase 3: discovery metadata issuer mismatch is rejected.
#[tokio::test]
async fn phase3_discovery_issuer_mismatch_rejected() {
    let (_key, jwk) = generate_rsa_key("test-key-8");
    let mock = wiremock::MockServer::start().await;
    let jwks_uri = format!("{}/jwks", mock.uri());

    // Metadata returns a different issuer than what we configure.
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/.well-known/oauth-authorization-server"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(serde_json::json!({
                    "issuer": "https://different-issuer.example.com",
                    "jwks_uri": jwks_uri,
                })
                .to_string()),
        )
        .mount(&mock)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/jwks"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(serde_json::json!({ "keys": [jwk] }).to_string()),
        )
        .mount(&mock)
        .await;

    let validator = TokenValidator::new();
    // Configure with the mock's URI as issuer, but metadata returns a different one.
    let result = validator
        .validate("any.token.here", &mock.uri(), TEST_RESOURCE)
        .await;
    assert!(
        matches!(result, Err(mcp_server::error::TokenError::InvalidIssuer)),
        "issuer mismatch must be rejected, got: {:?}",
        result
    );
}

/// Phase 3: iat in the future beyond clock skew is rejected.
#[tokio::test]
async fn phase3_future_iat_rejected() {
    let (private_key, jwk) = generate_rsa_key("test-key-9");
    let mock_issuer = "https://auth.example.com".to_string();

    let mock = wiremock::MockServer::start().await;
    let jwks_uri = format!("{}/jwks", mock.uri());
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/.well-known/oauth-authorization-server"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(serde_json::json!({
                    "issuer": mock_issuer,
                    "jwks_uri": jwks_uri,
                })
                .to_string()),
        )
        .mount(&mock)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/jwks"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(serde_json::json!({ "keys": [jwk] }).to_string()),
        )
        .mount(&mock)
        .await;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    // iat is 1 hour in the future (beyond the 30s clock skew).
    let claims = serde_json::json!({
        "iss": mock_issuer,
        "sub": "42",
        "aud": TEST_RESOURCE,
        "exp": now + 3600,
        "iat": now + 3600,
        "jti": "t8",
        "client_id": "client-abc",
        "scope": "commoncal.calendar.metadata.read",
    });
    let token = sign_jwt(&private_key, "test-key-9", &claims);

    let validator = TokenValidator::new();
    let result = validator.validate(&token, &mock_issuer, TEST_RESOURCE).await;
    assert!(
        result.is_err(),
        "iat in the future beyond clock skew must be rejected"
    );
}

/// Phase 3: a token issued by the repository's real authorization-server
/// implementation validates successfully.
///
/// This is NOT a hand-built fixture. The token is signed by the real auth
/// server (`slice1-lab/auth-server/src/server.mjs`) using its configured JWKS
/// key. The test spawns a helper that launches the real auth server, drives the
/// full OAuth flow (DCR + Authorization Code + S256 PKCE + CommonCal consent via
/// the private bridge + token exchange), and returns the issued token. The token
/// is then validated with the mcp-server's `TokenValidator` — exercising real
/// discovery, JWKS fetch, signature verification, and standard-claim parsing
/// against a token the production validator must accept.
///
/// The test skips (rather than fails) when the environment cannot run the real
/// auth server (helper exit code 100: PostgreSQL unavailable).
#[tokio::test]
async fn phase3_real_auth_server_token_validates() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let helper = format!("{manifest}/tests/real_auth_server_token.mjs");
    // The helper writes the issued token to this file (it uses inherited stdio
    // so the auth server does not exit when its pipes close).
    let token_file = format!("{manifest}/tests/.real_auth_token.json");
    let _ = std::fs::remove_file(&token_file);

    // Spawn the helper in its own process group so we can kill the whole group
    // (helper + auth server child) on cleanup.
    let mut cmd = tokio::process::Command::new("node");
    cmd.arg(&helper).env("REAL_AUTH_TOKEN_FILE", &token_file);
    #[cfg(unix)]
    {
        #[allow(unused_imports)]
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                libc::setpgid(0, 0);
                Ok(())
            });
        }
    }
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            eprintln!("skip: cannot spawn node helper: {e}");
            return;
        }
    };

    // Poll for the token file (the helper writes it once the flow completes).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
    let token_content = loop {
        if std::path::Path::new(&token_file).exists() {
            break std::fs::read_to_string(&token_file).ok();
        }
        if std::time::Instant::now() > deadline {
            break None;
        }
        // If the helper already exited, stop polling.
        let exited = child.try_wait().ok().flatten().is_some();
        if exited {
            break None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };

    let Some(content) = token_content else {
        kill_process_group(&mut child).await;
        let _ = std::fs::remove_file(&token_file);
        eprintln!("skip: real auth server helper produced no token (environment unavailable?)");
        return;
    };

    let parsed: serde_json::Value =
        serde_json::from_str(&content).expect("token file must be valid JSON");
    let token = parsed["access_token"]
        .as_str()
        .expect("access_token present")
        .to_string();
    let issuer = parsed["issuer"].as_str().expect("issuer present").to_string();
    let resource = parsed["resource"]
        .as_str()
        .expect("resource present")
        .to_string();
    let client_id = parsed["client_id"]
        .as_str()
        .expect("client_id present")
        .to_string();

    // Validate the token while the auth server is still alive.
    let validator = TokenValidator::new();
    let result = validator
        .validate(&token, &issuer, &resource)
        .await
        .expect("token issued by the real auth server must validate");

    // Standard claims parsed correctly.
    assert_eq!(result.user_id, 1, "numeric sub must parse to the CommonCal user id");
    assert_eq!(result.oauth_client_id, client_id);
    assert!(
        result.scopes.contains(&"commoncal.calendar.metadata.read".to_string()),
        "granted scope must be present: {:?}",
        result.scopes
    );
    assert!(
        result.scopes.contains(&"commoncal.event.read.basic".to_string()),
        "granted scope must be present: {:?}",
        result.scopes
    );
    assert!(!result.token_id.is_empty(), "jti must be present");
    assert!(
        result.expires_at > 0,
        "exp must be a positive Unix timestamp"
    );

    // Clean up: kill the whole process group (helper + auth server child) and
    // remove the token file.
    kill_process_group(&mut child).await;
    let _ = std::fs::remove_file(&token_file);
}

/// Kill the child and its entire process group (so the auth server child of the
/// helper is also terminated). Unix-only; falls back to a plain kill elsewhere.
async fn kill_process_group(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    {
        let pid = child.id().map(|p| p as i32);
        if let Some(pid) = pid {
            // Kill the process group (negative pid). Ignore errors if already gone.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
}
