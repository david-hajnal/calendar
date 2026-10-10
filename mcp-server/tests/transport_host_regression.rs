//! Exercises production Host configuration and OAuth middleware with a mock issuer.
//! Requests initialize and list an empty MCP handler; no calendar events are accessed.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use rmcp::{
    ServerHandler,
    transport::streamable_http_server::{
        session::local::LocalSessionManager, tower::StreamableHttpService,
    },
};
use tower::ServiceExt;

#[derive(Clone)]
struct ReadOnlyHandler;
impl ServerHandler for ReadOnlyHandler {}

async fn initialize_status(host: &str) -> StatusCode {
    let service = StreamableHttpService::new(
        || Ok(ReadOnlyHandler),
        LocalSessionManager::default().into(),
        mcp_server::transport::server_config("https://mcal.hajnal.space/mcp").unwrap(),
    );
    let router = Router::new().nest_service("/mcp", service);
    let response = router.oneshot(Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", host)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"host-regression","version":"1"}}}"#))
        .unwrap()).await.unwrap();
    response.status()
}

#[tokio::test]
async fn public_hostname_initialize_is_accepted() {
    assert_eq!(initialize_status("mcal.hajnal.space").await, StatusCode::OK);
}

#[tokio::test]
async fn public_hostname_with_https_port_initialize_is_accepted() {
    assert_eq!(
        initialize_status("mcal.hajnal.space:443").await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn unexpected_hostname_initialize_is_rejected() {
    assert_eq!(
        initialize_status("unexpected.example").await,
        StatusCode::FORBIDDEN
    );
}

const RESOURCE: &str = "https://mcal.hajnal.space/mcp";

async fn authenticated_router() -> (Router, String, wiremock::MockServer) {
    use base64::Engine;
    use rsa::{pkcs8::EncodePrivateKey, traits::PublicKeyParts};
    let mock = wiremock::MockServer::start().await;
    let issuer = mock.uri();
    let private = rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 2048).unwrap();
    let public = private.to_public_key();
    let encoder = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let jwk = serde_json::json!({"kty":"RSA","alg":"RS256","use":"sig","kid":"regression","n":encoder.encode(public.n().to_bytes_be()),"e":encoder.encode(public.e().to_bytes_be())});
    for (path, body) in [
        (
            "/.well-known/oauth-authorization-server",
            serde_json::json!({"issuer":issuer,"jwks_uri":format!("{issuer}/jwks")}),
        ),
        ("/jwks", serde_json::json!({"keys":[jwk]})),
    ] {
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(path))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(body))
            .mount(&mock)
            .await;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = serde_json::json!({"iss":issuer,"aud":RESOURCE,"sub":"42","exp":now+300,"iat":now,"jti":"regression","client_id":"read-only-regression","scope":"commoncal.calendar.metadata.read commoncal.event.read.basic","amr":["pwd"]});
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some("regression".into());
    let pem = private.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF).unwrap();
    let token = jsonwebtoken::encode(
        &header,
        &claims,
        &jsonwebtoken::EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap(),
    )
    .unwrap();
    let service = StreamableHttpService::new(
        || Ok(ReadOnlyHandler),
        LocalSessionManager::default().into(),
        mcp_server::transport::server_config(RESOURCE).unwrap(),
    );
    let router =
        Router::new()
            .nest_service("/mcp", service)
            .layer(axum::middleware::from_fn_with_state(
                mcp_server::auth::AuthState::new(issuer, RESOURCE.into()),
                mcp_server::auth::auth_middleware,
            ));
    (router, token, mock)
}

fn rpc_request(
    host: &str,
    token: Option<&str>,
    session: Option<&str>,
    method: &str,
) -> Request<Body> {
    let params = if method == "initialize" {
        serde_json::json!({"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"host-regression","version":"1"}})
    } else {
        serde_json::json!({})
    };
    let mut builder = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", host)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    if let Some(session) = session {
        builder = builder
            .header("mcp-session-id", session)
            .header("mcp-protocol-version", "2025-03-26");
    }
    let mut message = serde_json::json!({"jsonrpc":"2.0","method":method,"params":params});
    if !method.starts_with("notifications/") {
        message["id"] = serde_json::json!(1);
    }
    builder.body(Body::from(message.to_string())).unwrap()
}

#[tokio::test]
async fn authenticated_public_initialize_and_tools_list_and_rejections() {
    let (router, token, _issuer) = authenticated_router().await;
    for host in ["mcal.hajnal.space", "mcal.hajnal.space:443"] {
        let initialized = router
            .clone()
            .oneshot(rpc_request(host, Some(&token), None, "initialize"))
            .await
            .unwrap();
        assert_eq!(initialized.status(), StatusCode::OK);
        let session = initialized
            .headers()
            .get("mcp-session-id")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let initialization = decoded_result(initialized).await;
        assert_eq!(initialization["protocolVersion"], "2025-03-26");
        let notification = router
            .clone()
            .oneshot(rpc_request(
                host,
                Some(&token),
                Some(&session),
                "notifications/initialized",
            ))
            .await
            .unwrap();
        assert_eq!(notification.status(), StatusCode::ACCEPTED);
        let listed = router
            .clone()
            .oneshot(rpc_request(
                host,
                Some(&token),
                Some(&session),
                "tools/list",
            ))
            .await
            .unwrap();
        assert_eq!(listed.status(), StatusCode::OK);
        assert!(decoded_result(listed).await["tools"].is_array());
    }
    for token in [None, Some("invalid-test-token")] {
        for method in ["initialize", "tools/list"] {
            let rejected = router
                .clone()
                .oneshot(rpc_request("mcal.hajnal.space", token, None, method))
                .await
                .unwrap();
            assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
            let challenge = rejected
                .headers()
                .get("www-authenticate")
                .unwrap()
                .to_str()
                .unwrap();
            assert!(challenge.starts_with("Bearer "));
            assert!(challenge.contains("resource_metadata=\"https://mcal.hajnal.space/.well-known/oauth-protected-resource\""));
        }
    }
    for host in ["unexpected.example", "mcal.hajnal.space.attacker.example"] {
        let mut request = rpc_request(host, Some(&token), None, "initialize");
        request
            .headers_mut()
            .insert("x-forwarded-host", "mcal.hajnal.space".parse().unwrap());
        let rejected = router.clone().oneshot(request).await.unwrap();
        assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    }
}

async fn decoded_result(response: axum::response::Response) -> serde_json::Value {
    use http_body_util::BodyExt;
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let text = std::str::from_utf8(&body).unwrap();
    let payload = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim))
        .find(|data| !data.is_empty())
        .unwrap_or(text);
    let message: serde_json::Value = serde_json::from_str(payload).unwrap();
    assert_eq!(message["jsonrpc"], "2.0");
    assert_eq!(message["id"], 1);
    assert!(message.get("error").is_none());
    message
        .get("result")
        .expect("successful JSON-RPC result")
        .clone()
}
