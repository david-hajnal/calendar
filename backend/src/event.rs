use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt::{self, Display, Formatter},
    str::FromStr,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use chrono::{Duration, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use serde::Serialize;
use sqlx::{FromRow, SqlitePool};

use crate::{
    authorization::{
        AuthorizationDecision, CalendarAction, CalendarRole, PlatformRole,
        authorize_calendar_action,
    },
    identity::UserStatus,
    recurrence::{
        ExpansionLimits, ModifiedOccurrence, RecurrenceRule, RecurringEvent, TimeInterval,
        expand_occurrences,
    },
};

const DEFAULT_MAX_RANGE_SECONDS: i64 = 366 * 24 * 60 * 60;
const DEFAULT_MAX_RANGE_DAYS: i64 = 366;

#[derive(Clone)]
pub struct EventRepository {
    pool: SqlitePool,
    max_range_seconds: i64,
    max_range_days: i64,
}

impl EventRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self::new_with_max_range(pool, DEFAULT_MAX_RANGE_SECONDS, DEFAULT_MAX_RANGE_DAYS)
    }

    pub fn new_with_max_range(
        pool: SqlitePool,
        max_range_seconds: i64,
        max_range_days: i64,
    ) -> Self {
        Self {
            pool,
            max_range_seconds,
            max_range_days,
        }
    }

    pub async fn create(&self, event: NewEvent) -> Result<Event, EventRepositoryError> {
        validate_timing(&event.timing)?;
        let timing = StoredTiming::from(&event.timing);
        let result = sqlx::query(
            "INSERT INTO events (
                calendar_id, title, description, location, status, event_kind,
                timed_start_utc, timed_end_utc, event_timezone,
                all_day_start_date, all_day_end_date, created_by_user_id,
                last_edited_by_user_id, version, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(event.calendar_id)
        .bind(&event.title)
        .bind(&event.description)
        .bind(&event.location)
        .bind(event.status.as_str())
        .bind(timing.kind)
        .bind(timing.timed_start_utc)
        .bind(timing.timed_end_utc)
        .bind(timing.timezone)
        .bind(timing.all_day_start_date)
        .bind(timing.all_day_end_date)
        .bind(event.created_by_user_id)
        .bind(event.last_edited_by_user_id)
        .bind(event.created_at)
        .bind(event.created_at)
        .execute(&self.pool)
        .await?;

        self.event(event.calendar_id, result.last_insert_rowid())
            .await?
            .ok_or(EventRepositoryError::NotFound)
    }

    pub async fn event(
        &self,
        calendar_id: i64,
        event_id: i64,
    ) -> Result<Option<Event>, EventRepositoryError> {
        let record = sqlx::query_as::<_, EventRecord>(
            "SELECT id, calendar_id, title, description, location, status, event_kind,
                    timed_start_utc, timed_end_utc, event_timezone,
                    all_day_start_date, all_day_end_date, created_by_user_id,
                    last_edited_by_user_id, version, created_at, updated_at
             FROM events
             WHERE calendar_id = ? AND id = ?",
        )
        .bind(calendar_id)
        .bind(event_id)
        .fetch_optional(&self.pool)
        .await?;

        record.map(Event::try_from).transpose()
    }

    pub async fn events_in_range(
        &self,
        calendar_id: i64,
        range: EventRange,
    ) -> Result<Vec<Event>, EventRepositoryError> {
        validate_range(&range, self.max_range_seconds, self.max_range_days)?;
        let records = sqlx::query_as::<_, EventRecord>(
            "SELECT id, calendar_id, title, description, location, status, event_kind,
                    timed_start_utc, timed_end_utc, event_timezone,
                    all_day_start_date, all_day_end_date, created_by_user_id,
                    last_edited_by_user_id, version, created_at, updated_at
             FROM events
             WHERE calendar_id = ?
               AND recurrence_rule IS NULL
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
             ORDER BY id",
        )
        .bind(calendar_id)
        .bind(range.end_utc)
        .bind(range.start_utc)
        .bind(&range.end_date)
        .bind(&range.start_date)
        .fetch_all(&self.pool)
        .await?;

        records.into_iter().map(Event::try_from).collect()
    }

    pub async fn update(
        &self,
        calendar_id: i64,
        event_id: i64,
        expected_version: i64,
        update: EventUpdate,
    ) -> Result<Event, EventRepositoryError> {
        validate_timing(&update.timing)?;
        let timing = StoredTiming::from(&update.timing);
        let result = sqlx::query(
            "UPDATE events
             SET title = ?, description = ?, location = ?, status = ?, event_kind = ?,
                 timed_start_utc = ?, timed_end_utc = ?, event_timezone = ?,
                 all_day_start_date = ?, all_day_end_date = ?,
                 last_edited_by_user_id = ?, version = version + 1, updated_at = ?
             WHERE calendar_id = ? AND id = ? AND version = ?",
        )
        .bind(&update.title)
        .bind(&update.description)
        .bind(&update.location)
        .bind(update.status.as_str())
        .bind(timing.kind)
        .bind(timing.timed_start_utc)
        .bind(timing.timed_end_utc)
        .bind(timing.timezone)
        .bind(timing.all_day_start_date)
        .bind(timing.all_day_end_date)
        .bind(update.last_edited_by_user_id)
        .bind(update.updated_at)
        .bind(calendar_id)
        .bind(event_id)
        .bind(expected_version)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() != 1 {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM events WHERE calendar_id = ? AND id = ?
                 )",
            )
            .bind(calendar_id)
            .bind(event_id)
            .fetch_one(&self.pool)
            .await?;
            return Err(if exists {
                EventRepositoryError::StaleVersion
            } else {
                EventRepositoryError::NotFound
            });
        }

        self.event(calendar_id, event_id)
            .await?
            .ok_or(EventRepositoryError::NotFound)
    }
}

const DEFAULT_REPLANNER_CHANNEL_CAPACITY: usize = 1024;
const DEFAULT_REPLANNER_MAX_CONCURRENT: usize = 8;

/// Bounded notification replanner that drains a channel with limited concurrency.
#[derive(Clone)]
pub struct NotificationReplanner {
    sender: tokio::sync::mpsc::Sender<i64>,
}

impl NotificationReplanner {
    /// Create a new replanner with a bounded channel and spawn the worker loop.
    pub fn new(on_notify: Arc<dyn Fn(i64) + Send + Sync>) -> Self {
        let (sender, receiver) = tokio::sync::mpsc::channel(DEFAULT_REPLANNER_CHANNEL_CAPACITY);
        let semaphore = Arc::new(tokio::sync::Semaphore::new(
            DEFAULT_REPLANNER_MAX_CONCURRENT,
        ));
        let semaphore_clone = semaphore.clone();
        tokio::spawn(async move {
            let mut rx = receiver;
            while let Some(event_id) = rx.recv().await {
                let semaphore = semaphore_clone.clone();
                let on_notify = on_notify.clone();
                tokio::spawn(async move {
                    let _permit = semaphore.acquire().await.unwrap();
                    (on_notify)(event_id);
                });
            }
        });
        Self { sender }
    }

    pub async fn send(&self, event_id: i64) {
        let _ = self.sender.send(event_id).await;
    }

    /// Create a no-op replanner that discards all notifications.
    pub fn noop() -> Self {
        let (sender, _receiver) = tokio::sync::mpsc::channel(DEFAULT_REPLANNER_CHANNEL_CAPACITY);
        drop(_receiver);
        Self { sender }
    }
}

#[derive(Clone)]
pub struct EventService {
    pool: SqlitePool,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    notification_replanner: Arc<NotificationReplanner>,
}

