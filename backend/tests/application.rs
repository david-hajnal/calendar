use axum::{
    body::Body,
    http::{Request, StatusCode, header::CONTENT_TYPE},
};
use commoncal_backend::{
    config::{AppConfig, Environment},
    http::build_router,
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

#[test]
fn production_configuration_rejects_a_missing_session_secret() {
    let result = AppConfig::new(Environment::Production, "127.0.0.1:3000", None);

    let error = result.expect_err("production must require a session secret");
    assert_eq!(
        error.to_string(),
        "SESSION_SECRET is required in production"
    );
}
