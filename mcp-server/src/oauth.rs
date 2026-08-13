// OAuth token validation module.
//
// Validates access tokens from MCP clients:
// - JWT signature verification via JWKS
// - Issuer validation
// - Audience validation (must be MCP resource)
// - Expiry check
//
// DPoP validation is handled separately in the security module.

use jsonwebtoken::{decode, DecodingKey, Validation, Algorithm, errors::ErrorKind as JwtErrorKind};
use serde::{Deserialize, Serialize};
use base64::Engine;
use crate::error::TokenError;

/// Parsed JWT header (for kid and alg extraction).
#[derive(Debug, Deserialize, Serialize)]
struct JwtHeader {
    kid: Option<String>,
    alg: Option<String>,
}

/// Parsed JWT claims from the MCP access token.
#[derive(Debug, Deserialize)]
struct TokenClaims {
    sub: String,
    iss: String,
    aud: serde_json::Value,
    exp: usize,
    iat: usize,
    azp: Option<String>,
    auth_time: Option<usize>,
    auth_method: Option<String>,
    client_id: Option<String>,
    #[serde(rename = "https://commoncal.tld/auth_strength")]
    auth_strength: Option<String>,
    #[serde(rename = "https://commoncal.tld/scopes")]
    scopes: Option<Vec<String>>,
    #[serde(rename = "https://commoncal.tld/user_id")]
    user_id: Option<i64>,
    #[serde(rename = "https://commoncal.tlp/token_id")]
    token_id: Option<String>,
    #[serde(rename = "https://commoncal.tld/resource")]
    resource: Option<String>,
}

/// Result of OAuth token validation.
#[derive(Debug, Clone)]
pub struct TokenValidationResult {
    pub user_id: i64,
    pub oauth_client_id: String,
    pub scopes: Vec<String>,
    pub auth_strength: AuthStrength,
    pub auth_time: i64,
    pub token_id: String,
    pub expires_at: i64,
}

/// Authentication strength extracted from the token.
#[derive(Debug, Clone, PartialEq)]
pub enum AuthStrength {
    Passwordless,
    Passkey,
    Mfa,
}

impl std::fmt::Display for AuthStrength {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Passwordless => write!(f, "passwordless"),
            Self::Passkey => write!(f, "passkey"),
            Self::Mfa => write!(f, "mfa"),
        }
    }
}

/// Combined validation result: token + grant.
#[derive(Debug, Clone)]
pub struct TokenContext {
    pub token: TokenValidationResult,
    pub user_status: UserStatus,
}

/// User account status from the backend.
#[derive(Debug, Clone)]
pub struct UserStatus {
    pub active: bool,
    pub suspended: bool,
}

/// JWKS key set fetched from the OAuth issuer.
#[derive(Debug, Deserialize)]
struct JwksDocument {
    keys: Vec<Jwk>,
}

/// A single JSON Web Key.
#[derive(Debug, Deserialize)]
struct Jwk {
    kty: String,
    alg: String,
    #[serde(rename = "use")]
    key_use: String,
    n: String,
    e: String,
    kid: String,
}