impl EventService {
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            clock: Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system clock is before Unix epoch")
                    .as_secs() as i64
            }),
            notification_replanner: Arc::new(NotificationReplanner::noop()),
        }
    }

    pub fn new_at(pool: SqlitePool, now: i64) -> Self {
        Self {
            pool,
            clock: Arc::new(move || now),
            notification_replanner: Arc::new(NotificationReplanner::noop()),
        }
    }

    pub fn new_with_notification_replanner(
        pool: SqlitePool,
        on_notify: Arc<dyn Fn(i64) + Send + Sync>,
    ) -> Self {
        let replanner = NotificationReplanner::new(on_notify);
        Self {
            pool,
            clock: Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system clock is before Unix epoch")
                    .as_secs() as i64
            }),
            notification_replanner: Arc::new(replanner),
        }
    }

    pub fn new_at_with_notification_replanner(
        pool: SqlitePool,
        now: i64,
        on_notify: Arc<dyn Fn(i64) + Send + Sync>,
    ) -> Self {
        let replanner = NotificationReplanner::new(on_notify);
        Self {
            pool,
            clock: Arc::new(move || now),
            notification_replanner: Arc::new(replanner),
        }
    }

    pub fn validate_requested_range(&self, range: &EventRange) -> Result<(), EventServiceError> {
        validate_range(range, DEFAULT_MAX_RANGE_SECONDS, DEFAULT_MAX_RANGE_DAYS)
            .map_err(EventServiceError::from)
    }

    pub async fn create(
        &self,
        actor_user_id: i64,
        is_superadmin: bool,
        calendar_id: i64,
        event: EventMutation,
    ) -> Result<EventProjection, EventServiceError> {
        validate_mutation(&event)?;
        let now = (self.clock)();
        let timing = StoredTiming::from(&event.timing);
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        authorize_in_transaction(
            &mut transaction,
            actor_user_id,
            is_superadmin,
            calendar_id,
            CalendarAction::CreateEvent,
        )
        .await?;
        let result = sqlx::query(
            "INSERT INTO events (
                calendar_id, title, description, location, status, event_kind,
                timed_start_utc, timed_end_utc, event_timezone,
                all_day_start_date, all_day_end_date, created_by_user_id,
                last_edited_by_user_id, version, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(calendar_id)
        .bind(&event.title)
        .bind(&event.description)
        .bind(&event.location)
        .bind(event.status.as_str())
        .bind(timing.kind)
        .bind(timing.timed_start_utc)
        .bind(timing.timed_end_utc)
        .bind(timing.timezone)
        .bind(timing.all_day_start_date)
        .bind(timing.all_day_end_date)
        .bind(actor_user_id)
        .bind(actor_user_id)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        let event_id = result.last_insert_rowid();
        insert_event_audit(
            &mut transaction,
            actor_user_id,
            "event.create",
            event_id,
            now,
        )
        .await?;
        transaction.commit().await?;
        self.notification_replanner.send(event_id).await;
        self.get(actor_user_id, is_superadmin, calendar_id, event_id)
            .await
    }

    pub async fn create_recurring(
        &self,
        actor_user_id: i64,
        is_superadmin: bool,
        calendar_id: i64,
        event: EventMutation,
        recurrence_rule: String,
    ) -> Result<EventProjection, EventServiceError> {
        validate_mutation(&event)?;
        validate_recurrence(&event, &recurrence_rule)?;
        let now = (self.clock)();
        let timing = StoredTiming::from(&event.timing);
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        authorize_in_transaction(
            &mut transaction,
            actor_user_id,
            is_superadmin,
            calendar_id,
            CalendarAction::CreateEvent,
        )
        .await?;
        let result = sqlx::query(
            "INSERT INTO events (
                calendar_id, title, description, location, status, event_kind,
                timed_start_utc, timed_end_utc, event_timezone,
                all_day_start_date, all_day_end_date, created_by_user_id,
                last_edited_by_user_id, version, created_at, updated_at, recurrence_rule
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?)",
        )
        .bind(calendar_id)
        .bind(&event.title)
        .bind(&event.description)
        .bind(&event.location)
        .bind(event.status.as_str())
        .bind(timing.kind)
        .bind(timing.timed_start_utc)
        .bind(timing.timed_end_utc)
        .bind(timing.timezone)
        .bind(timing.all_day_start_date)
        .bind(timing.all_day_end_date)
        .bind(actor_user_id)
        .bind(actor_user_id)
        .bind(now)
        .bind(now)
        .bind(&recurrence_rule)
        .execute(&mut *transaction)
        .await?;
        let event_id = result.last_insert_rowid();
        insert_event_audit(
            &mut transaction,
            actor_user_id,
            "event.series.create",
            event_id,
            now,
        )
        .await?;
        transaction.commit().await?;
        self.notification_replanner.send(event_id).await;
        let mut projection = self
            .get(actor_user_id, is_superadmin, calendar_id, event_id)
            .await?;
        projection.recurrence_rule = Some(recurrence_rule);
        Ok(projection)
    }

    pub async fn get(
        &self,
        actor_user_id: i64,
        is_superadmin: bool,
        calendar_id: i64,
        event_id: i64,
    ) -> Result<EventProjection, EventServiceError> {
        let access = self
            .read_access(actor_user_id, is_superadmin, calendar_id)
            .await?;
        let event = EventRepository::new(self.pool.clone())
            .event(calendar_id, event_id)
            .await?
            .ok_or(EventServiceError::NotFound)?;
        let mut projection = project_event(event, access);
        if matches!(access, EventAccess::Details) {
            projection.recurrence_rule =
                sqlx::query_scalar("SELECT recurrence_rule FROM events WHERE id = ?")
                    .bind(event_id)
                    .fetch_optional(&self.pool)
                    .await?
                    .flatten();
        }
        mark_external_projections(&self.pool, std::slice::from_mut(&mut projection)).await?;
        Ok(projection)
    }

    pub async fn list(
        &self,
        actor_user_id: i64,
        is_superadmin: bool,
        calendar_id: i64,
        range: EventRange,
    ) -> Result<Vec<EventProjection>, EventServiceError> {
        let access = self
            .read_access(actor_user_id, is_superadmin, calendar_id)
            .await?;
        let mut projected: Vec<_> = EventRepository::new(self.pool.clone())
            .events_in_range(calendar_id, range.clone())
            .await?
            .into_iter()
            .map(|event| Ok(project_event(event, access)))
            .collect::<Result<_, EventServiceError>>()?;
        let mut recurring = self
            .expanded_recurring_events(calendar_id, &range, access)
            .await?;
        projected.append(&mut recurring);
        let mut all_day_recurring = self
            .expanded_all_day_events(calendar_id, &range, access)
            .await?;
        projected.append(&mut all_day_recurring);
        mark_external_projections(&self.pool, &mut projected).await?;
        projected.sort_by_key(|event| (event.start_utc.unwrap_or(i64::MIN), event.id));
        Ok(projected)
    }

    async fn expanded_recurring_events(
        &self,
        calendar_id: i64,
        range: &EventRange,
        access: EventAccess,
    ) -> Result<Vec<EventProjection>, EventServiceError> {
        let series = sqlx::query_as::<_, RecurringSeriesRecord>(
            "SELECT id, calendar_id, title, description, location, status,
                    timed_start_utc, timed_end_utc, event_timezone,
                    created_by_user_id, last_edited_by_user_id, version,
                    created_at, updated_at, recurrence_rule
             FROM events
             WHERE calendar_id = ? AND recurrence_rule IS NOT NULL AND event_kind = 'timed'",
        )
        .bind(calendar_id)
        .fetch_all(&self.pool)
        .await?;
        let mut expanded = Vec::new();
        for series in series {
            let exceptions = sqlx::query_as::<_, RecurrenceExceptionRecord>(
                "SELECT recurrence_id, is_deleted, title, description, location, status,
                        timed_start_utc, timed_end_utc, event_timezone
                 FROM event_recurrence_exceptions WHERE series_id = ?",
            )
            .bind(series.id)
            .fetch_all(&self.pool)
            .await?;
            let excluded: HashSet<_> = exceptions
                .iter()
                .filter(|exception| exception.is_deleted)
                .filter_map(|exception| Utc.timestamp_opt(exception.recurrence_id, 0).single())
                .collect();
            let modified: HashMap<_, _> = exceptions
                .iter()
                .filter(|exception| !exception.is_deleted)
                .filter_map(|exception| {
                    Some((
                        Utc.timestamp_opt(exception.recurrence_id, 0).single()?,
                        ModifiedOccurrence {
                            start: Utc.timestamp_opt(exception.timed_start_utc?, 0).single()?,
                            end: Utc.timestamp_opt(exception.timed_end_utc?, 0).single()?,
                        },
                    ))
                })
                .collect();
            let timezone: Tz = series
                .event_timezone
                .parse()
                .map_err(|_| EventServiceError::InvalidInput)?;
            let starts_at = timezone.from_utc_datetime(
                &Utc.timestamp_opt(series.timed_start_utc, 0)
                    .single()
                    .ok_or(EventServiceError::InvalidInput)?
                    .naive_utc(),
            );
            let occurrences = expand_occurrences(
                &RecurringEvent {
                    starts_at,
                    duration: Duration::seconds(series.timed_end_utc - series.timed_start_utc),
                    rule: RecurrenceRule::parse(&series.recurrence_rule)
                        .map_err(|_| EventServiceError::InvalidInput)?,
                },
                TimeInterval {
                    start: Utc
                        .timestamp_opt(range.start_utc, 0)
                        .single()
                        .ok_or(EventServiceError::InvalidInput)?,
                    end: Utc
                        .timestamp_opt(range.end_utc, 0)
                        .single()
                        .ok_or(EventServiceError::InvalidInput)?,
                },
                &excluded,
                &modified,
                ExpansionLimits::default(),
            )
            .map_err(|_| EventServiceError::InvalidInput)?;
            for occurrence in occurrences {
                let exception = exceptions.iter().find(|exception| {
                    exception.recurrence_id == occurrence.recurrence_id.timestamp()
                        && !exception.is_deleted
                });
                expanded.push(project_occurrence(&series, exception, occurrence, access)?);
            }
        }
        Ok(expanded)
    }

    async fn expanded_all_day_events(
        &self,
        calendar_id: i64,
        range: &EventRange,
        access: EventAccess,
    ) -> Result<Vec<EventProjection>, EventServiceError> {
        let series = sqlx::query_as::<_, AllDaySeriesRecord>(
            "SELECT id, calendar_id, title, description, location, status,
                    all_day_start_date, all_day_end_date,
                    created_by_user_id, last_edited_by_user_id, version,
                    created_at, updated_at, recurrence_rule
             FROM events
             WHERE calendar_id = ? AND recurrence_rule IS NOT NULL
               AND event_kind = 'all_day'",
        )
        .bind(calendar_id)
        .fetch_all(&self.pool)
        .await?;
        let requested = TimeInterval {
            start: date_at_utc(&range.start_date)?,
            end: date_at_utc(&range.end_date)?,
        };
        let mut expanded = Vec::new();
        for series in series {
            let exceptions = sqlx::query_as::<_, AllDayExceptionRecord>(
                "SELECT recurrence_date, is_deleted, title, description, location, status,
                        all_day_start_date, all_day_end_date
                 FROM event_recurrence_exceptions
                 WHERE series_id = ? AND recurrence_date IS NOT NULL",
            )
            .bind(series.id)
            .fetch_all(&self.pool)
            .await?;
            let excluded: HashSet<_> = exceptions
                .iter()
                .filter(|exception| exception.is_deleted)
                .map(|exception| date_at_utc(&exception.recurrence_date))
                .collect::<Result<_, _>>()?;
            let modified: HashMap<_, _> = exceptions
                .iter()
                .filter(|exception| !exception.is_deleted)
                .map(|exception| {
                    Ok((
                        date_at_utc(&exception.recurrence_date)?,
                        ModifiedOccurrence {
                            start: date_at_utc(
                                exception
                                    .all_day_start_date
                                    .as_deref()
                                    .ok_or(EventServiceError::InvalidInput)?,
                            )?,
                            end: date_at_utc(
                                exception
                                    .all_day_end_date
                                    .as_deref()
                                    .ok_or(EventServiceError::InvalidInput)?,
                            )?,
                        },
                    ))
                })
                .collect::<Result<_, EventServiceError>>()?;
            let starts_at_utc = date_at_utc(&series.all_day_start_date)?;
            let ends_at_utc = date_at_utc(&series.all_day_end_date)?;
            let starts_at = chrono_tz::UTC.from_utc_datetime(&starts_at_utc.naive_utc());
            let occurrences = expand_occurrences(
                &RecurringEvent {
                    starts_at,
                    duration: ends_at_utc - starts_at_utc,
                    rule: RecurrenceRule::parse(&series.recurrence_rule)
                        .map_err(|_| EventServiceError::InvalidInput)?,
                },
                requested,
                &excluded,
                &modified,
                ExpansionLimits::default(),
            )
            .map_err(|_| EventServiceError::InvalidInput)?;
            for occurrence in occurrences {
                let recurrence_date = occurrence
                    .recurrence_id
                    .date_naive()
                    .format("%Y-%m-%d")
                    .to_string();
                let exception = exceptions.iter().find(|exception| {
                    exception.recurrence_date == recurrence_date && !exception.is_deleted
                });
                expanded.push(project_all_day_occurrence(
                    &series, exception, occurrence, access,
                )?);
            }
        }
        Ok(expanded)
    }

    pub async fn update_all_day_occurrence(
        &self,
        actor_user_id: i64,
        is_superadmin: bool,
        calendar_id: i64,
        series_id: i64,
        change: AllDayOccurrenceChange,
    ) -> Result<EventProjection, EventServiceError> {
        let AllDayOccurrenceChange {
            recurrence_date,
            expected_version,
            event,
        } = change;
        validate_mutation(&event)?;
        let EventTiming::AllDay {
            start_date,
            end_date,
        } = &event.timing
        else {
            return Err(EventServiceError::InvalidInput);
        };
        let now = (self.clock)();
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        authorize_in_transaction(
            &mut transaction,
            actor_user_id,
            is_superadmin,
            calendar_id,
            CalendarAction::EditAnyEvent,
        )
        .await?;
        ensure_not_imported(&mut transaction, series_id).await?;
        ensure_all_day_series_occurrence(
            &mut transaction,
            calendar_id,
            series_id,
            &recurrence_date,
        )
        .await?;
        bump_series_version(
            &mut transaction,
            calendar_id,
            series_id,
            expected_version,
            actor_user_id,
            now,
        )
        .await?;
        sqlx::query(
            "INSERT INTO event_recurrence_exceptions (
                series_id, recurrence_id, recurrence_date, is_deleted,
                title, description, location, status,
                timed_start_utc, timed_end_utc, event_timezone,
                all_day_start_date, all_day_end_date,
                last_edited_by_user_id, created_at, updated_at
             ) VALUES (?, NULL, ?, 0, ?, ?, ?, ?, NULL, NULL, NULL, ?, ?, ?, ?, ?)
             ON CONFLICT(series_id, recurrence_date) DO UPDATE SET
                is_deleted = 0, title = excluded.title,
                description = excluded.description, location = excluded.location,
                status = excluded.status, timed_start_utc = NULL,
                timed_end_utc = NULL, event_timezone = NULL,
                all_day_start_date = excluded.all_day_start_date,
                all_day_end_date = excluded.all_day_end_date,
                last_edited_by_user_id = excluded.last_edited_by_user_id,
                updated_at = excluded.updated_at",
        )
        .bind(series_id)
        .bind(&recurrence_date)
        .bind(&event.title)
        .bind(&event.description)
        .bind(&event.location)
        .bind(event.status.as_str())
        .bind(start_date)
        .bind(end_date)
        .bind(actor_user_id)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        insert_event_audit(
            &mut transaction,
            actor_user_id,
            "event.occurrence.update",
            series_id,
            now,
        )
        .await?;
        transaction.commit().await?;
        self.notification_replanner.send(series_id).await;
        self.get(actor_user_id, is_superadmin, calendar_id, series_id)
            .await
    }

    pub async fn delete_all_day_occurrence(
        &self,
        actor_user_id: i64,
        is_superadmin: bool,
        calendar_id: i64,
        series_id: i64,
        recurrence_date: &str,
        expected_version: i64,
    ) -> Result<EventProjection, EventServiceError> {
        let now = (self.clock)();
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        authorize_in_transaction(
            &mut transaction,
            actor_user_id,
            is_superadmin,
            calendar_id,
            CalendarAction::EditAnyEvent,
        )
        .await?;
        ensure_not_imported(&mut transaction, series_id).await?;
        ensure_all_day_series_occurrence(&mut transaction, calendar_id, series_id, recurrence_date)
            .await?;
        bump_series_version(
            &mut transaction,
            calendar_id,
            series_id,
            expected_version,
            actor_user_id,
            now,
        )
        .await?;
        sqlx::query(
            "INSERT INTO event_recurrence_exceptions (
                series_id, recurrence_id, recurrence_date, is_deleted,
                title, description, location, status,
                timed_start_utc, timed_end_utc, event_timezone,
                all_day_start_date, all_day_end_date,
                last_edited_by_user_id, created_at, updated_at
             ) VALUES (?, NULL, ?, 1, NULL, NULL, NULL, NULL,
                       NULL, NULL, NULL, NULL, NULL, ?, ?, ?)
             ON CONFLICT(series_id, recurrence_date) DO UPDATE SET
                is_deleted = 1, title = NULL, description = NULL, location = NULL,
                status = NULL, timed_start_utc = NULL, timed_end_utc = NULL,
                event_timezone = NULL, all_day_start_date = NULL,
                all_day_end_date = NULL,
                last_edited_by_user_id = excluded.last_edited_by_user_id,
                updated_at = excluded.updated_at",
        )
        .bind(series_id)
        .bind(recurrence_date)
        .bind(actor_user_id)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        insert_event_audit(
            &mut transaction,
            actor_user_id,
            "event.occurrence.delete",
            series_id,
            now,
        )
        .await?;
        transaction.commit().await?;
        self.notification_replanner.send(series_id).await;
        self.get(actor_user_id, is_superadmin, calendar_id, series_id)
            .await
    }

    pub async fn update_occurrence(
        &self,
        actor_user_id: i64,
        is_superadmin: bool,
        calendar_id: i64,
        series_id: i64,
        change: OccurrenceChange,
    ) -> Result<EventProjection, EventServiceError> {
        let OccurrenceChange {
            recurrence_id,
            expected_version,
            event,
        } = change;
        validate_mutation(&event)?;
        let EventTiming::Timed {
            start_utc,
            end_utc,
            timezone,
        } = &event.timing
        else {
            return Err(EventServiceError::InvalidInput);
        };
        timezone
            .parse::<Tz>()
            .map_err(|_| EventServiceError::InvalidInput)?;
        let now = (self.clock)();
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        authorize_in_transaction(
            &mut transaction,
            actor_user_id,
            is_superadmin,
            calendar_id,
            CalendarAction::EditAnyEvent,
        )
        .await?;
        ensure_not_imported(&mut transaction, series_id).await?;
        ensure_series_occurrence(&mut transaction, calendar_id, series_id, recurrence_id).await?;
        bump_series_version(
            &mut transaction,
            calendar_id,
            series_id,
            expected_version,
            actor_user_id,
            now,
        )
        .await?;
        sqlx::query(
            "INSERT INTO event_recurrence_exceptions (
                series_id, recurrence_id, is_deleted, title, description, location,
                status, timed_start_utc, timed_end_utc, event_timezone,
                last_edited_by_user_id, created_at, updated_at
             ) VALUES (?, ?, 0, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(series_id, recurrence_id) DO UPDATE SET
                is_deleted = 0, title = excluded.title,
                description = excluded.description, location = excluded.location,
                status = excluded.status, timed_start_utc = excluded.timed_start_utc,
                timed_end_utc = excluded.timed_end_utc,
                event_timezone = excluded.event_timezone,
                last_edited_by_user_id = excluded.last_edited_by_user_id,
                updated_at = excluded.updated_at",
        )
        .bind(series_id)
        .bind(recurrence_id)
        .bind(&event.title)
        .bind(&event.description)
        .bind(&event.location)
        .bind(event.status.as_str())
        .bind(start_utc)
        .bind(end_utc)
        .bind(timezone)
        .bind(actor_user_id)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        insert_event_audit(
            &mut transaction,
            actor_user_id,
            "event.occurrence.update",
            series_id,
            now,
        )
        .await?;
        transaction.commit().await?;
        self.notification_replanner.send(series_id).await;
        self.get(actor_user_id, is_superadmin, calendar_id, series_id)
            .await
    }

    pub async fn delete_occurrence(
        &self,
        actor_user_id: i64,
        is_superadmin: bool,
        calendar_id: i64,
        series_id: i64,
        recurrence_id: i64,
        expected_version: i64,
    ) -> Result<EventProjection, EventServiceError> {
        let now = (self.clock)();
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        authorize_in_transaction(
            &mut transaction,
            actor_user_id,
            is_superadmin,
            calendar_id,
            CalendarAction::EditAnyEvent,
        )
        .await?;
        ensure_not_imported(&mut transaction, series_id).await?;
        ensure_series_occurrence(&mut transaction, calendar_id, series_id, recurrence_id).await?;
        bump_series_version(
            &mut transaction,
            calendar_id,
            series_id,
            expected_version,
            actor_user_id,
            now,
        )
        .await?;
        sqlx::query(
            "INSERT INTO event_recurrence_exceptions (
                series_id, recurrence_id, is_deleted, title, description, location,
                status, timed_start_utc, timed_end_utc, event_timezone,
                last_edited_by_user_id, created_at, updated_at
             ) VALUES (?, ?, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, ?, ?, ?)
             ON CONFLICT(series_id, recurrence_id) DO UPDATE SET
                is_deleted = 1, title = NULL, description = NULL, location = NULL,
                status = NULL, timed_start_utc = NULL, timed_end_utc = NULL,
                event_timezone = NULL,
                last_edited_by_user_id = excluded.last_edited_by_user_id,
                updated_at = excluded.updated_at",
        )
        .bind(series_id)
        .bind(recurrence_id)
        .bind(actor_user_id)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        insert_event_audit(
            &mut transaction,
            actor_user_id,
            "event.occurrence.delete",
            series_id,
            now,
        )
        .await?;
        transaction.commit().await?;
        self.notification_replanner.send(series_id).await;
        self.get(actor_user_id, is_superadmin, calendar_id, series_id)
            .await
    }

    pub async fn update_this_and_following(
        &self,
        actor_user_id: i64,
        is_superadmin: bool,
        calendar_id: i64,
        series_id: i64,
        recurrence_id: i64,
    ) -> Result<(), EventServiceError> {
        let mut transaction = self.pool.begin().await?;
        authorize_in_transaction(
            &mut transaction,
            actor_user_id,
            is_superadmin,
            calendar_id,
            CalendarAction::EditAnyEvent,
        )
        .await?;
        ensure_not_imported(&mut transaction, series_id).await?;
        ensure_series_occurrence(&mut transaction, calendar_id, series_id, recurrence_id).await?;
        transaction.rollback().await?;
        Err(EventServiceError::NotSupported)
    }

    pub async fn update_all_day_this_and_following(
        &self,
        actor_user_id: i64,
        is_superadmin: bool,
        calendar_id: i64,
        series_id: i64,
        recurrence_date: &str,
    ) -> Result<(), EventServiceError> {
        let mut transaction = self.pool.begin().await?;
        authorize_in_transaction(
            &mut transaction,
            actor_user_id,
            is_superadmin,
            calendar_id,
            CalendarAction::EditAnyEvent,
        )
        .await?;
        ensure_not_imported(&mut transaction, series_id).await?;
        ensure_all_day_series_occurrence(&mut transaction, calendar_id, series_id, recurrence_date)
            .await?;
        transaction.rollback().await?;
        Err(EventServiceError::NotSupported)
    }

    pub async fn update(
        &self,
        actor_user_id: i64,
        is_superadmin: bool,
        source_calendar_id: i64,
        event_id: i64,
        change: EventChange,
    ) -> Result<EventProjection, EventServiceError> {
        let EventChange {
            expected_version,
            target_calendar_id,
            event,
        } = change;
        validate_mutation(&event)?;
        let now = (self.clock)();
        let timing = StoredTiming::from(&event.timing);
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        authorize_in_transaction(
            &mut transaction,
            actor_user_id,
            is_superadmin,
            source_calendar_id,
            CalendarAction::EditAnyEvent,
        )
        .await?;
        if target_calendar_id != source_calendar_id {
            authorize_in_transaction(
                &mut transaction,
                actor_user_id,
                is_superadmin,
                target_calendar_id,
                CalendarAction::EditAnyEvent,
            )
            .await?;
        }
        ensure_not_imported(&mut transaction, event_id).await?;
        let is_recurring: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM events
                WHERE calendar_id = ? AND id = ? AND recurrence_rule IS NOT NULL
             )",
        )
        .bind(source_calendar_id)
        .bind(event_id)
        .fetch_one(&mut *transaction)
        .await?;
        let result = sqlx::query(
            "UPDATE events
             SET calendar_id = ?, title = ?, description = ?, location = ?, status = ?,
                 event_kind = ?, timed_start_utc = ?, timed_end_utc = ?,
                 event_timezone = ?, all_day_start_date = ?, all_day_end_date = ?,
                 last_edited_by_user_id = ?, version = version + 1, updated_at = ?
             WHERE calendar_id = ? AND id = ? AND version = ?",
        )
        .bind(target_calendar_id)
        .bind(&event.title)
        .bind(&event.description)
        .bind(&event.location)
        .bind(event.status.as_str())
        .bind(timing.kind)
        .bind(timing.timed_start_utc)
        .bind(timing.timed_end_utc)
        .bind(timing.timezone)
        .bind(timing.all_day_start_date)
        .bind(timing.all_day_end_date)
        .bind(actor_user_id)
        .bind(now)
        .bind(source_calendar_id)
        .bind(event_id)
        .bind(expected_version)
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() != 1 {
            let current_version: Option<i64> =
                sqlx::query_scalar("SELECT version FROM events WHERE calendar_id = ? AND id = ?")
                    .bind(source_calendar_id)
                    .bind(event_id)
                    .fetch_optional(&mut *transaction)
                    .await?;
            transaction.rollback().await?;
            return match current_version {
                Some(current_version) => Err(EventServiceError::Conflict { current_version }),
                None => Err(EventServiceError::NotFound),
            };
        }
        if is_recurring {
            sqlx::query("DELETE FROM event_recurrence_exceptions WHERE series_id = ?")
                .bind(event_id)
                .execute(&mut *transaction)
                .await?;
        }
        insert_event_audit(
            &mut transaction,
            actor_user_id,
            "event.update",
            event_id,
            now,
        )
        .await?;
        transaction.commit().await?;
        self.notification_replanner.send(event_id).await;
        self.get(actor_user_id, is_superadmin, target_calendar_id, event_id)
            .await
    }

    pub async fn delete(
        &self,
        actor_user_id: i64,
        is_superadmin: bool,
        calendar_id: i64,
        event_id: i64,
    ) -> Result<(), EventServiceError> {
        let now = (self.clock)();
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        authorize_in_transaction(
            &mut transaction,
            actor_user_id,
            is_superadmin,
            calendar_id,
            CalendarAction::EditAnyEvent,
        )
        .await?;
        ensure_not_imported(&mut transaction, event_id).await?;
        sqlx::query(
            "UPDATE notification_jobs SET state = 'cancelled', updated_at = ?
             WHERE event_id = ? AND state = 'pending'",
        )
        .bind(now)
        .bind(event_id)
        .execute(&mut *transaction)
        .await?;
        let deleted = sqlx::query("DELETE FROM events WHERE calendar_id = ? AND id = ?")
            .bind(calendar_id)
            .bind(event_id)
            .execute(&mut *transaction)
            .await?;
        if deleted.rows_affected() != 1 {
            transaction.rollback().await?;
            return Err(EventServiceError::NotFound);
        }
        insert_event_audit(
            &mut transaction,
            actor_user_id,
            "event.delete",
            event_id,
            now,
        )
        .await?;
        transaction.commit().await?;
        self.notification_replanner.send(event_id).await;
        Ok(())
    }

    async fn read_access(
        &self,
        actor_user_id: i64,
        is_superadmin: bool,
        calendar_id: i64,
    ) -> Result<EventAccess, EventServiceError> {
        let role: Option<String> = sqlx::query_scalar(
            "SELECT role FROM calendar_acl WHERE calendar_id = ? AND user_id = ?",
        )
        .bind(calendar_id)
        .bind(actor_user_id)
        .fetch_optional(&self.pool)
        .await?;
        let role = role
            .as_deref()
            .and_then(|role| CalendarRole::from_str(role).ok())
            .ok_or(EventServiceError::NotFound)?;
        let platform_role = if is_superadmin {
            PlatformRole::Superadmin
        } else {
            PlatformRole::User
        };
        if authorize_calendar_action(
            UserStatus::Active,
            Some(platform_role),
            Some(role),
            CalendarAction::ReadDetails,
        ) == AuthorizationDecision::Allow
        {
            Ok(EventAccess::Details)
        } else if authorize_calendar_action(
            UserStatus::Active,
            Some(platform_role),
            Some(role),
            CalendarAction::ReadFreeBusy,
        ) == AuthorizationDecision::Allow
        {
            Ok(EventAccess::FreeBusy)
        } else {
            Err(EventServiceError::NotFound)
        }
    }
}

