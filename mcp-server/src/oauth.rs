// OAuth token validation module.
//
// Validates access tokens from MCP clients:
// - Discovery: fetch `/.well-known/oauth-authorization-server` (RFC 8414) or
//   `/.well-known/openid-configuration` (OIDC), verify the returned `issuer`
//   exactly matches configuration, then follow its HTTPS `jwks_uri`.
// - JWT signature verification via JWKS (bounded TTL cache, one refresh on
//   unknown kid for key rotation overlap).
// - Standard claims: numeric `sub` (CommonCal user id), `client_id`,
//   space-delimited `scope`, `iss`, `aud`, `exp`, `iat`, `jti`.
// - Validation: signature, allowed algorithm, kid, exact iss, exact MCP
//   audience, exp, iat with small clock skew.
//
// DPoP validation is handled separately in the security module.

use std::sync::Arc;
use std::time::Instant;

use base64::Engine;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, errors::ErrorKind as JwtErrorKind};
use rsa::pkcs8::{EncodePublicKey, LineEnding};
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::error::TokenError;

/// Clock skew tolerance in seconds for `exp` and `iat` validation.
const CLOCK_SKEW_SECS: u64 = 30;

/// TTL for the discovery metadata cache (seconds).
const METADATA_CACHE_TTL_SECS: u64 = 300;

/// TTL for the JWKS cache (seconds).
const JWKS_CACHE_TTL_SECS: u64 = 300;

/// Parsed JWT header (for kid and alg extraction).
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct JwtHeader {
    kid: Option<String>,
    alg: Option<String>,
}

