// Request/session-safe validated identity context.
//
// The validated identity is stored in a `tokio` task-local so that each
// in-flight request carries its own identity. This is concurrency-safe: two
// concurrent clients are processed in distinct tasks, so neither can observe
// the other's identity. This deliberately replaces the lab's global
// `CURRENT_CLAIMS` slot, which is only safe for sequential traffic.
//
// The auth middleware sets the value for the duration of the downstream
// request; the rmcp tool handlers read it at call time.

use crate::oauth::AuthStrength;

/// The validated identity for the current request.
#[derive(Clone, Debug, PartialEq)]
pub struct Identity {
    pub user_id: i64,
    pub client_id: String,
    pub scopes: Vec<String>,
    pub auth_strength: AuthStrength,
    pub auth_time: i64,
    pub token_id: String,
}

impl Identity {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

impl From<&crate::oauth::TokenValidationResult> for Identity {
    fn from(token: &crate::oauth::TokenValidationResult) -> Self {
        Self {
            user_id: token.user_id,
            client_id: token.oauth_client_id.clone(),
            scopes: token.scopes.clone(),
            auth_strength: token.auth_strength.clone(),
            auth_time: token.auth_time,
            token_id: token.token_id.clone(),
        }
    }
}

tokio::task_local! {
    /// The validated identity for the current request, if authenticated.
    pub static IDENTITY: Option<Identity>;
}

/// Read the validated identity for the current request.
///
/// Returns `None` when no identity has been established for this task (i.e. the
/// request did not pass the auth middleware). Tool handlers treat `None` as an
/// authorization failure and fail closed.
pub fn current() -> Option<Identity> {
    IDENTITY.try_with(|v| v.clone()).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_from_token_validation_result() {
        let token = crate::oauth::TokenValidationResult {
            user_id: 42,
            oauth_client_id: "client-1".to_string(),
            scopes: vec!["commoncal.calendar.metadata.read".to_string()],
            auth_strength: AuthStrength::Passkey,
            auth_time: 1_700_000_000,
            expires_at: 1_700_003_600,
            token_id: "token-1".to_string(),
        };
        let identity = Identity::from(&token);
        assert_eq!(identity.user_id, 42);
        assert_eq!(identity.client_id, "client-1");
        assert!(identity.has_scope("commoncal.calendar.metadata.read"));
        assert!(!identity.has_scope("commoncal.event.create"));
        assert_eq!(identity.auth_time, 1_700_000_000);
        assert_eq!(identity.token_id, "token-1");
    }

    #[tokio::test]
    async fn current_returns_none_outside_auth_scope() {
        assert!(current().is_none());
    }

    #[tokio::test]
    async fn current_returns_identity_within_auth_scope() {
        let identity = Identity {
            user_id: 7,
            client_id: "client-7".to_string(),
            scopes: vec![],
            auth_strength: AuthStrength::Mfa,
            auth_time: 0,
            token_id: "token-7".to_string(),
        };
        let observed = IDENTITY
            .scope(Some(identity.clone()), async { current() })
            .await;
        assert_eq!(observed, Some(identity));
        assert!(
            current().is_none(),
            "identity must not escape its request scope"
        );
    }

    #[tokio::test]
    async fn concurrent_tasks_do_not_share_identity() {
        let alice = Identity {
            user_id: 1,
            client_id: "alice".to_string(),
            scopes: vec![],
            auth_strength: AuthStrength::Passkey,
            auth_time: 0,
            token_id: "a".to_string(),
        };
        let bob = Identity {
            user_id: 2,
            client_id: "bob".to_string(),
            scopes: vec![],
            auth_strength: AuthStrength::Passkey,
            auth_time: 0,
            token_id: "b".to_string(),
        };

        let alice_task = tokio::spawn(async move {
            IDENTITY
                .scope(Some(alice.clone()), async {
                    // Yield to force interleaving with the other task.
                    tokio::task::yield_now().await;
                    current().map(|i| i.user_id)
                })
                .await
        });
        let bob_task = tokio::spawn(async move {
            IDENTITY
                .scope(Some(bob.clone()), async {
                    tokio::task::yield_now().await;
                    current().map(|i| i.user_id)
                })
                .await
        });

        let (alice_user, bob_user) = tokio::join!(alice_task, bob_task);
        assert_eq!(
            alice_user.unwrap().unwrap(),
            1,
            "alice must see her own identity"
        );
        assert_eq!(
            bob_user.unwrap().unwrap(),
            2,
            "bob must see his own identity"
        );
    }
}
