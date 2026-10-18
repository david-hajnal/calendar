// Rate limiter module.
//
// Fixed-window rate limiter for MCP tool calls. Each key (a user + OAuth
// client pair) is allowed at most `limit` tool calls per `window`. State is
// in-memory and bounded: expired entries are pruned as the map grows, so a
// large number of distinct clients cannot grow the map without bound.
//
// The limiter is shared across all MCP sessions via `Arc`, so the window is
// enforced per key across concurrent requests and sessions.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// In-memory fixed-window rate limiter.
///
/// `check` is atomic per key: the first `limit` calls within a window are
/// allowed (and recorded); subsequent calls are denied until the window rolls
/// over. A `disabled` limiter always allows and records nothing.
#[derive(Clone)]
pub struct RateLimiter {
    inner: Arc<Inner>,
}

struct Inner {
    enabled: bool,
    limit: u64,
    window: Duration,
    counters: Mutex<HashMap<String, (u64, Instant)>>,
}

/// Prune expired entries once the map grows past this many keys.
const MAX_TRACKED_KEYS: usize = 4096;

impl RateLimiter {
    /// A limiter allowing at most `limit` calls per key within `window`.
    ///
    /// `limit` must be at least 1 and `window` non-zero for the limiter to be
    /// meaningful; configuration validation enforces this.
    pub fn new(limit: u64, window: Duration) -> Self {
        Self {
            inner: Arc::new(Inner {
                enabled: true,
                limit,
                window,
                counters: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// A limiter that always allows (for development and tests).
    pub fn disabled() -> Self {
        Self {
            inner: Arc::new(Inner {
                enabled: false,
                limit: 0,
                window: Duration::ZERO,
                counters: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Whether the limiter is enforcing a limit (false for `disabled`).
    pub fn is_enabled(&self) -> bool {
        self.inner.enabled
    }

    /// Check whether a call for `key` is allowed, recording it if so.
    pub fn check(&self, key: &str) -> bool {
        self.check_at(key, Instant::now())
    }

    /// Like `check`, but with an explicit clock so tests are deterministic.
    fn check_at(&self, key: &str, now: Instant) -> bool {
        if !self.inner.enabled {
            return true;
        }
        let mut counters = self.inner.counters.lock().unwrap();
        // Bound memory: drop expired entries once the map grows large.
        if counters.len() > MAX_TRACKED_KEYS {
            counters
                .retain(|_, (_, started)| now.duration_since(*started) < self.inner.window);
        }
        let limit = self.inner.limit;
        let window = self.inner.window;
        match counters.get_mut(key) {
            Some((count, started)) if now.duration_since(*started) < window => {
                if *count < limit {
                    *count += 1;
                    true
                } else {
                    false
                }
            }
            // New key, or the previous window has elapsed: start a fresh window.
            _ => {
                counters.insert(key.to_string(), (1, now));
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_always_allows() {
        let limiter = RateLimiter::disabled();
        assert!(!limiter.is_enabled());
        for _ in 0..100 {
            assert!(limiter.check("client-1"));
        }
    }

    #[test]
    fn allows_under_limit_then_denies() {
        let limiter = RateLimiter::new(3, Duration::from_secs(60));
        let t0 = Instant::now();
        assert!(limiter.check_at("client-1", t0));
        assert!(limiter.check_at("client-1", t0 + Duration::from_secs(1)));
        assert!(limiter.check_at("client-1", t0 + Duration::from_secs(2)));
        // Fourth call within the same window is denied.
        assert!(!limiter.check_at("client-1", t0 + Duration::from_secs(3)));
    }

    #[test]
    fn window_reset_allows_again() {
        let limiter = RateLimiter::new(2, Duration::from_secs(60));
        let t0 = Instant::now();
        assert!(limiter.check_at("client-1", t0));
        assert!(limiter.check_at("client-1", t0 + Duration::from_secs(1)));
        assert!(!limiter.check_at("client-1", t0 + Duration::from_secs(2)));
        // After the window elapses, the counter resets and calls are allowed.
        assert!(limiter.check_at("client-1", t0 + Duration::from_secs(61)));
        assert!(limiter.check_at("client-1", t0 + Duration::from_secs(62)));
        assert!(!limiter.check_at("client-1", t0 + Duration::from_secs(63)));
    }

    #[test]
    fn keys_are_isolated() {
        let limiter = RateLimiter::new(1, Duration::from_secs(60));
        let t0 = Instant::now();
        assert!(limiter.check_at("client-1", t0));
        assert!(!limiter.check_at("client-1", t0 + Duration::from_secs(1)));
        // A different key has its own budget: its first call is still allowed.
        assert!(limiter.check_at("client-2", t0));
        // ...and its own budget is exhausted independently.
        assert!(!limiter.check_at("client-2", t0 + Duration::from_secs(1)));
    }

    #[test]
    fn shared_arc_state_is_consistent() {
        let limiter = RateLimiter::new(2, Duration::from_secs(60));
        let clone = limiter.clone();
        let t0 = Instant::now();
        assert!(limiter.check_at("client-1", t0));
        assert!(clone.check_at("client-1", t0 + Duration::from_secs(1)));
        // The clone shares the same counter, so the budget is exhausted.
        assert!(!limiter.check_at("client-1", t0 + Duration::from_secs(2)));
    }
}
