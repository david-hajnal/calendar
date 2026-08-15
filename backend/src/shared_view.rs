use std::{
    collections::HashSet,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use chrono_tz::Tz;
use serde::Serialize;
use sqlx::{FromRow, SqlitePool};

use crate::{
    event::{EventProjection, EventRange, EventService, EventServiceError},
    security::{SecretKey, SecretToken, TokenDomain},
};

const PUBLIC_TOKEN_PREFIX_LENGTH: usize = 8;
const MAX_PUBLIC_EVENTS: usize = 1_000;

#[derive(Clone)]
pub struct SharedViewService {
    pool: SqlitePool,
    token_key: SecretKey,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl SharedViewService {
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            token_key: SecretKey::generate(),
            clock: Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system clock is before Unix epoch")
                    .as_secs() as i64
            }),
        }
    }

    pub fn new_at(pool: SqlitePool, now: i64) -> Self {
        Self {
            pool,
            token_key: SecretKey::generate(),
            clock: Arc::new(move || now),
        }
    }

    pub fn new_with_key(pool: SqlitePool, token_key: SecretKey) -> Self {
        Self {
            pool,
            token_key,
            clock: Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system clock is before Unix epoch")
                    .as_secs() as i64
            }),
        }
    }

    pub fn new_at_with_key(pool: SqlitePool, token_key: SecretKey, now: i64) -> Self {
        Self {
            pool,
            token_key,
            clock: Arc::new(move || now),
        }
    }

    pub async fn list(
        &self,
        actor_user_id: i64,
    ) -> Result<Vec<SharedViewProjection>, SharedViewError> {
        let records = sqlx::query_as::<_, SharedViewRecord>(
            "SELECT id, owner_user_id, name, version, created_at, updated_at
             FROM shared_views
             WHERE owner_user_id = ?
             ORDER BY id",
        )
        .bind(actor_user_id)
        .fetch_all(&self.pool)
        .await?;
        let mut views = Vec::with_capacity(records.len());
        for record in records {
            views.push(self.project(record, actor_user_id).await?);
        }
        Ok(views)
    }

    pub async fn get(
        &self,
        actor_user_id: i64,
        view_id: i64,
    ) -> Result<SharedViewProjection, SharedViewError> {
        let record = self.owned_view(actor_user_id, view_id).await?;
        self.project(record, actor_user_id).await
    }

    pub async fn create(
        &self,
        actor_user_id: i64,
        name: String,
    ) -> Result<SharedViewProjection, SharedViewError> {
        validate_name(&name)?;
        let now = (self.clock)();
        let result = sqlx::query(
            "INSERT INTO shared_views (
                owner_user_id, name, version, created_at, updated_at
             ) VALUES (?, ?, 1, ?, ?)",
        )
        .bind(actor_user_id)
        .bind(name.trim())
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        self.get(actor_user_id, result.last_insert_rowid()).await
    }

    pub async fn update(
        &self,
        actor_user_id: i64,
        view_id: i64,
        name: String,
    ) -> Result<SharedViewProjection, SharedViewError> {
        validate_name(&name)?;
        let result = sqlx::query(
            "UPDATE shared_views
             SET name = ?, version = version + 1, updated_at = ?
             WHERE id = ? AND owner_user_id = ?",
        )
        .bind(name.trim())
        .bind((self.clock)())
        .bind(view_id)
        .bind(actor_user_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(SharedViewError::NotFound);
        }
        self.get(actor_user_id, view_id).await
    }

    pub async fn delete(&self, actor_user_id: i64, view_id: i64) -> Result<(), SharedViewError> {
        let result = sqlx::query("DELETE FROM shared_views WHERE id = ? AND owner_user_id = ?")
            .bind(view_id)
            .bind(actor_user_id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() != 1 {
            return Err(SharedViewError::NotFound);
        }
        Ok(())
    }

    pub async fn replace_calendars(
        &self,
        actor_user_id: i64,
        view_id: i64,
        calendars: Vec<SharedViewCalendarInput>,
    ) -> Result<SharedViewProjection, SharedViewError> {
        validate_calendars(&calendars)?;
        let now = (self.clock)();
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let owned: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM shared_views WHERE id = ? AND owner_user_id = ?
             )",
        )
        .bind(view_id)
        .bind(actor_user_id)
        .fetch_one(&mut *transaction)
        .await?;
        if !owned {
            transaction.rollback().await?;
            return Err(SharedViewError::NotFound);
        }
        for calendar in &calendars {
            let accessible: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM calendar_acl
                    WHERE calendar_id = ? AND user_id = ?
                 )",
            )
            .bind(calendar.calendar_id)
            .bind(actor_user_id)
            .fetch_one(&mut *transaction)
            .await?;
            if !accessible {
                transaction.rollback().await?;
                return Err(SharedViewError::NotFound);
            }
        }
        sqlx::query("DELETE FROM shared_view_calendars WHERE view_id = ?")
            .bind(view_id)
            .execute(&mut *transaction)
            .await?;
        for calendar in calendars {
            sqlx::query(
                "INSERT INTO shared_view_calendars (
                    view_id, calendar_id, position, color
                 ) VALUES (?, ?, ?, ?)",
            )
            .bind(view_id)
            .bind(calendar.calendar_id)
            .bind(calendar.position)
            .bind(calendar.color.trim())
            .execute(&mut *transaction)
            .await?;
        }
        sqlx::query(
            "UPDATE shared_views
             SET version = version + 1, updated_at = ?
             WHERE id = ?",
        )
        .bind(now)
        .bind(view_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        self.get(actor_user_id, view_id).await
    }

    pub async fn events(
        &self,
        event_service: &EventService,
        actor_user_id: i64,
        is_superadmin: bool,
        view_id: i64,
        range: EventRange,
    ) -> Result<Vec<EventProjection>, SharedViewError> {
        event_service
            .validate_requested_range(&range)
            .map_err(SharedViewError::Event)?;
        let view = self.get(actor_user_id, view_id).await?;
        let mut events = Vec::new();
        for source in view.calendars {
            match event_service
                .list(
                    actor_user_id,
                    is_superadmin,
                    source.calendar_id,
                    range.clone(),
                )
                .await
            {
                Ok(mut source_events) => events.append(&mut source_events),
                Err(EventServiceError::NotFound) => {}
                Err(error) => return Err(SharedViewError::Event(error)),
            }
        }
        events.sort_by_key(|event| {
            (
                event.start_utc.unwrap_or(i64::MIN),
                event.calendar_id,
                event.id,
            )
        });
        Ok(events)
    }

    pub async fn create_publication(
        &self,
        actor_user_id: i64,
        view_id: i64,
        configuration: PublicViewConfiguration,
    ) -> Result<IssuedPublicView, SharedViewError> {
        validate_public_configuration(&configuration)?;
        self.owned_view(actor_user_id, view_id).await?;
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM public_view_links WHERE view_id = ?)")
                .bind(view_id)
                .fetch_one(&self.pool)
                .await?;
        if exists {
            return Err(SharedViewError::Conflict);
        }
        let token = self.token_key.generate_token();
        let prefix = token_prefix(token.expose());
        let hash = self.token_key.hash_token(TokenDomain::PublicView, &token);
        let now = (self.clock)();
        sqlx::query(
            "INSERT INTO public_view_links (
                view_id, token_prefix, token_hash, projection, display_timezone,
                expires_at, revoked_at, version, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, NULL, 1, ?, ?)",
        )
        .bind(view_id)
        .bind(prefix)
        .bind(hash.as_bytes().as_slice())
        .bind(configuration.projection.as_str())
        .bind(&configuration.display_timezone)
        .bind(configuration.expires_at)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(IssuedPublicView {
            token: token.expose().to_owned(),
            publication: self.publication(actor_user_id, view_id).await?,
        })
    }

    pub async fn configure_publication(
        &self,
        actor_user_id: i64,
        view_id: i64,
        configuration: PublicViewConfiguration,
    ) -> Result<PublicViewManagementProjection, SharedViewError> {
        validate_public_configuration(&configuration)?;
        let now = (self.clock)();
        let result = sqlx::query(
            "UPDATE public_view_links
             SET projection = ?, display_timezone = ?, expires_at = ?,
                 version = version + 1, updated_at = ?
             WHERE view_id = ?
               AND EXISTS(
                   SELECT 1 FROM shared_views
                   WHERE id = ? AND owner_user_id = ?
               )",
        )
        .bind(configuration.projection.as_str())
        .bind(configuration.display_timezone)
        .bind(configuration.expires_at)
        .bind(now)
        .bind(view_id)
        .bind(view_id)
        .bind(actor_user_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(SharedViewError::NotFound);
        }
        self.publication(actor_user_id, view_id).await
    }

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

    pub async fn revoke_publication(
        &self,
        actor_user_id: i64,
        view_id: i64,
    ) -> Result<(), SharedViewError> {
        let now = (self.clock)();
        let result = sqlx::query(
            "UPDATE public_view_links
             SET revoked_at = ?, token_hash = X'0000000000000000000000000000000000000000000000000000000000000000', version = version + 1, updated_at = ?
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
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(SharedViewError::NotFound);
        }
        Ok(())
    }

    pub async fn generate_caldav_token(
        &self,
        actor_user_id: i64,
        view_id: i64,
    ) -> Result<String, SharedViewError> {
        let token = self.token_key.generate_token();
        let hash = self.token_key.hash_token(TokenDomain::Caldav, &token);
        let now = (self.clock)();
        let result = sqlx::query(
            "UPDATE public_view_links
             SET caldav_token_hash = ?, caldav_enabled = 1, version = version + 1, updated_at = ?
             WHERE view_id = ? AND EXISTS(
                 SELECT 1 FROM shared_views
                 WHERE id = ? AND owner_user_id = ?
             )",
        )
        .bind(hash.as_bytes().as_slice())
        .bind(now)
        .bind(view_id)
        .bind(view_id)
        .bind(actor_user_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(SharedViewError::NotFound);
        }
        Ok(token.expose().to_owned())
    }

    pub async fn revoke_caldav_token(
        &self,
        actor_user_id: i64,
        view_id: i64,
    ) -> Result<(), SharedViewError> {
        let now = (self.clock)();
        let result = sqlx::query(
            "UPDATE public_view_links
             SET caldav_token_hash = NULL, caldav_enabled = 0, version = version + 1, updated_at = ?
             WHERE view_id = ? AND EXISTS(
                 SELECT 1 FROM shared_views
                 WHERE id = ? AND owner_user_id = ?
             )",
        )
        .bind(now)
        .bind(view_id)
        .bind(view_id)
        .bind(actor_user_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(SharedViewError::NotFound);
        }
        Ok(())
    }

    pub async fn set_caldav_enabled(
        &self,
        actor_user_id: i64,
        view_id: i64,
        enabled: bool,
    ) -> Result<PublicViewManagementProjection, SharedViewError> {
        let now = (self.clock)();
        if enabled {
            let has_token: Option<bool> = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM public_view_links WHERE view_id = ? AND caldav_token_hash IS NOT NULL)",
            )
            .bind(view_id)
            .fetch_one(&self.pool)
            .await?;
            if !has_token.unwrap_or(false) {
                let token = self.token_key.generate_token();
                let hash = self.token_key.hash_token(TokenDomain::Caldav, &token);
                sqlx::query(
                    "UPDATE public_view_links
                     SET caldav_token_hash = ?, caldav_enabled = 1, version = version + 1, updated_at = ?
                     WHERE view_id = ? AND EXISTS(
                         SELECT 1 FROM shared_views
                         WHERE id = ? AND owner_user_id = ?
                     )",
                )
                .bind(hash.as_bytes().as_slice())
                .bind(now)
                .bind(view_id)
                .bind(view_id)
                .bind(actor_user_id)
                .execute(&self.pool)
                .await?;
            } else {
                sqlx::query(
                    "UPDATE public_view_links
                     SET caldav_enabled = 1, version = version + 1, updated_at = ?
                     WHERE view_id = ? AND EXISTS(
                         SELECT 1 FROM shared_views
                         WHERE id = ? AND owner_user_id = ?
                     )",
                )
                .bind(now)
                .bind(view_id)
                .bind(view_id)
                .bind(actor_user_id)
                .execute(&self.pool)
                .await?;
            }
        } else {
            sqlx::query(
                "UPDATE public_view_links
                 SET caldav_token_hash = NULL, caldav_enabled = 0, version = version + 1, updated_at = ?
                 WHERE view_id = ? AND EXISTS(
                     SELECT 1 FROM shared_views
                     WHERE id = ? AND owner_user_id = ?
                 )",
            )
            .bind(now)
            .bind(view_id)
            .bind(view_id)
            .bind(actor_user_id)
            .execute(&self.pool)
            .await?;
        }
        self.publication(actor_user_id, view_id).await
    }

    pub async fn public_metadata(
        &self,
        encoded_token: &str,
    ) -> Result<PublicViewMetadata, SharedViewError> {
        let resolved = self.resolve_publication(encoded_token).await?;
        Ok(PublicViewMetadata {
            name: resolved.name,
            projection: resolved.projection,
            display_timezone: resolved.display_timezone,
            expires_at: resolved.expires_at,
        })
    }

    pub async fn public_metadata_with_caldav(
        &self,
        encoded_token: &str,
    ) -> Result<PublicViewMetadataWithCaldav, SharedViewError> {
        let resolved = self.resolve_publication(encoded_token).await?;
        let has_caldav: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM public_view_links
                WHERE view_id = ? AND caldav_enabled = 1 AND caldav_token_hash IS NOT NULL
             )",
        )
        .bind(resolved.view_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(PublicViewMetadataWithCaldav {
            name: resolved.name,
            projection: resolved.projection,
            display_timezone: resolved.display_timezone,
            expires_at: resolved.expires_at,
            has_caldav,
        })
    }

    pub async fn public_events(
        &self,
        event_service: &EventService,
        encoded_token: &str,
        range: EventRange,
    ) -> Result<Vec<PublicEventProjection>, SharedViewError> {
        let resolved = self.resolve_publication(encoded_token).await?;
        let events = self
            .events(
                event_service,
                resolved.owner_user_id,
                false,
                resolved.view_id,
                range,
            )
            .await?;
        Ok(events
            .into_iter()
            .take(MAX_PUBLIC_EVENTS)
            .map(|event| project_public_event(event, resolved.projection))
            .collect())
    }

    pub async fn caldav_events(
        &self,
        event_service: &EventService,
        encoded_token: &str,
        range: EventRange,
    ) -> Result<Vec<EventProjection>, SharedViewError> {
        let resolved = self.resolve_caldav_publication(encoded_token).await?;
        let has_caldav: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM public_view_links
                WHERE view_id = ? AND caldav_enabled = 1 AND caldav_token_hash IS NOT NULL
             )",
        )
        .bind(resolved.view_id)
        .fetch_one(&self.pool)
        .await?;
        if !has_caldav {
            return Err(SharedViewError::NotFound);
        }
        self.events(
            event_service,
            resolved.owner_user_id,
            false,
            resolved.view_id,
            range,
        )
        .await
    }

    async fn publication(
        &self,
        actor_user_id: i64,
        view_id: i64,
    ) -> Result<PublicViewManagementProjection, SharedViewError> {
        let record = sqlx::query_as::<_, PublicViewManagementRecord>(
            "SELECT public_view_links.projection,
                    public_view_links.display_timezone,
                    public_view_links.expires_at,
                    public_view_links.revoked_at,
                    public_view_links.version,
                    public_view_links.caldav_enabled,
                    public_view_links.caldav_token_hash
             FROM public_view_links
             JOIN shared_views ON shared_views.id = public_view_links.view_id
             WHERE public_view_links.view_id = ? AND shared_views.owner_user_id = ?",
        )
        .bind(view_id)
        .bind(actor_user_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(SharedViewError::NotFound)?;
        record.try_into()
    }

    async fn resolve_publication(
        &self,
        encoded_token: &str,
    ) -> Result<ResolvedPublicView, SharedViewError> {
        let token =
            SecretToken::parse(encoded_token.to_owned()).ok_or(SharedViewError::NotFound)?;
        let record = sqlx::query_as::<_, PublicViewResolutionRecord>(
            "SELECT public_view_links.token_hash,
                    public_view_links.projection,
                    public_view_links.display_timezone,
                    public_view_links.expires_at,
                    public_view_links.revoked_at,
                    shared_views.id AS view_id,
                    shared_views.owner_user_id,
                    shared_views.name
             FROM public_view_links
             JOIN shared_views ON shared_views.id = public_view_links.view_id
             WHERE public_view_links.token_prefix = ?",
        )
        .bind(token_prefix(encoded_token))
        .fetch_optional(&self.pool)
        .await?
        .ok_or(SharedViewError::NotFound)?;
        let expected = record
            .token_hash
            .as_slice()
            .try_into()
            .map(crate::security::TokenHash::from_bytes)
            .map_err(|_| SharedViewError::NotFound)?;
        let token_matches = self
            .token_key
            .verify_token(TokenDomain::PublicView, &token, &expected);
        if (self.clock)() >= record.expires_at || !token_matches {
            return Err(SharedViewError::NotFound);
        }
        Ok(ResolvedPublicView {
            view_id: record.view_id,
            owner_user_id: record.owner_user_id,
            name: record.name,
            projection: record.projection.parse()?,
            display_timezone: record.display_timezone,
            expires_at: record.expires_at,
        })
    }

    async fn resolve_caldav_publication(
        &self,
        encoded_token: &str,
    ) -> Result<ResolvedCaldavView, SharedViewError> {
        let token =
            SecretToken::parse(encoded_token.to_owned()).ok_or(SharedViewError::NotFound)?;
        let record = sqlx::query_as::<_, CaldavResolutionRecord>(
            "SELECT public_view_links.caldav_token_hash,
                    public_view_links.expires_at,
                    shared_views.id AS view_id,
                    shared_views.owner_user_id,
                    shared_views.name
             FROM public_view_links
             JOIN shared_views ON shared_views.id = public_view_links.view_id
             WHERE public_view_links.caldav_token_hash IS NOT NULL
             LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?
        .ok_or(SharedViewError::NotFound)?;
        let expected = record
            .caldav_token_hash
            .as_slice()
            .try_into()
            .map(crate::security::TokenHash::from_bytes)
            .map_err(|_| SharedViewError::NotFound)?;
        let token_matches = self
            .token_key
            .verify_token(TokenDomain::Caldav, &token, &expected);
        if (self.clock)() >= record.expires_at || !token_matches {
            return Err(SharedViewError::NotFound);
        }
        Ok(ResolvedCaldavView {
            view_id: record.view_id,
            owner_user_id: record.owner_user_id,
        })
    }

    async fn owned_view(
        &self,
        actor_user_id: i64,
        view_id: i64,
    ) -> Result<SharedViewRecord, SharedViewError> {
        sqlx::query_as::<_, SharedViewRecord>(
            "SELECT id, owner_user_id, name, version, created_at, updated_at
             FROM shared_views
             WHERE id = ? AND owner_user_id = ?",
        )
        .bind(view_id)
        .bind(actor_user_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(SharedViewError::NotFound)
    }

    async fn project(
        &self,
        record: SharedViewRecord,
        actor_user_id: i64,
    ) -> Result<SharedViewProjection, SharedViewError> {
        let calendars = sqlx::query_as::<_, SharedViewCalendarProjection>(
            "SELECT shared_view_calendars.calendar_id,
                    shared_view_calendars.position,
                    shared_view_calendars.color
             FROM shared_view_calendars
             JOIN calendar_acl
               ON calendar_acl.calendar_id = shared_view_calendars.calendar_id
              AND calendar_acl.user_id = ?
             WHERE shared_view_calendars.view_id = ?
             ORDER BY shared_view_calendars.position",
        )
        .bind(actor_user_id)
        .bind(record.id)
        .fetch_all(&self.pool)
        .await?;
        Ok(SharedViewProjection {
            id: record.id,
            owner_user_id: record.owner_user_id,
            name: record.name,
            version: record.version,
            created_at: record.created_at,
            updated_at: record.updated_at,
            calendars,
        })
    }
}

fn validate_name(name: &str) -> Result<(), SharedViewError> {
    if name.trim().is_empty() || name.len() > 200 {
        Err(SharedViewError::InvalidInput)
    } else {
        Ok(())
    }
}

fn validate_calendars(calendars: &[SharedViewCalendarInput]) -> Result<(), SharedViewError> {
    let mut calendar_ids = HashSet::new();
    let mut positions = HashSet::new();
    for calendar in calendars {
        if calendar.position < 0
            || calendar.position as usize >= calendars.len()
            || calendar.color.trim().is_empty()
            || calendar.color.len() > 64
            || !calendar_ids.insert(calendar.calendar_id)
            || !positions.insert(calendar.position)
        {
            return Err(SharedViewError::InvalidInput);
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize)]
pub struct SharedViewProjection {
    pub id: i64,
    pub owner_user_id: i64,
    pub name: String,
    pub version: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub calendars: Vec<SharedViewCalendarProjection>,
}

#[derive(Clone, Debug, FromRow, Serialize)]
pub struct SharedViewCalendarProjection {
    pub calendar_id: i64,
    pub position: i64,
    pub color: String,
}

#[derive(Clone, Debug)]
pub struct SharedViewCalendarInput {
    pub calendar_id: i64,
    pub position: i64,
    pub color: String,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicViewProjection {
    FullDetails,
    TitleAndTime,
    FreeBusy,
}

impl PublicViewProjection {
    fn as_str(self) -> &'static str {
        match self {
            Self::FullDetails => "full_details",
            Self::TitleAndTime => "title_and_time",
            Self::FreeBusy => "free_busy",
        }
    }
}

impl std::str::FromStr for PublicViewProjection {
    type Err = SharedViewError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "full_details" => Ok(Self::FullDetails),
            "title_and_time" => Ok(Self::TitleAndTime),
            "free_busy" => Ok(Self::FreeBusy),
            _ => Err(SharedViewError::InvalidInput),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PublicViewConfiguration {
    pub projection: PublicViewProjection,
    pub display_timezone: String,
    pub expires_at: i64,
}

#[derive(Serialize)]
pub struct IssuedPublicView {
    pub token: String,
    #[serde(flatten)]
    pub publication: PublicViewManagementProjection,
}

#[derive(Serialize)]
pub struct PublicViewManagementProjection {
    pub projection: PublicViewProjection,
    pub display_timezone: String,
    pub expires_at: i64,
    pub revoked: bool,
    pub version: i64,
    pub caldav_enabled: bool,
    pub caldav_url: Option<String>,
}

#[derive(Serialize)]
pub struct PublicViewMetadata {
    pub name: String,
    pub projection: PublicViewProjection,
    pub display_timezone: String,
    pub expires_at: i64,
}

#[derive(Serialize)]
pub struct PublicViewMetadataWithCaldav {
    pub name: String,
    pub projection: PublicViewProjection,
    pub display_timezone: String,
    pub expires_at: i64,
    pub has_caldav: bool,
}

#[derive(Serialize)]
pub struct PublicEventProjection {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<&'static str>,
    pub event_kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_utc: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_utc: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub busy: Option<bool>,
}

#[derive(FromRow)]
struct SharedViewRecord {
    id: i64,
    owner_user_id: i64,
    name: String,
    version: i64,
    created_at: i64,
    updated_at: i64,
}

#[derive(FromRow)]
struct PublicViewManagementRecord {
    projection: String,
    display_timezone: String,
    expires_at: i64,
    revoked_at: Option<i64>,
    version: i64,
    caldav_enabled: i64,
    caldav_token_hash: Option<Vec<u8>>,
}

impl TryFrom<PublicViewManagementRecord> for PublicViewManagementProjection {
    type Error = SharedViewError;

    fn try_from(record: PublicViewManagementRecord) -> Result<Self, Self::Error> {
        let caldav_enabled = record.caldav_enabled == 1;
        let caldav_url = if caldav_enabled && record.caldav_token_hash.is_some() {
            Some(format!("webcal://{}", "[REDACTED_TOKEN]"))
        } else {
            None
        };
        Ok(Self {
            projection: record.projection.parse()?,
            display_timezone: record.display_timezone,
            expires_at: record.expires_at,
            revoked: record.revoked_at.is_some(),
            version: record.version,
            caldav_enabled,
            caldav_url,
        })
    }
}

#[derive(FromRow)]
struct PublicViewResolutionRecord {
    token_hash: Vec<u8>,
    projection: String,
    display_timezone: String,
    expires_at: i64,
    #[allow(dead_code)]
    #[allow(unused_variables)]
    revoked_at: Option<i64>,
    view_id: i64,
    owner_user_id: i64,
    name: String,
}

struct ResolvedPublicView {
    view_id: i64,
    owner_user_id: i64,
    name: String,
    projection: PublicViewProjection,
    display_timezone: String,
    expires_at: i64,
}

struct ResolvedCaldavView {
    view_id: i64,
    owner_user_id: i64,
}

#[derive(FromRow)]
struct CaldavResolutionRecord {
    caldav_token_hash: Vec<u8>,
    expires_at: i64,
    view_id: i64,
    owner_user_id: i64,
    #[allow(dead_code)]
    name: String,
}

fn validate_public_configuration(
    configuration: &PublicViewConfiguration,
) -> Result<(), SharedViewError> {
    if configuration.display_timezone.len() > 100
        || configuration.display_timezone.parse::<Tz>().is_err()
    {
        return Err(SharedViewError::InvalidInput);
    }
    Ok(())
}

fn token_prefix(token: &str) -> &str {
    &token[..PUBLIC_TOKEN_PREFIX_LENGTH]
}

fn project_public_event(
    event: EventProjection,
    projection: PublicViewProjection,
) -> PublicEventProjection {
    let include_title = matches!(
        projection,
        PublicViewProjection::FullDetails | PublicViewProjection::TitleAndTime
    );
    let include_details = matches!(projection, PublicViewProjection::FullDetails);
    PublicEventProjection {
        title: include_title.then_some(event.title).flatten(),
        description: include_details.then_some(event.description).flatten(),
        location: include_details.then_some(event.location).flatten(),
        status: include_details.then_some(event.status),
        event_kind: event.event_kind,
        start_utc: event.start_utc,
        end_utc: event.end_utc,
        timezone: if include_title { event.timezone } else { None },
        start_date: event.start_date,
        end_date: event.end_date,
        busy: matches!(projection, PublicViewProjection::FreeBusy).then_some(true),
    }
}

#[derive(Debug)]
pub enum SharedViewError {
    Conflict,
    Database(sqlx::Error),
    Event(EventServiceError),
    InvalidInput,
    NotFound,
}

impl From<sqlx::Error> for SharedViewError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    struct TestDb {
        _file: NamedTempFile,
        pool: SqlitePool,
    }

    impl TestDb {
        async fn new() -> Self {
            let file = NamedTempFile::new().unwrap();
            let conn_str = format!("sqlite:{}", file.path().to_str().unwrap());
            let pool = SqlitePool::connect(&conn_str).await.unwrap();
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS shared_views (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    owner_user_id INTEGER NOT NULL,
                    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
                    version INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                )",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS shared_view_calendars (
                    view_id INTEGER NOT NULL,
                    calendar_id INTEGER NOT NULL,
                    position INTEGER NOT NULL CHECK (position >= 0),
                    color TEXT NOT NULL CHECK (length(trim(color)) > 0),
                    PRIMARY KEY (view_id, calendar_id),
                    UNIQUE (view_id, position)
                )",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS public_view_links (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    view_id INTEGER NOT NULL UNIQUE,
                    token_prefix TEXT NOT NULL UNIQUE CHECK (length(token_prefix) = 8),
                    token_hash BLOB NOT NULL CHECK (length(token_hash) = 32),
                    projection TEXT NOT NULL CHECK (projection IN ('full_details', 'title_and_time', 'free_busy')),
                    display_timezone TEXT NOT NULL CHECK (length(trim(display_timezone)) > 0),
                    expires_at INTEGER NOT NULL,
                    revoked_at INTEGER,
                    version INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    caldav_token_hash BLOB,
                    caldav_enabled INTEGER DEFAULT 0 CHECK (caldav_enabled IN (0, 1))
                )",
            )
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS calendar_acl (
                    calendar_id INTEGER NOT NULL,
                    user_id INTEGER NOT NULL,
                    PRIMARY KEY (calendar_id, user_id)
                )",
            )
            .execute(&pool)
            .await
            .unwrap();
            Self { _file: file, pool }
        }
    }

    #[tokio::test]
    async fn test_rotate_publication_revokes_old_token() {
        let db = TestDb::new().await;
        let key = SecretKey::generate();
        let now = 1000i64;
        let service = SharedViewService::new_at_with_key(db.pool, key, now);

        let user_id: i64 = 1;
        let view_id: i64 = 1;

        sqlx::query(
            "INSERT INTO shared_views (id, owner_user_id, name, version, created_at, updated_at)
             VALUES (?, ?, 'Test View', 1, ?, ?)",
        )
        .bind(view_id)
        .bind(user_id)
        .bind(now)
        .bind(now)
        .execute(&service.pool)
        .await
        .unwrap();

        let config = PublicViewConfiguration {
            projection: PublicViewProjection::FullDetails,
            display_timezone: "UTC".to_string(),
            expires_at: now + 86400,
        };
        let pub1 = service
            .create_publication(user_id, view_id, config)
            .await
            .unwrap();

        let meta1 = service.public_metadata(&pub1.token).await.unwrap();
        assert_eq!(meta1.name, "Test View");

        let pub2 = service.rotate_publication(user_id, view_id).await.unwrap();

        let meta2 = service.public_metadata(&pub2.token).await.unwrap();
        assert_eq!(meta2.name, "Test View");

        let result = service.public_metadata(&pub1.token).await;
        assert!(
            matches!(result, Err(SharedViewError::NotFound)),
            "old token should be revoked but still works"
        );
    }

    #[tokio::test]
    async fn test_rotate_publication_new_token_is_different() {
        let db = TestDb::new().await;
        let key = SecretKey::generate();
        let now = 1000i64;
        let service = SharedViewService::new_at_with_key(db.pool, key, now);

        let user_id: i64 = 1;
        let view_id: i64 = 1;

        sqlx::query(
            "INSERT INTO shared_views (id, owner_user_id, name, version, created_at, updated_at)
             VALUES (?, ?, 'Test View', 1, ?, ?)",
        )
        .bind(view_id)
        .bind(user_id)
        .bind(now)
        .bind(now)
        .execute(&service.pool)
        .await
        .unwrap();

        let config = PublicViewConfiguration {
            projection: PublicViewProjection::FullDetails,
            display_timezone: "UTC".to_string(),
            expires_at: now + 86400,
        };
        let pub1 = service
            .create_publication(user_id, view_id, config)
            .await
            .unwrap();

        let pub2 = service.rotate_publication(user_id, view_id).await.unwrap();

        assert_ne!(pub1.token, pub2.token);
    }
}