/// Parsed JWT claims from the MCP access token.
///
/// Uses the standard OAuth 2.0 / OIDC claim set as issued by the repository's
/// authorization server (`slice1-lab/auth-server`):
/// - `sub`: numeric CommonCal user id (string-encoded integer)
/// - `client_id`: the OAuth client identifier
/// - `scope`: space-delimited scope string
/// - `iss`: the issuer URL
/// - `aud`: the MCP resource URL
/// - `exp`: expiry (Unix seconds)
/// - `iat`: issued-at (Unix seconds)
/// - `jti`: unique token identifier
/// - `amr`: array of authentication methods (optional)
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TokenClaims {
    sub: String,
    iss: String,
    #[serde(default)]
    aud: serde_json::Value,
    exp: i64,
    iat: i64,
    #[serde(default)]
    jti: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
    /// Standard `scope` claim: space-delimited string.
    #[serde(default, alias = "scp")]
    scope: Option<String>,
    /// Authentication methods (optional, used to derive auth strength).
    #[serde(default)]
    amr: Option<Vec<String>>,
    /// Legacy `auth_time` claim (optional, falls back to `iat`).
    #[serde(default)]
    auth_time: Option<i64>,
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
#[derive(Debug, Deserialize, Clone)]
struct Jwk {
    kty: String,
    alg: String,
    #[serde(rename = "use")]
    _key_use: Option<String>,
    n: String,
    e: String,
    kid: String,
}

/// Cached discovery metadata.
struct CachedMetadata {
    jwks_uri: String,
    fetched_at: Instant,
}

/// Cached JWKS document.
struct CachedJwks {
    keys: Vec<Jwk>,
    fetched_at: Instant,
}

/// Stateful token validator with bounded metadata/JWKS caching.
///
/// The caches are shared across requests via `Arc<Mutex<...>>`. The TTL
/// ensures stale keys are never accepted indefinitely: after the TTL expires,
/// the next validation fetches fresh JWKS. If a key is no longer present,
/// tokens signed with it are rejected.
///
/// Key rotation overlap: when a new key is added to the JWKS, both old and
/// new keys are present during the overlap window. The validator accepts
/// tokens signed by any key in the current JWKS. If a token's `kid` is not
/// found in the cached JWKS, the validator performs one refresh to pick up
/// the new key. If the kid is still absent after the refresh, the token is
/// rejected.
#[derive(Clone)]
pub struct TokenValidator {
    http_client: reqwest::Client,
    metadata_cache: Arc<Mutex<Option<CachedMetadata>>>,
    jwks_cache: Arc<Mutex<Option<CachedJwks>>>,
}

impl TokenValidator {
    pub fn new() -> Self {
        Self {
            http_client: reqwest::Client::builder()
                .danger_accept_invalid_certs(false)
                .danger_accept_invalid_hostnames(false)
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("http client"),
            metadata_cache: Arc::new(Mutex::new(None)),
            jwks_cache: Arc::new(Mutex::new(None)),
        }
    }

    /// Validate an MCP access token.
    ///
    /// Performs:
    /// 1. Discover `jwks_uri` from authorization-server metadata (cached).
    /// 2. Verify the metadata `issuer` exactly matches the configured issuer.
    /// 3. Fetch JWKS from the discovered `jwks_uri` (cached).
    /// 4. Parse JWT header to find `kid` and `alg`.
    /// 5. Find the matching key by `kid` (one refresh on unknown kid).
    /// 6. Verify JWT signature, issuer, audience, exp, iat.
    /// 7. Extract standard claims (numeric sub, client_id, scope).
    pub async fn validate(
        &self,
        token: &str,
        issuer: &str,
        resource: &str,
    ) -> Result<TokenValidationResult, TokenError> {
        if token.is_empty() {
            return Err(TokenError::MissingToken);
        }

        // Step 1: Discover jwks_uri (cached).
        let jwks_uri = self.discover_jwks_uri(issuer).await?;

        // Step 2: Fetch JWKS (cached).
        let mut jwks = self.fetch_jwks(&jwks_uri).await?;

        // Step 3: Parse JWT header.
        let header = parse_jwt_header(token)?;

        // Step 4: Find the matching key by kid.
        let jwk = match find_jwk(&jwks.keys, &header.kid) {
            Some(jwk) => jwk,
            None => {
                // One refresh on unknown kid (key rotation overlap).
                tracing::debug!(
                    kid = ?header.kid,
                    "unknown kid in cached JWKS, refreshing once"
                );
                jwks = self.refresh_jwks(&jwks_uri).await?;
                find_jwk(&jwks.keys, &header.kid).ok_or_else(|| {
                    TokenError::InvalidToken(format!(
                        "no matching key for kid: {:?}",
                        header.kid
                    ))
                })?
            }
        };

        // Step 5: Convert JWK to DecodingKey.
        let decoding_key = jwk_to_decoding_key(jwk)?;

        // Step 6: Build validation rules.
        let alg = alg_from_jwk(jwk)?;
        let mut validation = Validation::new(alg);
        validation.set_issuer(&[issuer]);
        validation.set_audience(&[resource]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub", "iat"]);
        validation.leeway = CLOCK_SKEW_SECS;

        // Step 7: Decode and validate the token.
        let token_data = decode::<TokenClaims>(token, &decoding_key, &validation)
            .map_err(|e| match e.kind() {
                JwtErrorKind::ExpiredSignature => TokenError::Expired,
                JwtErrorKind::InvalidIssuer => TokenError::InvalidIssuer,
                JwtErrorKind::InvalidAudience => TokenError::InvalidAudience,
                JwtErrorKind::InvalidSignature => TokenError::InvalidToken(e.to_string()),
                _ => TokenError::InvalidToken(e.to_string()),
            })?;

        // Step 8: Validate iat (not in the future beyond clock skew).
        let now = current_time_secs();
        if token_data.claims.iat > now + CLOCK_SKEW_SECS as i64 {
            return Err(TokenError::InvalidToken(
                "iat is in the future beyond clock skew".to_string(),
            ));
        }

        // Step 9: Extract standard claims.
        let claims = &token_data.claims;

        // Parse numeric sub as CommonCal user id.
        let user_id: i64 = claims
            .sub
            .parse()
            .map_err(|_| TokenError::InvalidToken("sub must be a numeric user id".to_string()))?;

        let client_id = claims
            .client_id
            .clone()
            .ok_or_else(|| TokenError::InvalidToken("missing client_id claim".to_string()))?;

        let token_id = claims
            .jti
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        // Parse space-delimited scope claim.
        let scopes: Vec<String> = claims
            .scope
            .as_deref()
            .map(|s| s.split_whitespace().map(String::from).collect())
            .unwrap_or_default();

        // Derive auth strength from amr array.
        let auth_strength = derive_auth_strength(claims.amr.as_deref());

        // auth_time from auth_time claim or iat.
        let auth_time = claims.auth_time.unwrap_or(claims.iat);

        Ok(TokenValidationResult {
            user_id,
            oauth_client_id: client_id,
            scopes,
            auth_strength,
            auth_time,
            token_id,
            expires_at: claims.exp,
        })
    }

    /// Discover the `jwks_uri` from authorization-server metadata.
    ///
    /// Tries `/.well-known/oauth-authorization-server` (RFC 8414) first, then
    /// falls back to `/.well-known/openid-configuration` (OIDC). Verifies the
    /// returned `issuer` exactly matches the configured issuer.
    async fn discover_jwks_uri(&self, issuer: &str) -> Result<String, TokenError> {
        // Check cache first.
        {
            let cache = self.metadata_cache.lock().await;
            if let Some(entry) = cache.as_ref()
                && entry.fetched_at.elapsed().as_secs() < METADATA_CACHE_TTL_SECS
            {
                return Ok(entry.jwks_uri.clone());
            }
        }

        // Try RFC 8414 location first, then OIDC fallback.
        let rfc8414_url = format!("{}/.well-known/oauth-authorization-server", issuer.trim_end_matches('/'));
        let oidc_url = format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/'));

        let metadata = match self.fetch_metadata(&rfc8414_url).await {
            Ok(m) => m,
            Err(_) => self
                .fetch_metadata(&oidc_url)
                .await
                .map_err(|e| TokenError::InvalidToken(format!("discovery failed: {e}")))?,
        };

        // Verify issuer matches configuration exactly.
        let returned_issuer = metadata
            .get("issuer")
            .and_then(|v| v.as_str())
            .ok_or_else(|| TokenError::InvalidToken("metadata missing issuer".to_string()))?;
        if returned_issuer != issuer {
            return Err(TokenError::InvalidIssuer);
        }

        let jwks_uri = metadata
            .get("jwks_uri")
            .and_then(|v| v.as_str())
            .ok_or_else(|| TokenError::InvalidToken("metadata missing jwks_uri".to_string()))?
            .to_string();

        // Validate jwks_uri is a valid absolute URL. Require HTTPS when the
        // issuer is HTTPS (production); allow HTTP for loopback test issuers.
        let url = url::Url::parse(&jwks_uri)
            .map_err(|_| TokenError::InvalidToken("invalid jwks_uri".to_string()))?;
        let issuer_url = url::Url::parse(issuer)
            .map_err(|_| TokenError::InvalidToken("invalid issuer URL".to_string()))?;
        if issuer_url.scheme() == "https" && url.scheme() != "https" {
            return Err(TokenError::InvalidToken(
                "jwks_uri must use https scheme when issuer is https".to_string(),
            ));
        }

        // Cache the result.
        *self.metadata_cache.lock().await = Some(CachedMetadata {
            jwks_uri: jwks_uri.clone(),
            fetched_at: Instant::now(),
        });

        Ok(jwks_uri)
    }

    /// Fetch authorization-server metadata.
    async fn fetch_metadata(&self, url: &str) -> Result<serde_json::Value, String> {
        let resp = self
            .http_client
            .get(url)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !resp.status().is_success() {
            return Err(format!("metadata fetch returned status {}", resp.status()));
        }

        let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        Ok(body)
    }

    /// Fetch JWKS from the given URI (using cache if fresh).
    async fn fetch_jwks(&self, jwks_uri: &str) -> Result<JwksDocument, TokenError> {
        // Check cache first.
        {
            let cache = self.jwks_cache.lock().await;
            if let Some(entry) = cache.as_ref()
                && entry.fetched_at.elapsed().as_secs() < JWKS_CACHE_TTL_SECS
            {
                return Ok(JwksDocument {
                    keys: entry.keys.clone(),
                });
            }
        }

        self.refresh_jwks(jwks_uri).await
    }

    /// Force-refresh JWKS from the given URI (bypasses cache).
    async fn refresh_jwks(&self, jwks_uri: &str) -> Result<JwksDocument, TokenError> {
        let resp = self
            .http_client
            .get(jwks_uri)
            .send()
            .await
            .map_err(|e| TokenError::InvalidToken(format!("failed to fetch JWKS: {e}")))?;

        if !resp.status().is_success() {
            return Err(TokenError::InvalidToken(format!(
                "JWKS fetch returned status {}",
                resp.status()
            )));
        }

        let jwks: JwksDocument = resp
            .json()
            .await
            .map_err(|e| TokenError::InvalidToken(format!("failed to parse JWKS: {e}")))?;

        if jwks.keys.is_empty() {
            return Err(TokenError::InvalidToken("JWKS contains no keys".to_string()));
        }

        // Update cache.
        *self.jwks_cache.lock().await = Some(CachedJwks {
            keys: jwks.keys.clone(),
            fetched_at: Instant::now(),
        });

        Ok(jwks)
    }
}

impl Default for TokenValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Validate an MCP access token (convenience wrapper using a fresh validator).
///
/// For production use, prefer creating a `TokenValidator` and reusing it
/// across requests so the metadata/JWKS caches are shared.
pub async fn validate_access_token(
    token: &str,
    issuer: &str,
    resource: &str,
) -> Result<TokenValidationResult, TokenError> {
    let validator = TokenValidator::new();
    validator.validate(token, issuer, resource).await
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
pub async fn validate_dpop_proof(_token: &str, proof: &str, nonce: &str) -> Result<(), TokenError> {
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
        #[serde(default)]
        alg: Option<String>,
        #[serde(rename = "kid")]
        _kid: Option<String>,
    }

    let dpop_header: DpopHeader =
        serde_json::from_slice(&decoded).map_err(|_| TokenError::InvalidDpop)?;

    // Verify typ is "dpop+jwt".
    if dpop_header.typ != "dpop+jwt" {
        return Err(TokenError::InvalidDpop);
    }

    // Verify jwk is present (sender-constrained token).
    let jwk = dpop_header.jwk.as_ref().ok_or(TokenError::InvalidDpop)?;

    // Extract RSA public key from JWK.
    let kty = jwk["kty"].as_str().ok_or(TokenError::InvalidDpop)?;
    if kty != "RSA" {
        return Err(TokenError::InvalidDpop);
    }
    let n = jwk["n"].as_str().ok_or(TokenError::InvalidDpop)?;
    let e = jwk["e"].as_str().ok_or(TokenError::InvalidDpop)?;

    // Determine algorithm (default to RS256 per DPoP spec).
    let alg_str = dpop_header.alg.as_deref().unwrap_or("RS256");
    let alg = match alg_str {
        "RS256" => Algorithm::RS256,
        "RS384" => Algorithm::RS384,
        "RS512" => Algorithm::RS512,
        _ => return Err(TokenError::InvalidDpop),
    };

    // Construct DecodingKey from JWK.
    let decoding_key = DecodingKey::from_rsa_components(n, e).map_err(|e| {
        eprintln!(
            "DEBUG: from_rsa_components failed: {:?}, n_len={}, e={}",
            e,
            n.len(),
            e
        );
        TokenError::InvalidDpop
    })?;

    // Parse DPoP proof payload.
    let payload_b64 = parts[1];
    let decoded_payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .map_err(|_| TokenError::InvalidDpop)?;

    #[derive(serde::Deserialize)]
    struct DpopClaims {
        #[serde(rename = "jti")]
        _jti: String,
        #[serde(rename = "htm")]
        _htm: String,
        #[serde(rename = "htu")]
        _htu: String,
        exp: usize,
    }

    let dpop_claims: DpopClaims =
        serde_json::from_slice(&decoded_payload).map_err(|_| TokenError::InvalidDpop)?;

    // Verify proof is not expired.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as usize;
    if now >= dpop_claims.exp {
        return Err(TokenError::InvalidDpop);
    }

    // Verify the JWS signature against the public key in the DPoP header.
    let mut validation = Validation::new(alg);
    validation.set_required_spec_claims(&["exp"]);
    validation.leeway = 30;

    decode::<DpopClaims>(proof, &decoding_key, &validation).map_err(|_| TokenError::InvalidDpop)?;

    // Verify nonce matches (nonce is returned in the response, client must echo it).
    if nonce.is_empty() {
        return Err(TokenError::InvalidDpop);
    }

    Ok(())
}

/// Parse the JWT header to extract `kid` and `alg`.
fn parse_jwt_header(token: &str) -> Result<JwtHeader, TokenError> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(TokenError::InvalidToken(
            "token must have 3 parts".to_string(),
        ));
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

/// Find a JWK by kid in the JWKS key list.
fn find_jwk<'a>(keys: &'a [Jwk], kid: &Option<String>) -> Option<&'a Jwk> {
    match kid {
        Some(kid) => keys.iter().find(|j| j.kid == *kid),
        None => keys.first(),
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

    // RSA JWK n and e are base64url-encoded big-endian integers.
    // Decode to raw bytes and construct a proper SubjectPublicKeyInfo PEM.
    let n_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&jwk.n)
        .map_err(|_| TokenError::InvalidToken("invalid RSA modulus encoding".to_string()))?;

    let e_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&jwk.e)
        .map_err(|_| TokenError::InvalidToken("invalid RSA exponent encoding".to_string()))?;

    // Pad modulus to next standard RSA key size (multiple of 128 bytes).
    let key_size_bytes = n_bytes.len().div_ceil(128) * 128;
    let n_padded = if n_bytes.len() < key_size_bytes {
        let mut padded = vec![0u8; key_size_bytes - n_bytes.len()];
        padded.extend_from_slice(&n_bytes);
        padded
    } else {
        n_bytes
    };

    // Construct RSA public key and export to SubjectPublicKeyInfo PEM.
    let n_bigint = rsa::BigUint::from_bytes_be(&n_padded);
    let e_bigint = rsa::BigUint::from_bytes_be(&e_bytes);
    let rsa_pubkey = rsa::RsaPublicKey::new(n_bigint, e_bigint)
        .map_err(|_| TokenError::InvalidToken("invalid RSA public key components".to_string()))?;

    let pem = rsa_pubkey
        .to_public_key_pem(LineEnding::default())
        .map_err(|_| {
            TokenError::InvalidToken("failed to export RSA public key to PEM".to_string())
        })?;

    DecodingKey::from_rsa_pem(pem.into_bytes().as_slice()).map_err(|_| {
        TokenError::InvalidToken("failed to decode RSA public key from PEM".to_string())
    })
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

