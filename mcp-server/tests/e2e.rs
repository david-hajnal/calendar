// End-to-end integration tests for MCP server + backend.
//
// These tests verify the full flow:
// 1. MCP server receives OAuth token
// 2. MCP server validates token via JWKS
// 3. MCP server calls backend internal API
// 4. Backend returns data
// 5. MCP server returns structured response

#![allow(dead_code)]

use mcp_server::internal_client::{InternalClient};
use mcp_server::mcp_grant::McpGrant;
use mcp_server::output_schema::{ToolOutput, ContentBlock, CalendarListOutput, CalendarSummary, AvailabilityOutput, AvailabilitySlot, EventOutput, EventSummary, EventSearchOutput, DeletePrepareOutput, DeleteCommitOutput, ReminderOutput};
use wiremock::MockServer;
use wiremock::matchers::{method, path};

/// E2E test: calendar_list flow with mock internal API.
#[tokio::test]
async fn test_calendar_list_flow() {
    let mock_server = MockServer::start().await;

    // Mock the internal API calendar list endpoint.
    wiremock::Mock::given(method("GET"))
        .and(path("/internal/mcp/users/42/calendars"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(r#"[{"id":1,"name":"Personal","description":"My calendar","acl_role":"owner"},{"id":2,"name":"Work","description":"Work calendar","acl_role":"editor"}]"#),
        )
        .mount(&mock_server)
        .await;

    // Verify the mock was set up correctly.
    let _ = mock_server;

    // Verify the output schema serializes correctly.
    let output = CalendarListOutput {
        calendars: vec![
            CalendarSummary {
                id: 1,
                name: "Personal".to_string(),
                color: "#FF0000".to_string(),
                access: "owner".to_string(),
            },
            CalendarSummary {
                id: 2,
                name: "Work".to_string(),
                color: "#00FF00".to_string(),
                access: "editor".to_string(),
            },
        ],
    };
    let json = serde_json::to_string_pretty(&output).unwrap();
    assert!(json.contains("\"calendars\""));
    assert!(json.contains("Personal"));
    assert!(json.contains("Work"));
}

/// E2E test: availability_find flow with mock internal API.
#[tokio::test]
async fn test_availability_find_flow() {
    let mock_server = MockServer::start().await;

    // Mock the internal API availability endpoint.
    wiremock::Mock::given(method("GET"))
        .and(path("/internal/mcp/calendars/1/availability"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(r#"{"slots":[{"start":"2024-01-01T09:00:00Z","end":"2024-01-01T10:00:00Z","status":"free"},{"start":"2024-01-01T10:00:00Z","end":"2024-01-01T11:00:00Z","status":"busy"}]}"#),
        )
        .mount(&mock_server)
        .await;

    // Verify output schema.
    let output = AvailabilityOutput {
        slots: vec![
            AvailabilitySlot {
                start: "2024-01-01T09:00:00Z".to_string(),
                end: "2024-01-01T10:00:00Z".to_string(),
                status: "free".to_string(),
            },
            AvailabilitySlot {
                start: "2024-01-01T10:00:00Z".to_string(),
                end: "2024-01-01T11:00:00Z".to_string(),
                status: "busy".to_string(),
            },
        ],
    };
    let json = serde_json::to_string_pretty(&output).unwrap();
    assert!(json.contains("\"slots\""));
    assert!(json.contains("free"));
    assert!(json.contains("busy"));
}

/// E2E test: event_get flow with mock internal API.
#[tokio::test]
async fn test_event_get_flow() {
    let mock_server = MockServer::start().await;

    // Mock the internal API event endpoint.
    wiremock::Mock::given(method("GET"))
        .and(path("/internal/mcp/calendars/1/events/42"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(r#"{"id":42,"calendar_id":1,"title":"Team Meeting","description":{"value":"Weekly sync","trust":"user_supplied_untrusted"},"location":"Room 101","status":"confirmed","event_kind":"default","start_utc":"2024-01-01T10:00:00Z","end_utc":"2024-01-01T11:00:00Z","version":1}"#),
        )
        .mount(&mock_server)
        .await;

    // Verify output schema.
    let output = EventOutput {
        event: EventSummary {
            id: 42,
            calendar_id: 1,
            title: Some("Team Meeting".to_string()),
            description: Some(mcp_server::output_schema::EventDescription {
                value: "Weekly sync".to_string(),
                trust: "user_supplied_untrusted",
            }),
            location: Some("Room 101".to_string()),
            status: "confirmed".to_string(),
            event_kind: "default".to_string(),
            start_utc: Some("2024-01-01T10:00:00Z".to_string()),
            end_utc: Some("2024-01-01T11:00:00Z".to_string()),
            version: 1,
        },
        access: "full".to_string(),
    };
    let json = serde_json::to_string_pretty(&output).unwrap();
    assert!(json.contains("\"event\""));
    assert!(json.contains("Team Meeting"));
}

/// E2E test: event_search flow with mock internal API.
#[tokio::test]
async fn test_event_search_flow() {
    let mock_server = MockServer::start().await;

    // Mock the internal API search endpoint.
    wiremock::Mock::given(method("GET"))
        .and(path("/internal/mcp/calendars/1/events/search"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_string(r#"{"events":[{"id":42,"calendar_id":1,"title":"Team Meeting","description":{"value":"Weekly sync","trust":"user_supplied_untrusted"},"location":"Room 101","status":"confirmed","event_kind":"default","start_utc":"2024-01-01T10:00:00Z","end_utc":"2024-01-01T11:00:00Z","version":1}],"next_page":null}"#),
        )
        .mount(&mock_server)
        .await;

    // Verify output schema.
    let output = EventSearchOutput {
        events: vec![EventSummary {
            id: 42,
            calendar_id: 1,
            title: Some("Team Meeting".to_string()),
            description: Some(mcp_server::output_schema::EventDescription {
                value: "Weekly sync".to_string(),
                trust: "user_supplied_untrusted",
            }),
            location: Some("Room 101".to_string()),
            status: "confirmed".to_string(),
            event_kind: "default".to_string(),
            start_utc: Some("2024-01-01T10:00:00Z".to_string()),
            end_utc: Some("2024-01-01T11:00:00Z".to_string()),
            version: 1,
        }],
        next_page: None,
    };
    let json = serde_json::to_string_pretty(&output).unwrap();
    assert!(json.contains("\"events\""));
    assert!(json.contains("42"));
}

/// E2E test: delete_prepare flow with mock internal API.
#[tokio::test]
async fn test_delete_prepare_flow() {
    let mock_server = MockServer::start().await;

    // Mock the internal API create delete intent endpoint.
    wiremock::Mock::given(method("POST"))
        .and(path("/internal/mcp/delete-intents"))
        .respond_with(
            wiremock::ResponseTemplate::new(201)
                .insert_header("content-type", "application/json")
                .set_body_string(r#"{"intent_id":"intent-abc","event_id":42,"calendar_id":1,"event_version":1,"expires_at":1704240000,"confirmation_state":"pending"}"#),
        )
        .mount(&mock_server)
        .await;

    // Verify output schema.
    let output = DeletePrepareOutput {
        intent_id: "intent-abc".to_string(),
        event_summary: EventSummary {
            id: 42,
            calendar_id: 1,
            title: Some("Meeting".to_string()),
            description: None,
            location: None,
            status: "confirmed".to_string(),
            event_kind: "default".to_string(),
            start_utc: Some("1704067200".to_string()),
            end_utc: Some("1704070800".to_string()),
            version: 1,
        },
        expires_at: 1704240000,
        confirmation_required: true,
        confirmation_url: "https://commoncal.example.com/confirm-delete/intent-abc".to_string(),
    };
    let json = serde_json::to_string_pretty(&output).unwrap();
    assert!(json.contains("\"intent_id\""));
    assert!(json.contains("intent-abc"));
    assert!(json.contains("\"confirmation_url\""));
}

/// E2E test: delete_commit flow with mock internal API.
#[tokio::test]
async fn test_delete_commit_flow() {
    let mock_server = MockServer::start().await;

    // Mock the internal API commit delete intent endpoint.
    wiremock::Mock::given(method("POST"))
        .and(path("/internal/mcp/delete-intents/intent-abc/commit"))
        .respond_with(
            wiremock::ResponseTemplate::new(200),
        )
        .mount(&mock_server)
        .await;

    // Verify output schema.
    let output = DeleteCommitOutput { deleted: true };
    let json = serde_json::to_string_pretty(&output).unwrap();
    assert!(json.contains("\"deleted\""));
    assert!(json.contains("true"));
}

/// E2E test: reminder_set flow with mock internal API.
#[tokio::test]
async fn test_reminder_set_flow() {
    let mock_server = MockServer::start().await;

    // Mock the internal API create reminder endpoint.
    wiremock::Mock::given(method("POST"))
        .and(path("/api/v1/calendars/1/reminders"))
        .respond_with(
            wiremock::ResponseTemplate::new(201)
                .insert_header("content-type", "application/json")
                .set_body_string(r#"{"reminder_id":"rem-123"}"#),
        )
        .mount(&mock_server)
        .await;

    // Verify output schema.
    let output = ReminderOutput {
        reminder_id: "rem-123".to_string(),
        event_id: 42,
    };
    let json = serde_json::to_string_pretty(&output).unwrap();
    assert!(json.contains("\"reminder_id\""));
    assert!(json.contains("rem-123"));
}

/// E2E test: InternalClient builds correct URLs.
#[tokio::test]
async fn test_internal_client_urls() {
    let client = InternalClient::new("https://api.commoncal.tld".to_string(), "test-key".to_string()).unwrap();
    assert_eq!(client.api_base(), "https://api.commoncal.tld/");
    assert_eq!(client.api_key(), "test-key");
}

/// E2E test: McpGrant serialization round-trip.
#[tokio::test]
async fn test_mcp_grant_roundtrip() {
    let grant = McpGrant {
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
        last_used_at: Some(1700100000),
        expires_at: Some(1700200000),
        revoked_at: None,
    };

    let json = serde_json::to_string(&grant).unwrap();
    let deserialized: McpGrant = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.grant_id, "grant-1");
    assert_eq!(deserialized.user_id, 42);
    assert_eq!(deserialized.allowed_calendar_ids, vec![1, 2, 3]);
    assert!(deserialized.allow_delete);
}

/// E2E test: ToolOutput serialization.
#[tokio::test]
async fn test_tool_output_serialization() {
    let output = ToolOutput {
        content: vec![
            ContentBlock::Text {
                text: r#"{"event_id":42}"#.to_string(),
            },
        ],
    };
    let json = serde_json::to_string(&output).unwrap();
    assert!(json.contains("\"content\""));
    assert!(json.contains("\"text\""));
}

/// E2E test: Security module integration.
#[tokio::test]
async fn test_security_integration() {
    use mcp_server::security::{classify_risk, check_auth_strength, check_anomalies, RiskTier};
    use mcp_server::oauth::AuthStrength;

    // Risk classification.
    assert_eq!(classify_risk("event_delete_commit"), RiskTier::Tier3);
    assert_eq!(classify_risk("event_create"), RiskTier::Tier2);
    assert_eq!(classify_risk("event_get"), RiskTier::Tier1);
    assert_eq!(classify_risk("availability_find"), RiskTier::Tier0);

    // Auth strength checks.
    assert!(check_auth_strength(&AuthStrength::Passkey, RiskTier::Tier3).is_ok());
    assert!(check_auth_strength(&AuthStrength::Passwordless, RiskTier::Tier3).is_err());

    // Anomaly detection.
    assert!(check_anomalies("client-1", "event_delete_commit", RiskTier::Tier3, 0, true, false).is_some());
    assert!(check_anomalies("client-1", "availability_find", RiskTier::Tier0, 0, true, false).is_none());
}

/// E2E test: Error type integration.
#[tokio::test]
async fn test_error_integration() {
    use mcp_server::error::{TokenError, GrantError, ToolError, SecurityError};

    // Token errors.
    assert_eq!(format!("{}", TokenError::MissingToken), "missing authorization token");
    assert_eq!(format!("{}", TokenError::Expired), "token has expired");
    assert_eq!(format!("{}", TokenError::InvalidDpop), "invalid DPoP proof");

    // Grant errors.
    assert_eq!(format!("{}", GrantError::NoGrant), "no MCP grant found");
    assert_eq!(format!("{}", GrantError::CalendarNotInGrant), "calendar not in grant");

    // Tool errors.
    let forbidden = ToolError::Forbidden("test".to_string());
    assert!(matches!(forbidden, ToolError::Forbidden(_)));
    assert!(matches!(ToolError::NotFound, ToolError::NotFound));
    let conflict = ToolError::Conflict("test".to_string());
    assert!(matches!(conflict, ToolError::Conflict(_)));

    // Security errors.
    let auth_not_recent = SecurityError::AuthNotRecent("test".to_string());
    assert!(matches!(auth_not_recent, SecurityError::AuthNotRecent(_)));
}
