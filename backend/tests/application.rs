use axum::{
    body::Body,
    http::{Request, StatusCode, header::CONTENT_TYPE},
};
use commoncal_backend::{
    config::{AppConfig, Environment},
    http::{ResponseSecurityConfig, build_router, secure_responses, serve_frontend},
};
use http_body_util::BodyExt;
use tower::ServiceExt;

#[tokio::test]
async fn health_endpoints_report_success() {
    let app = build_router();

    for path in ["/health/live", "/health/ready"] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(CONTENT_TYPE).unwrap(),
            "application/json"
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, r#"{"status":"ok"}"#);
    }
}

#[tokio::test]
async fn request_id_is_propagated_to_the_response() {
    let app = build_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/health/live")
                .header("x-request-id", "test-request-123")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.headers().get("x-request-id").unwrap(),
        "test-request-123"
    );
}

#[tokio::test]
async fn unknown_routes_return_the_safe_json_error_envelope() {
    let app = build_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/does-not-exist")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response.headers().get(CONTENT_TYPE).unwrap(),
        "application/json"
    );
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        body,
        r#"{"error":{"code":"not_found","message":"Resource not found"}}"#
    );
}

#[tokio::test]
async fn json_mutations_reject_oversized_bodies() {
    let app = build_router();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login-links")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(vec![b'x'; 1024 * 1024 + 1]))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn json_mutations_reject_non_json_content_types() {
    let app = build_router();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login-links")
                .header(CONTENT_TYPE, "text/plain")
                .body(Body::from(r#"{"email":"member@example.test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn json_mutations_within_the_limit_reach_the_handler() {
    let app = build_router();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login-links")
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"email":"member@example.test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn compiled_frontend_is_served_by_the_application_router() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("index.html"),
        "<main>CommonCal</main>",
    )
    .unwrap();

    let response = serve_frontend(build_router(), directory.path())
        .oneshot(
            Request::builder()
                .uri("/calendar")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body, "<main>CommonCal</main>");
}

#[tokio::test]
async fn response_hardening_covers_html_api_public_and_authentication_routes() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("index.html"),
        "<main>CommonCal</main>",
    )
    .unwrap();
    let app = serve_frontend(build_router(), directory.path());

    for (path, cache_control, robots) in [
        ("/calendar", "no-cache", None),
        ("/health/live", "no-store", None),
        (
            "/api/v1/public/views/example",
            "private, no-store",
            Some("noindex, nofollow"),
        ),
        ("/api/v1/auth/login-links", "no-store", None),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.headers().get("content-security-policy").unwrap(),
            "default-src 'self'; base-uri 'self'; object-src 'none'; frame-ancestors 'none'"
        );
        assert_eq!(
            response.headers().get("x-content-type-options").unwrap(),
            "nosniff"
        );
        assert_eq!(
            response.headers().get("referrer-policy").unwrap(),
            "strict-origin-when-cross-origin"
        );
        assert_eq!(
            response.headers().get("permissions-policy").unwrap(),
            "camera=(), geolocation=(), microphone=(), payment=(), usb=()"
        );
        assert_eq!(
            response.headers().get("cache-control").unwrap(),
            cache_control
        );
        assert_eq!(
            response
                .headers()
                .get("x-robots-tag")
                .and_then(|value| value.to_str().ok()),
            robots
        );
        assert!(
            response
                .headers()
                .get("strict-transport-security")
                .is_none()
        );
    }
}

#[tokio::test]
async fn hsts_is_emitted_only_for_explicit_production_https_configuration() {
    let response = secure_responses(build_router(), ResponseSecurityConfig::production_https())
        .oneshot(
            Request::builder()
                .uri("/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.headers().get("strict-transport-security").unwrap(),
        "max-age=31536000; includeSubDomains"
    );
}

#[test]
fn production_configuration_rejects_a_missing_session_secret() {
    let result = AppConfig::new(Environment::Production, "127.0.0.1:3000", None);

    let error = result.expect_err("production must require a session secret");
    assert_eq!(
        error.to_string(),
        "SESSION_SECRET is required in production"
    );
}
