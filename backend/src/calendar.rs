use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    future::Future,
    pin::Pin,
    str::FromStr,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::{
    authorization::{
        AuthorizationDecision, CalendarAction, CalendarRole, PlatformRole,
        authorize_calendar_action,
    },
    identity::UserStatus,
};

#[derive(Clone)]
pub struct CalendarService {
    pool: SqlitePool,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    notification_canceller: Arc<dyn PendingNotificationCanceller>,
}

impl CalendarService {
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool: pool.clone(),
            clock: Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system clock is before Unix epoch")
                    .as_secs() as i64
            }),
            notification_canceller: Arc::new(crate::notification::NotificationService::new(
                pool.clone(),
            )),
        }
    }

    pub fn new_at(pool: SqlitePool, now: i64) -> Self {
        let mut service = Self::new(pool);
        service.clock = Arc::new(move || now);
        service
    }

    pub fn new_at_with_notification_canceller(
        pool: SqlitePool,
        now: i64,
        notification_canceller: Arc<dyn PendingNotificationCanceller>,
    ) -> Self {
        Self {
            pool,
            clock: Arc::new(move || now),
            notification_canceller,
        }
    }

    pub async fn list(
        &self,
        actor_user_id: i64,
        is_superadmin: bool,
    ) -> Result<Vec<CalendarProjection>, CalendarServiceError> {
        let records = sqlx::query_as::<_, VisibleCalendarRecord>(
            "SELECT calendars.id, calendars.owner_user_id, calendars.name,
                    calendars.description, calendars.color, calendars.default_timezone,
                    calendars.default_event_visibility,
                    calendars.default_notification_rules_json, calendars.archived,
                    calendars.version, calendars.created_at, calendars.updated_at,
                    calendar_acl.role
             FROM calendars
             JOIN calendar_acl ON calendar_acl.calendar_id = calendars.id
              WHERE calendar_acl.user_id = ?
                AND calendars.archived = 0
              ORDER BY calendars.id",
        )
        .bind(actor_user_id)
        .fetch_all(&self.pool)
        .await?;

        records
            .into_iter()
            .map(|record| self.project(record, is_superadmin))
            .collect()
    }

    pub async fn get(
        &self,
        actor_user_id: i64,
        is_superadmin: bool,
        calendar_id: i64,
    ) -> Result<CalendarProjection, CalendarServiceError> {
        let record = self
            .visible_calendar(actor_user_id, calendar_id)
            .await?
            .ok_or(CalendarServiceError::NotFound)?;
        self.project(record, is_superadmin)
    }

    pub async fn create(
        &self,
        actor_user_id: i64,
        calendar: NewCalendar,
    ) -> Result<CalendarProjection, CalendarServiceError> {
        validate_calendar_fields(
            &calendar.name,
            &calendar.color,
            &calendar.default_timezone,
            &calendar.default_event_visibility,
        )?;
        let now = (self.clock)();
        let mut transaction = self.pool.begin().await?;
        let result = sqlx::query(
            "INSERT INTO calendars (
                owner_user_id, name, description, color, default_timezone,
                default_event_visibility, default_notification_rules_json, archived,
                version, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, 0, 1, ?, ?)",
        )
        .bind(actor_user_id)
        .bind(&calendar.name)
        .bind(&calendar.description)
        .bind(&calendar.color)
        .bind(&calendar.default_timezone)
        .bind(&calendar.default_event_visibility)
        .bind(&calendar.default_notification_rules_json)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        let id = result.last_insert_rowid();
        sqlx::query(
            "INSERT INTO calendar_acl (
                calendar_id, user_id, role, created_at, updated_at
             ) VALUES (?, ?, 'owner', ?, ?)",
        )
        .bind(id)
        .bind(actor_user_id)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        insert_audit(&mut transaction, actor_user_id, "calendar.create", id, now).await?;
        transaction.commit().await?;
        self.get(actor_user_id, false, id).await
    }

    pub async fn update(
        &self,
        actor_user_id: i64,
        is_superadmin: bool,
        calendar_id: i64,
        expected_version: i64,
        mut update: CalendarUpdate,
    ) -> Result<CalendarProjection, CalendarServiceError> {
        validate_calendar_fields(
            &update.name,
            &update.color,
            &update.default_timezone,
            &update.default_event_visibility,
        )?;
        let current = self
            .authorized_calendar(
                actor_user_id,
                is_superadmin,
                calendar_id,
                CalendarAction::ManageSettings,
            )
            .await?;
        update.archived = current.archived;
        update.updated_at = (self.clock)();
        let result = CalendarRepository::new(self.pool.clone())
            .update_calendar(calendar_id, expected_version, update)
            .await;
        match result {
            Ok(_) => {}
            Err(CalendarRepositoryError::StaleVersion) => {
                let current_version =
                    sqlx::query_scalar("SELECT version FROM calendars WHERE id = ?")
                        .bind(calendar_id)
                        .fetch_optional(&self.pool)
                        .await?
                        .ok_or(CalendarServiceError::NotFound)?;
                return Err(CalendarServiceError::Conflict { current_version });
            }
            Err(CalendarRepositoryError::NotFound) => {
                return Err(CalendarServiceError::NotFound);
            }
            Err(CalendarRepositoryError::InvalidRole) => {
                return Err(CalendarServiceError::InvalidInput);
            }
            Err(CalendarRepositoryError::InactiveTarget) => {
                return Err(CalendarServiceError::OperationConflict);
            }
            Err(CalendarRepositoryError::Database(error)) => {
                return Err(CalendarServiceError::Database(error));
            }
        }
        self.get(actor_user_id, is_superadmin, calendar_id).await
    }

    pub async fn set_archived(
        &self,
        actor_user_id: i64,
        is_superadmin: bool,
        calendar_id: i64,
        archived: bool,
    ) -> Result<CalendarProjection, CalendarServiceError> {
        self.authorized_calendar(
            actor_user_id,
            is_superadmin,
            calendar_id,
            CalendarAction::ManageSettings,
        )
        .await?;
        let now = (self.clock)();
        let mut transaction = self.pool.begin().await?;
        let is_archived: i32 = sqlx::query_scalar("SELECT archived FROM calendars WHERE id = ?")
            .bind(calendar_id)
            .fetch_one(&mut *transaction)
            .await?;
        if !archived && is_archived == 0 {
            return Err(CalendarServiceError::NotFound);
        }
        sqlx::query(
            "UPDATE calendars
             SET archived = ?, version = version + 1, updated_at = ?
             WHERE id = ?",
        )
        .bind(archived)
        .bind(now)
        .bind(calendar_id)
        .execute(&mut *transaction)
        .await?;
        insert_audit(
            &mut transaction,
            actor_user_id,
            if archived {
                "calendar.archive"
            } else {
                "calendar.restore"
            },
            calendar_id,
            now,
        )
        .await?;
        transaction.commit().await?;
        self.get(actor_user_id, is_superadmin, calendar_id).await
    }

    pub async fn delete(
        &self,
        actor_user_id: i64,
        is_superadmin: bool,
        calendar_id: i64,
    ) -> Result<(), CalendarServiceError> {
        self.authorized_calendar(
            actor_user_id,
            is_superadmin,
            calendar_id,
            CalendarAction::DeleteCalendar,
        )
        .await?;
        let now = (self.clock)();
        let mut transaction = self.pool.begin().await?;
        let deleted = sqlx::query("DELETE FROM calendars WHERE id = ?")
            .bind(calendar_id)
            .execute(&mut *transaction)
            .await?;
        if deleted.rows_affected() != 1 {
            transaction.rollback().await?;
            return Err(CalendarServiceError::NotFound);
        }
        insert_audit(
            &mut transaction,
            actor_user_id,
            "calendar.delete",
            calendar_id,
            now,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn list_acl(
        &self,
        actor_user_id: i64,
        is_superadmin: bool,
        calendar_id: i64,
    ) -> Result<Vec<CalendarAclProjection>, CalendarServiceError> {
        self.authorized_calendar(
            actor_user_id,
            is_superadmin,
            calendar_id,
            CalendarAction::ManageAcl,
        )
        .await?;
        sqlx::query_as::<_, CalendarAclRecord>(
            "SELECT calendar_id, user_id, role, created_at, updated_at
             FROM calendar_acl WHERE calendar_id = ? ORDER BY user_id",
        )
        .bind(calendar_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(CalendarAclProjection::try_from)
        .collect()
    }

    pub async fn set_acl(
        &self,
        actor_user_id: i64,
        is_superadmin: bool,
        calendar_id: i64,
        target_user_id: i64,
        role: CalendarRole,
    ) -> Result<CalendarAclProjection, CalendarServiceError> {
        self.authorized_calendar(
            actor_user_id,
            is_superadmin,
            calendar_id,
            CalendarAction::ManageAcl,
        )
        .await?;
        if role == CalendarRole::Owner {
            return Err(CalendarServiceError::InvalidInput);
        }

        let now = (self.clock)();
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let target_status: Option<String> =
            sqlx::query_scalar("SELECT status FROM users WHERE id = ?")
                .bind(target_user_id)
                .fetch_optional(&mut *transaction)
                .await?;
        if target_status.as_deref() != Some("active") {
            transaction.rollback().await?;
            return Err(CalendarServiceError::OperationConflict);
        }
        let existing_role: Option<String> = sqlx::query_scalar(
            "SELECT role FROM calendar_acl WHERE calendar_id = ? AND user_id = ?",
        )
        .bind(calendar_id)
        .bind(target_user_id)
        .fetch_optional(&mut *transaction)
        .await?;
        if existing_role.as_deref() == Some("owner") {
            transaction.rollback().await?;
            return Err(CalendarServiceError::OperationConflict);
        }
        sqlx::query(
            "INSERT INTO calendar_acl (
                calendar_id, user_id, role, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(calendar_id, user_id) DO UPDATE
             SET role = excluded.role, updated_at = excluded.updated_at",
        )
        .bind(calendar_id)
        .bind(target_user_id)
        .bind(role_name(role))
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        insert_acl_audit(
            &mut transaction,
            actor_user_id,
            if existing_role.is_some() {
                "calendar.acl.update"
            } else {
                "calendar.acl.grant"
            },
            calendar_id,
            target_user_id,
            now,
        )
        .await?;
        let entry = sqlx::query_as::<_, CalendarAclRecord>(
            "SELECT calendar_id, user_id, role, created_at, updated_at
             FROM calendar_acl WHERE calendar_id = ? AND user_id = ?",
        )
        .bind(calendar_id)
        .bind(target_user_id)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        CalendarAclProjection::try_from(entry)
    }

    pub async fn revoke_acl(
        &self,
        actor_user_id: i64,
        is_superadmin: bool,
        calendar_id: i64,
        target_user_id: i64,
    ) -> Result<(), CalendarServiceError> {
        self.authorized_calendar(
            actor_user_id,
            is_superadmin,
            calendar_id,
            CalendarAction::ManageAcl,
        )
        .await?;
        let now = (self.clock)();
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let role: Option<String> = sqlx::query_scalar(
            "SELECT role FROM calendar_acl WHERE calendar_id = ? AND user_id = ?",
        )
        .bind(calendar_id)
        .bind(target_user_id)
        .fetch_optional(&mut *transaction)
        .await?;
        match role.as_deref() {
            Some("owner") => {
                transaction.rollback().await?;
                return Err(CalendarServiceError::OperationConflict);
            }
            Some(_) => {}
            None => {
                transaction.rollback().await?;
                return Err(CalendarServiceError::NotFound);
            }
        }
        sqlx::query("DELETE FROM calendar_acl WHERE calendar_id = ? AND user_id = ?")
            .bind(calendar_id)
            .bind(target_user_id)
            .execute(&mut *transaction)
            .await?;
        self.notification_canceller
            .cancel_pending(&mut transaction, calendar_id, target_user_id)
            .await?;
        insert_acl_audit(
            &mut transaction,
            actor_user_id,
            "calendar.acl.revoke",
            calendar_id,
            target_user_id,
            now,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn transfer_ownership(
        &self,
        actor_user_id: i64,
        is_superadmin: bool,
        calendar_id: i64,
        new_owner_user_id: i64,
        expected_version: i64,
    ) -> Result<CalendarProjection, CalendarServiceError> {
        self.authorized_calendar(
            actor_user_id,
            is_superadmin,
            calendar_id,
            CalendarAction::TransferOwnership,
        )
        .await?;
        if actor_user_id == new_owner_user_id {
            return Err(CalendarServiceError::OperationConflict);
        }

        let now = (self.clock)();
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let target_status: Option<String> =
            sqlx::query_scalar("SELECT status FROM users WHERE id = ?")
                .bind(new_owner_user_id)
                .fetch_optional(&mut *transaction)
                .await?;
        if target_status.as_deref() != Some("active") {
            transaction.rollback().await?;
            return Err(CalendarServiceError::OperationConflict);
        }
        let updated = sqlx::query(
            "UPDATE calendars
             SET owner_user_id = ?, version = version + 1, updated_at = ?
             WHERE id = ? AND owner_user_id = ? AND version = ?",
        )
        .bind(new_owner_user_id)
        .bind(now)
        .bind(calendar_id)
        .bind(actor_user_id)
        .bind(expected_version)
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() != 1 {
            transaction.rollback().await?;
            return Err(CalendarServiceError::OperationConflict);
        }
        sqlx::query(
            "UPDATE calendar_acl SET role = 'manager', updated_at = ?
             WHERE calendar_id = ? AND user_id = ? AND role = 'owner'",
        )
        .bind(now)
        .bind(calendar_id)
        .bind(actor_user_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO calendar_acl (
                calendar_id, user_id, role, created_at, updated_at
             ) VALUES (?, ?, 'owner', ?, ?)
             ON CONFLICT(calendar_id, user_id) DO UPDATE
             SET role = 'owner', updated_at = excluded.updated_at",
        )
        .bind(calendar_id)
        .bind(new_owner_user_id)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        insert_acl_audit(
            &mut transaction,
            actor_user_id,
            "calendar.acl.transfer",
            calendar_id,
            new_owner_user_id,
            now,
        )
        .await?;
        transaction.commit().await?;
        self.get(actor_user_id, is_superadmin, calendar_id).await
    }

    async fn authorized_calendar(
        &self,
        actor_user_id: i64,
        is_superadmin: bool,
        calendar_id: i64,
        action: CalendarAction,
    ) -> Result<VisibleCalendarRecord, CalendarServiceError> {
        let record = self
            .visible_calendar(actor_user_id, calendar_id)
            .await?
            .ok_or(CalendarServiceError::NotFound)?;
        let role =
            CalendarRole::from_str(&record.role).map_err(|_| CalendarServiceError::InvalidInput)?;
        let platform_role = if is_superadmin {
            PlatformRole::Superadmin
        } else {
            PlatformRole::User
        };
        match authorize_calendar_action(UserStatus::Active, Some(platform_role), Some(role), action)
        {
            AuthorizationDecision::Allow => Ok(record),
            AuthorizationDecision::Deny => Err(CalendarServiceError::NotFound),
        }
    }

    async fn visible_calendar(
        &self,
        actor_user_id: i64,
        calendar_id: i64,
    ) -> Result<Option<VisibleCalendarRecord>, CalendarServiceError> {
        sqlx::query_as(
            "SELECT calendars.id, calendars.owner_user_id, calendars.name,
                    calendars.description, calendars.color, calendars.default_timezone,
                    calendars.default_event_visibility,
                    calendars.default_notification_rules_json, calendars.archived,
                    calendars.version, calendars.created_at, calendars.updated_at,
                    calendar_acl.role
             FROM calendars
             JOIN calendar_acl ON calendar_acl.calendar_id = calendars.id
             WHERE calendars.id = ? AND calendar_acl.user_id = ?",
        )
        .bind(calendar_id)
        .bind(actor_user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(Into::into)
    }

    fn project(
        &self,
        record: VisibleCalendarRecord,
        is_superadmin: bool,
    ) -> Result<CalendarProjection, CalendarServiceError> {
        let role =
            CalendarRole::from_str(&record.role).map_err(|_| CalendarServiceError::InvalidInput)?;
        let platform_role = if is_superadmin {
            PlatformRole::Superadmin
        } else {
            PlatformRole::User
        };
        let can_read_details = authorize_calendar_action(
            UserStatus::Active,
            Some(platform_role),
            Some(role),
            CalendarAction::ReadDetails,
        ) == AuthorizationDecision::Allow;
        let can_read_free_busy = authorize_calendar_action(
            UserStatus::Active,
            Some(platform_role),
            Some(role),
            CalendarAction::ReadFreeBusy,
        ) == AuthorizationDecision::Allow;
        if !can_read_free_busy {
            return Err(CalendarServiceError::NotFound);
        }

        Ok(if can_read_details {
            CalendarProjection {
                id: record.id,
                access: "details",
                role: record.role,
                owner_user_id: Some(record.owner_user_id),
                name: Some(record.name),
                description: record.description,
                color: Some(record.color),
                default_timezone: Some(record.default_timezone),
                default_event_visibility: Some(record.default_event_visibility),
                default_notification_rules_json: record.default_notification_rules_json,
                archived: Some(record.archived),
                version: Some(record.version),
                created_at: Some(record.created_at),
                updated_at: Some(record.updated_at),
            }
        } else {
            CalendarProjection {
                id: record.id,
                access: "free_busy",
                role: record.role,
                owner_user_id: None,
                name: None,
                description: None,
                color: None,
                default_timezone: None,
                default_event_visibility: None,
                default_notification_rules_json: None,
                archived: None,
                version: None,
                created_at: None,
                updated_at: None,
            }
        })
    }
}

fn validate_calendar_fields(
    name: &str,
    color: &str,
    default_timezone: &str,
    default_event_visibility: &str,
) -> Result<(), CalendarServiceError> {
    if name.trim().is_empty()
        || color.trim().is_empty()
        || default_timezone.trim().is_empty()
        || !matches!(default_event_visibility, "default" | "public" | "private")
    {
        return Err(CalendarServiceError::InvalidInput);
    }
    Ok(())
}

async fn insert_audit(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    actor_user_id: i64,
    action: &'static str,
    calendar_id: i64,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO audit_log (
            actor_user_id, action, target_type, target_id, metadata_json, created_at
         ) VALUES (?, ?, 'calendar', ?, NULL, ?)",
    )
    .bind(actor_user_id)
    .bind(action)
    .bind(calendar_id.to_string())
    .bind(now)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn insert_acl_audit(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    actor_user_id: i64,
    action: &'static str,
    calendar_id: i64,
    target_user_id: i64,
    now: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO audit_log (
            actor_user_id, action, target_type, target_id, metadata_json, created_at
         ) VALUES (?, ?, 'calendar_acl', ?, NULL, ?)",
    )
    .bind(actor_user_id)
    .bind(action)
    .bind(format!("{calendar_id}:{target_user_id}"))
    .bind(now)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub trait PendingNotificationCanceller: Send + Sync {
    fn cancel_pending<'a>(
        &'a self,
        connection: &'a mut SqliteConnection,
        calendar_id: i64,
        user_id: i64,
    ) -> Pin<Box<dyn Future<Output = Result<(), sqlx::Error>> + Send + 'a>>;
}

#[derive(FromRow)]
struct VisibleCalendarRecord {
    id: i64,
    owner_user_id: i64,
    name: String,
    description: Option<String>,
    color: String,
    default_timezone: String,
    default_event_visibility: String,
    default_notification_rules_json: Option<String>,
    archived: bool,
    version: i64,
    created_at: i64,
    updated_at: i64,
    role: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CalendarProjection {
    pub id: i64,
    pub access: &'static str,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_user_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_timezone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_event_visibility: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_notification_rules_json: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CalendarAclProjection {
    pub user_id: i64,
    pub role: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl TryFrom<CalendarAclRecord> for CalendarAclProjection {
    type Error = CalendarServiceError;

    fn try_from(record: CalendarAclRecord) -> Result<Self, Self::Error> {
        CalendarRole::from_str(&record.role).map_err(|_| CalendarServiceError::InvalidInput)?;
        Ok(Self {
            user_id: record.user_id,
            role: record.role,
            created_at: record.created_at,
            updated_at: record.updated_at,
        })
    }
}

#[derive(Debug)]
pub enum CalendarServiceError {
    Database(sqlx::Error),
    InvalidInput,
    NotFound,
    Conflict { current_version: i64 },
    OperationConflict,
}

impl Display for CalendarServiceError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(_) => formatter.write_str("calendar operation failed"),
            Self::InvalidInput => formatter.write_str("invalid calendar input"),
            Self::NotFound => formatter.write_str("calendar not found"),
            Self::Conflict { .. } => formatter.write_str("calendar version conflict"),
            Self::OperationConflict => formatter.write_str("calendar operation conflicts"),
        }
    }
}

impl Error for CalendarServiceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            Self::InvalidInput
            | Self::NotFound
            | Self::Conflict { .. }
            | Self::OperationConflict => None,
        }
    }
}

