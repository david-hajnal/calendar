use sqlx::SqlitePool;

use crate::caldav::types::{
    CaldavAuthError, CaldavClientProperties, CaldavEventChange, CaldavEventResource,
};

type ChangeRow = (i64, i64, Option<i64>, String, i64, Option<String>);

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
             ) VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(event_id) DO NOTHING",
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

    /// Evaluate recurrence-aware overlap before applying the resource limit.
    /// A limit+1 result lets the handler report overflow instead of truncating.
    pub async fn list_events_in_range(
        &self,
        calendar_id: i64,
        start_utc: i64,
        end_utc: i64,
        limit: usize,
    ) -> Result<Vec<i64>, CaldavAuthError> {
        use crate::{
            event::{EventRepository, EventTiming},
            recurrence::{
                ExpansionLimits, ModifiedOccurrence, RecurrenceRule, RecurringEvent, TimeInterval,
                occurrence_overlaps,
            },
        };
        use chrono::{Duration, TimeZone, Utc};
        let requested = TimeInterval {
            start: Utc
                .timestamp_opt(start_utc, 0)
                .single()
                .ok_or(CaldavAuthError::Persistence)?,
            end: Utc
                .timestamp_opt(end_utc, 0)
                .single()
                .ok_or(CaldavAuthError::Persistence)?,
        };
        let ids = self.list_exposed_event_ids(calendar_id).await?;
        let mut matches = Vec::new();
        for id in ids {
            let event = EventRepository::new(self.pool.clone())
                .event(calendar_id, id)
                .await
                .map_err(|_| CaldavAuthError::Persistence)?
                .ok_or(CaldavAuthError::Persistence)?;
            let rule: Option<String> =
                sqlx::query_scalar("SELECT recurrence_rule FROM events WHERE id = ?")
                    .bind(id)
                    .fetch_one(&self.pool)
                    .await
                    .map_err(|_| CaldavAuthError::Persistence)?;
            let date = |value: &str| -> Result<chrono::DateTime<Utc>, CaldavAuthError> {
                Ok(chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                    .map_err(|_| CaldavAuthError::Persistence)?
                    .and_hms_opt(0, 0, 0)
                    .ok_or(CaldavAuthError::Persistence)?
                    .and_utc())
            };
            let (start, end, timezone, all_day) = match event.timing {
                EventTiming::Timed {
                    start_utc,
                    end_utc,
                    timezone,
                } => (
                    Utc.timestamp_opt(start_utc, 0)
                        .single()
                        .ok_or(CaldavAuthError::Persistence)?,
                    Utc.timestamp_opt(end_utc, 0)
                        .single()
                        .ok_or(CaldavAuthError::Persistence)?,
                    timezone
                        .parse::<chrono_tz::Tz>()
                        .map_err(|_| CaldavAuthError::Persistence)?,
                    false,
                ),
                EventTiming::AllDay {
                    start_date,
                    end_date,
                } => (date(&start_date)?, date(&end_date)?, chrono_tz::UTC, true),
            };
            let overlaps = if let Some(rule) = rule {
                type ExceptionRow = (
                    bool,
                    Option<i64>,
                    Option<String>,
                    Option<i64>,
                    Option<i64>,
                    Option<String>,
                    Option<String>,
                );
                let rows: Vec<ExceptionRow> = sqlx::query_as("SELECT is_deleted, recurrence_id, recurrence_date, timed_start_utc, timed_end_utc, all_day_start_date, all_day_end_date FROM event_recurrence_exceptions WHERE series_id = ?").bind(id).fetch_all(&self.pool).await.map_err(|_| CaldavAuthError::Persistence)?;
                let mut excluded = std::collections::HashSet::new();
                let mut modified = std::collections::HashMap::new();
                for (
                    deleted,
                    recurrence_id,
                    recurrence_date,
                    start_utc,
                    end_utc,
                    start_date,
                    end_date,
                ) in rows
                {
                    let recurrence_id = if all_day {
                        date(
                            recurrence_date
                                .as_deref()
                                .ok_or(CaldavAuthError::Persistence)?,
                        )?
                    } else {
                        Utc.timestamp_opt(recurrence_id.ok_or(CaldavAuthError::Persistence)?, 0)
                            .single()
                            .ok_or(CaldavAuthError::Persistence)?
                    };
                    if deleted {
                        excluded.insert(recurrence_id);
                    } else {
                        let (start, end) = if all_day {
                            (
                                date(start_date.as_deref().ok_or(CaldavAuthError::Persistence)?)?,
                                date(end_date.as_deref().ok_or(CaldavAuthError::Persistence)?)?,
                            )
                        } else {
                            (
                                Utc.timestamp_opt(
                                    start_utc.ok_or(CaldavAuthError::Persistence)?,
                                    0,
                                )
                                .single()
                                .ok_or(CaldavAuthError::Persistence)?,
                                Utc.timestamp_opt(end_utc.ok_or(CaldavAuthError::Persistence)?, 0)
                                    .single()
                                    .ok_or(CaldavAuthError::Persistence)?,
                            )
                        };
                        modified.insert(recurrence_id, ModifiedOccurrence { start, end });
                    }
                }
                occurrence_overlaps(
                    &RecurringEvent {
                        starts_at: start.with_timezone(&timezone),
                        duration: Duration::seconds((end - start).num_seconds()),
                        rule: RecurrenceRule::parse(&rule)
                            .map_err(|_| CaldavAuthError::Persistence)?,
                    },
                    requested,
                    &excluded,
                    &modified,
                    ExpansionLimits::default(),
                )
                .map_err(|error| match error {
                    crate::recurrence::RecurrenceError::ComplexityLimitExceeded
                    | crate::recurrence::RecurrenceError::OccurrenceLimitExceeded => {
                        CaldavAuthError::QueryLimit
                    }
                    _ => CaldavAuthError::Persistence,
                })?
            } else {
                start < requested.end && end > requested.start
            };
            if overlaps {
                matches.push(id);
            }
            if matches.len() > limit {
                break;
            }
        }
        Ok(matches)
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

    /// Last change per logical href in a fixed revision window. Deleted names
    /// also associate older unnamed changes with that href, including recreation.
    pub async fn coalesced_changes(
        &self,
        calendar_id: i64,
        after: i64,
        through: i64,
        limit: usize,
    ) -> Result<Vec<CaldavEventChange>, CaldavAuthError> {
        let rows: Vec<ChangeRow> = sqlx::query_as(
            "WITH named AS (
                SELECT ch.*, COALESCE(ch.resource_name,
                    (SELECT r.resource_name FROM caldav_event_resources r WHERE r.calendar_id = ch.calendar_id AND r.event_id = ch.event_id AND r.deleted_at IS NULL),
                    (SELECT d.resource_name FROM caldav_event_changes d WHERE d.calendar_id = ch.calendar_id AND d.event_id = ch.event_id AND d.resource_name IS NOT NULL ORDER BY d.id DESC LIMIT 1)) AS name
                FROM caldav_event_changes ch WHERE ch.calendar_id = ? AND ch.id > ? AND ch.id <= ?
             ), final AS (SELECT MAX(id) AS id FROM named GROUP BY COALESCE(name, 'event:' || event_id))
             SELECT n.id, n.calendar_id, n.event_id, n.change_type, n.created_at, n.name FROM named n JOIN final f ON n.id = f.id ORDER BY n.id LIMIT ?"
        ).bind(calendar_id).bind(after).bind(through).bind(limit as i64).fetch_all(&self.pool).await.map_err(|_| CaldavAuthError::Persistence)?;
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
