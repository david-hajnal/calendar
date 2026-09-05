CREATE TABLE user_invitation_rate_limits (
    inviter_user_id   INTEGER NOT NULL REFERENCES users(id),
    date              INTEGER NOT NULL,
    count             INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (inviter_user_id, date)
);

CREATE INDEX idx_user_invitation_rate_limits_email_date
ON invitations(normalized_email, created_at)
WHERE consumed_at IS NULL AND revoked_at IS NULL;
