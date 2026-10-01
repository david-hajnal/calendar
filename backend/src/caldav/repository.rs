use sqlx::SqlitePool;

use crate::caldav::types::{
    CaldavAuthError, CaldavClientProperties, CaldavEventChange, CaldavEventResource,
};

type ChangeRow = (i64, i64, Option<i64>, String, i64, Option<String>);

/// Render a UTC instant as an ISO calendar date (`YYYY-MM-DD`) in the UTC zone.
fn utc_date(utc: i64) -> String {
    chrono::DateTime::from_timestamp(utc, 0)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

#[derive(Clone)]
pub struct CaldavRepository {
    pool: SqlitePool,
}

impl CaldavRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Return the stable DAV identity for an event, creating it on first use.
    pub async fn ensure_resource(
        &self,
        calendar_id: i64,
        event_id: i64,
        now: i64,
    ) -> Result<CaldavEventResource, CaldavAuthError> {
        if let Some(resource) = self.resolve_resource(calendar_id, event_id).await? {
            return Ok(resource);
        }
        let uid = uuid::Uuid::new_v4().to_string();
        let resource_name = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO caldav_event_resources (
                event_id, calendar_id, uid, resource_name, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(event_id)
        .bind(calendar_id)
        .bind(&uid)
        .bind(&resource_name)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|_| CaldavAuthError::Persistence)?;
        self.resolve_resource(calendar_id, event_id)
            .await?
            .ok_or(CaldavAuthError::Persistence)
    }

    pub async fn resolve_resource(
        &self,
        calendar_id: i64,
        event_id: i64,
    ) -> Result<Option<CaldavEventResource>, CaldavAuthError> {
        let row: Option<(Option<i64>, i64, String, String)> = sqlx::query_as(
            "SELECT event_id, calendar_id, uid, resource_name
             FROM caldav_event_resources
             WHERE calendar_id = ? AND event_id = ? AND deleted_at IS NULL",
        )
        .bind(calendar_id)
        .bind(event_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| CaldavAuthError::Persistence)?;
        Ok(row.and_then(|(event_id, calendar_id, uid, resource_name)| {
            event_id.map(|event_id| CaldavEventResource {
                event_id,
                calendar_id,
                uid,
                resource_name,
            })
        }))
    }

    pub async fn resolve_by_name(
        &self,
        calendar_id: i64,
        resource_name: &str,
    ) -> Result<Option<CaldavEventResource>, CaldavAuthError> {
        let row: Option<(Option<i64>, i64, String, String)> = sqlx::query_as(
            "SELECT event_id, calendar_id, uid, resource_name
             FROM caldav_event_resources
             WHERE calendar_id = ? AND resource_name = ? AND deleted_at IS NULL",
        )
        .bind(calendar_id)
        .bind(resource_name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| CaldavAuthError::Persistence)?;
        Ok(row.and_then(|(event_id, calendar_id, uid, resource_name)| {
            event_id.map(|event_id| CaldavEventResource {
                event_id,
                calendar_id,
                uid,
                resource_name,
            })
        }))
    }

    /// Create a DAV resource mapping with a client-supplied UID and resource
    /// name. Fails with `ResourceExists` if the name is taken or `UidConflict`
    /// if the UID is already mapped in this calendar.
    pub async fn create_resource(
        &self,
        calendar_id: i64,
        event_id: i64,
        uid: &str,
        resource_name: &str,
        now: i64,
    ) -> Result<CaldavEventResource, CaldavAuthError> {
        if self
            .resolve_by_name(calendar_id, resource_name)
            .await?
            .is_some()
        {
            return Err(CaldavAuthError::ResourceExists);
        }
        let uid_taken: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM caldav_event_resources
                WHERE calendar_id = ? AND uid = ? AND deleted_at IS NULL
             )",
        )
        .bind(calendar_id)
        .bind(uid)
        .fetch_one(&self.pool)
        .await
        .map_err(|_| CaldavAuthError::Persistence)?;
        if uid_taken {
            return Err(CaldavAuthError::UidConflict);
        }
        sqlx::query(
            "INSERT INTO caldav_event_resources (
                event_id, calendar_id, uid, resource_name, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(event_id)
        .bind(calendar_id)
        .bind(uid)
        .bind(resource_name)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|_| CaldavAuthError::Persistence)?;
        self.resolve_by_name(calendar_id, resource_name)
            .await?
            .ok_or(CaldavAuthError::Persistence)
    }

    /// Convert a live resource into a durable tombstone: clear the event
    /// reference and stamp the deletion. Returns `true` when a live row was
    /// tombstoned, `false` when no live resource matched (already deleted or
    /// absent).
    pub async fn tombstone_resource(
        &self,
        calendar_id: i64,
        event_id: i64,
        now: i64,
    ) -> Result<bool, CaldavAuthError> {
        let result = sqlx::query(
            "UPDATE caldav_event_resources
                 SET event_id = NULL, deleted_at = ?, updated_at = ?
               WHERE calendar_id = ? AND event_id = ? AND deleted_at IS NULL",
        )
        .bind(now)
        .bind(now)
        .bind(calendar_id)
        .bind(event_id)
        .execute(&self.pool)
        .await
        .map_err(|_| CaldavAuthError::Persistence)?;
        Ok(result.rows_affected() == 1)
    }

    /// Reverse a tombstone: restore the event reference and clear the deletion
    /// stamp. Used to compensate when the domain delete fails after the
    /// resource was tombstoned. Matches the specific tombstoned row by its
    /// resource name so other tombstones in the calendar are untouched.
    pub async fn restore_resource(
        &self,
        calendar_id: i64,
        event_id: i64,
        resource_name: &str,
        now: i64,
    ) -> Result<(), CaldavAuthError> {
        sqlx::query(
            "UPDATE caldav_event_resources
                 SET event_id = ?, deleted_at = NULL, updated_at = ?
               WHERE calendar_id = ? AND resource_name = ?
                 AND event_id IS NULL AND deleted_at IS NOT NULL",
        )
        .bind(event_id)
        .bind(now)
        .bind(calendar_id)
        .bind(resource_name)
        .execute(&self.pool)
        .await
        .map_err(|_| CaldavAuthError::Persistence)?;
        Ok(())
    }

    /// True when a durable tombstone exists for the named resource in this
    /// calendar, i.e. the resource was deleted but its mapping was retained.
    pub async fn tombstone_exists(
        &self,
        calendar_id: i64,
        resource_name: &str,
    ) -> Result<bool, CaldavAuthError> {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM caldav_event_resources
                WHERE calendar_id = ? AND resource_name = ? AND deleted_at IS NOT NULL
             )",
        )
        .bind(calendar_id)
        .bind(resource_name)
        .fetch_one(&self.pool)
        .await
        .map_err(|_| CaldavAuthError::Persistence)?;
        Ok(exists)
    }

    /// Event IDs currently exposed by this slice: timed and all-day events
    /// (recurring and non-recurring) that still exist in the calendar.
    /// Callers ensure a stable resource mapping for each before serializing.
    pub async fn list_exposed_event_ids(
        &self,
        calendar_id: i64,
    ) -> Result<Vec<i64>, CaldavAuthError> {
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM events
              WHERE calendar_id = ?
                AND event_kind IN ('timed', 'all_day')
              ORDER BY id",
        )
        .bind(calendar_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| CaldavAuthError::Persistence)?;
        Ok(ids)
    }

    /// Event IDs that overlap the given UTC time range, bounded by `limit`.
    ///
    /// Timed events are matched against their UTC instants; all-day events are
    /// matched against their calendar dates (the range endpoints are converted
    /// to UTC dates), so a query window that touches a day includes the
    /// all-day events spanning that day. Recurring events are included when
    /// their series start is before the range end (a safe superset: the
    /// caller serializes the full series and the client filters occurrences).
    pub async fn list_events_in_range(
        &self,
        calendar_id: i64,
        start_utc: i64,
        end_utc: i64,
        limit: usize,
    ) -> Result<Vec<i64>, CaldavAuthError> {
        let start_date = utc_date(start_utc);
        let end_date = utc_date(end_utc);
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM events
              WHERE calendar_id = ?
                AND (
                     (
                         recurrence_rule IS NULL
                         AND (
                              (
                                  event_kind = 'timed'
                                  AND timed_start_utc < ?
                                  AND timed_end_utc > ?
                              )
                              OR
                              (
                                  event_kind = 'all_day'
                                  AND all_day_start_date < ?
                                  AND all_day_end_date > ?
                              )
                         )
                     )
                     OR
                     (
                         recurrence_rule IS NOT NULL
                         AND (
                              (event_kind = 'timed' AND timed_start_utc < ?)
                              OR
                              (event_kind = 'all_day' AND all_day_start_date < ?)
                         )
                     )
                 )
              ORDER BY id
              LIMIT ?",
        )
        .bind(calendar_id)
        .bind(end_utc)
        .bind(start_utc)
        .bind(&end_date)
        .bind(&start_date)
        .bind(end_utc)
        .bind(&end_date)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| CaldavAuthError::Persistence)?;
        Ok(ids)
    }

    /// Read the ordered change log for one calendar, starting after the given
    /// revision. Returns changes with `id > after_revision` in ascending
    /// revision order, bounded by `limit`. This is the ordered change-log
    /// input that sync-token paging (T11) builds on.
    pub async fn list_changes_since(
        &self,
        calendar_id: i64,
        after_revision: i64,
        limit: usize,
    ) -> Result<Vec<CaldavEventChange>, CaldavAuthError> {
        let rows: Vec<ChangeRow> = sqlx::query_as(
            "SELECT id, calendar_id, event_id, change_type, created_at, resource_name
              FROM caldav_event_changes
              WHERE calendar_id = ? AND id > ?
              ORDER BY id
              LIMIT ?",
        )
        .bind(calendar_id)
        .bind(after_revision)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|_| CaldavAuthError::Persistence)?;
        Ok(rows
            .into_iter()
            .map(
                |(id, calendar_id, event_id, change_type, created_at, resource_name)| {
                    CaldavEventChange {
                        id,
                        calendar_id,
                        event_id,
                        change_type,
                        created_at,
                        resource_name,
                    }
                },
            )
            .collect())
    }

    /// Stamp the DAV resource name onto the most recent change-log entry for a
    /// calendar. Used by the DAV delete path, where the tombstone is applied
    /// before the domain delete records the change, so the name cannot be
    /// resolved from the live mapping at record time.
    pub async fn stamp_change_resource_name(
        &self,
        calendar_id: i64,
        event_id: i64,
        resource_name: &str,
    ) -> Result<(), CaldavAuthError> {
        sqlx::query(
            "UPDATE caldav_event_changes
                 SET resource_name = ?
               WHERE calendar_id = ? AND event_id = ? AND change_type = 'deleted'
                 AND resource_name IS NULL",
        )
        .bind(resource_name)
        .bind(calendar_id)
        .bind(event_id)
        .execute(&self.pool)
        .await
        .map_err(|_| CaldavAuthError::Persistence)?;
        Ok(())
    }

    /// Return the highest change-log revision for a calendar, or 0 when the
    /// calendar has no recorded changes. Used to mint the initial snapshot
    /// sync token.
    pub async fn latest_revision(&self, calendar_id: i64) -> Result<i64, CaldavAuthError> {
        let revision: Option<i64> =
            sqlx::query_scalar("SELECT MAX(id) FROM caldav_event_changes WHERE calendar_id = ?")
                .bind(calendar_id)
                .fetch_one(&self.pool)
                .await
                .map_err(|_| CaldavAuthError::Persistence)?;
        Ok(revision.unwrap_or(0))
    }

    /// Replace the allowlisted client properties for an event (T16). The
    /// incoming value is the source of truth: an empty value clears all
    /// preserved metadata. The row is upserted so both first-time writes and
    /// updates (including explicit removals) are handled uniformly.
    pub async fn save_client_properties(
        &self,
        event_id: i64,
        properties: &CaldavClientProperties,
        now: i64,
    ) -> Result<(), CaldavAuthError> {
        let json = serde_json::to_string(properties).map_err(|_| CaldavAuthError::Persistence)?;
        sqlx::query(
            "INSERT INTO caldav_event_properties (event_id, properties_json, updated_at)
             VALUES (?, ?, ?)
             ON CONFLICT(event_id) DO UPDATE SET
                properties_json = excluded.properties_json,
                updated_at = excluded.updated_at",
        )
        .bind(event_id)
        .bind(&json)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|_| CaldavAuthError::Persistence)?;
        Ok(())
    }

    /// Load the allowlisted client properties for an event, or `None` when no
    /// metadata has been preserved.
    pub async fn load_client_properties(
        &self,
        event_id: i64,
    ) -> Result<Option<CaldavClientProperties>, CaldavAuthError> {
        let row: Option<String> = sqlx::query_scalar(
            "SELECT properties_json FROM caldav_event_properties WHERE event_id = ?",
        )
        .bind(event_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| CaldavAuthError::Persistence)?;
        let Some(json) = row else {
            return Ok(None);
        };
        let properties: CaldavClientProperties =
            serde_json::from_str(&json).map_err(|_| CaldavAuthError::Persistence)?;
        Ok(Some(properties))
    }
}