async fn ensure_not_imported(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    event_id: i64,
) -> Result<(), EventServiceError> {
    let imported: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM external_event_mapping WHERE event_id = ?)",
    )
    .bind(event_id)
    .fetch_one(&mut **transaction)
    .await?;
    if imported {
        Err(EventServiceError::ReadOnly)
    } else {
        Ok(())
    }
}

async fn authorize_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    actor_user_id: i64,
    is_superadmin: bool,
    calendar_id: i64,
    action: CalendarAction,
) -> Result<(), EventServiceError> {
    let role: Option<String> =
        sqlx::query_scalar("SELECT role FROM calendar_acl WHERE calendar_id = ? AND user_id = ?")
            .bind(calendar_id)
            .bind(actor_user_id)
            .fetch_optional(&mut **transaction)
            .await?;
    let role = role
        .as_deref()
        .and_then(|role| CalendarRole::from_str(role).ok())
        .ok_or(EventServiceError::NotFound)?;
    let platform_role = if is_superadmin {
        PlatformRole::Superadmin
    } else {
        PlatformRole::User
    };
    if authorize_calendar_action(UserStatus::Active, Some(platform_role), Some(role), action)
        == AuthorizationDecision::Allow
    {
        Ok(())
    } else {
        Err(EventServiceError::NotFound)
    }
}