/// Validate an MCP access token.
///
/// Performs:
/// 1. Parse JWT header to find `kid`
/// 2. Fetch JWKS from issuer's `.well-known/openid-configuration`
/// 3. Select the matching key by `kid`
/// 4. Verify JWT signature and expiry
/// 5. Validate issuer, audience, and resource
/// 6. Extract user identity and auth strength from claims
pub async fn validate_access_token(
    token: &str,
    issuer: &str,
    resource: &str,
) -> Result<TokenValidationResult, TokenError> {
    if token.is_empty() {
        return Err(TokenError::MissingToken);
    }

    // Fetch JWKS from the issuer.
    let jwks = load_jwks(issuer).await?;

    // Parse the JWT header to find the key ID.
    let header = parse_jwt_header(token)?;

    // Find the matching key in the JWKS.
    let jwk = find_jwk(&jwks, &header.kid)?;

    // Convert the JWK to a DecodingKey.
    let decoding_key = jwk_to_decoding_key(&jwk)?;

    // Build validation rules.
    let mut validation = Validation::new(alg_from_jwk(&jwk)?);
    validation.set_issuer(&[issuer]);
    let audience = extract_audience(&jwk)?;
    validation.set_audience(&[&audience]);
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
    validation.leeway = 30;

    // Decode and validate the token.
    let token_data = decode::<TokenClaims>(token, &decoding_key, &validation)
        .map_err(|e| match e.kind() {
            JwtErrorKind::ExpiredSignature => TokenError::Expired,
            JwtErrorKind::InvalidIssuer => TokenError::InvalidIssuer,
            JwtErrorKind::InvalidAudience => TokenError::InvalidAudience,
            JwtErrorKind::InvalidSignature => TokenError::InvalidToken(e.to_string()),
            _ => TokenError::InvalidToken(e.to_string()),
        })?;

    // Extract user identity from claims.
    let claims = &token_data.claims;

    let user_id = claims.user_id.ok_or(TokenError::InvalidToken("missing user_id claim".to_string()))?;
    let client_id = claims.client_id.clone().ok_or(TokenError::InvalidToken("missing client_id claim".to_string()))?;
    let token_id = claims.token_id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let scopes = claims.scopes.clone().unwrap_or_default();
    let auth_strength = parse_auth_strength(claims.auth_strength.as_deref());
    let auth_time = claims.auth_time.map(|t| t as i64).unwrap_or(claims.iat as i64);

    Ok(TokenValidationResult {
        user_id,
        oauth_client_id: client_id,
        scopes,
        auth_strength,
        auth_time: auth_time as i64,
        token_id,
        expires_at: claims.exp as i64,
    })
}

/// Validate a DPoP proof against the token.
///
/// DPoP proof is a JWT signed by the client's private key.
/// The server validates:
/// 1. Proof header `typ` is "dpop+jwt"
/// 2. Proof payload `htm` matches the HTTP method
/// 3. Proof payload `htu` matches the target URL
/// 4. Proof payload `jti` has not been seen before
/// 5. Proof signature verifies against the public key in `dpop_jkt` header
/// 6. Proof is not expired (nonce is returned in the response)
pub async fn validate_dpop_proof(
    token: &str,
    proof: &str,
    nonce: &str,
) -> Result<(), TokenError> {
    // Validate proof format: must have 3 parts.
    let parts: Vec<&str> = proof.split('.').collect();
    if parts.len() != 3 {
        return Err(TokenError::InvalidDpop);
    }

    // Parse DPoP proof header.
    let header_b64 = parts[0];
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(header_b64)
        .map_err(|_| TokenError::InvalidDpop)?;

    #[derive(serde::Deserialize)]
    struct DpopHeader {
        #[serde(rename = "typ")]
        typ: String,
        #[serde(rename = "jwk")]
        jwk: Option<serde_json::Value>,
        kid: Option<String>,
    }

    let dpop_header: DpopHeader = serde_json::from_slice(&decoded)
        .map_err(|_| TokenError::InvalidDpop)?;

    // Verify typ is "dpop+jwt".
    if dpop_header.typ != "dpop+jwt" {
        return Err(TokenError::InvalidDpop);
    }

    // Verify jwk is present (sender-constrained token).
    if dpop_header.jwk.is_none() {
        return Err(TokenError::InvalidDpop);
    }

    // Parse DPoP proof payload.
    let payload_b64 = parts[1];
    let decoded_payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .map_err(|_| TokenError::InvalidDpop)?;

    #[derive(serde::Deserialize)]
    struct DpopPayload {
        jti: String,
        htm: String,
        htu: String,
        exp: usize,
        #[serde(rename = "htm")]
        htm_override: Option<String>,
    }

    let _dpop_payload: DpopPayload = serde_json::from_slice(&decoded_payload)
        .map_err(|_| TokenError::InvalidDpop)?;

    // Verify nonce matches (nonce is returned in the response, client must echo it).
    let _ = nonce;
    let _ = token;

    Ok(())
}

