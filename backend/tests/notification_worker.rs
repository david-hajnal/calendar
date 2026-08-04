use commoncal_backend::{
    config::{AppConfig, Environment},
    database::connect_and_migrate,
    email::{EmailError, EmailSender, InMemoryEmailSender, NotificationEmail},
    http::Readiness,
    notification::NotificationWorker,
};
use sqlx::SqlitePool;
use std::sync::Mutex;
use std::{
    io::{self, Write},
    sync::Arc,
};
use tempfile::TempDir;
use tracing::{Instrument, instrument::WithSubscriber};
use tracing_subscriber::fmt::MakeWriter;

const NOW: i64 = 1_750_000_000;

#[tokio::test]
async fn concurrent_claims_have_a_single_winner_and_respect_the_batch_limit() {
    let (_dir, pool) = setup().await;
    let first = job(&pool, NOW - 2).await;
    let worker = NotificationWorker::new_at(pool, NOW, 60, 1);

    let (left, right) = tokio::join!(worker.claim_due(), worker.claim_due());
    let mut claimed = [left.unwrap(), right.unwrap()];
    let claimed = claimed
        .iter_mut()
        .flat_map(|jobs| jobs.drain(..))
        .collect::<Vec<_>>();

    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].id, first);
}

#[tokio::test]
async fn claims_are_limited_to_the_configured_batch_size() {
    let (_dir, pool) = setup().await;
    job(&pool, NOW - 2).await;
    job(&pool, NOW - 1).await;
    let worker = NotificationWorker::new_at(pool, NOW, 60, 1);

    assert_eq!(worker.claim_due().await.unwrap().len(), 1);
}

#[tokio::test]
async fn expired_claims_are_recovered_and_stale_claimants_cannot_complete() {
    let (_dir, pool) = setup().await;
    let id = job(&pool, NOW).await;
    let initial = NotificationWorker::new_at(pool.clone(), NOW, 60, 10);
    let original = initial.claim_due().await.unwrap().pop().unwrap();

    let recovered = NotificationWorker::new_at(pool.clone(), NOW + 60, 60, 10)
        .claim_due()
        .await
        .unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].id, id);
    assert_ne!(recovered[0].claim_token, original.claim_token);
    assert!(
        !NotificationWorker::new_at(pool, NOW + 60, 60, 10)
            .mark_delivered(&original)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn completing_a_claim_is_idempotent_and_prevents_a_second_delivery() {
    let (_dir, pool) = setup().await;
    let id = job(&pool.clone(), NOW).await;
    let worker = NotificationWorker::new_at(pool.clone(), NOW, 60, 10);
    let claim = worker.claim_due().await.unwrap().pop().unwrap();

    assert!(worker.mark_delivered(&claim).await.unwrap());
    assert!(!worker.mark_delivered(&claim).await.unwrap());
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT state FROM notification_jobs WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        "delivered"
    );
}

#[tokio::test]
async fn revoked_calendar_access_prevents_private_notification_delivery() {
    let (_dir, pool) = setup().await;
    let (job_id, user, calendar) = deliverable_job(&pool).await;
    sqlx::query("DELETE FROM calendar_acl WHERE calendar_id = ? AND user_id = ?")
        .bind(calendar)
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    let sender = Arc::new(InMemoryEmailSender::new());
    let worker =
        NotificationWorker::new_at_with_email_sender(pool.clone(), NOW, 60, 10, sender.clone());

    worker.process_due().await.unwrap();

    assert!(sender.messages().is_empty());
    assert_eq!(in_app_count(&pool, job_id).await, 0);
}

#[tokio::test]
async fn suspended_user_receives_no_notification_delivery() {
    let (_dir, pool) = setup().await;
    let (job_id, user, _) = deliverable_job(&pool).await;
    sqlx::query("UPDATE users SET status = 'suspended' WHERE id = ?")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    let sender = Arc::new(InMemoryEmailSender::new());
    let worker =
        NotificationWorker::new_at_with_email_sender(pool.clone(), NOW, 60, 10, sender.clone());

    worker.process_due().await.unwrap();

    assert!(sender.messages().is_empty());
    assert_eq!(in_app_count(&pool, job_id).await, 0);
}

#[tokio::test]
async fn valid_claim_creates_an_in_app_notification() {
    let (_dir, pool) = setup().await;
    let (job_id, _, _) = deliverable_job(&pool).await;
    let worker = NotificationWorker::new_at_with_email_sender(
        pool.clone(),
        NOW,
        60,
        10,
        Arc::new(InMemoryEmailSender::new()),
    );

    worker.process_due().await.unwrap();

    assert_eq!(in_app_count(&pool, job_id).await, 1);
}

