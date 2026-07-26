CREATE UNIQUE INDEX invitations_one_pending_per_email_idx
ON invitations(normalized_email)
WHERE revoked_at IS NULL AND consumed_at IS NULL;