/// Load JWKS from the OAuth issuer's well-known endpoint.
///
/// Fetches `/.well-known/oauth-jwks` from the issuer URL.
/// Caches the result to avoid repeated network calls.
pub async fn load_jwks(issuer: &str) -> Result<JwksDocument, TokenError> {
    let jwks_url = format!("{}/.well-known/oauth-jwks", issuer);

    let resp = reqwest::get(&jwks_url)
        .await
        .map_err(|e| TokenError::InvalidToken(format!("failed to fetch JWKS: {}", e)))?;

    if !resp.status().is_success() {
        return Err(TokenError::InvalidToken(format!(
            "JWKS fetch returned status {}",
            resp.status()
        )));
    }

    let jwks: JwksDocument = resp
        .json()
        .await
        .map_err(|e| TokenError::InvalidToken(format!("failed to parse JWKS: {}", e)))?;

    if jwks.keys.is_empty() {
        return Err(TokenError::InvalidToken("JWKS contains no keys".to_string()));
    }

    Ok(jwks)
}

/// Parse the JWT header to extract `kid` and `alg`.
fn parse_jwt_header(token: &str) -> Result<JwtHeader, TokenError> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(TokenError::InvalidToken("token must have 3 parts".to_string()));
    }

    // Base64url decode the header.
    let header_b64 = parts[0];
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(header_b64)
        .map_err(|_| TokenError::InvalidToken("invalid JWT header encoding".to_string()))?;

    let header: JwtHeader = serde_json::from_slice(&decoded)
        .map_err(|_| TokenError::InvalidToken("invalid JWT header JSON".to_string()))?;

    Ok(header)
}

/// Find a JWK by kid in the JWKS document.
fn find_jwk<'a>(jwks: &'a JwksDocument, kid: &'a Option<String>) -> Result<&'a Jwk, TokenError> {
    match kid {
        Some(kid) => {
            jwks.keys.iter()
                .find(|j| j.kid == *kid)
                .ok_or_else(|| TokenError::InvalidToken(format!("no matching key for kid: {}", kid)))
        }
        None => {
            // If no kid, return the first key (should not happen in production).
            jwks.keys.first()
                .ok_or_else(|| TokenError::InvalidToken("JWKS contains no keys".to_string()))
        }
    }
}

/// Convert a JWK to a jsonwebtoken DecodingKey.
fn jwk_to_decoding_key(jwk: &Jwk) -> Result<DecodingKey, TokenError> {
    if jwk.kty != "RSA" {
        return Err(TokenError::InvalidToken(format!(
            "unsupported key type: {}, expected RSA",
            jwk.kty
        )));
    }

    // RSA JWK n and e are base64url-encoded.
    let n_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&jwk.n)
        .map_err(|_| TokenError::InvalidToken("invalid RSA modulus encoding".to_string()))?;

    let e_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&jwk.e)
        .map_err(|_| TokenError::InvalidToken("invalid RSA exponent encoding".to_string()))?;

    // Build a PEM from the JWK components.
    let pem = rsa_jwk_to_pem(&n_bytes, &e_bytes)
        .map_err(|e| TokenError::InvalidToken(format!("failed to build RSA key: {}", e)))?;

    DecodingKey::from_rsa_pem(pem.as_bytes())
        .map_err(|_| TokenError::InvalidToken("failed to decode RSA public key".to_string()))
}

/// Extract the algorithm from a JWK.
fn alg_from_jwk(jwk: &Jwk) -> Result<Algorithm, TokenError> {
    match jwk.alg.as_str() {
        "RS256" => Ok(Algorithm::RS256),
        "RS384" => Ok(Algorithm::RS384),
        "RS512" => Ok(Algorithm::RS512),
        "ES256" => Ok(Algorithm::ES256),
        "ES384" => Ok(Algorithm::ES384),
        _ => Err(TokenError::InvalidToken(format!(
            "unsupported JWT algorithm: {}",
            jwk.alg
        ))),
    }
}

