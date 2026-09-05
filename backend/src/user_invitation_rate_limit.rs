use std::sync::Arc;

use crate::rate_limiter::FixedWindowRateLimiter;
use crate::write_rate_limit::RateLimitExceeded;

/// Shared state for the user invitation rate limiter.
#[derive(Clone)]
pub struct UserInvitationRateLimiterState {
    pub limiter: Arc<FixedWindowRateLimiter>,
}

/// Default: 20 invitations per day (86400 seconds).
pub const USER_INVITATION_MAX_REQUESTS: u32 = 20;
pub const USER_INVITATION_WINDOW_SECONDS: i64 = 86_400;

/// Check rate limit for user-initiated invitation requests.
///
/// Returns `RateLimitExceeded` (429) when the user has exceeded the limit,
/// `Ok(())` when within the limit.
pub fn check_user_invitation_rate_limit(
    limiter: &UserInvitationRateLimiterState,
    user_id: i64,
) -> Result<(), RateLimitExceeded> {
    let key = format!("user_invitation:{}", user_id);
    let (allowed, retry_after) = limiter.limiter.check_by_key(&key);
    if !allowed {
        Err(RateLimitExceeded { retry_after })
    } else {
        Ok(())
    }
}

/// Check rate limit for email-based resend requests.
///
/// Returns `RateLimitExceeded` (429) when the email has exceeded the limit,
/// `Ok(())` when within the limit.
pub fn check_user_invitation_resend_rate_limit(
    limiter: &UserInvitationRateLimiterState,
    email: &str,
) -> Result<(), RateLimitExceeded> {
    let key = format!("user_invitation_resend:{}", email);
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
    ) -> UserInvitationRateLimiterState {
        let limiter = FixedWindowRateLimiter::new_at(max_requests, window_seconds, now);
        UserInvitationRateLimiterState {
            limiter: Arc::new(limiter),
        }
    }

    #[test]
    fn test_check_allows_under_limit() {
        let limiter = make_limiter(5, 86_400, 1000);

        for i in 0..5 {
            let result = check_user_invitation_rate_limit(&limiter, 1);
            assert!(result.is_ok(), "request {} should be allowed", i + 1);
        }
    }

    #[test]
    fn test_check_blocks_over_limit() {
        let limiter = make_limiter(5, 86_400, 1000);

        for i in 0..5 {
            let result = check_user_invitation_rate_limit(&limiter, 1);
            assert!(result.is_ok(), "request {} should be allowed", i + 1);
        }

        let result = check_user_invitation_rate_limit(&limiter, 1);
        assert!(result.is_err(), "6th request should be rate limited");
    }

    #[test]
    fn test_check_different_users_independent() {
        let limiter = make_limiter(1, 86_400, 1000);

        assert!(check_user_invitation_rate_limit(&limiter, 1).is_ok());
        assert!(check_user_invitation_rate_limit(&limiter, 1).is_err());
        assert!(check_user_invitation_rate_limit(&limiter, 2).is_ok());
    }

    #[test]
    fn test_resend_rate_limit_by_email() {
        let limiter = make_limiter(3, 86_400, 1000);

        assert!(check_user_invitation_resend_rate_limit(&limiter, "test@example.com").is_ok());
        assert!(check_user_invitation_resend_rate_limit(&limiter, "test@example.com").is_ok());
        assert!(check_user_invitation_resend_rate_limit(&limiter, "test@example.com").is_ok());
        assert!(
            check_user_invitation_resend_rate_limit(&limiter, "test@example.com").is_err(),
            "4th request for same email should be rate limited"
        );

        // Different email should be independent
        assert!(
            check_user_invitation_resend_rate_limit(&limiter, "other@example.com").is_ok(),
            "different email should be independent"
        );
    }
}
