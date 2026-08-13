use std::fmt;

const BCRYPT_COST: u32 = 12;

#[derive(Debug, PartialEq)]
pub enum PasswordError {
    InvalidPassword,
    HashError(String),
}

impl fmt::Display for PasswordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPassword => write!(f, "invalid password"),
            Self::HashError(msg) => write!(f, "hash error: {msg}"),
        }
    }
}

impl std::error::Error for PasswordError {}

pub fn hash_password(password: &str) -> Result<String, PasswordError> {
    if password.is_empty() || password.len() > 72 {
        return Err(PasswordError::InvalidPassword);
    }
    bcrypt::hash(password, BCRYPT_COST).map_err(|e| PasswordError::HashError(e.to_string()))
}

pub fn verify_password(password: &str, hash: &str) -> Result<bool, PasswordError> {
    bcrypt::verify(password, hash).map_err(|e| PasswordError::HashError(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify_roundtrip() {
        let password = "secure-dev-password-123";
        let hash = hash_password(password).expect("hash should succeed");
        assert!(verify_password(password, &hash).expect("verify should succeed"));
    }

    #[test]
    fn verify_wrong_password_fails() {
        let password = "correct-horse";
        let wrong = "battery-staple";
        let hash = hash_password(password).expect("hash should succeed");
        assert!(!verify_password(wrong, &hash).expect("verify should succeed"));
    }

    #[test]
    fn hash_empty_password_rejected() {
        assert_eq!(hash_password(""), Err(PasswordError::InvalidPassword));
    }

    #[test]
    fn hash_returns_bcrypt_format() {
        let hash = hash_password("test").expect("hash should succeed");
        assert!(hash.starts_with("$2b$12$"));
    }
}
