use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use std::sync::Mutex;

/// In-memory fixed-window rate limiter for write endpoints.
#[allow(dead_code)]
pub struct FixedWindowRateLimiter {
    max_requests: u32,
    window_seconds: i64,
    buckets: Mutex<HashMap<String, RateLimitBucket>>,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}

pub struct RateLimitBucket {
    pub window_started_at: i64,
    pub attempts: u32,
}

impl FixedWindowRateLimiter {
    pub fn new(max_requests: u32, window_seconds: i64) -> Self {
        assert!(max_requests > 0, "max_requests must be positive");
        assert!(window_seconds > 0, "window_seconds must be positive");
        Self {
            max_requests,
            window_seconds,
            buckets: Mutex::new(HashMap::new()),
            clock: Arc::new(|| {
                Utc::now()
                    .timestamp()
            }),
        }
    }

    pub fn new_at(max_requests: u32, window_seconds: i64, now: i64) -> Self {
        let mut limiter = Self::new(max_requests, window_seconds);
        limiter.clock = Arc::new(move || now);
        limiter
    }

    pub fn check(&self, key: &WriteRateLimitKey) -> (bool, i64) {
        let bucket_key = format!("user:{}:tier:{}", key.user_id, key.tier);
        let now = (self.clock)();
        let mut buckets = self.buckets.lock().unwrap();
        let tier_config = key.tier.config();
        let bucket = buckets.entry(bucket_key).or_insert_with(|| RateLimitBucket {
            window_started_at: now,
            attempts: 0,
        });
        if now - bucket.window_started_at >= self.window_seconds {
            bucket.window_started_at = now;
            bucket.attempts = 0;
        }
        if bucket.attempts >= self.max_requests {
            let retry_after = tier_config.window_seconds - (now - bucket.window_started_at);
            return (false, retry_after);
        }
        bucket.attempts += 1;
        (true, 0)
    }