impl From<sqlx::Error> for CalendarServiceError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

#[derive(Clone)]
pub struct CalendarRepository {
    pool: SqlitePool,
}

impl CalendarRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn create_calendar(
        &self,
        owner_user_id: i64,
        calendar: NewCalendar,
    ) -> Result<Calendar, CalendarRepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let result = sqlx::query(
            "INSERT INTO calendars (
                owner_user_id, name, description, color, default_timezone,
                default_event_visibility, default_notification_rules_json, archived,
                version, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, 0, 1, ?, ?)",
        )
        .bind(owner_user_id)
        .bind(&calendar.name)
        .bind(&calendar.description)
        .bind(&calendar.color)
        .bind(&calendar.default_timezone)
        .bind(&calendar.default_event_visibility)
        .bind(&calendar.default_notification_rules_json)
        .bind(calendar.created_at)
        .bind(calendar.created_at)
        .execute(&mut *transaction)
        .await?;
        let id = result.last_insert_rowid();

        sqlx::query(
            "INSERT INTO calendar_acl (
                calendar_id, user_id, role, created_at, updated_at
             ) VALUES (?, ?, 'owner', ?, ?)",
        )
        .bind(id)
        .bind(owner_user_id)
        .bind(calendar.created_at)
        .bind(calendar.created_at)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;

        Ok(Calendar {
            id,
            owner_user_id,
            name: calendar.name,
            description: calendar.description,
            color: calendar.color,
            default_timezone: calendar.default_timezone,
            default_event_visibility: calendar.default_event_visibility,
            default_notification_rules_json: calendar.default_notification_rules_json,
            archived: false,
            version: 1,
            created_at: calendar.created_at,
            updated_at: calendar.created_at,
        })
    }

    pub async fn calendar(
        &self,
        calendar_id: i64,
    ) -> Result<Option<Calendar>, CalendarRepositoryError> {
        sqlx::query_as(
            "SELECT id, owner_user_id, name, description, color, default_timezone,
                    default_event_visibility, default_notification_rules_json, archived,
                    version, created_at, updated_at
             FROM calendars WHERE id = ?",
        )
        .bind(calendar_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(Into::into)
    }

    pub async fn acl_entries(
        &self,
        calendar_id: i64,
    ) -> Result<Vec<CalendarAcl>, CalendarRepositoryError> {
        let records = sqlx::query_as::<_, CalendarAclRecord>(
            "SELECT calendar_id, user_id, role, created_at, updated_at
             FROM calendar_acl WHERE calendar_id = ? ORDER BY user_id",
        )
        .bind(calendar_id)
        .fetch_all(&self.pool)
        .await?;

        records.into_iter().map(CalendarAcl::try_from).collect()
    }

    pub async fn add_acl(
        &self,
        calendar_id: i64,
        user_id: i64,
        role: CalendarRole,
        now: i64,
    ) -> Result<CalendarAcl, CalendarRepositoryError> {
        if role == CalendarRole::Owner {
            return Err(CalendarRepositoryError::InvalidRole);
        }
        sqlx::query(
            "INSERT INTO calendar_acl (
                calendar_id, user_id, role, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(calendar_id)
        .bind(user_id)
        .bind(role_name(role))
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(CalendarAcl {
            calendar_id,
            user_id,
            role,
            created_at: now,
            updated_at: now,
        })
    }

    pub async fn transfer_ownership(
        &self,
        calendar_id: i64,
        current_owner_user_id: i64,
        new_owner_user_id: i64,
        expected_version: i64,
        now: i64,
    ) -> Result<Calendar, CalendarRepositoryError> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let target_is_active: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM users WHERE id = ? AND status = 'active'
             )",
        )
        .bind(new_owner_user_id)
        .fetch_one(&mut *transaction)
        .await?;
        if !target_is_active {
            transaction.rollback().await?;
            return Err(CalendarRepositoryError::InactiveTarget);
        }

        let demoted = sqlx::query(
            "UPDATE calendar_acl SET role = 'manager', updated_at = ?
             WHERE calendar_id = ? AND user_id = ? AND role = 'owner'",
        )
        .bind(now)
        .bind(calendar_id)
        .bind(current_owner_user_id)
        .execute(&mut *transaction)
        .await?;
        if demoted.rows_affected() != 1 {
            transaction.rollback().await?;
            return Err(CalendarRepositoryError::NotFound);
        }

        sqlx::query(
            "INSERT INTO calendar_acl (
                calendar_id, user_id, role, created_at, updated_at
             ) VALUES (?, ?, 'owner', ?, ?)
             ON CONFLICT(calendar_id, user_id) DO UPDATE
             SET role = 'owner', updated_at = excluded.updated_at",
        )
        .bind(calendar_id)
        .bind(new_owner_user_id)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await?;

        let updated = sqlx::query(
            "UPDATE calendars
             SET owner_user_id = ?, version = version + 1, updated_at = ?
             WHERE id = ? AND owner_user_id = ? AND version = ?",
        )
        .bind(new_owner_user_id)
        .bind(now)
        .bind(calendar_id)
        .bind(current_owner_user_id)
        .bind(expected_version)
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() != 1 {
            transaction.rollback().await?;
            return Err(CalendarRepositoryError::StaleVersion);
        }

        let calendar = sqlx::query_as(
            "SELECT id, owner_user_id, name, description, color, default_timezone,
                    default_event_visibility, default_notification_rules_json, archived,
                    version, created_at, updated_at
             FROM calendars WHERE id = ?",
        )
        .bind(calendar_id)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(calendar)
    }

    pub async fn update_calendar(
        &self,
        calendar_id: i64,
        expected_version: i64,
        update: CalendarUpdate,
    ) -> Result<Calendar, CalendarRepositoryError> {
        let result = sqlx::query(
            "UPDATE calendars
             SET name = ?, description = ?, color = ?, default_timezone = ?,
                 default_event_visibility = ?, default_notification_rules_json = ?,
                 archived = ?, version = version + 1, updated_at = ?
             WHERE id = ? AND version = ?",
        )
        .bind(&update.name)
        .bind(&update.description)
        .bind(&update.color)
        .bind(&update.default_timezone)
        .bind(&update.default_event_visibility)
        .bind(&update.default_notification_rules_json)
        .bind(update.archived)
        .bind(update.updated_at)
        .bind(calendar_id)
        .bind(expected_version)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() != 1 {
            return Err(CalendarRepositoryError::StaleVersion);
        }

        self.calendar(calendar_id)
            .await?
            .ok_or(CalendarRepositoryError::NotFound)
    }
}