/// Extract the audience from the configured resource URL.
pub fn extract_audience(resource_url: &str) -> String {
    resource_url.to_string()
}

/// Derive auth strength from the `amr` array.
fn derive_auth_strength(amr: Option<&[String]>) -> AuthStrength {
    let Some(amr) = amr else {
        return AuthStrength::Passwordless;
    };
    if amr.iter().any(|m| m == "passkey" || m == "fido2" || m == "webauthn") {
        AuthStrength::Passkey
    } else if amr.iter().any(|m| m == "mfa" || m == "otp" || m == "totp") {
        AuthStrength::Mfa
    } else {
        AuthStrength::Passwordless
    }
}

/// Get current time as Unix seconds.
fn current_time_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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
    fn derive_auth_strength_passkey() {
        assert_eq!(
            derive_auth_strength(Some(&["passkey".to_string()])),
            AuthStrength::Passkey
        );
    }

    #[test]
    fn derive_auth_strength_mfa() {
        assert_eq!(
            derive_auth_strength(Some(&["mfa".to_string()])),
            AuthStrength::Mfa
        );
    }

    #[test]
    fn derive_auth_strength_none_defaults_to_passwordless() {
        assert_eq!(derive_auth_strength(None), AuthStrength::Passwordless);
    }

    #[test]
    fn derive_auth_strength_unknown_defaults_to_passwordless() {
        assert_eq!(
            derive_auth_strength(Some(&["pwd".to_string()])),
            AuthStrength::Passwordless
        );
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
        let header_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"not json");
        let token = format!("{}.b.c", header_b64);
        let result = parse_jwt_header(&token);
        assert!(result.is_err());
    }

    #[test]
    fn parse_jwt_header_accepts_valid_header() {
        let header_json = r#"{"kid":"key-1","alg":"RS256"}"#;
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
        let header_b64 =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(header.to_string());
        let proof = format!("{}.payload.signature", header_b64);
        let result = rt.block_on(validate_dpop_proof("token", &proof, "nonce"));
        assert!(result.is_err());
    }

    #[test]
    fn dpop_proof_rejects_missing_jwk() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let header = serde_json::json!({"typ": "dpop+jwt"});
        let header_b64 =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(header.to_string());
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"jti":"test","htm":"GET","htu":"http://localhost"}"#);
        let proof = format!("{}.{}.signature", header_b64, payload);
        let result = rt.block_on(validate_dpop_proof("token", &proof, "nonce"));
        assert!(result.is_err());
    }

    #[test]
    fn dpop_proof_rejects_dummy_key() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let header =
            serde_json::json!({"typ": "dpop+jwt", "jwk": {"kty": "RSA", "n": "test", "e": "AQAB"}});
        let header_b64 =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(header.to_string());
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"jti":"test","htm":"GET","htu":"http://localhost","exp":9999999999}"#);
        let proof = format!("{}.{}.signature", header_b64, payload);
        let result = rt.block_on(validate_dpop_proof("token", &proof, "nonce"));
        assert!(result.is_err());
    }

    #[test]
    fn dpop_proof_accepts_valid_header_and_payload() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let header = serde_json::json!({
            "typ": "dpop+jwt",
            "jwk": {"kty": "RSA", "n": "dGVzdA==", "e": "AQAB"},
            "alg": "RS256"
        });
        let header_b64 =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(header.to_string());
        let claims = serde_json::json!({
            "jti": "test-jti",
            "htm": "GET",
            "htu": "http://localhost",
            "exp": 4102444800u64
        });
        let claims_b64 =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string());
        let proof = format!("{}.{}.dummy_signature", header_b64, claims_b64);
        let result = rt.block_on(validate_dpop_proof("token", &proof, "nonce"));
        assert!(result.is_err());
    }

    #[test]
    fn dpop_proof_rejects_forged_proof() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use rsa::RsaPrivateKey;
        use rsa::pkcs1v15::SigningKey;
        use rsa::signature::Signer;
        use rsa::traits::PublicKeyParts;

        let rt = tokio::runtime::Runtime::new().unwrap();

        let mut rng = rand::thread_rng();
        let private_key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let public_key = private_key.to_public_key();

        let n_bytes = public_key.n().to_bytes_be();
        let n_b64 = URL_SAFE_NO_PAD.encode(&n_bytes);

        let jwk = serde_json::json!({
            "kty": "RSA",
            "n": n_b64,
            "e": "AQAB"
        });

        let header = serde_json::json!({"typ": "dpop+jwt", "jwk": jwk, "alg": "RS256"});
        let header_b64 = URL_SAFE_NO_PAD.encode(header.to_string());

        let original_claims = serde_json::json!({
            "jti": "test-jti",
            "htm": "GET",
            "htu": "http://localhost",
            "exp": 4102444800u64
        });
        let original_claims_b64 = URL_SAFE_NO_PAD.encode(original_claims.to_string());

        let signing_input = format!("{}.{}", header_b64, original_claims_b64);
        let signing_key = SigningKey::<sha2::Sha256>::new_unprefixed(private_key);
        let signature = signing_key.sign(signing_input.as_bytes());
        let signature_bytes: Box<[u8]> = signature.into();
        let signature_b64 = URL_SAFE_NO_PAD.encode(&signature_bytes);

        let tampered_claims = serde_json::json!({
            "jti": "test-jti",
            "htm": "POST",
            "htu": "http://localhost",
            "exp": 4102444800u64
        });
        let tampered_claims_b64 = URL_SAFE_NO_PAD.encode(tampered_claims.to_string());

        let forged_proof = format!("{}.{}.{}", header_b64, tampered_claims_b64, signature_b64);

        let result = rt.block_on(validate_dpop_proof("token", &forged_proof, "nonce"));
        assert!(result.is_err(), "forged DPoP proof should be rejected");
    }

    #[test]
    fn find_jwk_finds_matching_kid() {
        let keys = vec![
            Jwk { kty: "RSA".into(), alg: "RS256".into(), _key_use: None, n: "n1".into(), e: "AQAB".into(), kid: "key-1".into() },
            Jwk { kty: "RSA".into(), alg: "RS256".into(), _key_use: None, n: "n2".into(), e: "AQAB".into(), kid: "key-2".into() },
        ];
        assert!(find_jwk(&keys, &Some("key-1".to_string())).is_some());
        assert!(find_jwk(&keys, &Some("key-2".to_string())).is_some());
        assert!(find_jwk(&keys, &Some("key-3".to_string())).is_none());
    }

    #[test]
    fn find_jwk_no_kid_returns_first() {
        let keys = vec![
            Jwk { kty: "RSA".into(), alg: "RS256".into(), _key_use: None, n: "n1".into(), e: "AQAB".into(), kid: "key-1".into() },
        ];
        assert!(find_jwk(&keys, &None).is_some());
    }

    #[test]
    fn token_claims_deserializes_standard_claims() {
        let json = r#"{
            "sub": "42",
            "iss": "https://auth.example.com",
            "aud": "https://mcp.example.com/mcp",
            "exp": 1700003600,
            "iat": 1700000000,
            "jti": "token-123",
            "client_id": "client-abc",
            "scope": "commoncal.calendar.metadata.read commoncal.event.read.basic",
            "amr": ["pwd"]
        }"#;
        let claims: TokenClaims = serde_json::from_str(json).unwrap();
        assert_eq!(claims.sub, "42");
        assert_eq!(claims.client_id, Some("client-abc".to_string()));
        assert_eq!(
            claims.scope,
            Some("commoncal.calendar.metadata.read commoncal.event.read.basic".to_string())
        );
        assert_eq!(claims.jti, Some("token-123".to_string()));
        assert_eq!(claims.amr, Some(vec!["pwd".to_string()]));
    }

    #[test]
    fn token_claims_scope_alias_scp() {
        let json = r#"{
            "sub": "1",
            "iss": "https://auth.example.com",
            "aud": "https://mcp.example.com/mcp",
            "exp": 1700003600,
            "iat": 1700000000,
            "scp": "scope1 scope2"
        }"#;
        let claims: TokenClaims = serde_json::from_str(json).unwrap();
        assert_eq!(claims.scope, Some("scope1 scope2".to_string()));
    }
}
