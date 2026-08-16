// Security module: risk classification, rate limiting, auth strength checks.

use crate::error::SecurityError;

/// Risk tier for a tool.
#[derive(Debug, Clone, PartialEq, PartialOrd)]
pub enum RiskTier {
    Tier0, // availability — read-only, minimal data
    Tier1, // event details — sensitive read
    Tier2, // create/update — mutation + strong auth
    Tier3, // delete — step-up + confirmation
}

/// Classify a tool by its risk tier.
pub fn classify_risk(tool_name: &str) -> RiskTier {
    match tool_name {
        "availability_find" | "availability_get" => RiskTier::Tier0,
        "event_get" | "event_search" | "calendar_list" => RiskTier::Tier1,
        "event_create" | "event_update" | "reminder_set" => RiskTier::Tier2,
        "event_delete_prepare" | "event_delete_commit" => RiskTier::Tier3,
        _ => RiskTier::Tier2,
    }
}

/// Check if authentication strength meets the tier requirement.
pub fn check_auth_strength(
    auth_strength: &crate::oauth::AuthStrength,
    tier: RiskTier,
) -> Result<(), SecurityError> {
    match tier {
        RiskTier::Tier0 | RiskTier::Tier1 => {
            // Any authenticated user is fine
            Ok(())
        }
        RiskTier::Tier2 => {
            // Requires strong authentication (passkey or MFA)
            match auth_strength {
                crate::oauth::AuthStrength::Passkey | crate::oauth::AuthStrength::Mfa => Ok(()),
                crate::oauth::AuthStrength::Passwordless => Err(SecurityError::WeakAuthentication(
                    "strong authentication required for write operations".to_string(),
                )),
            }
        }
        RiskTier::Tier3 => {
            // Requires passkey or MFA
            match auth_strength {
                crate::oauth::AuthStrength::Passkey | crate::oauth::AuthStrength::Mfa => Ok(()),
                crate::oauth::AuthStrength::Passwordless => Err(SecurityError::WeakAuthentication(
                    "passkey or MFA required for destructive operations".to_string(),
                )),
            }
        }
    }
}

/// Check if authentication was performed recently enough.
pub fn require_recent_auth(auth_time: i64, max_age_secs: i64) -> Result<(), SecurityError> {
    let now = super::config::current_time_secs();
    if now - auth_time > max_age_secs {
        Err(SecurityError::AuthNotRecent(format!(
            "authentication must be within {} seconds",
            max_age_secs
        )))
    } else {
        Ok(())
    }
}

/// Anomaly detection hook.
///
/// Returns `Some(reason)` if the request should be flagged or blocked.
/// Checks:
/// 1. Too many failures from same client (brute force)
/// 2. Unusual time window (off-hours for sensitive ops)
/// 3. Rate limit exceeded
pub fn check_anomalies(
    client_id: &str,
    tool_name: &str,
    risk_tier: RiskTier,
    recent_failures: usize,
    is_off_hours: bool,
    rate_limit_exceeded: bool,
) -> Option<String> {
    // Check 1: Too many failures from same client.
    if recent_failures >= 10 {
        return Some(format!(
            "too many recent failures from client {}: brute force protection",
            client_id
        ));
    }

    // Check 2: Off-hours sensitive operation.
    if is_off_hours && risk_tier >= RiskTier::Tier3 {
        return Some(format!(
            "off-hours {} operation flagged for review",
            tool_name
        ));
    }

    // Check 3: Rate limit exceeded.
    if rate_limit_exceeded {
        return Some(format!("rate limit exceeded for client {}", client_id));
    }

    None
}

