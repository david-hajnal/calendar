// Rate limiter module.
//
// Fixed-window rate limiter for MCP tools.
// Slice 14 will implement real rate limiting.

// No state is needed until real rate limiting is implemented.
#[derive(Clone, Default)]
pub struct RateLimiter;

impl RateLimiter {
    pub fn new() -> Self {
        Self
    }

    pub fn disabled() -> Self {
        Self
    }

    /// Check if a request is allowed under the rate limit.
    ///
    /// For the tracer bullet, always allows.
    /// Slice 14 will implement real rate limiting.
    pub fn check(&self, _key: &str, _limit: u64, _window_secs: i64) -> bool {
        // Tracer bullet: always allow.
        true
    }
}
