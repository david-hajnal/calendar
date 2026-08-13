use axum::{
    extract::{ConnectInfo, State, Request},
    middleware::Next,
    response::Response,
};
use std::net::SocketAddr as StdSocketAddr;
use std::sync::Arc;
use crate::rate_limiter::FixedWindowRateLimiter;
use crate::write_rate_limit::RateLimitExceeded;

/// Shared state for the public rate limiter.
#[derive(Clone)]
pub struct PublicRateLimiterState {
    pub limiter: Arc<FixedWindowRateLimiter>,
}

/// Middleware that enforces public rate limits on unauthenticated clients.
pub async fn public_rate_limit_middleware(
    State(limiter_state): State<PublicRateLimiterState>,
    connect_info: Option<ConnectInfo<StdSocketAddr>>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, RateLimitExceeded> {
    let ip = request
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|v| v.trim().to_string())
        .or_else(|| connect_info.map(|ci| ci.ip().to_string()))
        .unwrap_or_else(|| "unknown".to_owned());
    let key = format!("public:{}", ip);
    let (allowed, retry_after) = limiter_state.limiter.check_by_key(&key);
    if !allowed {
        return Err(RateLimitExceeded { retry_after });
    }
    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::middleware;
    use tower::ServiceExt;

    fn make_limiter(max_requests: u32, window_seconds: i64, now: i64) -> PublicRateLimiterState {
        let limiter = FixedWindowRateLimiter::new_at(max_requests, window_seconds, now);
        PublicRateLimiterState {
            limiter: Arc::new(limiter),
        }
    }

    fn build_app(
        limiter_state: PublicRateLimiterState,
    ) -> axum::Router {
        axum::Router::new()
            .route(
                "/api/v1/calendars",
                axum::routing::get(handler).post(handler),
            )
            .layer(middleware::from_fn_with_state(limiter_state, public_rate_limit_middleware))
    }

    async fn handler(_: axum::extract::Request) -> &'static str {
        "ok"
    }

    fn make_request(method: &str, path: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap()
    }

    // --- test_allows_under_limit ---

    #[tokio::test]
    async fn test_allows_under_limit() {
        let limiter_state = make_limiter(5, 60, 1000);
        let app = build_app(limiter_state);

        let request = make_request("GET", "/api/v1/calendars");
        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    // --- test_blocks_over_limit ---

    #[tokio::test]
    async fn test_blocks_over_limit() {
        let limiter_state = make_limiter(2, 60, 1000);

        // Exhaust the limit with prior requests.
        for _ in 0..2 {
            let app = build_app(limiter_state.clone());
            let request = make_request("GET", "/api/v1/calendars");
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }

        // Next request should be blocked.
        let app = build_app(limiter_state);
        let request = make_request("GET", "/api/v1/calendars");
        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    // --- test_retry_after_header ---

    #[tokio::test]
    async fn test_retry_after_header() {
        let limiter_state = make_limiter(1, 60, 1000);

        // First request goes through.
        let app1 = build_app(limiter_state.clone());
        let request1 = make_request("GET", "/api/v1/calendars");
        let response1 = app1.oneshot(request1).await.unwrap();
        assert_eq!(response1.status(), StatusCode::OK);

        // Second request is rate limited.
        let app2 = build_app(limiter_state);
        let request2 = make_request("GET", "/api/v1/calendars");
        let response2 = app2.oneshot(request2).await.unwrap();

        assert_eq!(response2.status(), StatusCode::TOO_MANY_REQUESTS);

        let retry_after = response2
            .headers()
            .get("x-retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<i64>().ok());
        assert!(retry_after.is_some(), "x-retry-after header should be present");
        assert!(retry_after.unwrap() > 0, "retry_after should be positive");
    }

    // --- test_unknown_ip_fallback ---

    #[tokio::test]
    async fn test_unknown_ip_fallback() {
        let limiter_state = make_limiter(1, 60, 1000);

        // First request goes through with unknown IP.
        let app1 = build_app(limiter_state.clone());
        let request1 = make_request("GET", "/api/v1/calendars");
        let response1 = app1.oneshot(request1).await.unwrap();
        assert_eq!(response1.status(), StatusCode::OK);

        // Second request with no ConnectInfo (unknown IP) should be blocked.
        let app2 = build_app(limiter_state);
        let request2 = make_request("GET", "/api/v1/calendars");
        let response2 = app2.oneshot(request2).await.unwrap();

        assert_eq!(response2.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    // --- test_independent_ips ---

    #[tokio::test]
    async fn test_independent_ips() {
        let limiter_state = make_limiter(1, 60, 1000);

        // Exhaust limit for "unknown" IP (no ConnectInfo).
        let app1 = build_app(limiter_state.clone());
        let request1 = make_request("GET", "/api/v1/calendars");
        let response1 = app1.oneshot(request1).await.unwrap();
        assert_eq!(response1.status(), StatusCode::OK);

        let app2 = build_app(limiter_state);
        let request2 = make_request("GET", "/api/v1/calendars");
        let response2 = app2.oneshot(request2).await.unwrap();
        assert_eq!(response2.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    // --- test_no_limiter_configured ---

    #[tokio::test]
    async fn test_no_limiter_configured() {
        let limiter_state = make_limiter(1_000_000, 60, 1000);

        for i in 0..100 {
            let app = build_app(limiter_state.clone());
            let request = make_request("GET", "/api/v1/calendars");
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "request {} should be allowed", i + 1);
        }
    }
}
