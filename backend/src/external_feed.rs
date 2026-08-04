use std::{
    collections::HashMap,
    path::PathBuf,
    str::FromStr,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use url::Url;

use crate::{
    authorization::{
        AuthorizationDecision, CalendarAction, CalendarRole, PlatformRole,
        authorize_calendar_action,
    },
    ics::{
        IcsParserLimits, NormalizedDateValue, NormalizedEvent, NormalizedTiming, parse_calendar,
    },
    ics_http::{ReqwestTransport, SafeHttpClient, SafeHttpConfig, TokioDnsResolver},
    identity::UserStatus,
    security::SecretKey,
};

const DEFAULT_REFRESH_SECONDS: i64 = 3600;

#[derive(Clone)]
pub struct ExternalFeedService {
    pool: SqlitePool,
    key: SecretKey,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}

#[derive(Clone, Debug, Serialize, sqlx::FromRow)]
pub struct FeedProjection {
    pub id: i64,
    pub calendar_id: i64,
    pub source_url_display: String,
    pub refresh_interval_seconds: i64,
    pub last_attempt_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub next_refresh_at: i64,
    pub last_error_code: Option<String>,
    pub disabled_at: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct NewFeed {
    pub source_url: String,
    pub refresh_interval_seconds: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct FetchResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

pub trait FeedFetcher: Send + Sync {
    fn fetch<'a>(
        &'a self,
        url: &'a str,
        etag: Option<&'a str>,
        last_modified: Option<&'a str>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<FetchResponse, FeedError>> + Send + 'a>,
    >;
}

/// The production adapter deliberately delegates every outbound request to the
/// hardened ICS client.  It only adds cache validators; URLs and response bodies
/// never enter logs or HTTP responses from this layer.
#[derive(Clone)]
pub struct SafeIcsFeedFetcher {
    client: SafeHttpClient<TokioDnsResolver, ReqwestTransport>,
}

/// Explicitly opt-in fixture adapter for browser tests.  Production code never
/// selects it; it makes imports reproducible without allowing local-network fetches.
#[derive(Clone)]
pub struct FixtureIcsFeedFetcher {
    fixture: PathBuf,
}

impl FixtureIcsFeedFetcher {
    pub fn from_path(path: impl Into<PathBuf>) -> Self {
        Self {
            fixture: path.into(),
        }
    }
}

impl FeedFetcher for FixtureIcsFeedFetcher {
    fn fetch<'a>(
        &'a self,
        _url: &'a str,
        _etag: Option<&'a str>,
        _last_modified: Option<&'a str>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<FetchResponse, FeedError>> + Send + 'a>,
    > {
        Box::pin(async move {
            Ok(FetchResponse {
                status: 200,
                body: tokio::fs::read(&self.fixture)
                    .await
                    .map_err(|_| FeedError::FetchFailed)?,
                etag: Some("commoncal-e2e-fixture-v1".into()),
                last_modified: None,
            })
        })
    }
}

impl SafeIcsFeedFetcher {
    pub fn production() -> Result<Self, FeedError> {
        Ok(Self {
            client: SafeHttpClient::production(SafeHttpConfig::default())
                .map_err(|_| FeedError::FetchFailed)?,
        })
    }
}

impl FeedFetcher for SafeIcsFeedFetcher {
    fn fetch<'a>(
        &'a self,
        url: &'a str,
        etag: Option<&'a str>,
        last_modified: Option<&'a str>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<FetchResponse, FeedError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let mut headers = HashMap::new();
            if let Some(etag) = etag {
                headers.insert("if-none-match".to_owned(), etag.to_owned());
            }
            if let Some(last_modified) = last_modified {
                headers.insert("if-modified-since".to_owned(), last_modified.to_owned());
            }
            let response = self
                .client
                .fetch_with_headers(url, &headers)
                .await
                .map_err(|_| FeedError::FetchFailed)?;
            Ok(FetchResponse {
                status: response.status(),
                body: response.body().to_vec(),
                etag: response.header("etag").map(str::to_owned),
                last_modified: response.header("last-modified").map(str::to_owned),
            })
        })
    }
}

#[derive(Debug)]
pub enum FeedError {
    Denied,
    InvalidInput,
    NotFound,
    FetchFailed,
    ParseFailed,
    Database(sqlx::Error),
}
impl From<sqlx::Error> for FeedError {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e)
    }
}