async fn ensure_series_occurrence(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    calendar_id: i64,
    series_id: i64,
    recurrence_id: i64,
) -> Result<(), EventServiceError> {
    let row: Option<(i64, i64, String, String)> = sqlx::query_as(
        "SELECT timed_start_utc, timed_end_utc, event_timezone, recurrence_rule
         FROM events
         WHERE calendar_id = ? AND id = ? AND event_kind = 'timed'
           AND recurrence_rule IS NOT NULL",
    )
    .bind(calendar_id)
    .bind(series_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let (start_utc, end_utc, timezone, recurrence_rule) = row.ok_or(EventServiceError::NotFound)?;
    let timezone: Tz = timezone
        .parse()
        .map_err(|_| EventServiceError::InvalidInput)?;
    let starts_at = timezone.from_utc_datetime(
        &Utc.timestamp_opt(start_utc, 0)
            .single()
            .ok_or(EventServiceError::InvalidInput)?
            .naive_utc(),
    );
    let requested_start = Utc
        .timestamp_opt(recurrence_id, 0)
        .single()
        .ok_or(EventServiceError::InvalidInput)?;
    let occurrences = expand_occurrences(
        &RecurringEvent {
            starts_at,
            duration: Duration::seconds(end_utc - start_utc),
            rule: RecurrenceRule::parse(&recurrence_rule)
                .map_err(|_| EventServiceError::InvalidInput)?,
        },
        TimeInterval {
            start: requested_start,
            end: requested_start + Duration::seconds(1),
        },
        &HashSet::new(),
        &HashMap::new(),
        ExpansionLimits::default(),
    )
    .map_err(|_| EventServiceError::InvalidInput)?;
    if occurrences
        .iter()
        .any(|occurrence| occurrence.recurrence_id.timestamp() == recurrence_id)
    {
        Ok(())
    } else {
        Err(EventServiceError::NotFound)
    }
}

async fn ensure_all_day_series_occurrence(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    calendar_id: i64,
    series_id: i64,
    recurrence_date: &str,
) -> Result<(), EventServiceError> {
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT all_day_start_date, all_day_end_date, recurrence_rule
         FROM events
         WHERE calendar_id = ? AND id = ? AND event_kind = 'all_day'
           AND recurrence_rule IS NOT NULL",
    )
    .bind(calendar_id)
    .bind(series_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let (start_date, end_date, recurrence_rule) = row.ok_or(EventServiceError::NotFound)?;
    let starts_at_utc = date_at_utc(&start_date)?;
    let ends_at_utc = date_at_utc(&end_date)?;
    let requested_start = date_at_utc(recurrence_date)?;
    let occurrences = expand_occurrences(
        &RecurringEvent {
            starts_at: chrono_tz::UTC.from_utc_datetime(&starts_at_utc.naive_utc()),
            duration: ends_at_utc - starts_at_utc,
            rule: RecurrenceRule::parse(&recurrence_rule)
                .map_err(|_| EventServiceError::InvalidInput)?,
        },
        TimeInterval {
            start: requested_start,
            end: requested_start + Duration::days(1),
        },
        &HashSet::new(),
        &HashMap::new(),
        ExpansionLimits::default(),
    )
    .map_err(|_| EventServiceError::InvalidInput)?;
    if occurrences
        .iter()
        .any(|occurrence| occurrence.recurrence_id.date_naive().to_string() == recurrence_date)
    {
        Ok(())
    } else {
        Err(EventServiceError::NotFound)
    }
}

async fn bump_series_version(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    calendar_id: i64,
    series_id: i64,
    expected_version: i64,
    actor_user_id: i64,
    now: i64,
) -> Result<(), EventServiceError> {
    let updated = sqlx::query(
        "UPDATE events
         SET version = version + 1, last_edited_by_user_id = ?, updated_at = ?
         WHERE calendar_id = ? AND id = ? AND recurrence_rule IS NOT NULL AND version = ?",
    )
    .bind(actor_user_id)
    .bind(now)
    .bind(calendar_id)
    .bind(series_id)
    .bind(expected_version)
    .execute(&mut **transaction)
    .await?;
    if updated.rows_affected() == 1 {
        return Ok(());
    }
    let current_version: Option<i64> = sqlx::query_scalar(
        "SELECT version FROM events
         WHERE calendar_id = ? AND id = ? AND recurrence_rule IS NOT NULL",
    )
    .bind(calendar_id)
    .bind(series_id)
    .fetch_optional(&mut **transaction)
    .await?;
    match current_version {
        Some(current_version) => Err(EventServiceError::Conflict { current_version }),
        None => Err(EventServiceError::NotFound),
    }
}

async fn insert_event_audit(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    actor_user_id: i64,
    action: &'static str,
    event_id: i64,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO audit_log (
            actor_user_id, action, target_type, target_id, metadata_json, created_at
         ) VALUES (?, ?, 'event', ?, NULL, ?)",
    )
    .bind(actor_user_id)
    .bind(action)
    .bind(event_id.to_string())
    .bind(now)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn validate_mutation(event: &EventMutation) -> Result<(), EventServiceError> {
    if event.title.trim().is_empty() {
        return Err(EventServiceError::InvalidInput);
    }
    validate_timing(&event.timing).map_err(|_| EventServiceError::InvalidInput)
}

#[derive(Clone, Debug)]
pub struct EventMutation {
    pub title: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub status: EventStatus,
    pub timing: EventTiming,
}

#[derive(Clone, Debug)]
pub struct EventChange {
    pub expected_version: i64,
    pub target_calendar_id: i64,
    pub event: EventMutation,
}

#[derive(Clone, Debug)]
pub struct OccurrenceChange {
    pub recurrence_id: i64,
    pub expected_version: i64,
    pub event: EventMutation,
}

#[derive(Clone, Debug)]
pub struct AllDayOccurrenceChange {
    pub recurrence_date: String,
    pub expected_version: i64,
    pub event: EventMutation,
}

#[derive(Clone, Copy)]
enum EventAccess {
    Details,
    FreeBusy,
}

#[derive(Clone, Debug, Serialize)]
pub struct EventProjection {
    pub id: i64,
    pub calendar_id: i64,
    pub access: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    pub status: &'static str,
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
    pub created_by_user_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_edited_by_user_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurrence_rule: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurrence_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurrence_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_external: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
}

async fn mark_external_projections(
    pool: &SqlitePool,
    projections: &mut [EventProjection],
) -> Result<(), EventServiceError> {
    for projection in projections {
        let is_external = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM external_event_mapping WHERE event_id = ?)",
        )
        .bind(projection.id)
        .fetch_one(pool)
        .await?;
        if is_external {
            projection.is_external = Some(true);
            projection.read_only = Some(true);
        }
    }
    Ok(())
}