fn role_name(role: CalendarRole) -> &'static str {
    match role {
        CalendarRole::Owner => "owner",
        CalendarRole::Manager => "manager",
        CalendarRole::Editor => "editor",
        CalendarRole::Viewer => "viewer",
        CalendarRole::FreeBusyViewer => "free_busy_viewer",
    }
}

#[derive(Clone, Debug, Eq, PartialEq, FromRow)]
pub struct Calendar {
    pub id: i64,
    pub owner_user_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub color: String,
    pub default_timezone: String,
    pub default_event_visibility: String,
    pub default_notification_rules_json: Option<String>,
    pub archived: bool,
    pub version: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewCalendar {
    pub name: String,
    pub description: Option<String>,
    pub color: String,
    pub default_timezone: String,
    pub default_event_visibility: String,
    pub default_notification_rules_json: Option<String>,
    pub created_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CalendarUpdate {
    pub name: String,
    pub description: Option<String>,
    pub color: String,
    pub default_timezone: String,
    pub default_event_visibility: String,
    pub default_notification_rules_json: Option<String>,
    pub archived: bool,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CalendarAcl {
    pub calendar_id: i64,
    pub user_id: i64,
    pub role: CalendarRole,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(FromRow)]
struct CalendarAclRecord {
    calendar_id: i64,
    user_id: i64,
    role: String,
    created_at: i64,
    updated_at: i64,
}

impl TryFrom<CalendarAclRecord> for CalendarAcl {
    type Error = CalendarRepositoryError;

    fn try_from(record: CalendarAclRecord) -> Result<Self, Self::Error> {
        let role = CalendarRole::from_str(&record.role)
            .map_err(|_| CalendarRepositoryError::InvalidRole)?;
        Ok(Self {
            calendar_id: record.calendar_id,
            user_id: record.user_id,
            role,
            created_at: record.created_at,
            updated_at: record.updated_at,
        })
    }
}

#[derive(Debug)]
pub enum CalendarRepositoryError {
    Database(sqlx::Error),
    InactiveTarget,
    InvalidRole,
    NotFound,
    StaleVersion,
}

impl Display for CalendarRepositoryError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(_) => formatter.write_str("calendar persistence failed"),
            Self::InactiveTarget => formatter.write_str("new calendar owner is not active"),
            Self::InvalidRole => formatter.write_str("invalid calendar role"),
            Self::NotFound => formatter.write_str("calendar record not found"),
            Self::StaleVersion => formatter.write_str("calendar version is stale"),
        }
    }
}

impl Error for CalendarRepositoryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            Self::InactiveTarget | Self::InvalidRole | Self::NotFound | Self::StaleVersion => None,
        }
    }
}

impl From<sqlx::Error> for CalendarRepositoryError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}
