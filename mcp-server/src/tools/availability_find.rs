// availability_find tool handler.
//
// Returns availability slots for specified calendars within a time range.
// Requires:
// - Valid OAuth token
// - McpGrant with allow_availability=true
// - Time range within max 31 days

use axum::http::{Response, StatusCode};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::error::ToolError;
use crate::internal_client::{InternalClient, CalendarInfo};
use crate::mcp_grant::{check_calendar_access, get_grant};
use crate::oauth::TokenValidationResult;
use crate::output_schema::{AvailabilityOutput, AvailabilitySlot, ContentBlock, ToolOutput};

/// Maximum allowed time range in days.
const MAX_RANGE_DAYS: i64 = 31;

#[derive(Debug, Deserialize)]
pub struct AvailabilityFindParams {
    pub calendar_ids: Vec<i64>,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Serialize)]
pub struct AvailabilityFindOutput {
    pub slots: Vec<AvailabilitySlot>,
}

/// Validate that the time range is within the allowed maximum.
pub fn validate_time_range(from: &str, to: &str) -> Result<(), ToolError> {
    let from_ts = parse_utc_timestamp(from)
        .map_err(|e| ToolError::BadRequest(format!("invalid 'from' timestamp: {}", e)))?;
    let to_ts = parse_utc_timestamp(to)
        .map_err(|e| ToolError::BadRequest(format!("invalid 'to' timestamp: {}", e)))?;

    if to_ts <= from_ts {
        return Err(ToolError::BadRequest(
            "'to' must be after 'from'".to_string(),
        ));
    }

    let range_secs = to_ts - from_ts;
    let max_secs = MAX_RANGE_DAYS * 24 * 3600;

    if range_secs > max_secs {
        return Err(ToolError::BadRequest(format!(
            "time range exceeds maximum of {} days ({} seconds)",
            MAX_RANGE_DAYS, max_secs
        )));
    }

    Ok(())
}

/// Parse a UTC timestamp string (ISO 8601 or Unix epoch) to i64.
pub fn parse_utc_timestamp(s: &str) -> Result<i64, String> {
    // Try Unix epoch first.
    if let Ok(ts) = s.parse::<i64>() {
        return Ok(ts);
    }

    // Try ISO 8601 via str::parse.
    match s.parse::<DateTime<Utc>>() {
        Ok(dt) => Ok(dt.timestamp()),
        Err(_) => Err(format!("cannot parse timestamp: {}", s)),
    }
}

/// Handle the availability_find tool call.
///
/// Full authorization pipeline:
/// 1. Validate OAuth token → TokenValidationResult
/// 2. Load McpGrant → check allow_availability
/// 3. Validate time range (max 31 days)
/// 4. Filter calendar_ids by grant's allowed_calendar_ids
/// 5. Call internal API for each calendar
/// 6. Return structured availability output
pub async fn handle(
    token: &TokenValidationResult,
    db_pool: &SqlitePool,
    internal_client: &InternalClient,
    params: AvailabilityFindParams,
) -> Result<Response<axum::body::Body>, ToolError> {
    // Step 1: Load the McpGrant.
    let grant = get_grant(db_pool, token.user_id, &token.oauth_client_id)
        .await
        .map_err(|e| ToolError::Internal(format!("grant lookup failed: {}", e)))?;

    let grant = grant.ok_or(ToolError::Forbidden("no MCP grant found".to_string()))?;

    // Step 2: Check tool permission.
    if !crate::mcp_grant::check_tool_permission(&grant, "availability_find") {
        return Err(ToolError::Forbidden(
            "availability_find requires availability permission".to_string(),
        ));
    }

    // Step 3: Validate time range.
    validate_time_range(&params.from, &params.to)?;

    // Step 4: Filter calendar_ids by grant.
    let allowed_ids: Vec<i64> = params
        .calendar_ids
        .into_iter()
        .filter(|id| check_calendar_access(&grant, *id))
        .collect();

    if allowed_ids.is_empty() {
        return Ok(build_empty_response());
    }

    // Step 5: For the tracer bullet, return mock availability.
    // Slice 6 will wire to the real internal API.
    let output = AvailabilityOutput {
        slots: vec![
            AvailabilitySlot {
                start: params.from.clone(),
                end: params.to.clone(),
                status: "free".to_string(),
            },
        ],
    };

    let tool_output = ToolOutput {
        content: vec![ContentBlock::Text {
            text: serde_json::to_string_pretty(&output).unwrap_or_else(|_| "[]".to_string()),
        }],
    };

    let body = serde_json::to_string_pretty(&tool_output)
        .unwrap_or_else(|_| r#"{"content":[{"text":"[]"}]}"#.to_string());

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap())
}