fn project_event(event: Event, access: EventAccess) -> EventProjection {
    let details = matches!(access, EventAccess::Details);
    let (event_kind, start_utc, end_utc, timezone, start_date, end_date) = match event.timing {
        EventTiming::Timed {
            start_utc,
            end_utc,
            timezone,
        } => (
            "timed",
            Some(start_utc),
            Some(end_utc),
            details.then_some(timezone),
            None,
            None,
        ),
        EventTiming::AllDay {
            start_date,
            end_date,
        } => (
            "all_day",
            None,
            None,
            None,
            Some(start_date),
            Some(end_date),
        ),
    };
    EventProjection {
        id: event.id,
        calendar_id: event.calendar_id,
        access: if details { "details" } else { "free_busy" },
        title: details.then_some(event.title),
        description: if details { event.description } else { None },
        location: if details { event.location } else { None },
        status: event.status.as_str(),
        event_kind,
        start_utc,
        end_utc,
        timezone,
        start_date,
        end_date,
        created_by_user_id: details.then_some(event.created_by_user_id),
        last_edited_by_user_id: details.then_some(event.last_edited_by_user_id),
        version: details.then_some(event.version),
        created_at: details.then_some(event.created_at),
        updated_at: details.then_some(event.updated_at),
        recurrence_rule: None,
        series_id: None,
        recurrence_id: None,
        recurrence_date: None,
        is_external: None,
        read_only: None,
    }
}