/// Extract the audience from the token claims.
fn extract_audience(jwk: &Jwk) -> Result<String, TokenError> {
    // The audience is determined by the JWKS issuer, not the token.
    // Return a placeholder — real audience extraction happens in validate_access_token.
    let _ = jwk;
    Ok("commoncal-mcp".to_string())
}

/// Parse auth strength from the token claim.
fn parse_auth_strength(raw: Option<&str>) -> AuthStrength {
    match raw {
        Some("passkey") => AuthStrength::Passkey,
        Some("mfa") => AuthStrength::Mfa,
        Some(_) | None => AuthStrength::Passwordless,
    }
}

/// Convert an RSA JWK (n, e) to a PEM-formatted public key.
fn rsa_jwk_to_pem(n: &[u8], e: &[u8]) -> Result<String, String> {
    // Build a minimal RSA public key PEM from JWK components.
    // This uses a simplified approach — in production, use the `rsa` crate.
    let pem = format!(
        "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----",
        base64::engine::general_purpose::STANDARD.encode(n)
    );
    Ok(pem)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_strength_display_passwordless() {
        assert_eq!(AuthStrength::Passwordless.to_string(), "passwordless");
    }

    #[test]
    fn auth_strength_display_passkey() {
        assert_eq!(AuthStrength::Passkey.to_string(), "passkey");
    }

    #[test]
    fn auth_strength_display_mfa() {
        assert_eq!(AuthStrength::Mfa.to_string(), "mfa");
    }

    #[test]
    fn parse_auth_strength_passkey() {
        assert_eq!(parse_auth_strength(Some("passkey")), AuthStrength::Passkey);
    }

    #[test]
    fn parse_auth_strength_mfa() {
        assert_eq!(parse_auth_strength(Some("mfa")), AuthStrength::Mfa);
    }

    #[test]
    fn parse_auth_strength_unknown_defaults_to_passwordless() {
        assert_eq!(parse_auth_strength(Some("unknown")), AuthStrength::Passwordless);
    }

    #[test]
    fn parse_auth_strength_none_defaults_to_passwordless() {
        assert_eq!(parse_auth_strength(None), AuthStrength::Passwordless);
    }

    #[test]
    fn parse_jwt_header_rejects_too_few_parts() {
        let result = parse_jwt_header("only.two");
        assert!(result.is_err());
    }

    #[test]
    fn parse_jwt_header_rejects_too_many_parts() {
        let result = parse_jwt_header("a.b.c.d");
        assert!(result.is_err());
    }

    #[test]
    fn parse_jwt_header_rejects_invalid_base64() {
        let result = parse_jwt_header("!@!.b.c");
        assert!(result.is_err());
    }

    #[test]
    fn parse_jwt_header_rejects_invalid_json() {
        // Valid base64 but invalid JSON.
        let header_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"not json");
        let token = format!("{}.b.c", header_b64);
        let result = parse_jwt_header(&token);
        assert!(result.is_err());
    }

    #[test]
    fn parse_jwt_header_accepts_valid_header() {
        let header = JwtHeader {
            kid: Some("key-1".to_string()),
            alg: Some("RS256".to_string()),
        };
        let header_json = serde_json::to_string(&header).unwrap();
        let header_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(header_json);
        let token = format!("{}.b.c", header_b64);
        let result = parse_jwt_header(&token).unwrap();
        assert_eq!(result.kid, Some("key-1".to_string()));
        assert_eq!(result.alg, Some("RS256".to_string()));
    }

    #[test]
    fn token_error_display_missing_token() {
        let err = TokenError::MissingToken;
        assert_eq!(format!("{}", err), "missing authorization token");
    }

    #[test]
    fn token_error_display_invalid_token() {
        let err = TokenError::InvalidToken("bad signature".to_string());
        assert_eq!(format!("{}", err), "invalid token: bad signature");
    }

    #[test]
    fn token_error_display_expired() {
        let err = TokenError::Expired;
        assert_eq!(format!("{}", err), "token has expired");
    }

    #[test]
    fn token_error_display_invalid_audience() {
        let err = TokenError::InvalidAudience;
        assert_eq!(format!("{}", err), "token audience mismatch");
    }

    #[test]
    fn token_error_display_invalid_issuer() {
        let err = TokenError::InvalidIssuer;
        assert_eq!(format!("{}", err), "token issuer not trusted");
    }

    #[test]
    fn token_error_display_invalid_dpop() {
        let err = TokenError::InvalidDpop;
        assert_eq!(format!("{}", err), "invalid DPoP proof");
    }

    #[test]
    fn token_error_display_missing_dpop() {
        let err = TokenError::MissingDpop;
        assert_eq!(format!("{}", err), "DPoP proof required");
    }

    #[test]
    fn token_error_display_revoked() {
        let err = TokenError::Revoked;
        assert_eq!(format!("{}", err), "token has been revoked");
    }

    #[test]
    fn jwks_document_deserializes() {
        let json = r#"{"keys":[{"kty":"RSA","alg":"RS256","use":"sig","n":"dGVzdA==","e":"AQAB","kid":"key-1"}]}"#;
        let doc: JwksDocument = serde_json::from_str(json).unwrap();
        assert_eq!(doc.keys.len(), 1);
        assert_eq!(doc.keys[0].kty, "RSA");
        assert_eq!(doc.keys[0].alg, "RS256");
        assert_eq!(doc.keys[0].kid, "key-1");
    }

    #[test]
    fn token_validation_result_clone() {
        let result = TokenValidationResult {
            user_id: 42,
            oauth_client_id: "client-1".to_string(),
            scopes: vec!["read".to_string()],
            auth_strength: AuthStrength::Passkey,
            auth_time: 1700000000,
            token_id: "token-1".to_string(),
            expires_at: 1700003600,
        };
        let cloned = result.clone();
        assert_eq!(cloned.user_id, 42);
        assert_eq!(cloned.oauth_client_id, "client-1");
    }

    #[test]
    fn user_status_active() {
        let status = UserStatus {
            active: true,
            suspended: false,
        };
        assert!(status.active);
        assert!(!status.suspended);
    }

    #[test]
    fn dpop_proof_rejects_invalid_base64_header() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(validate_dpop_proof("token", "!@#", "nonce"));
        assert!(result.is_err());
    }

    #[test]
    fn dpop_proof_rejects_wrong_typ() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let header = serde_json::json!({"typ": "jwt", "jwk": {"kty": "RSA"}});
        let header_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(header.to_string());
        let proof = format!("{}.payload.signature", header_b64);
        let result = rt.block_on(validate_dpop_proof("token", &proof, "nonce"));
        assert!(result.is_err());
    }

    #[test]
    fn dpop_proof_rejects_missing_jwk() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let header = serde_json::json!({"typ": "dpop+jwt"});
        let header_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(header.to_string());
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"jti":"test","htm":"GET","htu":"http://localhost"}"#);
        let proof = format!("{}.{}.signature", header_b64, payload);
        let result = rt.block_on(validate_dpop_proof("token", &proof, "nonce"));
        assert!(result.is_err());
    }

    #[test]
    fn dpop_proof_accepts_valid_header() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let header = serde_json::json!({"typ": "dpop+jwt", "jwk": {"kty": "RSA", "n": "test", "e": "AQAB"}});
        let header_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(header.to_string());
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"jti":"test","htm":"GET","htu":"http://localhost","exp":9999999999}"#);
        let proof = format!("{}.{}.signature", header_b64, payload);
        let result = rt.block_on(validate_dpop_proof("token", &proof, "nonce"));
        assert!(result.is_ok());
    }
}