/// Build an empty availability response.
fn build_empty_response() -> Response<axum::body::Body> {
    let output = AvailabilityOutput { slots: vec![] };
    let body = serde_json::to_string_pretty(&output).unwrap_or_else(|_| "[]".to_string());

    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_time_range_accepts_same_day() {
        let result = validate_time_range("2024-01-01T00:00:00Z", "2024-01-01T23:59:59Z");
        assert!(result.is_ok());
    }

    #[test]
    fn validate_time_range_accepts_31_days() {
        let result = validate_time_range(
            "2024-01-01T00:00:00Z",
            "2024-01-31T23:59:59Z",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn validate_time_range_rejects_32_days() {
        let result = validate_time_range(
            "2024-01-01T00:00:00Z",
            "2024-02-02T00:00:00Z",
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(format!("{}", err).contains("31"));
    }

    #[test]
    fn validate_time_range_rejects_to_before_from() {
        let result = validate_time_range("2024-01-02T00:00:00Z", "2024-01-01T00:00:00Z");
        assert!(result.is_err());
    }

    #[test]
    fn validate_time_range_rejects_invalid_from() {
        let result = validate_time_range("not-a-date", "2024-01-02T00:00:00Z");
        assert!(result.is_err());
    }

    #[test]
    fn validate_time_range_rejects_invalid_to() {
        let result = validate_time_range("2024-01-01T00:00:00Z", "not-a-date");
        assert!(result.is_err());
    }

    #[test]
    fn validate_time_range_accepts_unix_epochs() {
        let result = validate_time_range("1704067200", "1704153600");
        assert!(result.is_ok());
    }

    #[test]
    fn validate_time_range_rejects_too_long_unix_range() {
        let result = validate_time_range("0", "31536001"); // > 31 days in seconds
        assert!(result.is_err());
    }

    #[test]
    fn parse_utc_timestamp_parses_iso8601() {
        let ts = parse_utc_timestamp("2024-01-01T12:00:00Z").unwrap();
        assert_eq!(ts, 1704110400);
    }

    #[test]
    fn parse_utc_timestamp_parses_unix_epoch() {
        let ts = parse_utc_timestamp("1704067200").unwrap();
        assert_eq!(ts, 1704067200);
    }

    #[test]
    fn parse_utc_timestamp_rejects_invalid() {
        assert!(parse_utc_timestamp("invalid").is_err());
    }

    #[test]
    fn availability_slot_serializes() {
        let slot = AvailabilitySlot {
            start: "2024-01-01T00:00:00Z".to_string(),
            end: "2024-01-01T01:00:00Z".to_string(),
            status: "free".to_string(),
        };
        let json = serde_json::to_string(&slot).unwrap();
        assert!(json.contains("\"start\""));
        assert!(json.contains("\"end\""));
        assert!(json.contains("\"status\""));
    }

    #[test]
    fn availability_output_serializes() {
        let output = AvailabilityOutput {
            slots: vec![
                AvailabilitySlot {
                    start: "00:00".to_string(),
                    end: "01:00".to_string(),
                    status: "free".to_string(),
                },
            ],
        };
        let json = serde_json::to_string_pretty(&output).unwrap();
        assert!(json.contains("\"slots\""));
    }

    #[test]
    fn availability_output_serializes_empty() {
        let output = AvailabilityOutput { slots: vec![] };
        let json = serde_json::to_string_pretty(&output).unwrap();
        assert!(json.contains("\"slots\""));
        assert!(json.contains("[]"));
    }
}
