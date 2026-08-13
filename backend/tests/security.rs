use std::collections::HashSet;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use commoncal_backend::security::{
    OneTimeTokenState, SecretKey, SessionCookieBuilder, TokenDomain,
};

const KEY_BYTES: [u8; 32] = [0x42; 32];

fn key() -> SecretKey {
    SecretKey::new(KEY_BYTES)
}

#[test]
fn generated_tokens_have_sufficient_entropy_and_do_not_repeat() {
    let key = key();
    let tokens: HashSet<_> = (0..128)
        .map(|_| key.generate_token().expose().to_owned())
        .collect();

    assert_eq!(tokens.len(), 128);
    assert!(tokens.iter().all(|token| {
        URL_SAFE_NO_PAD
            .decode(token)
            .is_ok_and(|bytes| bytes.len() == 32)
    }));
}

#[test]
fn correct_token_verifies() {
    let key = key();
    let token = key.generate_token();
    let hash = key.hash_token(TokenDomain::Invitation, &token);

    assert!(key.verify_token(TokenDomain::Invitation, &token, &hash));
}

#[test]
fn modified_token_fails_verification() {
    let key = key();
    let token = key.generate_token();
    let hash = key.hash_token(TokenDomain::Login, &token);
    let modified = format!("{}x", token.expose());

    assert!(!key.verify_encoded_token(TokenDomain::Login, &modified, &hash));
}

#[test]
fn tokens_from_different_domains_cannot_be_substituted() {
    let key = key();
    let token = key.generate_token();
    let invitation_hash = key.hash_token(TokenDomain::Invitation, &token);

    assert!(!key.verify_token(TokenDomain::Login, &token, &invitation_hash));
    assert!(!key.verify_token(TokenDomain::Session, &token, &invitation_hash));
}

#[test]
fn expired_token_fails() {
    let state = OneTimeTokenState {
        expires_at: 100,
        consumed_at: None,
        revoked_at: None,
    };

    assert!(!state.is_usable_at(100));
}

#[test]
fn consumed_token_fails() {
    let state = OneTimeTokenState {
        expires_at: 200,
        consumed_at: Some(99),
        revoked_at: None,
    };

    assert!(!state.is_usable_at(100));
}

#[test]
fn session_cookie_has_required_security_attributes_and_path() {
    let token = key().generate_token();
    let cookie = SessionCookieBuilder::new(&token).is_secure(true).build();

    assert!(cookie.contains("Secure"));
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Lax"));
    assert!(cookie.contains("Path=/"));
}

#[test]
fn csrf_token_from_another_session_fails() {
    let key = key();
    let first_session = key.generate_token();
    let second_session = key.generate_token();
    let csrf = key.generate_csrf_token(&first_session);

    assert!(key.validate_csrf_token(&first_session, &csrf));
    assert!(!key.validate_csrf_token(&second_session, &csrf));
}

#[test]
fn derive_different_inputs_produce_different_keys() {
    let key1 = SecretKey::derive(b"input1");
    let key2 = SecretKey::derive(b"input2");
    let enc1 = key1.encrypt_secret(b"data");
    let enc2 = key2.encrypt_secret(b"data");
    assert_ne!(enc1, enc2);
}

#[test]
fn derive_key_can_encrypt_and_decrypt() {
    let derived = SecretKey::derive(b"password");
    let plaintext = b"secret data";
    let encrypted = derived.encrypt_secret(plaintext);
    let decrypted = derived.decrypt_secret(&encrypted);
    assert_eq!(decrypted, Some(plaintext.to_vec()));
}

#[test]
fn decrypt_with_wrong_key_returns_none() {
    let key1 = SecretKey::new([0x42; 32]);
    let key2 = SecretKey::new([0x43; 32]);
    let encrypted = key1.encrypt_secret(b"secret");
    let decrypted = key2.decrypt_secret(&encrypted);
    assert!(decrypted.is_none());
}

#[test]
fn decrypt_with_tampered_ciphertext_returns_none() {
    let key = SecretKey::new([0x42; 32]);
    let mut encrypted = key.encrypt_secret(b"secret");
    encrypted[12] ^= 0xFF;
    let decrypted = key.decrypt_secret(&encrypted);
    assert!(decrypted.is_none());
}