/// Record an anomaly event for audit.
pub fn record_anomaly(client_id: &str, tool_name: &str, reason: &str, severity: &str) {
    tracing::warn!(
        client_id,
        tool = tool_name,
        reason,
        severity,
        "mcp_anomaly_detected"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_risk_availability() {
        assert_eq!(classify_risk("availability_find"), RiskTier::Tier0);
        assert_eq!(classify_risk("availability_get"), RiskTier::Tier0);
    }

    #[test]
    fn classify_risk_event_read() {
        assert_eq!(classify_risk("event_get"), RiskTier::Tier1);
        assert_eq!(classify_risk("event_search"), RiskTier::Tier1);
        assert_eq!(classify_risk("calendar_list"), RiskTier::Tier1);
    }

    #[test]
    fn classify_risk_event_write() {
        assert_eq!(classify_risk("event_create"), RiskTier::Tier2);
        assert_eq!(classify_risk("event_update"), RiskTier::Tier2);
        assert_eq!(classify_risk("reminder_set"), RiskTier::Tier2);
    }

    #[test]
    fn classify_risk_delete() {
        assert_eq!(classify_risk("event_delete_prepare"), RiskTier::Tier3);
        assert_eq!(classify_risk("event_delete_commit"), RiskTier::Tier3);
    }

    #[test]
    fn classify_risk_unknown_defaults_to_tier2() {
        assert_eq!(classify_risk("unknown_tool"), RiskTier::Tier2);
    }

    #[test]
    fn check_auth_strength_tier0_passes_any() {
        assert!(
            check_auth_strength(&crate::oauth::AuthStrength::Passwordless, RiskTier::Tier0).is_ok()
        );
        assert!(check_auth_strength(&crate::oauth::AuthStrength::Passkey, RiskTier::Tier0).is_ok());
        assert!(check_auth_strength(&crate::oauth::AuthStrength::Mfa, RiskTier::Tier0).is_ok());
    }

    #[test]
    fn check_auth_strength_tier1_passes_any() {
        assert!(
            check_auth_strength(&crate::oauth::AuthStrength::Passwordless, RiskTier::Tier1).is_ok()
        );
        assert!(check_auth_strength(&crate::oauth::AuthStrength::Passkey, RiskTier::Tier1).is_ok());
    }

    #[test]
    fn check_auth_strength_tier2_requires_strong() {
        assert!(check_auth_strength(&crate::oauth::AuthStrength::Passkey, RiskTier::Tier2).is_ok());
        assert!(check_auth_strength(&crate::oauth::AuthStrength::Mfa, RiskTier::Tier2).is_ok());
        assert!(
            check_auth_strength(&crate::oauth::AuthStrength::Passwordless, RiskTier::Tier2)
                .is_err()
        );
    }

    #[test]
    fn check_auth_strength_tier3_requires_passkey_or_mfa() {
        assert!(check_auth_strength(&crate::oauth::AuthStrength::Passkey, RiskTier::Tier3).is_ok());
        assert!(check_auth_strength(&crate::oauth::AuthStrength::Mfa, RiskTier::Tier3).is_ok());
        assert!(
            check_auth_strength(&crate::oauth::AuthStrength::Passwordless, RiskTier::Tier3)
                .is_err()
        );
    }

    #[test]
    fn check_anomalies_brute_force() {
        let result = check_anomalies(
            "client-1",
            "event_create",
            RiskTier::Tier2,
            10,
            false,
            false,
        );
        assert!(result.is_some());
        assert!(result.unwrap().contains("brute force"));
    }

    #[test]
    fn check_anomalies_off_hours_delete() {
        let result = check_anomalies(
            "client-1",
            "event_delete_commit",
            RiskTier::Tier3,
            0,
            true,
            false,
        );
        assert!(result.is_some());
        assert!(result.unwrap().contains("off-hours"));
    }

    #[test]
    fn check_anomalies_rate_limit() {
        let result = check_anomalies("client-1", "event_get", RiskTier::Tier1, 0, false, true);
        assert!(result.is_some());
        assert!(result.unwrap().contains("rate limit"));
    }

    #[test]
    fn check_anomalies_clean_request() {
        let result = check_anomalies(
            "client-1",
            "availability_find",
            RiskTier::Tier0,
            0,
            false,
            false,
        );
        assert!(result.is_none());
    }

    #[test]
    fn check_anomalies_off_hours_read_allowed() {
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

    #[test]
    fn check_anomalies_below_brute_force_threshold() {
        let result = check_anomalies("client-1", "event_create", RiskTier::Tier2, 9, false, false);
        assert!(result.is_none());
    }
}
