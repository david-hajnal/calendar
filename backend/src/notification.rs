use std::{
    fmt::{self, Display, Formatter},
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use chrono::{NaiveDate, TimeZone};
use chrono_tz::Tz;
use sqlx::{FromRow, SqliteConnection, SqlitePool};
use uuid::Uuid;

use crate::{
    calendar::PendingNotificationCanceller,
    email::{EmailError, EmailSender, NotificationEmail},
    event::{EventRange, EventService},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotificationPreference {
    pub reminder_minutes: i64,
    pub timezone: String,
    pub enabled: bool,
}
impl NotificationPreference {
    pub fn new(reminder_minutes: i64, timezone: impl Into<String>) -> Self {
        Self {
            reminder_minutes,
            timezone: timezone.into(),
            enabled: true,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub enum PreferenceScope {
    Account,
    Calendar(i64),
    Event(i64),
}
#[derive(Clone)]
pub struct NotificationService {
    pool: SqlitePool,
    now: i64,
    horizon: i64,
}

#[derive(Clone)]
pub struct NotificationWorker<E = crate::email::DevelopmentEmailSender> {
    pool: SqlitePool,
    now: i64,
    claim_duration: i64,
    batch_size: i64,
    max_attempts: i64,
    metrics: Arc<NotificationWorkerMetricsInner>,
    email_sender: Option<Arc<E>>,
}

const DEFAULT_MAX_ATTEMPTS: i64 = 3;
const RETRY_BACKOFF_SECONDS: i64 = 60;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NotificationWorkerMetrics {
    pub delivered: u64,
    pub transient_retries: u64,
    pub permanent_failures: u64,
    pub cancelled: u64,
}

#[derive(Default)]
struct NotificationWorkerMetricsInner {
    delivered: AtomicU64,
    transient_retries: AtomicU64,
    permanent_failures: AtomicU64,
    cancelled: AtomicU64,
}

#[derive(Clone, Debug, PartialEq, Eq, FromRow)]
pub struct ClaimedNotificationJob {
    pub id: i64,
    pub user_id: i64,
    pub calendar_id: i64,
    pub event_id: i64,
    pub occurrence_key: String,
    pub scheduled_at: i64,
    pub claim_token: String,
}

#[derive(Clone, Debug, PartialEq, Eq, FromRow, serde::Serialize)]
pub struct InAppNotification {
    pub id: i64,
    pub event_id: i64,
    pub event_title: String,
    pub created_at: i64,
    pub read_at: Option<i64>,
}

impl NotificationWorker {
    pub fn new(pool: SqlitePool, claim_duration: i64, batch_size: usize) -> Self {
        Self::new_at(
            pool,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock is before Unix epoch")
                .as_secs() as i64,
            claim_duration,
            batch_size,
        )
    }

    pub fn new_at(pool: SqlitePool, now: i64, claim_duration: i64, batch_size: usize) -> Self {
        Self {
            pool,
            now,
            claim_duration: claim_duration.max(1),
            batch_size: (batch_size.max(1)).min(i64::MAX as usize) as i64,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            metrics: Arc::new(NotificationWorkerMetricsInner::default()),
            email_sender: None,
        }
    }
}

impl<E> NotificationWorker<E> {
    pub async fn claim_due(&self) -> Result<Vec<ClaimedNotificationJob>, sqlx::Error> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query(
            "UPDATE notification_jobs
             SET state = 'pending', claim_token = NULL, claim_expires_at = NULL, updated_at = ?
             WHERE state = 'claimed' AND claim_expires_at <= ?",
        )
        .bind(self.now)
        .bind(self.now)
        .execute(&mut *transaction)
        .await?;

        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM notification_jobs
             WHERE state = 'pending' AND scheduled_at <= ? AND (next_attempt_at IS NULL OR next_attempt_at <= ?)
             ORDER BY scheduled_at, id
             LIMIT ?",
        )
        .bind(self.now)
        .bind(self.now)
        .bind(self.batch_size)
        .fetch_all(&mut *transaction)
        .await?;
        if ids.is_empty() {
            transaction.commit().await?;
            return Ok(Vec::new());
        }

        let claim_token = Uuid::new_v4().to_string();
        let claim_expires_at = self.now.saturating_add(self.claim_duration);
        for id in &ids {
            sqlx::query(
                "UPDATE notification_jobs
                 SET state = 'claimed', claim_token = ?, claim_expires_at = ?, updated_at = ?
                 WHERE id = ? AND state = 'pending'",
            )
            .bind(&claim_token)
            .bind(claim_expires_at)
            .bind(self.now)
            .bind(id)
            .execute(&mut *transaction)
            .await?;
        }
        let jobs = sqlx::query_as::<_, ClaimedNotificationJob>(
            "SELECT id, user_id, calendar_id, event_id, occurrence_key, scheduled_at, claim_token
             FROM notification_jobs
             WHERE claim_token = ?
             ORDER BY scheduled_at, id",
        )
        .bind(claim_token)
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(jobs)
    }

    pub async fn mark_delivered(&self, job: &ClaimedNotificationJob) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE notification_jobs
             SET state = 'delivered', delivered_at = ?, updated_at = ?
             WHERE id = ? AND state = 'claimed' AND claim_token = ? AND claim_expires_at > ?",
        )
        .bind(self.now)
        .bind(self.now)
        .bind(job.id)
        .bind(&job.claim_token)
        .bind(self.now)
        .execute(&self.pool)
        .await?;
        let delivered = result.rows_affected() == 1;
        if delivered {
            self.metrics.delivered.fetch_add(1, Ordering::Relaxed);
        }
        Ok(delivered)
    }

    pub fn metrics(&self) -> NotificationWorkerMetrics {
        NotificationWorkerMetrics {
            delivered: self.metrics.delivered.load(Ordering::Relaxed),
            transient_retries: self.metrics.transient_retries.load(Ordering::Relaxed),
            permanent_failures: self.metrics.permanent_failures.load(Ordering::Relaxed),
            cancelled: self.metrics.cancelled.load(Ordering::Relaxed),
        }
    }
}

impl<E> NotificationWorker<E>
where
    E: EmailSender + Send + Sync,
{
    pub fn new_at_with_email_sender(
        pool: SqlitePool,
        now: i64,
        claim_duration: i64,
        batch_size: usize,
        email_sender: Arc<E>,
    ) -> Self {
        Self::new_at_with_email_sender_and_retry_policy(
            pool,
            now,
            claim_duration,
            batch_size,
            DEFAULT_MAX_ATTEMPTS,
            email_sender,
        )
    }

    pub fn new_at_with_email_sender_and_retry_policy(
        pool: SqlitePool,
        now: i64,
        claim_duration: i64,
        batch_size: usize,
        max_attempts: i64,
        email_sender: Arc<E>,
    ) -> Self {
        Self {
            pool,
            now,
            claim_duration: claim_duration.max(1),
            batch_size: (batch_size.max(1)).min(i64::MAX as usize) as i64,
            max_attempts: max_attempts.max(1),
            metrics: Arc::new(NotificationWorkerMetricsInner::default()),
            email_sender: Some(email_sender),
        }
    }

    pub async fn process_due(&self) -> Result<(), NotificationWorkerError> {
        let jobs = self.claim_due().await?;
        for job in jobs {
            let Some(delivery) = self.delivery_target(&job).await? else {
                self.cancel_claim(&job).await?;
                continue;
            };
            sqlx::query(
                "INSERT INTO in_app_notifications (user_id, notification_job_id, created_at)
                 VALUES (?, ?, ?) ON CONFLICT(notification_job_id) DO NOTHING",
            )
            .bind(job.user_id)
            .bind(job.id)
            .bind(self.now)
            .execute(&self.pool)
            .await?;
            if let Err(error) = self
                .email_sender
                .as_ref()
                .expect("email sender is present for delivery")
                .send_notification(NotificationEmail::new(delivery.email, delivery.event_title))
                .await
            {
                self.record_delivery_failure(&job, error).await?;
                continue;
            }
            self.mark_delivered(&job).await?;
        }
        Ok(())
    }

    async fn delivery_target(
        &self,
        job: &ClaimedNotificationJob,
    ) -> Result<Option<NotificationDeliveryTarget>, sqlx::Error> {
        sqlx::query_as(
            "SELECT users.normalized_email AS email, events.title AS event_title
             FROM users
             JOIN calendar_acl ON calendar_acl.user_id = users.id
             JOIN events ON events.id = ? AND events.calendar_id = calendar_acl.calendar_id
             WHERE users.id = ? AND users.status = 'active' AND calendar_acl.calendar_id = ?",
        )
        .bind(job.event_id)
        .bind(job.user_id)
        .bind(job.calendar_id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn cancel_claim(&self, job: &ClaimedNotificationJob) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE notification_jobs SET state = 'cancelled', updated_at = ?
             WHERE id = ? AND state = 'claimed' AND claim_token = ?",
        )
        .bind(self.now)
        .bind(job.id)
        .bind(&job.claim_token)
        .execute(&self.pool)
        .await?;
        self.metrics.cancelled.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn record_delivery_failure(
        &self,
        job: &ClaimedNotificationJob,
        error: EmailError,
    ) -> Result<(), sqlx::Error> {
        let attempts: i64 = sqlx::query_scalar(
            "SELECT attempts FROM notification_jobs WHERE id = ? AND state = 'claimed' AND claim_token = ?",
        ).bind(job.id).bind(&job.claim_token).fetch_optional(&self.pool).await?.unwrap_or(0) + 1;
        let retry = error.is_transient() && attempts < self.max_attempts;
        if retry {
            let delay = RETRY_BACKOFF_SECONDS.saturating_mul(1_i64 << (attempts - 1).min(6));
            sqlx::query(
                "UPDATE notification_jobs SET state = 'pending', attempts = ?, next_attempt_at = ?, last_error_code = ?, claim_token = NULL, claim_expires_at = NULL, updated_at = ? WHERE id = ? AND state = 'claimed' AND claim_token = ?",
            ).bind(attempts).bind(self.now.saturating_add(delay)).bind(error.code().as_str()).bind(self.now).bind(job.id).bind(&job.claim_token).execute(&self.pool).await?;
            self.metrics
                .transient_retries
                .fetch_add(1, Ordering::Relaxed);
            Self::log_delivery_failure(error, attempts, true);
        } else {
            sqlx::query(
                "UPDATE notification_jobs SET state = 'failed', attempts = ?, last_error_code = ?, claim_token = NULL, claim_expires_at = NULL, updated_at = ? WHERE id = ? AND state = 'claimed' AND claim_token = ?",
            ).bind(attempts).bind(error.code().as_str()).bind(self.now).bind(job.id).bind(&job.claim_token).execute(&self.pool).await?;
            self.metrics
                .permanent_failures
                .fetch_add(1, Ordering::Relaxed);
            Self::log_delivery_failure(error, attempts, false);
        }
        Ok(())
    }

    pub fn log_delivery_failure(error: EmailError, attempt: i64, retry: bool) {
        if retry {
            tracing::warn!(
                error_code = error.code().as_str(),
                attempt,
                "notification delivery retry scheduled"
            );
        } else {
            tracing::warn!(
                error_code = error.code().as_str(),
                attempt,
                "notification delivery failed permanently"
            );
        }
    }
}

#[derive(FromRow)]
struct NotificationDeliveryTarget {
    email: String,
    event_title: String,
}

#[derive(Debug)]
pub enum NotificationWorkerError {
    Database(sqlx::Error),
    Email(EmailError),
}

impl From<sqlx::Error> for NotificationWorkerError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl From<EmailError> for NotificationWorkerError {
    fn from(error: EmailError) -> Self {
        Self::Email(error)
    }
}

impl Display for NotificationWorkerError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(error) => error.fmt(formatter),
            Self::Email(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for NotificationWorkerError {}
impl NotificationService {
    pub fn new(pool: SqlitePool) -> Self {
        Self::new_at(
            pool,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock is before Unix epoch")
                .as_secs() as i64,
            14 * 86_400,
        )
    }
    pub fn new_at(pool: SqlitePool, now: i64, horizon: i64) -> Self {
        Self { pool, now, horizon }
    }
    pub async fn list_in_app(&self, user: i64) -> Result<Vec<InAppNotification>, sqlx::Error> {
        sqlx::query_as(
            "SELECT in_app_notifications.id, notification_jobs.event_id, events.title AS event_title, in_app_notifications.created_at, in_app_notifications.read_at
             FROM in_app_notifications
             JOIN notification_jobs ON notification_jobs.id = in_app_notifications.notification_job_id
             JOIN events ON events.id = notification_jobs.event_id
             WHERE in_app_notifications.user_id = ? ORDER BY in_app_notifications.created_at DESC, in_app_notifications.id DESC",
        )
        .bind(user)
        .fetch_all(&self.pool)
        .await
    }
    /// Development test seam: synchronously creates an in-app delivery for an event the user can access.
    pub async fn create_test_delivery(&self, user: i64, event_id: i64) -> Result<(), sqlx::Error> {
        let calendar: i64 = sqlx::query_scalar(
            "SELECT events.calendar_id FROM events JOIN calendar_acl ON calendar_acl.calendar_id = events.calendar_id WHERE events.id = ? AND calendar_acl.user_id = ?",
        )
        .bind(event_id)
        .bind(user)
        .fetch_one(&self.pool)
        .await?;
        let mut transaction = self.pool.begin().await?;
        let job_id = sqlx::query(
            "INSERT INTO notification_jobs (user_id,calendar_id,event_id,occurrence_key,scheduled_at,state,created_at,updated_at,delivered_at) VALUES (?,?,?,?,?,'delivered',?,?,?)",
        )
        .bind(user)
        .bind(calendar)
        .bind(event_id)
        .bind(format!("test-{}", Uuid::new_v4()))
        .bind(self.now)
        .bind(self.now)
        .bind(self.now)
        .bind(self.now)
        .execute(&mut *transaction)
        .await?
        .last_insert_rowid();
        sqlx::query("INSERT INTO in_app_notifications (user_id, notification_job_id, created_at) VALUES (?, ?, ?)")
            .bind(user)
            .bind(job_id)
            .bind(self.now)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await
    }
    pub async fn set_preference(
        &self,
        user: i64,
        scope: PreferenceScope,
        preference: NotificationPreference,
    ) -> Result<(), sqlx::Error> {
        let (calendar, event) = match scope {
            PreferenceScope::Account => (None, None),
            PreferenceScope::Calendar(id) => (Some(id), None),
            PreferenceScope::Event(id) => (None, Some(id)),
        };
        sqlx::query("INSERT INTO notification_preferences (user_id,calendar_id,event_id,reminder_minutes,timezone,enabled,updated_at) VALUES (?,?,?,?,?,?,?) ON CONFLICT DO UPDATE SET reminder_minutes=excluded.reminder_minutes,timezone=excluded.timezone,enabled=excluded.enabled,updated_at=excluded.updated_at")
   .bind(user).bind(calendar).bind(event).bind(preference.reminder_minutes).bind(preference.timezone).bind(preference.enabled).bind(self.now).execute(&self.pool).await?;
        match scope {
            PreferenceScope::Account => self.replan_user(user).await,
            PreferenceScope::Calendar(id) => self.replan_calendar_user(id, user).await,
            PreferenceScope::Event(id) => self.replan_event(id).await,
        }
    }
    pub async fn effective_preference(
        &self,
        user: i64,
        calendar: i64,
        event: i64,
    ) -> Result<NotificationPreference, sqlx::Error> {
        let row = sqlx::query_as::<_, PreferenceRow>("SELECT reminder_minutes, timezone, enabled FROM notification_preferences WHERE user_id=? AND ((event_id=?) OR (event_id IS NULL AND calendar_id=?) OR (event_id IS NULL AND calendar_id IS NULL)) ORDER BY CASE WHEN event_id IS NOT NULL THEN 0 WHEN calendar_id IS NOT NULL THEN 1 ELSE 2 END LIMIT 1").bind(user).bind(event).bind(calendar).fetch_optional(&self.pool).await?;
        Ok(row
            .map(|r| NotificationPreference {
                reminder_minutes: r.reminder_minutes,
                timezone: r.timezone,
                enabled: r.enabled,
            })
            .unwrap_or(NotificationPreference {
                reminder_minutes: 0,
                timezone: "UTC".into(),
                enabled: false,
            }))
    }
    pub async fn replan_event(&self, event_id: i64) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE notification_jobs SET state='cancelled',updated_at=? WHERE event_id=? AND state='pending'")
            .bind(self.now)
            .bind(event_id)
            .execute(&self.pool)
            .await?;
        let users:Vec<i64>=sqlx::query_scalar("SELECT calendar_acl.user_id FROM calendar_acl JOIN events ON events.calendar_id=calendar_acl.calendar_id WHERE events.id=?").bind(event_id).fetch_all(&self.pool).await?;
        for user in users {
            self.plan(event_id, user).await?;
        }
        Ok(())
    }
    pub async fn cancel_pending(&self, calendar: i64, user: i64) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE notification_jobs SET state='cancelled',updated_at=? WHERE calendar_id=? AND user_id=? AND state='pending'").bind(self.now).bind(calendar).bind(user).execute(&self.pool).await?;
        Ok(())
    }
    async fn replan_user(&self, user: i64) -> Result<(), sqlx::Error> {
        let ids:Vec<i64>=sqlx::query_scalar("SELECT events.id FROM events JOIN calendar_acl ON calendar_acl.calendar_id=events.calendar_id WHERE calendar_acl.user_id=?").bind(user).fetch_all(&self.pool).await?;
        for id in ids {
            self.plan(id, user).await?;
        }
        Ok(())
    }
    async fn replan_calendar_user(&self, calendar: i64, user: i64) -> Result<(), sqlx::Error> {
        let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM events WHERE calendar_id=?")
            .bind(calendar)
            .fetch_all(&self.pool)
            .await?;
        for id in ids {
            self.plan(id, user).await?;
        }
        Ok(())
    }
    async fn plan(&self, event_id: i64, user: i64) -> Result<(), sqlx::Error> {
        let calendar: i64 = sqlx::query_scalar("SELECT calendar_id FROM events WHERE id=?")
            .bind(event_id)
            .fetch_one(&self.pool)
            .await?;
        let p = self.effective_preference(user, calendar, event_id).await?;
        sqlx::query("UPDATE notification_jobs SET state='cancelled',updated_at=? WHERE event_id=? AND user_id=? AND state='pending'").bind(self.now).bind(event_id).bind(user).execute(&self.pool).await?;
        if !p.enabled {
            return Ok(());
        }
        let service = EventService::new_at(self.pool.clone(), self.now);
        let start_date = chrono::Utc
            .timestamp_opt(self.now, 0)
            .single()
            .unwrap()
            .date_naive()
            .format("%F")
            .to_string();
        let end_date = chrono::Utc
            .timestamp_opt(self.now + self.horizon, 0)
            .single()
            .unwrap()
            .date_naive()
            .succ_opt()
            .unwrap()
            .format("%F")
            .to_string();
        let occurrences = service
            .list(
                user,
                false,
                calendar,
                EventRange {
                    start_utc: self.now,
                    end_utc: self.now + self.horizon,
                    start_date,
                    end_date,
                },
            )
            .await
            .map_err(|_| sqlx::Error::Protocol("notification occurrence lookup failed".into()))?;
        for o in occurrences
            .into_iter()
            .filter(|o| o.id == event_id || o.series_id == Some(event_id))
            .filter(|o| o.status != "cancelled")
        {
            let key = o
                .recurrence_id
                .map(|v| v.to_string())
                .or(o.recurrence_date.clone())
                .unwrap_or_else(|| o.id.to_string());
            let at = match (o.start_utc, o.start_date) {
                (Some(v), _) => v - p.reminder_minutes * 60,
                (None, Some(d)) => local_midnight(&d, &p.timezone)? - p.reminder_minutes * 60,
                _ => continue,
            };
            if at >= self.now && at <= self.now + self.horizon {
                sqlx::query("INSERT INTO notification_jobs (user_id,calendar_id,event_id,occurrence_key,scheduled_at,state,created_at,updated_at) VALUES (?,?,?,?,?,'pending',?,?) ON CONFLICT(user_id,event_id,occurrence_key,scheduled_at) DO UPDATE SET state='pending', updated_at=excluded.updated_at WHERE notification_jobs.state='cancelled'").bind(user).bind(calendar).bind(event_id).bind(key).bind(at).bind(self.now).bind(self.now).execute(&self.pool).await?;
            }
        }
        Ok(())
    }
}
impl PendingNotificationCanceller for NotificationService {
    fn cancel_pending<'a>(
        &'a self,
        connection: &'a mut SqliteConnection,
        calendar_id: i64,
        user_id: i64,
    ) -> Pin<Box<dyn Future<Output = Result<(), sqlx::Error>> + Send + 'a>> {
        Box::pin(async move {
            sqlx::query("UPDATE notification_jobs SET state='cancelled',updated_at=? WHERE calendar_id=? AND user_id=? AND state='pending'").bind(self.now).bind(calendar_id).bind(user_id).execute(connection).await?;
            Ok(())
        })
    }
}
fn local_midnight(date: &str, timezone: &str) -> Result<i64, sqlx::Error> {
    let tz: Tz = timezone
        .parse()
        .map_err(|_| sqlx::Error::Protocol("invalid timezone".into()))?;
    let d = NaiveDate::parse_from_str(date, "%F")
        .map_err(|_| sqlx::Error::Protocol("invalid date".into()))?;
    tz.from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .ok_or_else(|| sqlx::Error::Protocol("ambiguous midnight".into()))
        .map(|v| v.timestamp())
}
#[derive(FromRow)]
struct PreferenceRow {
    reminder_minutes: i64,
    timezone: String,
    enabled: bool,
}
