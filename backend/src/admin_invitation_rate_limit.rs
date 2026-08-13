use axum::{
    body::Body,
    http::{HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use std::sync::Arc;

use crate::rate_limiter::FixedWindowRateLimiter;
use crate::write_rate_limit::RateLimitExceeded;

/// Shared state for the admin invitation rate limiter.
#[derive(Clone)]
pub struct AdminInvitationRateLimiterState {
    pub limiter: Arc<FixedWindowRateLimiter>,
}

/// Check rate limit for admin invitation requests.
///
/// Returns `RateLimitExceeded` (429) when the user has exceeded the limit,
/// `Ok(())` when within the limit.
///
/// Unlike write rate limiting, superadmins are NOT bypassed.
pub fn check_admin_invitation_rate_limit(
    limiter: &AdminInvitationRateLimiterState,
    user_id: i64,
) -> Result<(), RateLimitExceeded> {
    let key = format!("admin:{}", user_id);
    let (allowed, retry_after) = limiter.limiter.check_by_key(&key);
    if !allowed {
        Err(RateLimitExceeded { retry_after })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_limiter(
        max_requests: u32,
        window_seconds: i64,
        now: i64,
    ) -> AdminInvitationRateLimiterState {
        let limiter = FixedWindowRateLimiter::new_at(max_requests, window_seconds, now);
        AdminInvitationRateLimiterState {
            limiter: Arc::new(limiter),
        }
    }

    // --- test_check_allows_under_limit ---

    #[test]
    fn test_check_allows_under_limit() {
        let limiter = make_limiter(5, 60, 1000);

        for i in 0..5 {
            let result = check_admin_invitation_rate_limit(&limiter, 1);
            assert!(result.is_ok(), "request {} should be allowed", i + 1);
        }
    }

    // --- test_check_blocks_over_limit ---

    #[test]
    fn test_check_blocks_over_limit() {
        let limiter = make_limiter(5, 60, 1000);

        for i in 0..5 {
            let result = check_admin_invitation_rate_limit(&limiter, 1);
            assert!(result.is_ok(), "request {} should be allowed", i + 1);
        }

        let result = check_admin_invitation_rate_limit(&limiter, 1);
        assert!(result.is_err(), "6th request should be rate limited");

        let err = result.unwrap_err();
        assert_eq!(err.retry_after, 60);
    }

    // --- test_check_different_users_independent ---

    #[test]
    fn test_check_different_users_independent() {
        let limiter = make_limiter(1, 60, 1000);

        // User 1 hits their limit.
        assert!(check_admin_invitation_rate_limit(&limiter, 1).is_ok());
        assert!(check_admin_invitation_rate_limit(&limiter, 1).is_err());

        // User 2 should still be allowed (independent keys).
        assert!(check_admin_invitation_rate_limit(&limiter, 2).is_ok());
    }

    // --- test_check_retry_after_value ---

    #[test]
    fn test_check_retry_after_value() {
        // check_by_key uses RateLimitTier::Public config for retry_after (60s window).
        let limiter = make_limiter(1, 60, 1000);

        assert!(check_admin_invitation_rate_limit(&limiter, 1).is_ok());
        let result = check_admin_invitation_rate_limit(&limiter, 1);
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert_eq!(err.retry_after, 60);
    }

    // --- test_check_rate_limit_exceeded_into_response ---

    #[test]
    fn test_check_rate_limit_exceeded_into_response() {
        let limiter = make_limiter(1, 60, 1000);
        assert!(check_admin_invitation_rate_limit(&limiter, 1).is_ok());
        let err = check_admin_invitation_rate_limit(&limiter, 1).unwrap_err();

        let response: Response = err.into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            response.headers().get("x-retry-after"),
            Some(&HeaderValue::from(60))
        );
    }

    // --- test_check_window_not_expired_within_window ---

    #[test]
    fn test_check_window_not_expired_within_window() {
        let limiter = make_limiter(1, 60, 1000);

        // First request at t=1000.
        assert!(check_admin_invitation_rate_limit(&limiter, 1).is_ok());

        // Request at t=1030 (within 60s window) should still be limited.
        assert!(check_admin_invitation_rate_limit(&limiter, 1).is_err());
    }

    // --- test_check_no_superadmin_bypass ---

    #[test]
    fn test_check_no_superadmin_bypass() {
        let limiter = make_limiter(1, 60, 1000);

        // Even "superadmin" user_id=999 is subject to rate limiting.
        assert!(check_admin_invitation_rate_limit(&limiter, 999).is_ok());
        assert!(check_admin_invitation_rate_limit(&limiter, 999).is_err());
    }
}