impl ExternalFeedService {
    pub fn new(pool: SqlitePool, key: SecretKey) -> Self {
        Self {
            pool,
            key,
            clock: Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("clock")
                    .as_secs() as i64
            }),
        }
    }
    pub fn new_at(pool: SqlitePool, key: SecretKey, now: i64) -> Self {
        Self {
            pool,
            key,
            clock: Arc::new(move || now),
        }
    }
    pub async fn create(
        &self,
        actor: i64,
        superadmin: bool,
        calendar_id: i64,
        input: NewFeed,
    ) -> Result<FeedProjection, FeedError> {
        let url = Url::parse(&input.source_url).map_err(|_| FeedError::InvalidInput)?;
        if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
            return Err(FeedError::InvalidInput);
        }
        let interval = input
            .refresh_interval_seconds
            .unwrap_or(DEFAULT_REFRESH_SECONDS);
        if interval < 60 {
            return Err(FeedError::InvalidInput);
        }
        self.authorize(actor, superadmin, calendar_id).await?;
        let now = (self.clock)();
        let result=sqlx::query("INSERT INTO external_feeds (calendar_id,source_url_encrypted,source_url_display,refresh_interval_seconds,next_refresh_at,created_by_user_id,created_at) VALUES (?,?,?,?,?,?,?)")
   .bind(calendar_id).bind(self.key.encrypt_secret(input.source_url.as_bytes())).bind(redact(&url)).bind(interval).bind(now).bind(actor).bind(now).execute(&self.pool).await?;
        self.feed(result.last_insert_rowid()).await
    }
    pub async fn list(
        &self,
        actor: i64,
        superadmin: bool,
        calendar_id: i64,
    ) -> Result<Vec<FeedProjection>, FeedError> {
        self.authorize(actor, superadmin, calendar_id).await?;
        let ids: Vec<i64> =
            sqlx::query_scalar("SELECT id FROM external_feeds WHERE calendar_id=? ORDER BY id")
                .bind(calendar_id)
                .fetch_all(&self.pool)
                .await?;
        let mut out = Vec::new();
        for id in ids {
            out.push(self.feed(id).await?)
        }
        Ok(out)
    }
    pub async fn disable(
        &self,
        actor: i64,
        superadmin: bool,
        id: i64,
    ) -> Result<FeedProjection, FeedError> {
        let calendar = self.calendar_for(id).await?;
        self.authorize(actor, superadmin, calendar).await?;
        sqlx::query("UPDATE external_feeds SET disabled_at=? WHERE id=?")
            .bind((self.clock)())
            .bind(id)
            .execute(&self.pool)
            .await?;
        self.feed(id).await
    }
    pub async fn delete(&self, actor: i64, superadmin: bool, id: i64) -> Result<(), FeedError> {
        let calendar = self.calendar_for(id).await?;
        self.authorize(actor, superadmin, calendar).await?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM events WHERE id IN (SELECT event_id FROM external_event_mapping WHERE feed_id=?)").bind(id).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM external_feeds WHERE id=?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn refresh<F: FeedFetcher>(
        &self,
        actor: i64,
        superadmin: bool,
        id: i64,
        fetcher: &F,
    ) -> Result<FeedProjection, FeedError> {
        let row:FeedRow=sqlx::query_as("SELECT calendar_id,source_url_encrypted,etag,last_modified,refresh_interval_seconds FROM external_feeds WHERE id=? AND disabled_at IS NULL").bind(id).fetch_optional(&self.pool).await?.ok_or(FeedError::NotFound)?;
        self.authorize(actor, superadmin, row.calendar_id).await?;
        let url = String::from_utf8(
            self.key
                .decrypt_secret(&row.source_url_encrypted)
                .ok_or(FeedError::FetchFailed)?,
        )
        .map_err(|_| FeedError::FetchFailed)?;
        let now = (self.clock)();
        let response = match fetcher
            .fetch(&url, row.etag.as_deref(), row.last_modified.as_deref())
            .await
        {
            Ok(v) => v,
            Err(e) => {
                self.record_failure(id, now, "fetch_failed").await?;
                return Err(e);
            }
        };
        if response.status == 304 {
            sqlx::query("UPDATE external_feeds SET last_attempt_at=?,last_success_at=?,last_error_code=NULL,consecutive_failures=0,next_refresh_at=? WHERE id=?").bind(now).bind(now).bind(now+row.refresh_interval_seconds).bind(id).execute(&self.pool).await?;
            return self.feed(id).await;
        }
        if response.status != 200 {
            self.record_failure(id, now, "http_failed").await?;
            return Err(FeedError::FetchFailed);
        }
        let parsed = parse_calendar(
            std::str::from_utf8(&response.body).map_err(|_| FeedError::ParseFailed)?,
            IcsParserLimits::default(),
        )
        .map_err(|_| FeedError::ParseFailed);
        let calendar = match parsed {
            Ok(v) => v,
            Err(e) => {
                self.record_failure(id, now, "parse_failed").await?;
                return Err(e);
            }
        };
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let sync: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(last_seen_sync_id), 0) + 1 FROM external_event_mapping WHERE feed_id = ?",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        for event in calendar.events {
            self.upsert_event(&mut tx, id, row.calendar_id, actor, sync, event)
                .await?;
        }
        sqlx::query("DELETE FROM events WHERE id IN (SELECT event_id FROM external_event_mapping WHERE feed_id=? AND last_seen_sync_id<>?)").bind(id).bind(sync).execute(&mut *tx).await?;
        sqlx::query("UPDATE external_feeds SET etag=?,last_modified=?,last_attempt_at=?,last_success_at=?,last_error_code=NULL,consecutive_failures=0,next_refresh_at=? WHERE id=?").bind(response.etag).bind(response.last_modified).bind(now).bind(now).bind(now+row.refresh_interval_seconds).bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        self.feed(id).await
    }
    async fn upsert_event(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        feed: i64,
        calendar: i64,
        actor: i64,
        sync: i64,
        event: NormalizedEvent,
    ) -> Result<(), FeedError> {
        let recurrence = recurrence(&event.recurrence_id);
        let hash = Sha256::digest(format!("{:?}", event).as_bytes()).to_vec();
        let existing: Option<(i64, Option<i64>, Option<Vec<u8>>)> = sqlx::query_as(
            "SELECT event_id, external_sequence, content_hash FROM external_event_mapping WHERE feed_id=? AND external_uid=? AND recurrence_id=?",
        )
        .bind(feed)
        .bind(&event.uid)
        .bind(&recurrence)
        .fetch_optional(&mut **tx)
        .await?;
        let (kind, ts, te, tz, ds, de) = timing(&event)?;
        let status = match event.status.as_deref() {
            Some("CANCELLED") => "cancelled",
            Some("TENTATIVE") => "tentative",
            _ => "confirmed",
        };
        let id = if let Some((id, sequence, content_hash)) = existing {
            if sequence != Some(event.sequence as i64)
                || content_hash.as_deref() != Some(hash.as_slice())
            {
                sqlx::query("UPDATE events SET title=?,description=?,location=?,status=?,event_kind=?,timed_start_utc=?,timed_end_utc=?,event_timezone=?,all_day_start_date=?,all_day_end_date=?,last_edited_by_user_id=?,version=version+1,updated_at=?,recurrence_rule=? WHERE id=?").bind(&event.summary).bind(&event.description).bind(&event.location).bind(status).bind(kind).bind(ts).bind(te).bind(tz).bind(ds).bind(de).bind(actor).bind(sync).bind(&event.rrule).bind(id).execute(&mut **tx).await?;
            }
            id
        } else {
            let r=sqlx::query("INSERT INTO events (calendar_id,title,description,location,status,event_kind,timed_start_utc,timed_end_utc,event_timezone,all_day_start_date,all_day_end_date,created_by_user_id,last_edited_by_user_id,version,created_at,updated_at,recurrence_rule) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,1,?,?,?)").bind(calendar).bind(&event.summary).bind(&event.description).bind(&event.location).bind(status).bind(kind).bind(ts).bind(te).bind(tz).bind(ds).bind(de).bind(actor).bind(actor).bind(sync).bind(sync).bind(&event.rrule).execute(&mut **tx).await?;
            r.last_insert_rowid()
        };
        sqlx::query("INSERT INTO external_event_mapping (feed_id,external_uid,recurrence_id,event_id,external_sequence,external_modified_at,content_hash,last_seen_sync_id) VALUES (?,?,?,?,?,?,?,?) ON CONFLICT(feed_id,external_uid,recurrence_id) DO UPDATE SET event_id=excluded.event_id,external_sequence=excluded.external_sequence,external_modified_at=excluded.external_modified_at,content_hash=excluded.content_hash,last_seen_sync_id=excluded.last_seen_sync_id").bind(feed).bind(&event.uid).bind(recurrence).bind(id).bind(event.sequence as i64).bind(event.last_modified.map(|v|v.timestamp())).bind(hash).bind(sync).execute(&mut **tx).await?;
        Ok(())
    }
    async fn record_failure(&self, id: i64, now: i64, code: &str) -> Result<(), FeedError> {
        // Cap retry delay at one day and calculate from the attempt time, so a
        // stalled feed cannot be retried in a tight loop or remain permanently stale.
        sqlx::query("UPDATE external_feeds SET last_attempt_at=?,last_error_code=?,consecutive_failures=consecutive_failures+1,next_refresh_at=? + MIN(refresh_interval_seconds * (1 << MIN(consecutive_failures, 6)), 86400) WHERE id=?")
            .bind(now)
            .bind(now)
            .bind(code)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    async fn calendar_for(&self, id: i64) -> Result<i64, FeedError> {
        sqlx::query_scalar("SELECT calendar_id FROM external_feeds WHERE id=?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(FeedError::NotFound)
    }
    async fn feed(&self, id: i64) -> Result<FeedProjection, FeedError> {
        sqlx::query_as("SELECT id,calendar_id,source_url_display,refresh_interval_seconds,last_attempt_at,last_success_at,next_refresh_at,last_error_code,disabled_at FROM external_feeds WHERE id=?").bind(id).fetch_optional(&self.pool).await?.ok_or(FeedError::NotFound)
    }
    async fn authorize(
        &self,
        actor: i64,
        superadmin: bool,
        calendar: i64,
    ) -> Result<(), FeedError> {
        let row:Option<(String,String)>=sqlx::query_as("SELECT u.status,a.role FROM users u JOIN calendar_acl a ON a.user_id=u.id WHERE u.id=? AND a.calendar_id=?").bind(actor).bind(calendar).fetch_optional(&self.pool).await?;
        let allowed = row
            .and_then(|(s, r)| {
                Some((
                    UserStatus::try_from(s.as_str()).ok()?,
                    CalendarRole::from_str(&r).ok()?,
                ))
            })
            .is_some_and(|(s, r)| {
                authorize_calendar_action(
                    s,
                    Some(if superadmin {
                        PlatformRole::Superadmin
                    } else {
                        PlatformRole::User
                    }),
                    Some(r),
                    CalendarAction::ManageSettings,
                ) == AuthorizationDecision::Allow
            });
        if allowed {
            Ok(())
        } else {
            Err(FeedError::Denied)
        }
    }
}
#[derive(sqlx::FromRow)]
struct FeedRow {
    calendar_id: i64,
    source_url_encrypted: Vec<u8>,
    etag: Option<String>,
    last_modified: Option<String>,
    refresh_interval_seconds: i64,
}
fn redact(url: &Url) -> String {
    format!(
        "{}://{}/…",
        url.scheme(),
        url.host_str().unwrap_or("invalid")
    )
}
fn recurrence(value: &Option<NormalizedDateValue>) -> String {
    match value {
        None => String::new(),
        Some(NormalizedDateValue::Timed(v)) => format!("t:{}", v.timestamp()),
        Some(NormalizedDateValue::AllDay(v)) => format!("d:{v}"),
    }
}
type EventTimingColumns = (
    &'static str,
    Option<i64>,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn timing(e: &NormalizedEvent) -> Result<EventTimingColumns, FeedError> {
    match &e.timing {
        NormalizedTiming::Timed {
            starts_at,
            ends_at,
            timezone,
        } => Ok((
            "timed",
            Some(starts_at.timestamp()),
            Some(ends_at.timestamp()),
            Some(timezone.clone().unwrap_or_else(|| "UTC".to_owned())),
            None,
            None,
        )),
        NormalizedTiming::AllDay {
            start_date,
            end_date,
        } => Ok((
            "all_day",
            None,
            None,
            None,
            Some(start_date.to_string()),
            Some(end_date.to_string()),
        )),
    }
}
