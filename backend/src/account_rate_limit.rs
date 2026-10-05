use crate::rate_limiter::FixedWindowRateLimiter;
use std::sync::Arc;

#[derive(Clone)]
pub struct AccountRateLimiter {
    emails: Arc<FixedWindowRateLimiter>,
    request_ips: Arc<FixedWindowRateLimiter>,
    email_actors: Arc<FixedWindowRateLimiter>,
    token_ips: Arc<FixedWindowRateLimiter>,
}
impl Default for AccountRateLimiter {
    fn default() -> Self {
        Self {
            emails: Arc::new(FixedWindowRateLimiter::new(5, 900)),
            request_ips: Arc::new(FixedWindowRateLimiter::new(20, 900)),
            email_actors: Arc::new(FixedWindowRateLimiter::new(5, 3600)),
            token_ips: Arc::new(FixedWindowRateLimiter::new(30, 900)),
        }
    }
}
impl AccountRateLimiter {
    pub fn new_at(now: i64) -> Self {
        Self {
            emails: Arc::new(FixedWindowRateLimiter::new_at(5, 900, now)),
            request_ips: Arc::new(FixedWindowRateLimiter::new_at(20, 900, now)),
            email_actors: Arc::new(FixedWindowRateLimiter::new_at(5, 3600, now)),
            token_ips: Arc::new(FixedWindowRateLimiter::new_at(30, 900, now)),
        }
    }
    pub fn allow_request(&self, email: &str, ip: &str) -> bool {
        // Check IP first so a blocked client cannot allocate unlimited email buckets.
        if !self.request_ips.check_by_key(ip).0 {
            return false;
        }
        self.emails.check_by_key(&email.trim().to_lowercase()).0
    }
    pub fn allow_email_change(&self, user_id: i64) -> bool {
        self.email_actors.check_by_key(&user_id.to_string()).0
    }
    pub fn allow_token(&self, ip: &str) -> bool {
        self.token_ips.check_by_key(ip).0
    }
}
