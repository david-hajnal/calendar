// Rate limiter module.
//
// Fixed-window rate limiter for MCP tools.
// Slice 14 will implement real rate limiting.

use std::sync::{Arc, Mutex};
use std::collections::HashMap;

#[derive(Clone)]
pub struct RateLimiter {
    inner: Option<Arc<InnerLimiter>>,
}

struct InnerLimiter {
    limiter: Mutex<FixedWindowLimiter>,
}

struct FixedWindowLimiter {
    windows: HashMap<String, WindowCount>,
}

struct WindowCount {
    count: u64,
    window_start: i64,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            inner: Some(Arc::new(InnerLimiter {
                limiter: Mutex::new(FixedWindowLimiter {
                    windows: HashMap::new(),
                }),
            })),
        }
    }

    pub fn disabled() -> Self {
        Self { inner: None }
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
