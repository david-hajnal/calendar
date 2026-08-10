# Implementation Plan: Publication token rotation fix

## 1. Problem

`rotate_publication` in `shared_view.rs:336-369` updates `token_hash` and `token_prefix` but does NOT set `revoked_at` on the old token. Old URLs continue to work.

## 2. Code changes

### 2a. `backend/src/shared_view.rs` — `rotate_publication` method (line 336-369)

Replace the current single UPDATE with a transaction that revokes the old token first:

```rust
pub async fn rotate_publication(
    &self,
    actor_user_id: i64,
    view_id: i64,
) -> Result<IssuedPublicView, SharedViewError> {
    let now = (self.clock)();
    let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;

    // Revoke the existing active publication token
    let revoked = sqlx::query(
        "UPDATE public_view_links
         SET revoked_at = ?, version = version + 1, updated_at = ?
         WHERE view_id = ? AND revoked_at IS NULL
           AND EXISTS(
               SELECT 1 FROM shared_views
               WHERE id = ? AND owner_user_id = ?
           )",
    )
    .bind(now)
    .bind(now)
    .bind(view_id)
    .bind(view_id)
    .bind(actor_user_id)
    .execute(&mut *transaction)
    .await?;

    if revoked.rows_affected() != 1 {
        transaction.rollback().await?;
        return Err(SharedViewError::NotFound);
    }

    // Issue the new token
    let token = self.token_key.generate_token();
    let prefix = token_prefix(token.expose());
    let hash = self.token_key.hash_token(TokenDomain::PublicView, &token);

    sqlx::query(
        "UPDATE public_view_links
         SET token_prefix = ?, token_hash = ?, version = version + 1, updated_at = ?
         WHERE view_id = ? AND revoked_at IS NOT NULL
           AND EXISTS(
               SELECT 1 FROM shared_views
               WHERE id = ? AND owner_user_id = ?
           )",
    )
    .bind(prefix)
    .bind(hash.as_bytes().as_slice())
    .bind(now)
    .bind(view_id)
    .bind(view_id)
    .bind(actor_user_id)
    .execute(&mut *transaction)
    .await?;

    transaction.commit().await?;

    Ok(IssuedPublicView {
        token: token.expose().to_owned(),
        publication: self.publication(actor_user_id, view_id).await?,
    })
}
```

Key change: the first UPDATE sets `revoked_at` on the old row. The second UPDATE only applies to the same row (now revoked), replacing the token.

### 2b. `backend/src/http.rs` — `rotate_publication` handler (line 1327-1339)

Add audit logging to the handler:

```rust
async fn rotate_publication(
    State(state): State<ApplicationState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(view_id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    let publication = state
        .shared_view_service
        .ok_or_else(ApiError::service_unavailable)?
        .rotate_publication(session.user.id, view_id)
        .await
        .map_err(map_shared_view_error)?;
    
    tracing::info!(
        user_id = session.user.id,
        view_id,
        error_code = "publication_token_rotated",
        "public view publication token rotated"
    );
    
    Ok(Json(publication))
}
```

### 2c. `backend/src/shared_view.rs` — `resolve_publication` validation (line 458-503)

Already correct — line 487 checks `record.revoked_at.is_some()` and returns `NotFound`. No change needed.

## 3. Test plan

### 3a. Unit test in `shared_view.rs`

```rust
#[tokio::test]
async fn test_rotate_publication_revokes_old_token() {
    let service = SharedViewService::new_at_with_key(pool, key, 1000);
    
    // Create publication
    let pub1 = service.create_publication(user_id, view_id, config).await.unwrap();
    
    // Verify old token works
    let meta1 = service.public_metadata(&pub1.token).await.unwrap();
    
    // Rotate
    let pub2 = service.rotate_publication(user_id, view_id).await.unwrap();
    
    // Old token should NOT work
    let result = service.public_metadata(&pub1.token).await;
    assert!(matches!(result, Err(SharedViewError::NotFound)));
    
    // New token should work
    let meta2 = service.public_metadata(&pub2.token).await.unwrap();
}
```

### 3b. Integration test in `http.rs`

Test that `POST /api/v1/views/:id/publication/rotate` returns new token and old token is invalidated.

## 4. Security review checklist

- [ ] Old token is revoked BEFORE new token is issued (atomic in transaction)
- [ ] No race condition where both tokens are valid simultaneously
- [ ] `resolve_publication` correctly checks `revoked_at`
- [ ] Audit log entry created for rotation
- [ ] Authorization check (owner_user_id) still enforced

## 5. Dependencies

No new crates. Uses existing transaction infrastructure.

## 6. Migration

No migration needed — `revoked_at` column already exists on `public_view_links`.