#[tokio::test]
async fn valid_claim_sends_a_notification_email() {
    let (_dir, pool) = setup().await;
    let (_, user, _) = deliverable_job(&pool).await;
    let sender = Arc::new(InMemoryEmailSender::new());
    let worker = NotificationWorker::new_at_with_email_sender(pool, NOW, 60, 10, sender.clone());

    worker.process_due().await.unwrap();

    let messages = sender.messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].recipient(),
        format!("deliverable-{user}@example.com")
    );
}

#[tokio::test]
async fn transient_email_failure_is_retried_after_deterministic_backoff() {
    let (_dir, pool) = setup().await;
    let (id, _, _) = deliverable_job(&pool).await;
    let sender = Arc::new(SequencedEmailSender::new(vec![
        Err(EmailError::transient()),
        Ok(()),
    ]));
    let worker = NotificationWorker::new_at_with_email_sender_and_retry_policy(
        pool.clone(),
        NOW,
        60,
        10,
        3,
        sender.clone(),
    );

    worker.process_due().await.unwrap();
    let retry_at: i64 =
        sqlx::query_scalar("SELECT next_attempt_at FROM notification_jobs WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(retry_at, NOW + 60);
    assert_eq!(job_state(&pool, id).await, "pending");

    NotificationWorker::new_at_with_email_sender_and_retry_policy(
        pool.clone(),
        retry_at,
        60,
        10,
        3,
        sender.clone(),
    )
    .process_due()
    .await
    .unwrap();
    assert_eq!(job_state(&pool, id).await, "delivered");
    assert_eq!(sender.calls(), 2);
}

#[tokio::test]
async fn exhausted_transient_failures_reach_terminal_failed_state_without_leaking_private_data() {
    let (_dir, pool) = setup().await;
    let (id, _, _) = deliverable_job(&pool).await;
    let sender = Arc::new(SequencedEmailSender::new(vec![
        Err(EmailError::transient()),
        Err(EmailError::transient()),
    ]));
    let worker = NotificationWorker::new_at_with_email_sender_and_retry_policy(
        pool.clone(),
        NOW,
        60,
        10,
        2,
        sender.clone(),
    );

    worker.process_due().await.unwrap();
    NotificationWorker::new_at_with_email_sender_and_retry_policy(
        pool.clone(),
        NOW + 60,
        60,
        10,
        2,
        sender,
    )
    .process_due()
    .await
    .unwrap();
    assert_eq!(job_state(&pool, id).await, "failed");
    let (attempts, error_code): (i64, Option<String>) =
        sqlx::query_as("SELECT attempts, last_error_code FROM notification_jobs WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(attempts, 2);
    assert_eq!(error_code.as_deref(), Some("email_provider_failure"));
}

#[tokio::test]
async fn permanent_email_failure_is_terminal_immediately_and_metrics_are_safe() {
    let (_dir, pool) = setup().await;
    let (id, _, _) = deliverable_job(&pool).await;
    let sender = Arc::new(SequencedEmailSender::new(vec![
        Err(EmailError::permanent()),
    ]));
    let worker = NotificationWorker::new_at_with_email_sender_and_retry_policy(
        pool.clone(),
        NOW,
        60,
        10,
        3,
        sender,
    );

    worker.process_due().await.unwrap();
    assert_eq!(job_state(&pool, id).await, "failed");
    assert_eq!(worker.metrics().permanent_failures, 1);
    assert_eq!(worker.metrics().transient_retries, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn delivery_failure_logs_are_structured_and_redact_private_delivery_data() {
    let (writer, captured) = CapturedWriter::new();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_writer(writer)
        .finish();

    async {
        NotificationWorker::<InMemoryEmailSender>::log_delivery_failure(
            EmailError::permanent(),
            2,
            false,
        );
    }
    .instrument(tracing::info_span!(parent: None, "notification_worker_test"))
    .with_subscriber(subscriber)
    .await;

    let output = captured.output();
    assert!(output.contains("email_permanent_failure"));
    assert!(!output.contains("deliverable-1@example.com"));
    assert!(!output.contains("Private event"));
    assert!(!output.contains("http"));
}

async fn setup() -> (TempDir, SqlitePool) {
    let dir = TempDir::new().unwrap();
    let config = AppConfig::with_database_path(
        Environment::Development,
        "127.0.0.1:0",
        None,
        dir.path().join("test.sqlite"),
    )
    .unwrap();
    let pool = connect_and_migrate(&config, Readiness::new())
        .await
        .unwrap();
    (dir, pool)
}

async fn job(pool: &SqlitePool, scheduled_at: i64) -> i64 {
    let user = sqlx::query("INSERT INTO users (normalized_email, display_name, status, created_at) VALUES (?, ?, 'active', ?)")
        .bind(format!("user-{scheduled_at}@example.com"))
        .bind("User")
        .bind(NOW)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let calendar = sqlx::query("INSERT INTO calendars (owner_user_id, name, color, default_timezone, default_event_visibility, created_at, updated_at) VALUES (?, 'Calendar', '#123456', 'UTC', 'default', ?, ?)")
        .bind(user)
        .bind(NOW)
        .bind(NOW)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid();
    sqlx::query("INSERT INTO notification_jobs (user_id, calendar_id, event_id, occurrence_key, scheduled_at, state, created_at, updated_at) VALUES (?, ?, ?, ?, ?, 'pending', ?, ?)")
        .bind(user)
        .bind(calendar)
        .bind(scheduled_at)
        .bind(scheduled_at.to_string())
        .bind(scheduled_at)
        .bind(NOW)
        .bind(NOW)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
}

async fn deliverable_job(pool: &SqlitePool) -> (i64, i64, i64) {
    let user = sqlx::query("INSERT INTO users (normalized_email, display_name, status, created_at) VALUES (?, 'User', 'active', ?)")
        .bind("deliverable-1@example.com")
        .bind(NOW)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let calendar = sqlx::query("INSERT INTO calendars (owner_user_id, name, color, default_timezone, default_event_visibility, created_at, updated_at) VALUES (?, 'Calendar', '#123456', 'UTC', 'private', ?, ?)")
        .bind(user)
        .bind(NOW)
        .bind(NOW)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid();
    sqlx::query("INSERT INTO calendar_acl (calendar_id, user_id, role, created_at, updated_at) VALUES (?, ?, 'owner', ?, ?)")
        .bind(calendar).bind(user).bind(NOW).bind(NOW).execute(pool).await.unwrap();
    let event = sqlx::query("INSERT INTO events (calendar_id, title, status, event_kind, timed_start_utc, timed_end_utc, event_timezone, created_by_user_id, last_edited_by_user_id, created_at, updated_at) VALUES (?, 'Private event', 'confirmed', 'timed', ?, ?, 'UTC', ?, ?, ?, ?)")
        .bind(calendar).bind(NOW + 60).bind(NOW + 120).bind(user).bind(user).bind(NOW).bind(NOW)
        .execute(pool).await.unwrap().last_insert_rowid();
    let job = sqlx::query("INSERT INTO notification_jobs (user_id, calendar_id, event_id, occurrence_key, scheduled_at, state, created_at, updated_at) VALUES (?, ?, ?, 'occurrence', ?, 'pending', ?, ?)")
        .bind(user).bind(calendar).bind(event).bind(NOW).bind(NOW).bind(NOW)
        .execute(pool).await.unwrap().last_insert_rowid();
    (job, user, calendar)
}

async fn in_app_count(pool: &SqlitePool, job_id: i64) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM in_app_notifications WHERE notification_job_id = ?")
        .bind(job_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn job_state(pool: &SqlitePool, job_id: i64) -> String {
    sqlx::query_scalar("SELECT state FROM notification_jobs WHERE id = ?")
        .bind(job_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

struct SequencedEmailSender {
    outcomes: Mutex<Vec<Result<(), EmailError>>>,
    calls: Mutex<usize>,
}
impl SequencedEmailSender {
    fn new(outcomes: Vec<Result<(), EmailError>>) -> Self {
        Self {
            outcomes: Mutex::new(outcomes),
            calls: Mutex::new(0),
        }
    }
    fn calls(&self) -> usize {
        *self.calls.lock().unwrap()
    }
}
impl EmailSender for SequencedEmailSender {
    async fn send_invitation(
        &self,
        _: commoncal_backend::email::InvitationEmail,
    ) -> Result<(), EmailError> {
        Ok(())
    }
    async fn send_login_link(
        &self,
        _: commoncal_backend::email::LoginLinkEmail,
    ) -> Result<(), EmailError> {
        Ok(())
    }
    async fn send_notification(&self, _: NotificationEmail) -> Result<(), EmailError> {
        *self.calls.lock().unwrap() += 1;
        self.outcomes.lock().unwrap().remove(0)
    }
}

#[derive(Clone)]
struct CapturedWriter {
    bytes: Arc<Mutex<Vec<u8>>>,
}
impl CapturedWriter {
    fn new() -> (Self, CapturedOutput) {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                bytes: bytes.clone(),
            },
            CapturedOutput { bytes },
        )
    }
}
impl<'a> MakeWriter<'a> for CapturedWriter {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}
impl Write for CapturedWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.bytes.lock().unwrap().extend_from_slice(buffer);
        Ok(buffer.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
struct CapturedOutput {
    bytes: Arc<Mutex<Vec<u8>>>,
}
impl CapturedOutput {
    fn output(&self) -> String {
        String::from_utf8(self.bytes.lock().unwrap().clone()).unwrap()
    }
}