    pub fn check_by_key(&self, key: &str) -> (bool, i64) {
        let now = (self.clock)();
        let mut buckets = self.buckets.lock().unwrap();
        let tier_config = RateLimitTier::Public.config();
        let bucket = buckets.entry(key.to_string()).or_insert_with(|| RateLimitBucket {
            window_started_at: now,
            attempts: 0,
        });
        if now - bucket.window_started_at >= self.window_seconds {
            bucket.window_started_at = now;
            bucket.attempts = 0;
        }
        if bucket.attempts >= self.max_requests {
            let retry_after = tier_config.window_seconds - (now - bucket.window_started_at);
            return (false, retry_after);
        }
        bucket.attempts += 1;
        // Evict buckets that have been idle for longer than the window.
        buckets.retain(|_, b| now - b.window_started_at < self.window_seconds);
        (true, 0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateLimitTier {
    Critical,
    Standard,
    Permissive,
    Public,
}

impl RateLimitTier {
    pub fn config(&self) -> RateLimitConfig {
        match self {
            RateLimitTier::Critical => RateLimitConfig {
                max_requests: 10,
                window_seconds: 60,
            },
            RateLimitTier::Standard => RateLimitConfig {
                max_requests: 30,
                window_seconds: 60,
            },
            RateLimitTier::Permissive => RateLimitConfig {
                max_requests: 60,
                window_seconds: 60,
            },
            RateLimitTier::Public => RateLimitConfig {
                max_requests: 15,
                window_seconds: 60,
            },
        }
    }
}

impl std::fmt::Display for RateLimitTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RateLimitTier::Critical => write!(f, "critical"),
            RateLimitTier::Standard => write!(f, "standard"),
            RateLimitTier::Permissive => write!(f, "permissive"),
            RateLimitTier::Public => write!(f, "public"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateLimitConfig {
    pub max_requests: u32,
    pub window_seconds: i64,
}

#[derive(Clone, Debug)]
pub struct WriteRateLimitKey {
    pub user_id: i64,
    pub tier: RateLimitTier,
}

/// Determine the rate-limit tier for a write endpoint based on HTTP method and path.
///
/// Path params like `:id`, `:user_id` are treated as wildcards.
/// The path is split by `/` and matched segment by segment.
pub fn write_endpoint_tier(method: &str, path: &str) -> Option<RateLimitTier> {
    let segments: Vec<&str> = path.split('/').collect();
    let method = method.to_uppercase();

    // Helper: check if a segment is a path parameter (starts with `:`)
    let is_param = |s: &str| s.starts_with(':');

    // Helper: match a segment against a literal or wildcard
    let matches = |seg: &str, target: &str| seg == target || is_param(seg);

    // Only allow write methods
    let is_write = matches!(method.as_str(), "POST" | "PATCH" | "DELETE" | "PUT");
    if !is_write {
        return None;
    }

    // `*/acl/*/` + (PUT or DELETE) → Critical
    for i in 0..segments.len().saturating_sub(1) {
        if segments[i] == "acl"
            && is_param(segments[i + 1])
            && (method == "PUT" || method == "DELETE")
        {
            return Some(RateLimitTier::Critical);
        }
    }

    // `*/transfer` + POST → Critical
    if segments.len() >= 2 && segments.last() == Some(&"transfer") && method == "POST" {
        return Some(RateLimitTier::Critical);
    }

    // `*/events` + (POST/PATCH/DELETE) → Standard
    // Match both `*/events` and `*/events/*`
    for i in 0..segments.len() {
        if segments[i] == "events" {
            // Match if "events" is the last segment or followed by a parameter
            if i + 1 == segments.len() || (i + 1 < segments.len() && is_param(segments[i + 1])) {
                return Some(RateLimitTier::Standard);
            }
        }
    }

    // `*/occurrences/*/` + (PATCH or DELETE) → Standard
    for i in 0..segments.len().saturating_sub(1) {
        if segments[i] == "occurrences"
            && is_param(segments[i + 1])
            && (method == "PATCH" || method == "DELETE")
        {
            return Some(RateLimitTier::Standard);
        }
    }

    // `*/occurrences/*/following` + PATCH → Standard
    for i in 0..segments.len().saturating_sub(2) {
        if segments[i] == "occurrences"
            && is_param(segments[i + 1])
            && matches(segments[i + 2], "following")
            && method == "PATCH"
        {
            return Some(RateLimitTier::Standard);
        }
    }

    // `*/external-feeds/*/` + (DELETE) → Standard
    for i in 0..segments.len().saturating_sub(1) {
        if segments[i] == "external-feeds"
            && is_param(segments[i + 1])
            && method == "DELETE"
        {
            return Some(RateLimitTier::Standard);
        }
    }

    // `*/external-feeds/*/disable` + POST → Standard
    for i in 0..segments.len().saturating_sub(2) {
        if segments[i] == "external-feeds"
            && is_param(segments[i + 1])
            && matches(segments[i + 2], "disable")
            && method == "POST"
        {
            return Some(RateLimitTier::Standard);
        }
    }

    // `*/external-feeds/*/refresh` + POST → Standard
    for i in 0..segments.len().saturating_sub(2) {
        if segments[i] == "external-feeds"
            && is_param(segments[i + 1])
            && matches(segments[i + 2], "refresh")
            && method == "POST"
        {
            return Some(RateLimitTier::Standard);
        }
    }

    // `*/calendars` + (POST/PATCH/DELETE) → Permissive
    if segments.len() >= 2 && segments.last() == Some(&"calendars") && method != "GET" {
        return Some(RateLimitTier::Permissive);
    }

    // `*/calendars/*/archive` + POST → Permissive
    for i in 0..segments.len().saturating_sub(2) {
        if segments[i] == "calendars"
            && is_param(segments[i + 1])
            && matches(segments[i + 2], "archive")
            && method == "POST"
        {
            return Some(RateLimitTier::Permissive);
        }
    }

    // `*/calendars/*/restore` + POST → Permissive
    for i in 0..segments.len().saturating_sub(2) {
        if segments[i] == "calendars"
            && is_param(segments[i + 1])
            && matches(segments[i + 2], "restore")
            && method == "POST"
        {
            return Some(RateLimitTier::Permissive);
        }
    }

    // `*/views/*/` + (POST/PATCH/DELETE) → Permissive
    for i in 0..segments.len().saturating_sub(1) {
        if segments[i] == "views"
            && is_param(segments[i + 1])
            && method != "GET"
        {
            return Some(RateLimitTier::Permissive);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(user_id: i64, tier: RateLimitTier) -> WriteRateLimitKey {
        WriteRateLimitKey {
            user_id,
            tier,
        }
    }

    // --- FixedWindowRateLimiter tests ---

    #[test]
    fn test_check_allows_within_limit() {
        let limiter = FixedWindowRateLimiter::new_at(5, 60, 1000);
        let key = make_key(1, RateLimitTier::Critical);
        for i in 0..5 {
            let (allowed, retry_after) = limiter.check(&key);
            assert!(allowed, "request {} should be allowed", i + 1);
            assert_eq!(retry_after, 0);
        }
    }

    #[test]
    fn test_check_blocks_over_limit() {
        let limiter = FixedWindowRateLimiter::new_at(3, 60, 1000);
        let key = make_key(1, RateLimitTier::Critical);
        for _ in 0..3 {
            limiter.check(&key);
        }
        let (allowed, retry_after) = limiter.check(&key);
        assert!(!allowed);
        assert!(retry_after > 0);
    }

    #[test]
    fn test_check_resets_after_window() {
        let limiter = FixedWindowRateLimiter::new_at(2, 60, 1000);
        let key = make_key(1, RateLimitTier::Critical);
        limiter.check(&key);
        limiter.check(&key);
        let (allowed, _) = limiter.check(&key);
        assert!(!allowed);
        // Advance past the window
        let limiter = FixedWindowRateLimiter::new_at(2, 60, 1061);
        let (allowed, _) = limiter.check(&key);
        assert!(allowed);
    }

    #[test]
    fn test_check_independent_keys() {
        let limiter = FixedWindowRateLimiter::new_at(1, 60, 1000);
        let key1 = make_key(1, RateLimitTier::Critical);
        let key2 = make_key(2, RateLimitTier::Critical);
        limiter.check(&key1);
        let (allowed, _) = limiter.check(&key1);
        assert!(!allowed);
        let (allowed, _) = limiter.check(&key2);
        assert!(allowed);
    }

    #[test]
    fn test_check_independent_tiers() {
        let limiter = FixedWindowRateLimiter::new_at(1, 60, 1000);
        let key_critical = make_key(1, RateLimitTier::Critical);
        let key_standard = make_key(1, RateLimitTier::Standard);
        limiter.check(&key_critical);
        let (allowed, _) = limiter.check(&key_critical);
        assert!(!allowed);
        let (allowed, _) = limiter.check(&key_standard);
        assert!(allowed);
    }

    #[test]
    fn test_check_retry_after_increases() {
        let limiter = FixedWindowRateLimiter::new_at(2, 60, 1000);
        let key = make_key(1, RateLimitTier::Critical);
        limiter.check(&key);
        limiter.check(&key);
        let (_, retry_after_1) = limiter.check(&key);
        // retry_after should be ~59 (60 - 1 second elapsed)
        assert!(retry_after_1 >= 58 && retry_after_1 <= 60);
    }

    // --- write_endpoint_tier tests ---

    #[test]
    fn test_write_endpoint_tier_critical_acl_put() {
        let tier = write_endpoint_tier("PUT", "/api/v1/calendars/:id/acl/:user_id");
        assert_eq!(tier, Some(RateLimitTier::Critical));
    }

    #[test]
    fn test_write_endpoint_tier_critical_acl_delete() {
        let tier = write_endpoint_tier("DELETE", "/api/v1/calendars/:id/acl/:user_id");
        assert_eq!(tier, Some(RateLimitTier::Critical));
    }

    #[test]
    fn test_write_endpoint_tier_critical_transfer() {
        let tier = write_endpoint_tier("POST", "/api/v1/calendars/:id/transfer");
        assert_eq!(tier, Some(RateLimitTier::Critical));
    }

    #[test]
    fn test_write_endpoint_tier_standard_event_create() {
        let tier = write_endpoint_tier("POST", "/api/v1/calendars/:id/events");
        assert_eq!(tier, Some(RateLimitTier::Standard));
    }

    #[test]
    fn test_write_endpoint_tier_standard_event_update() {
        let tier = write_endpoint_tier("PATCH", "/api/v1/calendars/:id/events/:event_id");
        assert_eq!(tier, Some(RateLimitTier::Standard));
    }

    #[test]
    fn test_write_endpoint_tier_standard_event_delete() {
        let tier = write_endpoint_tier("DELETE", "/api/v1/calendars/:id/events/:event_id");
        assert_eq!(tier, Some(RateLimitTier::Standard));
    }

    #[test]
    fn test_write_endpoint_tier_standard_occurrence_update() {
        let tier = write_endpoint_tier("PATCH", "/api/v1/calendars/:id/occurrences/:occ_id");
        assert_eq!(tier, Some(RateLimitTier::Standard));
    }

    #[test]
    fn test_write_endpoint_tier_standard_occurrence_following() {
        let tier = write_endpoint_tier("PATCH", "/api/v1/calendars/:id/occurrences/:occ_id/following");
        assert_eq!(tier, Some(RateLimitTier::Standard));
    }

    #[test]
    fn test_write_endpoint_tier_standard_feed() {
        let tier = write_endpoint_tier("DELETE", "/api/v1/calendars/:id/external-feeds/:feed_id");
        assert_eq!(tier, Some(RateLimitTier::Standard));
    }

    #[test]
    fn test_write_endpoint_tier_permissive_calendar() {
        let tier = write_endpoint_tier("POST", "/api/v1/calendars");
        assert_eq!(tier, Some(RateLimitTier::Permissive));
    }

    #[test]
    fn test_write_endpoint_tier_permissive_archive() {
        let tier = write_endpoint_tier("POST", "/api/v1/calendars/:id/archive");
        assert_eq!(tier, Some(RateLimitTier::Permissive));
    }

    #[test]
    fn test_write_endpoint_tier_permissive_views() {
        let tier = write_endpoint_tier("POST", "/api/v1/calendars/:id/views/:view_id");
        assert_eq!(tier, Some(RateLimitTier::Permissive));
    }

    #[test]
    fn test_write_endpoint_tier_read_endpoints_none() {
        let tier = write_endpoint_tier("GET", "/api/v1/calendars/:id/events");
        assert_eq!(tier, None);
    }

    #[test]
    fn test_write_endpoint_tier_auth_endpoints_none() {
        let tier = write_endpoint_tier("POST", "/api/v1/auth/login");
        assert_eq!(tier, None);
    }

    #[test]
    fn test_write_endpoint_tier_health_none() {
        let tier = write_endpoint_tier("GET", "/health");
        assert_eq!(tier, None);
    }

    // --- Public tier tests ---

    #[test]
    fn test_public_tier_config() {
        let config = RateLimitTier::Public.config();
        assert_eq!(config.max_requests, 15);
        assert_eq!(config.window_seconds, 60);
    }

    #[test]
    fn test_check_by_key_allows_within_limit() {
        let limiter = FixedWindowRateLimiter::new_at(15, 60, 1000);
        for i in 0..15 {
            let (allowed, retry_after) = limiter.check_by_key("ip:1.2.3.4");
            assert!(allowed, "request {} should be allowed", i + 1);
            assert_eq!(retry_after, 0);
        }
    }

    #[test]
    fn test_check_by_key_blocks_over_limit() {
        let limiter = FixedWindowRateLimiter::new_at(15, 60, 1000);
        for _ in 0..15 {
            limiter.check_by_key("ip:1.2.3.4");
        }
        let (allowed, retry_after) = limiter.check_by_key("ip:1.2.3.4");
        assert!(!allowed);
        assert!(retry_after > 0);
    }

    #[test]
    fn test_check_by_key_independent_keys() {
        let limiter = FixedWindowRateLimiter::new_at(15, 60, 1000);
        for _ in 0..15 {
            limiter.check_by_key("ip:1.2.3.4");
        }
        let (allowed, _) = limiter.check_by_key("admin:42");
        assert!(allowed);
    }

    #[test]
    fn test_check_by_key_resets_after_window() {
        let limiter = FixedWindowRateLimiter::new_at(15, 60, 1000);
        for _ in 0..15 {
            limiter.check_by_key("ip:1.2.3.4");
        }
        let limiter = FixedWindowRateLimiter::new_at(15, 60, 1061);
        let (allowed, _) = limiter.check_by_key("ip:1.2.3.4");
        assert!(allowed);
    }
}
