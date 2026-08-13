use crate::rate_limiter::{FixedWindowRateLimiter, WriteRateLimitKey, write_endpoint_tier};
use crate::sessions::AuthenticatedSession;
use axum::{
    Extension,
    body::Body,
    extract::{Request, State},
    http::{HeaderName, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::sync::Arc;

/// Shared state for the write rate limiter.
///
/// `FixedWindowRateLimiter` does not implement `Clone`, so we wrap it in `Arc`
/// to allow sharing across middleware invocations and tests.
#[derive(Clone)]
pub struct WriteRateLimiterState {
    pub limiter: Arc<FixedWindowRateLimiter>,
}

/// Rate limit exceeded error response.
#[derive(Debug)]
pub struct RateLimitExceeded {
    pub retry_after: i64,
}

impl IntoResponse for RateLimitExceeded {
    fn into_response(self) -> Response {
        let mut response = Response::new(Body::from(
            serde_json::json!({
                "error": {
                    "code": "rate_limited",
                    "message": "Too many requests, try again later",
                }
            })
            .to_string(),
        ));
        *response.status_mut() = StatusCode::TOO_MANY_REQUESTS;
        response.headers_mut().insert(
            HeaderName::from_static("x-retry-after"),
            HeaderValue::from(self.retry_after),
        );
        response
    }
}

/// Middleware that enforces write-rate limits on authenticated users.
///
/// Must run AFTER `authenticated_session` middleware (which provides AuthenticatedSession extension).
pub async fn write_rate_limit_middleware(
    State(limiter_state): State<WriteRateLimiterState>,
    Extension(session): Extension<AuthenticatedSession>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, RateLimitExceeded> {
    // Superadmins bypass rate limiting entirely.
    if session.user.is_superadmin {
        return Ok(next.run(request).await);
    }

    // Only apply to write endpoints; non-write endpoints bypass.
    let tier = write_endpoint_tier(request.method().as_str(), request.uri().path());
    let tier = match tier {
        Some(t) => t,
        None => return Ok(next.run(request).await),
    };

    let key = WriteRateLimitKey {
        user_id: session.user.id,
        tier,
    };

    let (allowed, retry_after) = limiter_state.limiter.check(&key);

    if !allowed {
        Err(RateLimitExceeded { retry_after })
    } else {
        Ok(next.run(request).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use axum::middleware;
    use tower::ServiceExt;

    fn make_limiter(max_requests: u32, window_seconds: i64, now: i64) -> WriteRateLimiterState {
        let limiter = FixedWindowRateLimiter::new_at(max_requests, window_seconds, now);
        WriteRateLimiterState {
            limiter: Arc::new(limiter),
        }
    }

    fn make_session(user_id: i64, is_superadmin: bool) -> AuthenticatedSession {
        let secret_key = crate::security::SecretKey::generate();
        let token = secret_key.generate_token();
        let csrf_token = secret_key.generate_csrf_token(&token).expose().to_owned();

        AuthenticatedSession::new_for_test(
            user_id,
            token,
            csrf_token,
            crate::invitations::ActiveUser {
                id: user_id,
                email: "test@example.com".into(),
                display_name: Some("Test User".into()),
                status: "active",
                is_superadmin,
            },
            1000,
            1000,
            4600,
        )
    }

    fn build_app(
        limiter_state: WriteRateLimiterState,
        session: AuthenticatedSession,
    ) -> axum::Router {
        axum::Router::new()
            .route(
                "/api/v1/calendars/:id/events",
                axum::routing::get(handler).post(handler),
            )
            .layer(middleware::from_fn_with_state(
                limiter_state,
                write_rate_limit_middleware,
            ))
            .layer(Extension(session))
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

    // --- test_middleware_allows_under_limit ---

    #[tokio::test]
    async fn test_middleware_allows_under_limit() {
        let limiter_state = make_limiter(5, 60, 1000);
        let session = make_session(1, false);
        let app = build_app(limiter_state, session);

        let request = make_request("POST", "/api/v1/calendars/:id/events");
        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    // --- test_middleware_blocks_over_limit ---

    #[tokio::test]
    async fn test_middleware_blocks_over_limit() {
        let limiter_state = make_limiter(2, 60, 1000);
        let session = make_session(1, false);

        // Exhaust the limit with prior requests.
        for _ in 0..2 {
            let app = build_app(limiter_state.clone(), session.clone());
            let request = make_request("POST", "/api/v1/calendars/:id/events");
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }

        // Next request should be blocked.
        let app = build_app(limiter_state, session);
        let request = make_request("POST", "/api/v1/calendars/:id/events");
        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    // --- test_middleware_superadmin_bypass ---

    #[tokio::test]
    async fn test_middleware_superadmin_bypass() {
        // Exhaust limit for a non-superadmin.
        let limiter_state = make_limiter(1, 60, 1000);
        let non_admin = make_session(1, false);

        let app1 = build_app(limiter_state.clone(), non_admin.clone());
        let request1 = make_request("POST", "/api/v1/calendars/:id/events");
        let response1 = app1.oneshot(request1).await.unwrap();
        assert_eq!(response1.status(), StatusCode::OK);

        let app2 = build_app(limiter_state, non_admin);
        let request2 = make_request("POST", "/api/v1/calendars/:id/events");
        let response2 = app2.oneshot(request2).await.unwrap();
        assert_eq!(response2.status(), StatusCode::TOO_MANY_REQUESTS);

        // Superadmin should still be allowed.
        let limiter_state_sa = make_limiter(1, 60, 1000);
        let session_sa = make_session(999, true);
        let app_sa = build_app(limiter_state_sa, session_sa);
        let request_sa = make_request("POST", "/api/v1/calendars/:id/events");
        let response_sa = app_sa.oneshot(request_sa).await.unwrap();

        assert_eq!(response_sa.status(), StatusCode::OK);
    }

    // --- test_middleware_non_write_methods_unaffected ---

    #[tokio::test]
    async fn test_middleware_non_write_methods_unaffected() {
        let limiter_state = make_limiter(1, 60, 1000);
        let session = make_session(1, false);

        // GET requests should bypass rate limiting.
        let app = build_app(limiter_state.clone(), session.clone());
        let request = make_request("GET", "/api/v1/calendars/:id/events");
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let app = build_app(limiter_state, session);
        let request = make_request("GET", "/api/v1/calendars/:id/events");
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    // --- test_middleware_retry_after_header ---

    #[tokio::test]
    async fn test_middleware_retry_after_header() {
        let limiter_state = make_limiter(1, 60, 1000);
        let session = make_session(1, false);

        // First request goes through.
        let app1 = build_app(limiter_state.clone(), session.clone());
        let request1 = make_request("POST", "/api/v1/calendars/:id/events");
        let response1 = app1.oneshot(request1).await.unwrap();
        assert_eq!(response1.status(), StatusCode::OK);

        // Second request is rate limited.
        let app2 = build_app(limiter_state, session);
        let request2 = make_request("POST", "/api/v1/calendars/:id/events");
        let response2 = app2.oneshot(request2).await.unwrap();

        assert_eq!(response2.status(), StatusCode::TOO_MANY_REQUESTS);

        let retry_after = response2
            .headers()
            .get("x-retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<i64>().ok());
        assert!(
            retry_after.is_some(),
            "x-retry-after header should be present"
        );
        assert!(retry_after.unwrap() > 0, "retry_after should be positive");
    }

    // --- test_middleware_no_limiter_configured ---
    // When limiter has effectively infinite capacity, all requests go through.

    #[tokio::test]
    async fn test_middleware_no_limiter_configured() {
        let limiter_state = make_limiter(1_000_000, 60, 1000);
        let session = make_session(1, false);

        for i in 0..100 {
            let app = build_app(limiter_state.clone(), session.clone());
            let request = make_request("POST", "/api/v1/calendars/:id/events");
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "request {} should be allowed",
                i + 1
            );
        }
    }
}
