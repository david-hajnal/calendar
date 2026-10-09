// Bearer-token authentication for the complete MCP service.
//
// This middleware wraps the entire rmcp Streamable HTTP service so that the
// first unauthenticated `initialize` receives a standards-compliant 401
// challenge (RFC 6750 `WWW-Authenticate`) whose `resource_metadata` parameter
// is the public protected-resource metadata URL derived from
// `MCP_PUBLIC_RESOURCE_URL`. No loopback or lab URL is ever emitted.
//
// On success the validated identity is published to the request's task-local
// (see `crate::identity`) for the duration of the downstream request, so the
// rmcp tool handlers can authorize against it.

use axum::http::{HeaderMap, Request, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::error::TokenError;
use crate::identity::{IDENTITY, Identity};
use crate::oauth::TokenValidator;

/// Configuration for the auth middleware.
#[derive(Clone)]
pub struct AuthState {
    pub issuer: String,
    pub resource: String,
    /// The public protected-resource metadata URL, e.g.
    /// `https://mcal.example.com/.well-known/oauth-protected-resource`.
    pub resource_metadata: String,
    /// Shared token validator with bounded metadata/JWKS caching.
    pub validator: TokenValidator,
}

impl AuthState {
    /// Derive the public protected-resource metadata URL from the configured
    /// MCP resource URL. The metadata lives at the MCP origin under
    /// `/.well-known/oauth-protected-resource`.
    pub fn resource_metadata_url(resource_url: &str) -> String {
        let base = resource_url.trim_end_matches('/');
        // Strip the trailing `/mcp` path segment if present so the metadata is
        // anchored at the MCP origin, not nested under the endpoint path.
        let origin = base
            .strip_suffix("/mcp")
            .unwrap_or(base)
            .trim_end_matches('/');
        format!("{origin}/.well-known/oauth-protected-resource")
    }

    pub fn new(issuer: String, resource: String) -> Self {
        Self {
            resource_metadata: Self::resource_metadata_url(&resource),
            issuer,
            resource,
            validator: TokenValidator::new(),
        }
    }
}

/// Extract the bearer token from the `Authorization` header.
fn extract_bearer_token(headers: &HeaderMap) -> Option<String> {
    let auth = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = auth.strip_prefix("Bearer ")?;
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    Some(token.to_string())
}

/// Sanitize a string for use in a `WWW-Authenticate` header value.
fn sanitize(s: &str) -> String {
    s.chars()
        .take(120)
        .map(|c| match c {
            '"' | '\\' => '-',
            c if (c as u32) < 0x20 => '-',
            c => c,
        })
        .collect()
}

/// Build a standards-compliant 401 challenge referencing the public
/// protected-resource metadata URL.
fn unauthorized(state: &AuthState, error: &str, description: &str) -> Response {
    let challenge = format!(
        "Bearer realm=\"mcp\", resource_metadata=\"{}\", error=\"{}\"",
        state.resource_metadata,
        sanitize(error)
    );
    let body = json!({
        "error": error,
        "error_description": description,
    });
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(header::WWW_AUTHENTICATE, challenge)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap_or_else(|_| Response::new(axum::body::Body::empty()))
}

/// Map a token validation error to an OAuth `error` code and description.
fn token_error_params(e: &TokenError) -> (&'static str, String) {
    match e {
        TokenError::MissingToken => (
            "invalid_request",
            "authorization token is required".to_string(),
        ),
        TokenError::Expired => ("invalid_token", "access token has expired".to_string()),
        TokenError::InvalidAudience => (
            "invalid_target",
            "token audience does not match this resource".to_string(),
        ),
        TokenError::InvalidIssuer => ("invalid_token", "token issuer is not trusted".to_string()),
        TokenError::Revoked => ("invalid_token", "token has been revoked".to_string()),
        _ => ("invalid_token", "invalid authorization token".to_string()),
    }
}

/// The auth middleware. Validates the bearer token and publishes the identity
/// to the request's task-local for the downstream rmcp service.
pub async fn auth_middleware(
    axum::extract::State(state): axum::extract::State<AuthState>,
    headers: HeaderMap,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let token = match extract_bearer_token(&headers) {
        Some(t) => t,
        None => {
            return unauthorized(
                &state,
                "invalid_request",
                "missing or malformed bearer token",
            );
        }
    };

    match state
        .validator
        .validate(&token, &state.issuer, &state.resource)
        .await
    {
        Ok(result) => {
            let identity = Identity::from(&result);
            tracing::debug!(
                user_id = identity.user_id,
                client_id = %identity.client_id,
                "mcp request authorized"
            );
            IDENTITY
                .scope(Some(identity), async move { next.run(request).await })
                .await
                .into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "mcp token validation failed");
            let (code, description) = token_error_params(&e);
            unauthorized(&state, code, &description)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_metadata_url_derived_from_mcp_resource() {
        assert_eq!(
            AuthState::resource_metadata_url("https://mcal.example.com/mcp"),
            "https://mcal.example.com/.well-known/oauth-protected-resource"
        );
    }

    #[test]
    fn resource_metadata_url_handles_trailing_slash() {
        assert_eq!(
            AuthState::resource_metadata_url("https://mcal.example.com/mcp/"),
            "https://mcal.example.com/.well-known/oauth-protected-resource"
        );
    }

    #[test]
    fn resource_metadata_url_handles_nested_path() {
        assert_eq!(
            AuthState::resource_metadata_url("https://mcal.example.com/api/mcp"),
            "https://mcal.example.com/api/.well-known/oauth-protected-resource"
        );
    }

    #[test]
    fn auth_state_new_populates_metadata() {
        let state = AuthState::new(
            "https://auth.example.com".to_string(),
            "https://mcal.example.com/mcp".to_string(),
        );
        assert_eq!(
            state.resource_metadata,
            "https://mcal.example.com/.well-known/oauth-protected-resource"
        );
        assert_eq!(state.issuer, "https://auth.example.com");
        assert_eq!(state.resource, "https://mcal.example.com/mcp");
    }

    #[test]
    fn challenge_uses_public_metadata_and_bearer_scheme() {
        let state = AuthState::new(
            "https://auth.example.com".to_string(),
            "https://mcal.example.com/mcp".to_string(),
        );
        let resp = unauthorized(&state, "invalid_token", "bad token");
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let www = resp
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(www.starts_with("Bearer "));
        assert!(www.contains(
            "resource_metadata=\"https://mcal.example.com/.well-known/oauth-protected-resource\""
        ));
        assert!(
            !www.contains("127.0.0.1"),
            "challenge must not leak a loopback URL"
        );
    }

    #[test]
    fn extract_bearer_token_parses_valid_header() {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Bearer abc123".parse().unwrap());
        assert_eq!(extract_bearer_token(&headers), Some("abc123".to_string()));
    }

    #[test]
    fn extract_bearer_token_rejects_missing_header() {
        assert_eq!(extract_bearer_token(&HeaderMap::new()), None);
    }

    #[test]
    fn extract_bearer_token_rejects_wrong_scheme() {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Basic abc123".parse().unwrap());
        assert_eq!(extract_bearer_token(&headers), None);
    }

    #[test]
    fn extract_bearer_token_rejects_empty_token() {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Bearer ".parse().unwrap());
        assert_eq!(extract_bearer_token(&headers), None);
    }

    #[test]
    fn token_error_params_maps_expired() {
        let (code, desc) = token_error_params(&TokenError::Expired);
        assert_eq!(code, "invalid_token");
        assert!(desc.contains("expired"));
    }

    #[test]
    fn token_error_params_maps_missing() {
        let (code, _) = token_error_params(&TokenError::MissingToken);
        assert_eq!(code, "invalid_request");
    }
}