fn validate_recurrence(
    event: &EventMutation,
    recurrence_rule: &str,
) -> Result<(), EventServiceError> {
    RecurrenceRule::parse(recurrence_rule).map_err(|_| EventServiceError::InvalidInput)?;
    match &event.timing {
        EventTiming::Timed { timezone, .. } => {
            timezone
                .parse::<Tz>()
                .map_err(|_| EventServiceError::InvalidInput)?;
            Ok(())
        }
        EventTiming::AllDay { .. } => Ok(()),
    }
}

#[derive(FromRow)]
struct RecurringSeriesRecord {
    id: i64,
    calendar_id: i64,
    title: String,
    description: Option<String>,
    location: Option<String>,
    status: String,
    timed_start_utc: i64,
    timed_end_utc: i64,
    event_timezone: String,
    created_by_user_id: i64,
    last_edited_by_user_id: i64,
    version: i64,
    created_at: i64,
    updated_at: i64,
    recurrence_rule: String,
}

#[derive(FromRow)]
struct RecurrenceExceptionRecord {
    recurrence_id: i64,
    is_deleted: bool,
    title: Option<String>,
    description: Option<String>,
    location: Option<String>,
    status: Option<String>,
    timed_start_utc: Option<i64>,
    timed_end_utc: Option<i64>,
    event_timezone: Option<String>,
}

