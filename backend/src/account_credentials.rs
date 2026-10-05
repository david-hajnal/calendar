use sqlx::{Sqlite, Transaction};

pub enum RevocationScope {
    DisabledAccount,
    PasswordReset,
    EmailChanged,
}

/// Revoke credentials inside the account transition's write transaction.
/// Password and email changes retain independent integration credentials.
pub async fn revoke_account_credentials(
    transaction: &mut Transaction<'_, Sqlite>,
    user_id: i64,
    now: i64,
    scope: RevocationScope,
) -> Result<(), sqlx::Error> {
    for statement in [
        "UPDATE sessions SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL",
        "UPDATE login_tokens SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL AND consumed_at IS NULL",
        "UPDATE password_reset_tokens SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL AND consumed_at IS NULL",
    ] {
        sqlx::query(statement)
            .bind(now)
            .bind(user_id)
            .execute(&mut **transaction)
            .await?;
    }
    if matches!(
        scope,
        RevocationScope::DisabledAccount | RevocationScope::EmailChanged
    ) {
        sqlx::query("UPDATE email_change_requests SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL AND consumed_at IS NULL")
            .bind(now).bind(user_id).execute(&mut **transaction).await?;
        sqlx::query("UPDATE invitations SET revoked_at = ? WHERE normalized_email = (SELECT normalized_email FROM users WHERE id = ?) AND revoked_at IS NULL AND consumed_at IS NULL")
            .bind(now).bind(user_id).execute(&mut **transaction).await?;
    }
    if matches!(scope, RevocationScope::DisabledAccount) {
        for statement in [
            "UPDATE mcp_grant SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL",
            "UPDATE caldav_credentials SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL",
        ] {
            sqlx::query(statement)
                .bind(now)
                .bind(user_id)
                .execute(&mut **transaction)
                .await?;
        }
    }
    Ok(())
}
