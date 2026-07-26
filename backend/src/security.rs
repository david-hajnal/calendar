use std::fmt::{self, Debug, Formatter};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

const TOKEN_BYTES: usize = 32;
const TOKEN_HASH_CONTEXT: &[u8] = b"commoncal/token-hash/v1\0";
const CSRF_CONTEXT: &[u8] = b"commoncal/csrf/v1\0";

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct SecretKey([u8; 32]);

impl SecretKey {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn generate() -> Self {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).expect("operating system random source unavailable");
        Self(bytes)
    }

    pub fn derive(secret: &[u8]) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"commoncal/secret-key/v1\0");
        digest.update(secret);
        Self(digest.finalize().into())
    }

    pub fn generate_token(&self) -> SecretToken {
        let mut bytes = [0_u8; TOKEN_BYTES];
        getrandom::fill(&mut bytes).expect("operating system random source unavailable");
        SecretToken(URL_SAFE_NO_PAD.encode(bytes))
    }

    pub fn hash_token(&self, domain: TokenDomain, token: &SecretToken) -> TokenHash {
        self.hash_encoded_token(domain, token.expose())
    }

    pub fn verify_token(
        &self,
        domain: TokenDomain,
        token: &SecretToken,
        expected: &TokenHash,
    ) -> bool {
        self.verify_encoded_token(domain, token.expose(), expected)
    }

    pub fn verify_encoded_token(
        &self,
        domain: TokenDomain,
        token: &str,
        expected: &TokenHash,
    ) -> bool {
        let mut mac = self.token_mac(domain);
        mac.update(token.as_bytes());
        mac.verify_slice(&expected.0).is_ok()
    }

    pub fn generate_csrf_token(&self, session: &SecretToken) -> CsrfToken {
        let nonce = self.generate_token();
        let tag = self.csrf_tag(session.expose(), nonce.expose());
        CsrfToken(format!(
            "{}.{}",
            nonce.expose(),
            URL_SAFE_NO_PAD.encode(tag)
        ))
    }

    pub fn validate_csrf_token(&self, session: &SecretToken, csrf: &CsrfToken) -> bool {
        let Some((nonce, encoded_tag)) = csrf.expose().split_once('.') else {
            return false;
        };
        let Ok(tag) = URL_SAFE_NO_PAD.decode(encoded_tag) else {
            return false;
        };

        let mac = self.csrf_mac(session.expose(), nonce);
        mac.verify_slice(&tag).is_ok()
    }

    fn hash_encoded_token(&self, domain: TokenDomain, token: &str) -> TokenHash {
        let mut mac = self.token_mac(domain);
        mac.update(token.as_bytes());
        TokenHash(mac.finalize().into_bytes().into())
    }

    fn token_mac(&self, domain: TokenDomain) -> HmacSha256 {
        let mut mac = HmacSha256::new_from_slice(&self.0).expect("HMAC accepts any key length");
        mac.update(TOKEN_HASH_CONTEXT);
        mac.update(domain.label());
        mac.update(&[0]);
        mac
    }

    fn csrf_tag(&self, session: &str, nonce: &str) -> [u8; 32] {
        self.csrf_mac(session, nonce).finalize().into_bytes().into()
    }

    fn csrf_mac(&self, session: &str, nonce: &str) -> HmacSha256 {
        let mut mac = HmacSha256::new_from_slice(&self.0).expect("HMAC accepts any key length");
        mac.update(CSRF_CONTEXT);
        mac.update(session.as_bytes());
        mac.update(&[0]);
        mac.update(nonce.as_bytes());
        mac
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenDomain {
    Invitation,
    Login,
    PublicView,
    Session,
}

impl TokenDomain {
    fn label(self) -> &'static [u8] {
        match self {
            Self::Invitation => b"invitation",
            Self::Login => b"login",
            Self::PublicView => b"public-view",
            Self::Session => b"session",
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct SecretToken(String);

impl SecretToken {
    pub fn parse(encoded: impl Into<String>) -> Option<Self> {
        let encoded = encoded.into();
        URL_SAFE_NO_PAD
            .decode(&encoded)
            .ok()
            .filter(|bytes| bytes.len() == TOKEN_BYTES)
            .map(|_| Self(encoded))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl Debug for SecretToken {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretToken([REDACTED])")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct TokenHash([u8; 32]);

impl TokenHash {
    pub(crate) fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct CsrfToken(String);

impl CsrfToken {
    pub fn from_encoded(encoded: impl Into<String>) -> Self {
        Self(encoded.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl Debug for CsrfToken {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("CsrfToken([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OneTimeTokenState {
    pub expires_at: i64,
    pub consumed_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

impl OneTimeTokenState {
    pub fn is_usable_at(self, now: i64) -> bool {
        now < self.expires_at && self.consumed_at.is_none() && self.revoked_at.is_none()
    }
}

pub struct SessionCookieBuilder<'a> {
    token: &'a SecretToken,
}

impl<'a> SessionCookieBuilder<'a> {
    pub fn new(token: &'a SecretToken) -> Self {
        Self { token }
    }

    pub fn build(self) -> String {
        format!(
            "__Host-commoncal_session={}; Path=/; Secure; HttpOnly; SameSite=Lax",
            self.token.expose()
        )
    }
}