#[derive(FromRow)]
struct AllDaySeriesRecord {
    id: i64,
    calendar_id: i64,
    title: String,
    description: Option<String>,
    location: Option<String>,
    status: String,
    all_day_start_date: String,
    all_day_end_date: String,
    created_by_user_id: i64,
    last_edited_by_user_id: i64,
    version: i64,
    created_at: i64,
    updated_at: i64,
    recurrence_rule: String,
}

#[derive(FromRow)]
struct AllDayExceptionRecord {
    recurrence_date: String,
    is_deleted: bool,
    title: Option<String>,
    description: Option<String>,
    location: Option<String>,
    status: Option<String>,
    all_day_start_date: Option<String>,
    all_day_end_date: Option<String>,
}

fn project_occurrence(
    series: &RecurringSeriesRecord,
    exception: Option<&RecurrenceExceptionRecord>,
    occurrence: crate::recurrence::Occurrence,
    access: EventAccess,
) -> Result<EventProjection, EventServiceError> {
    let details = matches!(access, EventAccess::Details);
    let title = exception
        .and_then(|value| value.title.clone())
        .unwrap_or_else(|| series.title.clone());
    let description = exception
        .map(|value| value.description.clone())
        .unwrap_or_else(|| series.description.clone());
    let location = exception
        .map(|value| value.location.clone())
        .unwrap_or_else(|| series.location.clone());
    let status = exception
        .and_then(|value| value.status.as_deref())
        .unwrap_or(&series.status);
    let timezone = exception
        .and_then(|value| value.event_timezone.clone())
        .unwrap_or_else(|| series.event_timezone.clone());
    Ok(EventProjection {
        id: series.id,
        calendar_id: series.calendar_id,
        access: if details { "details" } else { "free_busy" },
        title: details.then_some(title),
        description: if details { description } else { None },
        location: if details { location } else { None },
        status: match status {
            "tentative" => "tentative",
            "confirmed" => "confirmed",
            "cancelled" => "cancelled",
            _ => return Err(EventServiceError::InvalidInput),
        },
        event_kind: "timed",
        start_utc: Some(occurrence.start.timestamp()),
        end_utc: Some(occurrence.end.timestamp()),
        timezone: details.then_some(timezone),
        start_date: None,
        end_date: None,
        created_by_user_id: details.then_some(series.created_by_user_id),
        last_edited_by_user_id: details.then_some(series.last_edited_by_user_id),
        version: details.then_some(series.version),
        created_at: details.then_some(series.created_at),
        updated_at: details.then_some(series.updated_at),
        recurrence_rule: details.then_some(series.recurrence_rule.clone()),
        series_id: Some(series.id),
        recurrence_id: Some(occurrence.recurrence_id.timestamp()),
        recurrence_date: None,
        is_external: None,
        read_only: None,
    })
}

fn project_all_day_occurrence(
    series: &AllDaySeriesRecord,
    exception: Option<&AllDayExceptionRecord>,
    occurrence: crate::recurrence::Occurrence,
    access: EventAccess,
) -> Result<EventProjection, EventServiceError> {
    let details = matches!(access, EventAccess::Details);
    let title = exception
        .and_then(|value| value.title.clone())
        .unwrap_or_else(|| series.title.clone());
    let description = exception
        .map(|value| value.description.clone())
        .unwrap_or_else(|| series.description.clone());
    let location = exception
        .map(|value| value.location.clone())
        .unwrap_or_else(|| series.location.clone());
    let status = exception
        .and_then(|value| value.status.as_deref())
        .unwrap_or(&series.status);
    let recurrence_date = occurrence
        .recurrence_id
        .date_naive()
        .format("%Y-%m-%d")
        .to_string();
    Ok(EventProjection {
        id: series.id,
        calendar_id: series.calendar_id,
        access: if details { "details" } else { "free_busy" },
        title: details.then_some(title),
        description: if details { description } else { None },
        location: if details { location } else { None },
        status: match status {
            "tentative" => "tentative",
            "confirmed" => "confirmed",
            "cancelled" => "cancelled",
            _ => return Err(EventServiceError::InvalidInput),
        },
        event_kind: "all_day",
        start_utc: None,
        end_utc: None,
        timezone: None,
        start_date: Some(occurrence.start.date_naive().format("%Y-%m-%d").to_string()),
        end_date: Some(occurrence.end.date_naive().format("%Y-%m-%d").to_string()),
        created_by_user_id: details.then_some(series.created_by_user_id),
        last_edited_by_user_id: details.then_some(series.last_edited_by_user_id),
        version: details.then_some(series.version),
        created_at: details.then_some(series.created_at),
        updated_at: details.then_some(series.updated_at),
        recurrence_rule: details.then_some(series.recurrence_rule.clone()),
        series_id: Some(series.id),
        recurrence_id: None,
        recurrence_date: Some(recurrence_date),
        is_external: None,
        read_only: None,
    })
}

fn date_at_utc(value: &str) -> Result<chrono::DateTime<Utc>, EventServiceError> {
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| EventServiceError::InvalidInput)?;
    Ok(date
        .and_hms_opt(0, 0, 0)
        .ok_or(EventServiceError::InvalidInput)?
        .and_utc())
}

#[derive(Debug)]
pub enum EventServiceError {
    ComplexityLimitExceeded,
    Conflict { current_version: i64 },
    Database(sqlx::Error),
    InvalidInput,
    NotFound,
    NotSupported,
    ReadOnly,
}

