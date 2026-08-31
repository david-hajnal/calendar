//! JWT access-token validation against the candidate issuer, using the
//! discovery-advertised JWKS. Standard claims only (no namespaced claims).
//!
//! Used by the mcp-echo auth middleware and by the lab-prove harness (P4).

use base64::Engine;
use jsonwebtoken::{DecodingKey, Validation};
use rsa::pkcs8::EncodePublicKey;
use serde::Deserialize;
use serde_json::Value;

/// The standard claims the Gate 3 contract requires.
#[derive(Debug, Clone, Deserialize)]
pub struct AccessClaims {
    pub iss: String,
    pub sub: String,
    #[serde(default)]
    pub aud: Audience,
    pub exp: i64,
    pub iat: i64,
    #[serde(default)]
    pub jti: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    /// The candidate emits the standard `scope` claim. `scp` remains an alias
    /// only so the negative fixture decoder can report a precise mismatch.
    #[serde(default, alias = "scp")]
    pub scope: Option<String>,
    #[serde(default)]
    pub amr: Option<Vec<String>>,
    #[serde(default)]
    pub acr: Option<String>,
}

/// `aud` may be a single string or an array of strings.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Audience {
    One(String),
    Many(Vec<String>),
}

impl Audience {
    pub fn as_list(&self) -> Vec<String> {
        match self {
            Audience::One(s) => vec![s.clone()],
            Audience::Many(v) => v.clone(),
        }
    }
}

impl Default for Audience {
    fn default() -> Self {
        Audience::Many(Vec::new())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum JwtError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("discovery: {0}")]
    Discovery(String),
    #[error("jwks: {0}")]
    Jwks(String),
    #[error("token: {0}")]
    Token(String),
}

/// Discover the `jwks_uri` from the authorization-server metadata.
pub async fn discover_jwks_uri(http: &reqwest::Client, issuer: &str) -> Result<String, JwtError> {
    let url = format!("{issuer}/.well-known/oauth-authorization-server");
    let meta: Value = http
        .get(&url)
        .send()
        .await
        .map_err(|e| JwtError::Transport(e.to_string()))?
        .error_for_status()
        .map_err(|e| JwtError::Discovery(e.to_string()))?
        .json()
        .await
        .map_err(|e| JwtError::Discovery(e.to_string()))?;
    meta.get("jwks_uri")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| JwtError::Discovery("metadata missing jwks_uri".into()))
}

/// Fetch the JWKS document.
pub async fn fetch_jwks(http: &reqwest::Client, jwks_uri: &str) -> Result<Value, JwtError> {
    http.get(jwks_uri)
        .send()
        .await
        .map_err(|e| JwtError::Transport(e.to_string()))?
        .error_for_status()
        .map_err(|e| JwtError::Jwks(e.to_string()))?
        .json()
        .await
        .map_err(|e| JwtError::Jwks(e.to_string()))
}

/// Convert an RSA JWK (n, e) into a jsonwebtoken DecodingKey.
///
/// Mirrors the proven pattern in `mcp-server/src/oauth.rs`: decode base64url
/// big-endian integers, pad the modulus to a multiple of 128 bytes, build the
/// RSA public key, and export to SubjectPublicKeyInfo PEM.
fn jwk_to_decoding_key(jwk: &Value) -> Result<DecodingKey, JwtError> {
    if jwk.get("kty").and_then(|v| v.as_str()) != Some("RSA") {
        return Err(JwtError::Jwks("unsupported key type, expected RSA".into()));
    }
    let n = jwk
        .get("n")
        .and_then(|v| v.as_str())
        .ok_or_else(|| JwtError::Jwks("missing n".into()))?;
    let e = jwk
        .get("e")
        .and_then(|v| v.as_str())
        .ok_or_else(|| JwtError::Jwks("missing e".into()))?;
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let n_bytes = engine
        .decode(n)
        .map_err(|err| JwtError::Jwks(format!("bad n: {err}")))?;
    let e_bytes = engine
        .decode(e)
        .map_err(|err| JwtError::Jwks(format!("bad e: {err}")))?;

    let key_size_bytes = ((n_bytes.len() + 127) / 128) * 128;
    let n_padded = if n_bytes.len() < key_size_bytes {
        let mut padded = vec![0u8; key_size_bytes - n_bytes.len()];
        padded.extend_from_slice(&n_bytes);
        padded
    } else {
        n_bytes
    };

    let n_big = rsa::BigUint::from_bytes_be(&n_padded);
    let e_big = rsa::BigUint::from_bytes_be(&e_bytes);
    let rsa_pub = rsa::RsaPublicKey::new(n_big, e_big)
        .map_err(|err| JwtError::Jwks(format!("rsa key: {err}")))?;
    let pem = rsa_pub
        .to_public_key_pem(rsa::pkcs8::LineEnding::default())
        .map_err(|err| JwtError::Jwks(format!("pem: {err}")))?;
    DecodingKey::from_rsa_pem(pem.as_bytes())
        .map_err(|err| JwtError::Jwks(format!("decoding key: {err}")))
}

/// Validate a JWT access token against the issuer and the exact resource audience.
///
/// Checks: signature (via discovery JWKS), `iss`, `aud` (contains the exact
/// resource), `exp`, and extracts the standard claims.
pub async fn validate_access_token(
    http: &reqwest::Client,
    token: &str,
    issuer: &str,
    resource: &str,
) -> Result<AccessClaims, JwtError> {
    let jwks_uri = discover_jwks_uri(http, issuer).await?;
    let jwks = fetch_jwks(http, &jwks_uri).await?;
    let keys = jwks
        .get("keys")
        .and_then(|v| v.as_array())
        .ok_or_else(|| JwtError::Jwks("no keys".into()))?;

    // Decode header to find kid + alg.
    let header = jsonwebtoken::decode_header(token)
        .map_err(|err| JwtError::Token(format!("header: {err}")))?;
    let kid = header.kid;
    let alg = header.alg;

    let jwk = keys
        .iter()
        .find(|k| k.get("kid").and_then(|v| v.as_str()) == kid.as_deref())
        .ok_or_else(|| JwtError::Jwks(format!("no key for kid {kid:?}")))?;

    let decoding_key = jwk_to_decoding_key(jwk)?;

    let mut validation = Validation::new(alg);
    validation.set_issuer(&[issuer]);
    validation.set_audience(&[resource]);
    validation.leeway = 30;

    let data = jsonwebtoken::decode::<AccessClaims>(token, &decoding_key, &validation)
        .map_err(|err| JwtError::Token(err.to_string()))?;
    Ok(data.claims)
}
