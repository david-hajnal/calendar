use axum::body::Body;
use axum::http::header;
use axum::response::{IntoResponse, Response};

#[test]
fn content_length_behavior() {
    let body = "x".repeat(342);
    let mut response: Response = (
        axum::http::StatusCode::OK,
        [(header::CONTENT_TYPE, "text/calendar; charset=utf-8")],
        body.clone(),
    )
        .into_response();
    eprintln!(
        "before replace: cl = {:?}",
        response.headers().get(header::CONTENT_LENGTH)
    );
    *response.body_mut() = Body::empty();
    eprintln!(
        "after replace: cl = {:?}",
        response.headers().get(header::CONTENT_LENGTH)
    );

    // Variant: set content-length manually, then replace body.
    let mut response2: Response = (
        axum::http::StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/calendar; charset=utf-8"),
            (header::CONTENT_LENGTH, body.len().to_string().as_str()),
        ],
        body,
    )
        .into_response();
    eprintln!(
        "v2 before: cl = {:?}",
        response2.headers().get(header::CONTENT_LENGTH)
    );
    *response2.body_mut() = Body::empty();
    eprintln!(
        "v2 after: cl = {:?}",
        response2.headers().get(header::CONTENT_LENGTH)
    );
}