impl From<sqlx::Error> for EventServiceError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl From<EventRepositoryError> for EventServiceError {
    fn from(error: EventRepositoryError) -> Self {
        match error {
            EventRepositoryError::InvalidRange | EventRepositoryError::RangeTooLarge => {
                Self::InvalidInput
            }
            EventRepositoryError::NotFound => Self::NotFound,
            EventRepositoryError::StaleVersion => Self::Conflict { current_version: 0 },
            EventRepositoryError::Database(error) => Self::Database(error),
            EventRepositoryError::InvalidStoredEvent => {
                Self::Database(sqlx::Error::Protocol("stored event is invalid".to_owned()))
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Event {
    pub id: i64,
    pub calendar_id: i64,
    pub title: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub status: EventStatus,
    pub timing: EventTiming,
    pub created_by_user_id: i64,
    pub last_edited_by_user_id: i64,
    pub version: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewEvent {
    pub calendar_id: i64,
    pub title: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub status: EventStatus,
    pub timing: EventTiming,
    pub created_by_user_id: i64,
    pub last_edited_by_user_id: i64,
    pub created_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventUpdate {
    pub title: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub status: EventStatus,
    pub timing: EventTiming,
    pub last_edited_by_user_id: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EventTiming {
    Timed {
        start_utc: i64,
        end_utc: i64,
        timezone: String,
    },
    AllDay {
        start_date: String,
        end_date: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventStatus {
    Tentative,
    Confirmed,
    Cancelled,
}

impl EventStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Tentative => "tentative",
            Self::Confirmed => "confirmed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventRange {
    pub start_utc: i64,
    pub end_utc: i64,
    pub start_date: String,
    pub end_date: String,
}

#[derive(FromRow)]
struct EventRecord {
    id: i64,
    calendar_id: i64,
    title: String,
    description: Option<String>,
    location: Option<String>,
    status: String,
    event_kind: String,
    timed_start_utc: Option<i64>,
    timed_end_utc: Option<i64>,
    event_timezone: Option<String>,
    all_day_start_date: Option<String>,
    all_day_end_date: Option<String>,
    created_by_user_id: i64,
    last_edited_by_user_id: i64,
    version: i64,
    created_at: i64,
    updated_at: i64,
}

impl TryFrom<EventRecord> for Event {
    type Error = EventRepositoryError;

    fn try_from(record: EventRecord) -> Result<Self, Self::Error> {
        let status = match record.status.as_str() {
            "tentative" => EventStatus::Tentative,
            "confirmed" => EventStatus::Confirmed,
            "cancelled" => EventStatus::Cancelled,
            _ => return Err(EventRepositoryError::InvalidStoredEvent),
        };
        let timing = match record.event_kind.as_str() {
            "timed" => EventTiming::Timed {
                start_utc: record
                    .timed_start_utc
                    .ok_or(EventRepositoryError::InvalidStoredEvent)?,
                end_utc: record
                    .timed_end_utc
                    .ok_or(EventRepositoryError::InvalidStoredEvent)?,
                timezone: record
                    .event_timezone
                    .ok_or(EventRepositoryError::InvalidStoredEvent)?,
            },
            "all_day" => EventTiming::AllDay {
                start_date: record
                    .all_day_start_date
                    .ok_or(EventRepositoryError::InvalidStoredEvent)?,
                end_date: record
                    .all_day_end_date
                    .ok_or(EventRepositoryError::InvalidStoredEvent)?,
            },
            _ => return Err(EventRepositoryError::InvalidStoredEvent),
        };

        Ok(Self {
            id: record.id,
            calendar_id: record.calendar_id,
            title: record.title,
            description: record.description,
            location: record.location,
            status,
            timing,
            created_by_user_id: record.created_by_user_id,
            last_edited_by_user_id: record.last_edited_by_user_id,
            version: record.version,
            created_at: record.created_at,
            updated_at: record.updated_at,
        })
    }
}

struct StoredTiming<'a> {
    kind: &'static str,
    timed_start_utc: Option<i64>,
    timed_end_utc: Option<i64>,
    timezone: Option<&'a str>,
    all_day_start_date: Option<&'a str>,
    all_day_end_date: Option<&'a str>,
}

impl<'a> From<&'a EventTiming> for StoredTiming<'a> {
    fn from(timing: &'a EventTiming) -> Self {
        match timing {
            EventTiming::Timed {
                start_utc,
                end_utc,
                timezone,
            } => Self {
                kind: "timed",
                timed_start_utc: Some(*start_utc),
                timed_end_utc: Some(*end_utc),
                timezone: Some(timezone),
                all_day_start_date: None,
                all_day_end_date: None,
            },
            EventTiming::AllDay {
                start_date,
                end_date,
            } => Self {
                kind: "all_day",
                timed_start_utc: None,
                timed_end_utc: None,
                timezone: None,
                all_day_start_date: Some(start_date),
                all_day_end_date: Some(end_date),
            },
        }
    }
}

fn validate_timing(timing: &EventTiming) -> Result<(), EventRepositoryError> {
    let valid = match timing {
        EventTiming::Timed {
            start_utc,
            end_utc,
            timezone,
        } => start_utc < end_utc && !timezone.trim().is_empty(),
        EventTiming::AllDay {
            start_date,
            end_date,
        } => is_iso_date(start_date) && is_iso_date(end_date) && start_date < end_date,
    };
    if valid {
        Ok(())
    } else {
        Err(EventRepositoryError::InvalidRange)
    }
}

fn validate_range(
    range: &EventRange,
    max_range_seconds: i64,
    max_range_days: i64,
) -> Result<(), EventRepositoryError> {
    if range.start_utc >= range.end_utc
        || !is_iso_date(&range.start_date)
        || !is_iso_date(&range.end_date)
        || range.start_date >= range.end_date
    {
        return Err(EventRepositoryError::InvalidRange);
    }

    let utc_span = i128::from(range.end_utc) - i128::from(range.start_utc);
    let all_day_span = date_ordinal(&range.end_date) - date_ordinal(&range.start_date);
    if utc_span > i128::from(max_range_seconds) || all_day_span > max_range_days {
        return Err(EventRepositoryError::RangeTooLarge);
    }
    Ok(())
}

fn is_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let Ok(year) = value[0..4].parse::<u16>() else {
        return false;
    };
    let Ok(month) = value[5..7].parse::<u8>() else {
        return false;
    };
    let Ok(day) = value[8..10].parse::<u8>() else {
        return false;
    };
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(400) || (year.is_multiple_of(4) && !year.is_multiple_of(100)) => {
            29
        }
        2 => 28,
        _ => return false,
    };
    day > 0 && day <= days_in_month
}

fn date_ordinal(value: &str) -> i64 {
    let year = value[0..4].parse::<i64>().expect("date was validated");
    let month = value[5..7].parse::<i64>().expect("date was validated");
    let day = value[8..10].parse::<i64>().expect("date was validated");
    let adjustment = (14 - month) / 12;
    let adjusted_year = year + 4_800 - adjustment;
    let adjusted_month = month + 12 * adjustment - 3;

    day + (153 * adjusted_month + 2) / 5 + 365 * adjusted_year + adjusted_year / 4
        - adjusted_year / 100
        + adjusted_year / 400
        - 32_045
}

#[derive(Debug)]
pub enum EventRepositoryError {
    Database(sqlx::Error),
    InvalidRange,
    InvalidStoredEvent,
    NotFound,
    RangeTooLarge,
    StaleVersion,
}

impl Display for EventRepositoryError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(_) => formatter.write_str("event persistence failed"),
            Self::InvalidRange => formatter.write_str("event range is invalid"),
            Self::InvalidStoredEvent => formatter.write_str("stored event is invalid"),
            Self::NotFound => formatter.write_str("event record not found"),
            Self::RangeTooLarge => formatter.write_str("event range exceeds the configured limit"),
            Self::StaleVersion => formatter.write_str("event version is stale"),
        }
    }
}

impl Error for EventRepositoryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            Self::InvalidRange
            | Self::InvalidStoredEvent
            | Self::NotFound
            | Self::RangeTooLarge
            | Self::StaleVersion => None,
        }
    }
}

impl From<sqlx::Error> for EventRepositoryError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}
